-- Migration 020: Perfiles de agente (Motor MULTI-PERFIL — plan hermes-alinea, tarea A1).
--
-- Un PERFIL es la unidad de configuración de un agente (role-pack como
-- 'ingenieria', experto como 'servimec-tko', o 'preventa'). El registro vive
-- SOLO en el Core; los motores (Hermes/OpenClaw) siguen vanilla — compiladores
-- deterministas (tareas A4/A5) materializan `definition` a config nativa de
-- cada motor. El perfil es DATO; el compilador es código (mismo patrón que
-- `pipeline_templates.definition JSON`, migración 017).
--
-- Esquema JSON de `definition`: ver `PROFILE_SCHEMA.md` en la raíz del repo
-- (PERFIL v1 — pendiente ratificación de José, decisión A0 de Fable).
--
-- Membresía/visibilidad: NO hay tabla nueva. Vive en `resource_acl` (migración
-- 014) con resource_type='agent_profile' (TEXT libre, ya soportado sin
-- migración del ACL). El gate fail-closed de `GET /api/profiles` consulta esta
-- tabla por 'user' directo O por cualquiera de los 'role' del usuario
-- (replica el patrón de `ConversationService::update`,
-- crates/aionui-conversation/src/service.rs:1623-1647).

CREATE TABLE IF NOT EXISTS agent_profiles (
    id           TEXT PRIMARY KEY NOT NULL,
    name         TEXT NOT NULL UNIQUE,   -- slug estable, p.ej. 'ingenieria', 'servimec-tko'
    label        TEXT NOT NULL,          -- nombre visible, p.ej. 'Ingeniería (role-pack)'
    definition   TEXT NOT NULL,          -- JSON: PERFIL v1 completo (ver PROFILE_SCHEMA.md)
    is_active    INTEGER NOT NULL DEFAULT 1,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agent_profiles_active ON agent_profiles(is_active);
