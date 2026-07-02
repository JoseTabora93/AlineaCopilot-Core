//! Compilador determinista PERFIL → assistant (motor `"copilot"` — tarea A8).
//!
//! Decisión de arquitectura (ver `PROFILE_SCHEMA.md` y
//! `crates/aionui-db/src/models/agent_profile.rs`): los `assistants` visibles
//! en `/api/assistants` son la PROYECCIÓN de un `agent_profile` (tarea A1)
//! cuando ese perfil declara `"copilot"` en `engines`. Este módulo es el
//! compilador — igual que las tareas A4 (Hermes) y A5 (OpenClaw) materializan
//! el mismo perfil a config nativa de sus motores, este materializa el
//! perfil como una fila de `assistant_definitions` con
//! `source = 'generated'`.
//!
//! ## "Assistant gestionado"
//!
//! Un assistant materializado por este compilador se distingue de los
//! `builtin` y `user` existentes por `source = 'generated'` (valor ya
//! soportado por el `CHECK` de la migración 012, previamente sin productor).
//! `owner_type = 'system'` y `source_ref = <agent_profiles.id>` anclan la
//! fila al perfil de origen — `get_by_source_ref("generated", profile.id)`
//! es la forma canónica de encontrar el assistant materializado de un
//! perfil dado.
//!
//! El editor de assistants (`AssistantService::update`) NO tiene una rama
//! especial para `source = 'generated'` — cae en la rama `AssistantSource`
//! por defecto (ver `service::classify_source`, que este módulo NO toca). El
//! comportamiento elegido es documental, no técnico: cualquier edición
//! manual de un assistant gestionado sobrevive hasta la siguiente
//! materialización (creación/actualización del perfil de origen), momento en
//! el que `materialize_profile` vuelve a hacer `upsert` con los valores
//! derivados del perfil y la pisa. No se añadió un bloqueo de escritura HTTP
//! (a diferencia de `builtin`, que sí lo tiene en `AssistantService::update`)
//! porque el perfil es la fuente de verdad declarada por A1, y el pisado en
//! la siguiente materialización ya hace cumplir esa invariante sin ampliar
//! la superficie de la tarea A8 tocando `service.rs` (dominio compartido con
//! otras fuentes de assistants — builtin/user/extension).
//!
//! ## Idempotencia
//!
//! `IAssistantDefinitionRepository::upsert` (SQLite) bump-ea `updated_at`
//! incondicionalmente en cada llamada — correcto para el bootstrap de
//! builtins (que corre en cada arranque), pero violaría el requisito de A8
//! de que "re-materializar sin cambios = no-op". Por eso `materialize_profile`
//! primero lee el estado existente (`get_by_source_ref`) y compara campo a
//! campo contra lo que produciría la materialización; solo llama `upsert`
//! si algo cambió.

use aionui_common::{generate_prefixed_id, now_ms};
use aionui_db::{
    AgentProfileRow, AssistantDefinitionRow, IAssistantDefinitionRepository, UpsertAssistantDefinitionParams,
    parse_profile_definition,
};

use crate::error::AssistantError;

/// Prefijo estable del `assistant_key` derivado de `agent_profiles.name`.
/// `name` es inmutable en la práctica (ver `PROFILE_SCHEMA.md`), así que esta
/// clave es estable mientras el perfil exista.
pub fn assistant_key_for_profile(profile_name: &str) -> String {
    format!("profile:{profile_name}")
}

/// Backend de ejecución para assistants materializados desde un perfil.
/// `"aionrs"` es el backend genérico que el Core ya usa por defecto para
/// nuevos assistants (ver `AssistantService::resolve_default_agent_type`) —
/// habla el protocolo del proveedor configurado sin depender de un CLI de
/// terceros instalado, que es exactamente lo que necesita un assistant
/// generado sin intervención manual.
const GENERATED_AGENT_BACKEND: &str = "aionrs";

