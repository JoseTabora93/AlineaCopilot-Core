//! Validación del JSON `agent_profiles.definition` contra el esquema PERFIL v1
//! (`PROFILE_SCHEMA.md` en la raíz del repo — decisión A0 de Fable).
//!
//! Deliberadamente NO usa una crate de JSON-schema genérica: el esquema es
//! pequeño, estable y vive en un solo lugar del dominio (este módulo). Rechaza
//! campos desconocidos y valida los invariantes de negocio (`engines`,
//! `caps.period`, `caps.hard_usd >= soft_usd`) con mensajes de error claros
//! para la UI admin.
//!
//! `engines` acepta `"hermes"` (compilador tarea A4), `"openclaw"`
//! (compilador tarea A5) y `"copilot"` (compilador tarea A8 —
//! `aionui_assistant::profile_compiler`, materializa el perfil como
//! `assistant_definitions.source='generated'` dentro del propio Core).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Error de validación del esquema PERFIL v1. El mensaje es apto para mostrar
/// directamente en la UI admin (ya incluye el nombre del campo ofensor).
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct ProfileSchemaError(pub String);

const ENGINES: &[&str] = &["hermes", "openclaw", "copilot"];
const PERIODS: &[&str] = &["day", "week", "month"];

/// Campos de nivel superior reconocidos por PERFIL v1. Cualquier otra clave en
/// el objeto raíz se rechaza (evita drift silencioso entre lo que la UI manda
/// y lo que los compiladores esperan).
const TOP_LEVEL_FIELDS: &[&str] = &[
    "name",
    "label",
    "engines",
    "soul_md",
    "model",
    "mcp_allowlist",
    "skills",
    "kb_scope",
    "channels",
    "caps",
    "acl",
];
const MODEL_FIELDS: &[&str] = &["primary", "fallbacks"];
const CAPS_FIELDS: &[&str] = &["soft_usd", "hard_usd", "period"];
const ACL_FIELDS: &[&str] = &["etiqueta", "roles"];
const CHANNEL_FIELDS: &[&str] = &["type", "binding"];

/// Estructura fuertemente tipada de PERFIL v1, usada para validación
/// estructural (tipos correctos + campos obligatorios presentes) y,
/// públicamente, como el modelo que consumen los compiladores deterministas
/// (tarea A8: `aionui_assistant::profile_compiler` la lee vía
/// [`parse_profile_definition`] para materializar el assistant `"copilot"`).
/// El JSON crudo sigue siendo lo que se persiste en `agent_profiles.definition`
/// — este struct no se serializa de vuelta a la base de datos.
#[derive(Debug, Deserialize)]
pub struct ProfileV1 {
    pub name: String,
    pub label: String,
    pub engines: Vec<String>,
    pub soul_md: String,
    pub model: ProfileModel,
    pub mcp_allowlist: Vec<String>,
    pub skills: Vec<String>,
    #[allow(dead_code)]
    kb_scope: Vec<String>,
    channels: Vec<ProfileChannel>,
    caps: ProfileCaps,
    #[allow(dead_code)]
    acl: ProfileAcl,
}

#[derive(Debug, Deserialize)]
pub struct ProfileModel {
    pub primary: String,
    pub fallbacks: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ProfileChannel {
    #[serde(rename = "type")]
    kind: String,
    #[allow(dead_code)]
    binding: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProfileCaps {
    soft_usd: f64,
    hard_usd: f64,
    period: String,
}

#[derive(Debug, Deserialize)]
struct ProfileAcl {
    #[allow(dead_code)]
    etiqueta: String,
    #[allow(dead_code)]
    roles: Vec<String>,
}

/// Verifica que un objeto JSON solo contenga las claves de `allowed`. Devuelve
/// el nombre del primer campo desconocido encontrado, si lo hay.
fn reject_unknown_fields(obj: &Value, allowed: &[&str], context: &str) -> Result<(), ProfileSchemaError> {
    let Some(map) = obj.as_object() else {
        return Err(ProfileSchemaError(format!("'{context}' must be a JSON object")));
    };
    for key in map.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(ProfileSchemaError(format!(
                "definition contains unknown field '{context}.{key}'"
            )));
        }
    }
    Ok(())
}

