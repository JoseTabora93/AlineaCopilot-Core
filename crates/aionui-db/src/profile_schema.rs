//! Validación del JSON `agent_profiles.definition` contra el esquema PERFIL v1
//! (`PROFILE_SCHEMA.md` en la raíz del repo — decisión A0 de Fable).
//!
//! Deliberadamente NO usa una crate de JSON-schema genérica: el esquema es
//! pequeño, estable y vive en un solo lugar del dominio (este módulo). Rechaza
//! campos desconocidos y valida los invariantes de negocio (`engines`,
//! `caps.period`, `caps.hard_usd >= soft_usd`) con mensajes de error claros
//! para la UI admin.

use serde::Deserialize;
use serde_json::Value;

/// Error de validación del esquema PERFIL v1. El mensaje es apto para mostrar
/// directamente en la UI admin (ya incluye el nombre del campo ofensor).
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct ProfileSchemaError(pub String);

const ENGINES: &[&str] = &["hermes", "openclaw"];
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

/// Estructura fuertemente tipada de PERFIL v1, usada solo para validación
/// estructural (tipos correctos + campos obligatorios presentes). El JSON
/// crudo es lo que se persiste en `agent_profiles.definition` — este struct
/// no se serializa de vuelta a la base de datos.
#[derive(Debug, Deserialize)]
struct ProfileV1 {
    name: String,
    #[allow(dead_code)]
    label: String,
    engines: Vec<String>,
    #[allow(dead_code)]
    soul_md: String,
    model: ProfileModel,
    #[allow(dead_code)]
    mcp_allowlist: Vec<String>,
    #[allow(dead_code)]
    skills: Vec<String>,
    #[allow(dead_code)]
    kb_scope: Vec<String>,
    channels: Vec<ProfileChannel>,
    caps: ProfileCaps,
    #[allow(dead_code)]
    acl: ProfileAcl,
}

#[derive(Debug, Deserialize)]
struct ProfileModel {
    #[allow(dead_code)]
    primary: String,
    #[allow(dead_code)]
    fallbacks: Vec<String>,
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
/// contenido). Devuelve el struct parseado si todo es válido.
pub fn validate_profile_definition(raw: &str, expected_name: &str) -> Result<(), ProfileSchemaError> {
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

    Ok(())
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
}