/// Resultado de [`materialize_profile`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaterializeOutcome {
    /// Se creó la fila `assistant_definitions` (no existía para este perfil).
    Created,
    /// Existía y se actualizó porque algún campo derivado cambió.
    Updated,
    /// Existía y era idéntica a la que se iba a escribir — no-op deliberado
    /// (no se llamó `upsert`, así que `updated_at` no se tocó).
    Unchanged,
    /// El perfil no declara `"copilot"` en `engines` (o está inactivo) y no
    /// había assistant materializado que retirar — no-op.
    NotApplicable,
    /// El perfil no declara `"copilot"` (o está inactivo) pero SÍ había un
    /// assistant materializado de una materialización anterior — se retiró
    /// (soft-delete).
    Retired,
}

/// Materializa (o retira) el assistant `"copilot"` correspondiente a `profile`.
///
/// Se invoca tras crear/actualizar/(des)activar un `agent_profile` (ver
/// `crates/aionui-app/src/router/profiles.rs`). Comportamiento:
///
/// - Si `profile.is_active` y `"copilot" ∈ definition.engines`: upsert
///   idempotente de la fila `assistant_definitions` (`source='generated'`).
/// - En cualquier otro caso (perfil inactivo, o `"copilot"` ya no está en
///   `engines`): si existe un assistant materializado de una corrida
///   anterior, se retira (soft-delete). Nunca falla si no había nada que
///   retirar.
///
/// El JSON de `profile.definition` ya fue validado contra PERFIL v1 al
/// crear/actualizar (tarea A1, `validate_profile_definition`); este
/// compilador lo vuelve a parsear (barato) vía `parse_profile_definition`
/// para extraer `soul_md`, `model.primary`, `skills`, `mcp_allowlist`.
pub async fn materialize_profile(
    definition_repo: &dyn IAssistantDefinitionRepository,
    profile: &AgentProfileRow,
) -> Result<MaterializeOutcome, AssistantError> {
    let existing = definition_repo
        .get_by_source_ref("generated", &profile.id)
        .await
        .map_err(|e| AssistantError::Internal(format!("lookup generated assistant by source_ref: {e}")))?;

    if !profile.is_active {
        return retire_if_present(definition_repo, existing).await;
    }

    let parsed = parse_profile_definition(&profile.definition, &profile.name)
        .map_err(|e| AssistantError::BadRequest(format!("profile definition failed schema validation: {e}")))?;

    if !parsed.engines.iter().any(|e| e == "copilot") {
        return retire_if_present(definition_repo, existing).await;
    }

    let desired = DesiredDefinition {
        assistant_key: assistant_key_for_profile(&profile.name),
        source_ref: profile.id.clone(),
        name: profile.label.clone(),
        description: profile.label.clone(),
        rule_inline_content: parsed.soul_md.clone(),
        default_model_value: parsed.model.primary.clone(),
        default_skill_ids_json: encode_json_array(&parsed.skills),
        default_mcp_ids_json: encode_json_array(&parsed.mcp_allowlist),
    };

    if let Some(existing) = &existing
        && desired.matches(existing)
    {
        return Ok(MaterializeOutcome::Unchanged);
    }

    let definition_id = existing
        .as_ref()
        .map(|e| e.definition_id.clone())
        .unwrap_or_else(|| generate_prefixed_id("asstdef"));
    let outcome = if existing.is_some() {
        MaterializeOutcome::Updated
    } else {
        MaterializeOutcome::Created
    };

    definition_repo
        .upsert(&desired.to_params(&definition_id))
        .await
        .map_err(|e| AssistantError::Internal(format!("upsert generated assistant definition: {e}")))?;

    Ok(outcome)
}

async fn retire_if_present(
    definition_repo: &dyn IAssistantDefinitionRepository,
    existing: Option<AssistantDefinitionRow>,
) -> Result<MaterializeOutcome, AssistantError> {
    let Some(existing) = existing else {
        return Ok(MaterializeOutcome::NotApplicable);
    };
    let removed = definition_repo
        .soft_delete(&existing.definition_id, now_ms())
        .await
        .map_err(|e| AssistantError::Internal(format!("soft-delete generated assistant definition: {e}")))?;
    Ok(if removed {
        MaterializeOutcome::Retired
    } else {
        // Ya estaba soft-deleted (carrera con otra materialización, o
        // `get_by_source_ref` ya filtra `deleted_at IS NULL` así que esta
        // rama es defensiva más que alcanzable en la práctica).
        MaterializeOutcome::NotApplicable
    })
}