/// Valida `raw` (JSON crudo de `agent_profiles.definition`) contra el esquema
/// PERFIL v1. `expected_name` es el `name` de la fila/columna — debe coincidir
/// con `definition.name` (evita drift entre la clave de la tabla y el
/// contenido).
pub fn validate_profile_definition(raw: &str, expected_name: &str) -> Result<(), ProfileSchemaError> {
    parse_profile_definition(raw, expected_name).map(|_| ())
}

/// Valida `raw` igual que [`validate_profile_definition`] y además devuelve el
/// struct [`ProfileV1`] ya parseado. Usado por los compiladores deterministas
/// (tarea A8: `aionui_assistant::profile_compiler`) que necesitan leer
/// `soul_md`, `model.primary`, `skills`, `mcp_allowlist`, etc. sin duplicar la
/// validación de esquema.
pub fn parse_profile_definition(raw: &str, expected_name: &str) -> Result<ProfileV1, ProfileSchemaError> {
    let value: Value =
        serde_json::from_str(raw).map_err(|e| ProfileSchemaError(format!("definition is not valid JSON: {e}")))?;

    reject_unknown_fields(&value, TOP_LEVEL_FIELDS, "definition")?;
    if let Some(model) = value.get("model") {
        reject_unknown_fields(model, MODEL_FIELDS, "definition.model")?;
    }
    if let Some(caps) = value.get("caps") {
        reject_unknown_fields(caps, CAPS_FIELDS, "definition.caps")?;
    }
    if let Some(acl) = value.get("acl") {
        reject_unknown_fields(acl, ACL_FIELDS, "definition.acl")?;
    }
    if let Some(channels) = value.get("channels").and_then(Value::as_array) {
        for ch in channels {
            reject_unknown_fields(ch, CHANNEL_FIELDS, "definition.channels[]")?;
        }
    }

    let profile: ProfileV1 = serde_json::from_value(value)
        .map_err(|e| ProfileSchemaError(format!("definition missing/invalid required field: {e}")))?;

    if profile.name != expected_name {
        return Err(ProfileSchemaError(format!(
            "definition.name ('{}') must match the profile's name ('{}')",
            profile.name, expected_name
        )));
    }

    if profile.engines.is_empty() {
        return Err(ProfileSchemaError("definition.engines must not be empty".to_string()));
    }
    for engine in &profile.engines {
        if !ENGINES.contains(&engine.as_str()) {
            return Err(ProfileSchemaError(format!(
                "definition.engines contains invalid value '{engine}' (must be one of: {})",
                ENGINES.join(", ")
            )));
        }
    }

    for ch in &profile.channels {
        if ch.kind.trim().is_empty() {
            return Err(ProfileSchemaError(
                "definition.channels[].type must not be empty".to_string(),
            ));
        }
    }

    if !PERIODS.contains(&profile.caps.period.as_str()) {
        return Err(ProfileSchemaError(format!(
            "definition.caps.period must be one of: {}",
            PERIODS.join(", ")
        )));
    }
    if profile.caps.soft_usd < 0.0 || profile.caps.hard_usd < 0.0 {
        return Err(ProfileSchemaError(
            "definition.caps.soft_usd and hard_usd must be non-negative".to_string(),
        ));
    }
    if profile.caps.hard_usd < profile.caps.soft_usd {
        return Err(ProfileSchemaError(
            "definition.caps.hard_usd must be >= soft_usd".to_string(),
        ));
    }
    if profile.model.primary.trim().is_empty() {
        return Err(ProfileSchemaError(
            "definition.model.primary must not be empty".to_string(),
        ));
    }

    Ok(profile)
}

/// Extrae `definition.mcp_allowlist` de un `agent_profiles.definition` YA
/// validado (tarea A2 — emisión del token de identidad con perfil,
/// `crates/aionui-ai-agent/src/factory/acp.rs`).
///
/// Deliberadamente NO reutiliza `ProfileV1` (privado a este módulo, sus
/// campos son `#[allow(dead_code)]` porque hoy solo se usan para validar
/// estructura) — este parseo es de solo lectura y tolerante: si `raw` no es
/// JSON válido o `mcp_allowlist` falta/tiene el tipo incorrecto, devuelve
/// `Vec::new()` en lugar de propagar un error. La fila ya pasó
/// `validate_profile_definition` al crearse/actualizarse, así que esto solo
/// protege contra un futuro drift de esquema sin tumbar la emisión del token
/// (fail-closed en la SEMÁNTICA de scopes — lista vacía sigue siendo
/// deny-by-default — no en la disponibilidad del agente).
pub fn extract_mcp_allowlist(raw: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    value
        .get("mcp_allowlist")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

/// `definition.caps` YA validado, expuesto al enforcement de techo de gasto
/// por perfil (Fase perfiles — tarea C4) y al endpoint `GET /api/profiles`
/// (para que el pipeline de preventa lea `caps.hard_usd` sin re-parsear el
/// `definition` crudo). Serializable directo a la respuesta JSON del API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileCapsView {
    pub soft_usd: f64,
    pub hard_usd: f64,
    pub period: String,
}

