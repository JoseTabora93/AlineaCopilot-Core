//! Gate de visibilidad de assistants gestionados (`source='generated'`,
//! motor `"copilot"` — tarea A8) en `GET /api/assistants`.
//!
//! Replica el patrón de `list_visible_profiles`
//! (`crates/aionui-app/src/router/profiles.rs`, tarea A1): un usuario ve el
//! assistant materializado de un perfil solo si tiene un grant en
//! `resource_acl` (resource_type='agent_profile') — directo por `user_id` o
//! por cualquiera de sus roles — o si es admin (los admins ven todo el
//! catálogo activo sin depender de grants explícitos, igual que en
//! `list_visible_profiles`). Los assistants `builtin`/`user` NO pasan por
//! este gate — su visibilidad actual no cambia.

use std::collections::HashMap;

use aionui_api_types::AssistantResponse;
use aionui_auth::CurrentUser;

use crate::error::AssistantError;
use crate::state::ProfileVisibilityGate;

/// Filtra `items` (la lista ya mezclada builtin+user+generated que produce
/// `AssistantService::list`) aplicando el gate de A1 al subconjunto
/// `source='generated'`. Los assistants no gestionados pasan sin cambios.
///
/// `current` es el usuario autenticado (poblado por `auth_middleware`
/// externo — ver `crates/aionui-app/src/router/routes.rs`).
pub async fn filter_visible_assistants(
    gate: &ProfileVisibilityGate,
    current: &CurrentUser,
    items: Vec<AssistantResponse>,
) -> Result<Vec<AssistantResponse>, AssistantError> {
    // Todos los assistants "generated" y no-generated se separan primero:
    // los no-generated (builtin/user, `AssistantSource::Builtin`/`User` en el
    // DTO actual) no pasan por este gate en absoluto.
    let definitions = gate
        .definition_repo
        .list()
        .await
        .map_err(|e| AssistantError::Internal(format!("list assistant definitions for visibility gate: {e}")))?;
    let generated_source_ref_by_key: HashMap<String, String> = definitions
        .into_iter()
        .filter(|d| d.source == "generated")
        .filter_map(|d| d.source_ref.map(|source_ref| (d.assistant_key, source_ref)))
        .collect();

    if generated_source_ref_by_key.is_empty() {
        // Fast path: no hay ningún assistant gestionado en el catálogo —
        // nada que filtrar.
        return Ok(items);
    }

    if current.is_admin() {
        // Mismo criterio que `list_visible_profiles`: los admins ven todo el
        // catálogo activo sin depender de grants explícitos.
        return Ok(items);
    }

    // Perfiles activos con la caché de membresía resuelta una sola vez
    // (evita N consultas ACL por assistant listado cuando varios assistants
    // provienen del mismo perfil — no debería pasar con `assistant_key`
    // único por perfil, pero es defensivo y barato).
    let mut visible_profile_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut checked_profile_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let Some(profile_id) = generated_source_ref_by_key.get(&item.id) else {
            // No es un assistant gestionado (o no tiene source_ref) — pasa
            // sin filtrar.
            result.push(item);
            continue;
        };

        if !checked_profile_ids.contains(profile_id) {
            let visible = is_profile_visible_to_user(gate, current, profile_id).await?;
            checked_profile_ids.insert(profile_id.clone());
            if visible {
                visible_profile_ids.insert(profile_id.clone());
            }
        }

        if visible_profile_ids.contains(profile_id) {
            result.push(item);
        }
    }

    Ok(result)
}