/// Snapshot de los campos que este compilador deriva del perfil, usado tanto
/// para construir los parámetros de `upsert` como para la comparación de
/// idempotencia contra la fila existente.
struct DesiredDefinition {
    assistant_key: String,
    source_ref: String,
    name: String,
    description: String,
    rule_inline_content: String,
    default_model_value: String,
    default_skill_ids_json: String,
    default_mcp_ids_json: String,
}

impl DesiredDefinition {
    /// `true` si `existing` ya refleja exactamente lo que produciría esta
    /// materialización — el caller debe omitir el `upsert` en ese caso
    /// (requisito de idempotencia: no bump-ear `updated_at` sin cambios
    /// reales).
    fn matches(&self, existing: &AssistantDefinitionRow) -> bool {
        existing.assistant_key == self.assistant_key
            && existing.source == "generated"
            && existing.owner_type == "system"
            && existing.source_ref.as_deref() == Some(self.source_ref.as_str())
            && existing.name == self.name
            && existing.description.as_deref() == Some(self.description.as_str())
            && existing.agent_backend == GENERATED_AGENT_BACKEND
            && existing.rule_resource_type == "inline"
            && existing.rule_inline_content.as_deref() == Some(self.rule_inline_content.as_str())
            && existing.default_model_mode == "fixed"
            && existing.default_model_value.as_deref() == Some(self.default_model_value.as_str())
            && existing.default_skills_mode == "fixed"
            && existing.default_skill_ids == self.default_skill_ids_json
            && existing.default_mcps_mode == "fixed"
            && existing.default_mcp_ids == self.default_mcp_ids_json
    }

    fn to_params<'a>(&'a self, definition_id: &'a str) -> UpsertAssistantDefinitionParams<'a> {
        UpsertAssistantDefinitionParams {
            definition_id,
            assistant_key: &self.assistant_key,
            source: "generated",
            owner_type: "system",
            source_ref: Some(&self.source_ref),
            source_version: None,
            source_hash: None,
            name: &self.name,
            name_i18n: "{}",
            description: Some(&self.description),
            description_i18n: "{}",
            avatar_type: "none",
            avatar_value: None,
            agent_backend: GENERATED_AGENT_BACKEND,
            rule_resource_type: "inline",
            rule_resource_ref: None,
            rule_inline_content: Some(&self.rule_inline_content),
            recommended_prompts: "[]",
            recommended_prompts_i18n: "{}",
            default_model_mode: "fixed",
            default_model_value: Some(&self.default_model_value),
            default_permission_mode: "auto",
            default_permission_value: None,
            default_skills_mode: "fixed",
            default_skill_ids: &self.default_skill_ids_json,
            custom_skill_names: "[]",
            default_disabled_builtin_skill_ids: "[]",
            default_mcps_mode: "fixed",
            default_mcp_ids: &self.default_mcp_ids_json,
        }
    }
}