/// Extrae `definition.caps` de un `agent_profiles.definition` YA validado
/// (tarea C4). Igual de tolerante que `extract_mcp_allowlist`: si `raw` no es
/// JSON válido o `caps` falta/tiene el tipo incorrecto, devuelve `None` en
/// vez de propagar un error — un perfil sin `caps` bien formado se trata como
/// "sin límite de perfil" (solo aplica el límite de usuario), nunca bloquea
/// la sesión por un problema de parseo.
pub fn extract_caps(raw: &str) -> Option<ProfileCapsView> {
    let value = serde_json::from_str::<Value>(raw).ok()?;
    let caps = value.get("caps")?;
    let soft_usd = caps.get("soft_usd")?.as_f64()?;
    let hard_usd = caps.get("hard_usd")?.as_f64()?;
    let period = caps.get("period")?.as_str()?.to_owned();
    Some(ProfileCapsView {
        soft_usd,
        hard_usd,
        period,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_json(name: &str) -> String {
        format!(
            r##"{{
                "name": "{name}",
                "label": "Test",
                "engines": ["hermes"],
                "soul_md": "# Test",
                "model": {{ "primary": "zai/glm-5.1", "fallbacks": [] }},
                "mcp_allowlist": [],
                "skills": [],
                "kb_scope": [],
                "channels": [{{ "type": "web" }}],
                "caps": {{ "soft_usd": 1.0, "hard_usd": 2.0, "period": "month" }},
                "acl": {{ "etiqueta": "interno", "roles": ["admin"] }}
            }}"##
        )
    }

    #[test]
    fn accepts_valid_profile() {
        let json = valid_json("ingenieria");
        assert!(validate_profile_definition(&json, "ingenieria").is_ok());
    }

    #[test]
    fn rejects_invalid_json() {
        let err = validate_profile_definition("{not json", "x").unwrap_err();
        assert!(err.0.contains("not valid JSON"));
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let json = r##"{
            "name": "x", "label": "X", "engines": ["hermes"], "soul_md": "s",
            "model": {"primary": "m", "fallbacks": []}, "mcp_allowlist": [], "skills": [],
            "kb_scope": [], "channels": [], "caps": {"soft_usd":1.0,"hard_usd":2.0,"period":"month"},
            "acl": {"etiqueta":"interno","roles":[]},
            "totally_unknown_field": true
        }"##;
        let err = validate_profile_definition(json, "x").unwrap_err();
        assert!(err.0.contains("unknown field"), "got: {}", err.0);
        assert!(err.0.contains("totally_unknown_field"));
    }

    #[test]
    fn rejects_unknown_nested_field() {
        let json = r##"{
            "name": "x", "label": "X", "engines": ["hermes"], "soul_md": "s",
            "model": {"primary": "m", "fallbacks": [], "sneaky": 1}, "mcp_allowlist": [], "skills": [],
            "kb_scope": [], "channels": [], "caps": {"soft_usd":1.0,"hard_usd":2.0,"period":"month"},
            "acl": {"etiqueta":"interno","roles":[]}
        }"##;
        let err = validate_profile_definition(json, "x").unwrap_err();
        assert!(err.0.contains("definition.model.sneaky"), "got: {}", err.0);
    }

    #[test]
    fn rejects_name_mismatch() {
        let json = valid_json("ingenieria");
        let err = validate_profile_definition(&json, "otro").unwrap_err();
        assert!(err.0.contains("must match"));
    }

    #[test]
    fn rejects_invalid_engine() {
        let json = r##"{
            "name": "x", "label": "X", "engines": ["not-a-real-engine"], "soul_md": "s",
            "model": {"primary": "m", "fallbacks": []}, "mcp_allowlist": [], "skills": [],
            "kb_scope": [], "channels": [], "caps": {"soft_usd":1.0,"hard_usd":2.0,"period":"month"},
            "acl": {"etiqueta":"interno","roles":[]}
        }"##;
        let err = validate_profile_definition(json, "x").unwrap_err();
        assert!(err.0.contains("invalid value"));
    }

    #[test]
    fn accepts_copilot_engine_alone_and_combined() {
        let json = r##"{
            "name": "x", "label": "X", "engines": ["copilot"], "soul_md": "s",
            "model": {"primary": "m", "fallbacks": []}, "mcp_allowlist": [], "skills": [],
            "kb_scope": [], "channels": [], "caps": {"soft_usd":1.0,"hard_usd":2.0,"period":"month"},
            "acl": {"etiqueta":"interno","roles":[]}
        }"##;
        assert!(validate_profile_definition(json, "x").is_ok());

        let combined = r##"{
            "name": "x", "label": "X", "engines": ["hermes", "openclaw", "copilot"], "soul_md": "s",
            "model": {"primary": "m", "fallbacks": []}, "mcp_allowlist": [], "skills": [],
            "kb_scope": [], "channels": [], "caps": {"soft_usd":1.0,"hard_usd":2.0,"period":"month"},
            "acl": {"etiqueta":"interno","roles":[]}
        }"##;
        assert!(validate_profile_definition(combined, "x").is_ok());
    }

    #[test]
    fn rejects_empty_engines() {
        let json = r##"{
            "name": "x", "label": "X", "engines": [], "soul_md": "s",
            "model": {"primary": "m", "fallbacks": []}, "mcp_allowlist": [], "skills": [],
            "kb_scope": [], "channels": [], "caps": {"soft_usd":1.0,"hard_usd":2.0,"period":"month"},
            "acl": {"etiqueta":"interno","roles":[]}
        }"##;
        let err = validate_profile_definition(json, "x").unwrap_err();
        assert!(err.0.contains("must not be empty"));
    }

    #[test]
    fn rejects_invalid_period() {
        let json = r##"{
            "name": "x", "label": "X", "engines": ["hermes"], "soul_md": "s",
            "model": {"primary": "m", "fallbacks": []}, "mcp_allowlist": [], "skills": [],
            "kb_scope": [], "channels": [], "caps": {"soft_usd":1.0,"hard_usd":2.0,"period":"fortnight"},
            "acl": {"etiqueta":"interno","roles":[]}
        }"##;
        let err = validate_profile_definition(json, "x").unwrap_err();
        assert!(err.0.contains("caps.period"));
    }

    #[test]
    fn rejects_hard_below_soft() {
        let json = r##"{
            "name": "x", "label": "X", "engines": ["hermes"], "soul_md": "s",
            "model": {"primary": "m", "fallbacks": []}, "mcp_allowlist": [], "skills": [],
            "kb_scope": [], "channels": [], "caps": {"soft_usd":5.0,"hard_usd":2.0,"period":"month"},
            "acl": {"etiqueta":"interno","roles":[]}
        }"##;
        let err = validate_profile_definition(json, "x").unwrap_err();
        assert!(err.0.contains("hard_usd must be >= soft_usd"));
    }

    #[test]
    fn rejects_missing_required_field() {
        let json = r##"{
            "name": "x", "label": "X", "engines": ["hermes"], "soul_md": "s",
            "mcp_allowlist": [], "skills": [], "kb_scope": [], "channels": [],
            "caps": {"soft_usd":1.0,"hard_usd":2.0,"period":"month"},
            "acl": {"etiqueta":"interno","roles":[]}
        }"##;
        let err = validate_profile_definition(json, "x").unwrap_err();
        assert!(err.0.contains("missing/invalid required field"));
    }

    #[test]
    fn accepts_all_three_example_profiles_from_schema_doc() {
        let ingenieria = r##"{
            "name": "ingenieria",
            "label": "Ingeniería (role-pack)",
            "engines": ["hermes", "openclaw"],
            "soul_md": "# Ingeniería Ingelmec",
            "model": { "primary": "zai/glm-5.1", "fallbacks": ["openrouter/anthropic/claude-haiku-4-5"] },
            "mcp_allowlist": ["ingelmec-kb", "zoho-mail", "dxf-takeoff", "hvac-calc"],
            "skills": ["ingenieria/electrico", "ingenieria/hvac", "ingenieria/fire", "ingenieria/data", "alcance-bom"],
            "kb_scope": ["normas", "boletas", "proyectos-tecnicos"],
            "channels": [{ "type": "web" }, { "type": "telegram", "binding": "per-user" }],
            "caps": { "soft_usd": 5.0, "hard_usd": 10.0, "period": "month" },
            "acl": { "etiqueta": "interno", "roles": ["ingenieria", "admin"] }
        }"##;
        assert!(validate_profile_definition(ingenieria, "ingenieria").is_ok());

        let tko = r##"{
            "name": "servimec-tko",
            "label": "ServiMec — Soporte técnico TKO",
            "engines": ["hermes"],
            "soul_md": "# ServiMec TKO",
            "model": { "primary": "zai/glm-5.1", "fallbacks": ["openrouter/anthropic/claude-haiku-4-5", "openrouter/qwen/qwen3-coder"] },
            "mcp_allowlist": ["ingelmec-kb"],
            "skills": [],
            "kb_scope": ["boletas", "tko"],
            "channels": [{ "type": "telegram", "binding": "per-user" }],
            "caps": { "soft_usd": 3.0, "hard_usd": 6.0, "period": "month" },
            "acl": { "etiqueta": "interno", "roles": ["tecnica", "admin"] }
        }"##;
        assert!(validate_profile_definition(tko, "servimec-tko").is_ok());

        let preventa = r##"{
            "name": "preventa",
            "label": "Preventa MEP",
            "engines": ["openclaw"],
            "soul_md": "# Preventa MEP",
            "model": { "primary": "zai/glm-5.1", "fallbacks": ["openrouter/anthropic/claude-haiku-4-5"] },
            "mcp_allowlist": ["ingelmec-kb", "zoho-mail", "dxf-takeoff", "docgen"],
            "skills": ["alcance-bom", "ingenieria/electrico", "ingenieria/hvac"],
            "kb_scope": ["normas", "biblioteca-lineas", "proyectos-comerciales"],
            "channels": [{ "type": "web" }],
            "caps": { "soft_usd": 4.0, "hard_usd": 8.0, "period": "month" },
            "acl": { "etiqueta": "interno", "roles": ["comercial", "tecnica", "admin"] }
        }"##;
        assert!(validate_profile_definition(preventa, "preventa").is_ok());
    }

    // Tarea A2 — extracción de mcp_allowlist para poblar scopes del token.
    #[test]
    fn extract_mcp_allowlist_returns_configured_servers() {
        let json = valid_json("ingenieria").replace(
            r#""mcp_allowlist": [],"#,
            r#""mcp_allowlist": ["ingelmec-kb", "zoho-mail"],"#,
        );
        assert_eq!(
            extract_mcp_allowlist(&json),
            vec!["ingelmec-kb".to_string(), "zoho-mail".to_string()]
        );
    }

    #[test]
    fn extract_mcp_allowlist_empty_when_missing_or_invalid() {
        assert_eq!(extract_mcp_allowlist("{not json"), Vec::<String>::new());
        assert_eq!(extract_mcp_allowlist(r#"{"other":1}"#), Vec::<String>::new());
        assert_eq!(extract_mcp_allowlist(&valid_json("x")), Vec::<String>::new());
    }

    // Tarea C4 — extracción de `caps` para el pre-flight de techo por perfil
    // y para `GET /api/profiles`.
    #[test]
    fn extract_caps_returns_configured_thresholds() {
        let json = valid_json("ingenieria").replace(
            r#""caps": { "soft_usd": 1.0, "hard_usd": 2.0, "period": "month" },"#,
            r#""caps": { "soft_usd": 5.0, "hard_usd": 10.0, "period": "month" },"#,
        );
        let caps = extract_caps(&json).expect("caps present");
        assert_eq!(caps.soft_usd, 5.0);
        assert_eq!(caps.hard_usd, 10.0);
        assert_eq!(caps.period, "month");
    }

    #[test]
    fn extract_caps_none_when_missing_or_invalid() {
        assert_eq!(extract_caps("{not json"), None);
        assert_eq!(extract_caps(r#"{"other":1}"#), None);
        assert_eq!(extract_caps(r#"{"caps": {"soft_usd": 1.0}}"#), None);
        assert_eq!(extract_caps(r#"{"caps": "not-an-object"}"#), None);
    }
}