/// `true` si `current` tiene membresía (directa o por rol) sobre el perfil
/// `profile_id` en `resource_acl` (resource_type='agent_profile'). Mismo
/// gate FAIL-CLOSED que `list_visible_profiles`: sin ninguna entrada de
/// membresía, `false`.
async fn is_profile_visible_to_user(
    gate: &ProfileVisibilityGate,
    current: &CurrentUser,
    profile_id: &str,
) -> Result<bool, AssistantError> {
    if gate
        .acl_repo
        .is_member("agent_profile", profile_id, &current.id)
        .await
        .map_err(|e| AssistantError::Internal(format!("check agent_profile membership: {e}")))?
    {
        return Ok(true);
    }
    for role in &current.roles {
        if gate
            .acl_repo
            .is_member("agent_profile", profile_id, role)
            .await
            .map_err(|e| AssistantError::Internal(format!("check agent_profile role membership: {e}")))?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aionui_api_types::AssistantSource;
    use aionui_db::{
        IAgentProfileRepository, IAssistantDefinitionRepository, IResourceAclRepository, NewAgentProfile,
        SqliteAgentProfileRepository, SqliteAssistantDefinitionRepository, SqliteResourceAclRepository,
        init_database_memory,
    };
    use std::sync::Arc;

    fn definition_json(name: &str) -> String {
        serde_json::json!({
            "name": name,
            "label": format!("Label {name}"),
            "engines": ["copilot"],
            "soul_md": "# soul",
            "model": { "primary": "zai/glm-5.1", "fallbacks": [] },
            "mcp_allowlist": [],
            "skills": [],
            "kb_scope": [],
            "channels": [{ "type": "web" }],
            "caps": { "soft_usd": 1.0, "hard_usd": 2.0, "period": "month" },
            "acl": { "etiqueta": "interno", "roles": ["admin"] }
        })
        .to_string()
    }

    fn mk_response(id: &str) -> AssistantResponse {
        AssistantResponse {
            id: id.to_string(),
            source: AssistantSource::User,
            name: id.to_string(),
            name_i18n: Default::default(),
            description: None,
            description_i18n: Default::default(),
            avatar: None,
            enabled: true,
            sort_order: 0,
            preset_agent_type: "aionrs".to_string(),
            enabled_skills: vec![],
            custom_skill_names: vec![],
            disabled_builtin_skills: vec![],
            context: None,
            context_i18n: Default::default(),
            prompts: vec![],
            prompts_i18n: Default::default(),
            models: vec![],
            last_used_at: None,
        }
    }

    async fn setup() -> (
        ProfileVisibilityGate,
        Arc<dyn IAgentProfileRepository>,
        Arc<dyn IAssistantDefinitionRepository>,
        Arc<dyn IResourceAclRepository>,
    ) {
        let db = init_database_memory().await.unwrap();
        let profile_repo: Arc<dyn IAgentProfileRepository> =
            Arc::new(SqliteAgentProfileRepository::new(db.pool().clone()));
        let definition_repo: Arc<dyn IAssistantDefinitionRepository> =
            Arc::new(SqliteAssistantDefinitionRepository::new(db.pool().clone()));
        let acl_repo: Arc<dyn IResourceAclRepository> = Arc::new(SqliteResourceAclRepository::new(db.pool().clone()));
        let gate = ProfileVisibilityGate {
            profile_repo: profile_repo.clone(),
            acl_repo: acl_repo.clone(),
            definition_repo: definition_repo.clone(),
        };
        (gate, profile_repo, definition_repo, acl_repo)
    }

    fn user(id: &str, roles: &[&str]) -> CurrentUser {
        CurrentUser {
            id: id.to_string(),
            username: id.to_string(),
            roles: roles.iter().map(|r| r.to_string()).collect(),
        }
    }

    #[tokio::test]
    async fn user_without_grant_does_not_see_generated_assistant() {
        let (gate, profile_repo, definition_repo, _acl) = setup().await;
        let profile = profile_repo
            .create(NewAgentProfile {
                name: "ingenieria".into(),
                label: "Ingeniería".into(),
                definition: definition_json("ingenieria"),
            })
            .await
            .unwrap();
        crate::profile_compiler::materialize_profile(definition_repo.as_ref(), &profile)
            .await
            .unwrap();

        let items = vec![mk_response("profile:ingenieria")];
        let u = user("u1", &[]);
        let visible = filter_visible_assistants(&gate, &u, items).await.unwrap();
        assert!(
            visible.is_empty(),
            "sin grant, el usuario no debe ver el assistant gestionado"
        );
    }

    #[tokio::test]
    async fn user_with_role_grant_sees_generated_assistant() {
        let (gate, profile_repo, definition_repo, acl_repo) = setup().await;
        let profile = profile_repo
            .create(NewAgentProfile {
                name: "ingenieria".into(),
                label: "Ingeniería".into(),
                definition: definition_json("ingenieria"),
            })
            .await
            .unwrap();
        crate::profile_compiler::materialize_profile(definition_repo.as_ref(), &profile)
            .await
            .unwrap();
        acl_repo
            .grant("agent_profile", &profile.id, "ingenieria", "read")
            .await
            .unwrap();

        let items = vec![mk_response("profile:ingenieria")];
        let u = user("u1", &["ingenieria"]);
        let visible = filter_visible_assistants(&gate, &u, items).await.unwrap();
        assert_eq!(
            visible.len(),
            1,
            "con grant de rol, el usuario debe ver el assistant gestionado"
        );
        assert_eq!(visible[0].id, "profile:ingenieria");
    }

    #[tokio::test]
    async fn admin_sees_generated_assistant_without_explicit_grant() {
        let (gate, profile_repo, definition_repo, _acl) = setup().await;
        let profile = profile_repo
            .create(NewAgentProfile {
                name: "ingenieria".into(),
                label: "Ingeniería".into(),
                definition: definition_json("ingenieria"),
            })
            .await
            .unwrap();
        crate::profile_compiler::materialize_profile(definition_repo.as_ref(), &profile)
            .await
            .unwrap();

        let items = vec![mk_response("profile:ingenieria")];
        let admin = user("admin1", &["admin"]);
        let visible = filter_visible_assistants(&gate, &admin, items).await.unwrap();
        assert_eq!(visible.len(), 1, "admin ve el catálogo sin necesitar grant explícito");
    }

    #[tokio::test]
    async fn non_generated_assistants_are_never_filtered() {
        let (gate, _profile_repo, _definition_repo, _acl) = setup().await;
        let items = vec![mk_response("builtin-office"), mk_response("custom-123-abcd")];
        let u = user("u1", &[]);
        let visible = filter_visible_assistants(&gate, &u, items).await.unwrap();
        assert_eq!(
            visible.len(),
            2,
            "assistants no gestionados no deben pasar por el gate de A1"
        );
    }
}