fn encode_json_array(values: &[String]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aionui_db::{IAgentProfileRepository, NewAgentProfile, SqliteAgentProfileRepository, init_database_memory};

    fn definition_json(name: &str, engines: &[&str], soul_md: &str, model_primary: &str) -> String {
        serde_json::json!({
            "name": name,
            "label": format!("Label {name}"),
            "engines": engines,
            "soul_md": soul_md,
            "model": { "primary": model_primary, "fallbacks": [] },
            "mcp_allowlist": ["ingelmec-kb"],
            "skills": ["alcance-bom"],
            "kb_scope": [],
            "channels": [{ "type": "web" }],
            "caps": { "soft_usd": 1.0, "hard_usd": 2.0, "period": "month" },
            "acl": { "etiqueta": "interno", "roles": ["admin"] }
        })
        .to_string()
    }

    async fn setup() -> (
        aionui_db::Database,
        std::sync::Arc<dyn IAgentProfileRepository>,
        std::sync::Arc<dyn IAssistantDefinitionRepository>,
    ) {
        let db = init_database_memory().await.unwrap();
        let profile_repo: std::sync::Arc<dyn IAgentProfileRepository> =
            std::sync::Arc::new(SqliteAgentProfileRepository::new(db.pool().clone()));
        let definition_repo: std::sync::Arc<dyn IAssistantDefinitionRepository> =
            std::sync::Arc::new(aionui_db::SqliteAssistantDefinitionRepository::new(db.pool().clone()));
        (db, profile_repo, definition_repo)
    }

    async fn create_profile(
        repo: &dyn IAgentProfileRepository,
        name: &str,
        engines: &[&str],
        soul_md: &str,
        model_primary: &str,
    ) -> AgentProfileRow {
        repo.create(NewAgentProfile {
            name: name.to_string(),
            label: format!("Label {name}"),
            definition: definition_json(name, engines, soul_md, model_primary),
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn materializes_new_assistant_for_copilot_profile() {
        let (_db, profile_repo, definition_repo) = setup().await;
        let profile = create_profile(
            &*profile_repo,
            "ingenieria",
            &["copilot"],
            "# Ingeniería",
            "zai/glm-5.1",
        )
        .await;

        let outcome = materialize_profile(&*definition_repo, &profile).await.unwrap();
        assert_eq!(outcome, MaterializeOutcome::Created);

        let def = definition_repo
            .get_by_source_ref("generated", &profile.id)
            .await
            .unwrap()
            .expect("assistant definition must exist");
        assert_eq!(def.assistant_key, "profile:ingenieria");
        assert_eq!(def.source, "generated");
        assert_eq!(def.owner_type, "system");
        assert_eq!(def.rule_resource_type, "inline");
        assert_eq!(def.rule_inline_content.as_deref(), Some("# Ingeniería"));
        assert_eq!(def.default_model_mode, "fixed");
        assert_eq!(def.default_model_value.as_deref(), Some("zai/glm-5.1"));
        assert_eq!(def.default_skill_ids, r#"["alcance-bom"]"#);
        assert_eq!(def.default_mcp_ids, r#"["ingelmec-kb"]"#);

        // Also reachable via the assistant_key that /api/assistants uses.
        let by_key = definition_repo.get_by_key("profile:ingenieria").await.unwrap();
        assert!(by_key.is_some());
    }

    #[tokio::test]
    async fn ignores_profile_without_copilot_engine() {
        let (_db, profile_repo, definition_repo) = setup().await;
        let profile = create_profile(&*profile_repo, "servimec-tko", &["hermes"], "# TKO", "zai/glm-5.1").await;

        let outcome = materialize_profile(&*definition_repo, &profile).await.unwrap();
        assert_eq!(outcome, MaterializeOutcome::NotApplicable);
        assert!(
            definition_repo
                .get_by_source_ref("generated", &profile.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn rematerializing_unchanged_profile_is_noop_and_does_not_bump_updated_at() {
        let (_db, profile_repo, definition_repo) = setup().await;
        let profile = create_profile(&*profile_repo, "preventa", &["copilot"], "# Preventa", "zai/glm-5.1").await;

        materialize_profile(&*definition_repo, &profile).await.unwrap();
        let first = definition_repo
            .get_by_source_ref("generated", &profile.id)
            .await
            .unwrap()
            .unwrap();

        // Re-materialize the exact same profile (no field changed).
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let outcome = materialize_profile(&*definition_repo, &profile).await.unwrap();
        assert_eq!(outcome, MaterializeOutcome::Unchanged);

        let second = definition_repo
            .get_by_source_ref("generated", &profile.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            second.updated_at, first.updated_at,
            "unchanged re-materialization must not bump updated_at"
        );
        assert_eq!(second.definition_id, first.definition_id);
    }

    #[tokio::test]
    async fn changing_soul_md_updates_the_materialized_assistant() {
        let (_db, profile_repo, definition_repo) = setup().await;
        let profile = create_profile(&*profile_repo, "preventa", &["copilot"], "# v1", "zai/glm-5.1").await;
        materialize_profile(&*definition_repo, &profile).await.unwrap();
        let before = definition_repo
            .get_by_source_ref("generated", &profile.id)
            .await
            .unwrap()
            .unwrap();

        let updated_row = profile_repo
            .update(
                &profile.id,
                aionui_db::AgentProfileUpdate {
                    label: None,
                    definition: Some(definition_json(
                        "preventa",
                        &["copilot"],
                        "# v2 (cambio real)",
                        "zai/glm-5.1",
                    )),
                    is_active: None,
                },
            )
            .await;
        updated_row.unwrap();
        let profile = profile_repo.get(&profile.id).await.unwrap().unwrap();

        let outcome = materialize_profile(&*definition_repo, &profile).await.unwrap();
        assert_eq!(outcome, MaterializeOutcome::Updated);

        let after = definition_repo
            .get_by_source_ref("generated", &profile.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.rule_inline_content.as_deref(), Some("# v2 (cambio real)"));
        assert_eq!(after.definition_id, before.definition_id, "definition_id is stable");
        assert!(after.updated_at >= before.updated_at);
    }

    #[tokio::test]
    async fn deactivating_profile_retires_the_materialized_assistant() {
        let (_db, profile_repo, definition_repo) = setup().await;
        let profile = create_profile(&*profile_repo, "preventa", &["copilot"], "# Preventa", "zai/glm-5.1").await;
        materialize_profile(&*definition_repo, &profile).await.unwrap();
        assert!(
            definition_repo
                .get_by_source_ref("generated", &profile.id)
                .await
                .unwrap()
                .is_some()
        );

        profile_repo
            .update(
                &profile.id,
                aionui_db::AgentProfileUpdate {
                    label: None,
                    definition: None,
                    is_active: Some(false),
                },
            )
            .await
            .unwrap();
        let profile = profile_repo.get(&profile.id).await.unwrap().unwrap();

        let outcome = materialize_profile(&*definition_repo, &profile).await.unwrap();
        assert_eq!(outcome, MaterializeOutcome::Retired);
        assert!(
            definition_repo
                .get_by_source_ref("generated", &profile.id)
                .await
                .unwrap()
                .is_none(),
            "soft-deleted rows must not resurface via get_by_source_ref"
        );

        // Re-running retire on an already-retired profile is a safe no-op.
        let outcome = materialize_profile(&*definition_repo, &profile).await.unwrap();
        assert_eq!(outcome, MaterializeOutcome::NotApplicable);
    }

    #[tokio::test]
    async fn removing_copilot_from_engines_retires_previously_materialized_assistant() {
        let (_db, profile_repo, definition_repo) = setup().await;
        let profile = create_profile(
            &*profile_repo,
            "ingenieria",
            &["hermes", "copilot"],
            "# Ingeniería",
            "zai/glm-5.1",
        )
        .await;
        materialize_profile(&*definition_repo, &profile).await.unwrap();
        assert!(
            definition_repo
                .get_by_source_ref("generated", &profile.id)
                .await
                .unwrap()
                .is_some()
        );

        profile_repo
            .update(
                &profile.id,
                aionui_db::AgentProfileUpdate {
                    label: None,
                    definition: Some(definition_json(
                        "ingenieria",
                        &["hermes"],
                        "# Ingeniería",
                        "zai/glm-5.1",
                    )),
                    is_active: None,
                },
            )
            .await
            .unwrap();
        let profile = profile_repo.get(&profile.id).await.unwrap().unwrap();

        let outcome = materialize_profile(&*definition_repo, &profile).await.unwrap();
        assert_eq!(outcome, MaterializeOutcome::Retired);
        assert!(
            definition_repo
                .get_by_source_ref("generated", &profile.id)
                .await
                .unwrap()
                .is_none()
        );
    }
}
