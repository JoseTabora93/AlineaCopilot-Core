//! Modelo de `agent_profiles` (Motor MULTI-PERFIL — plan hermes-alinea, tarea A1).
//!
//! El perfil es DATO: `definition` guarda el JSON completo (esquema PERFIL v1,
//! ver `PROFILE_SCHEMA.md` en la raíz del repo). El Core no interpreta el
//! contenido de `definition` más allá de validarlo contra el esquema al
//! crear/actualizar (`crate::profile_schema`); los compiladores deterministas
//! (tareas A4/A5) son los que lo materializan a config nativa de cada motor.

use aionui_common::TimestampMs;
use serde::{Deserialize, Serialize};

/// Fila de `agent_profiles`.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AgentProfileRow {
    pub id: String,
    /// Slug único y estable (p.ej. 'ingenieria', 'servimec-tko').
    pub name: String,
    /// Nombre visible para humanos.
    pub label: String,
    /// JSON: PERFIL v1 completo. Ver `PROFILE_SCHEMA.md`.
    pub definition: String,
    pub is_active: bool,
    pub created_at: TimestampMs,
    pub updated_at: TimestampMs,
}
