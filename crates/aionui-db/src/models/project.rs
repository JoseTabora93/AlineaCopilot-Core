//! Modelos del módulo Proyectos (Alinea Fase 2 #2 — slice 3).
//!
//! La membresía de proyecto NO tiene fila propia: vive en `resource_acl`
//! (resource_type='project'). `ResourceAclRow` la mapea para listar miembros.

use aionui_common::TimestampMs;
use serde::{Deserialize, Serialize};

/// Un proyecto = contenedor de conversaciones/tareas (slices 4-7).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// 'preventa_mep' | 'diseno_mep' | 'generico' — matchea pipeline_templates.
    pub project_type: String,
    /// 'active' | 'archived'.
    pub status: String,
    pub created_by: String,
    pub created_at: TimestampMs,
    pub updated_at: TimestampMs,
}

/// Playbook reusable que Hermes instancia/adapta (slice 6). `definition` es JSON.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct PipelineTemplateRow {
    pub id: String,
    pub name: String,
    pub project_type: String,
    pub version: i64,
    /// JSON: `{"tasks":[{key,title,assignee_kind,requires_human_review,...}]}`.
    pub definition: String,
    pub is_active: bool,
    pub created_at: TimestampMs,
}

/// Una entrada de `resource_acl`. Para Proyectos se usa con
/// resource_type='project'; el `perm` ∈ 'read'|'write'|'owner'.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ResourceAclRow {
    pub id: String,
    pub resource_type: String,
    pub resource_id: String,
    pub principal_type: String,
    pub principal_id: String,
    pub perm: String,
    pub created_at: TimestampMs,
}
