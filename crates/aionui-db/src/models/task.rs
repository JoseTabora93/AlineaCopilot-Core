//! Modelos de tareas/subtareas (Alinea Fase 2 #2 — slice 4).

use aionui_common::TimestampMs;
use serde::{Deserialize, Serialize};

/// Una tarea (o subtarea, si `parent_task_id` es `Some`) de un proyecto.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskRow {
    pub id: String,
    pub project_id: String,
    /// `Some` = es subtarea de esa tarea padre.
    pub parent_task_id: Option<String>,
    pub title: String,
    pub instructions: Option<String>,
    /// 'human' | 'agent'.
    pub assignee_kind: String,
    pub assignee_id: Option<String>,
    /// todo | in_progress | in_review | done | rejected | blocked.
    pub status: String,
    pub requires_human_review: bool,
    /// 'bom' | 'alcances_obra' | 'doc' | ... (artefacto terminal esperado).
    pub produces_artifact: Option<String>,
    pub order_index: i64,
    /// Liga la conversación que ejecuta una tarea de agente (slice 7).
    pub conversation_id: Option<String>,
    pub created_by: String,
    pub created_at: TimestampMs,
    pub updated_at: TimestampMs,
    pub started_at: Option<TimestampMs>,
    pub completed_at: Option<TimestampMs>,
}

/// Entregable producido por una tarea (BOM, alcances de obra, doc, archivo…).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskArtifactRow {
    pub id: String,
    pub task_id: String,
    /// 'bom' | 'alcances_obra' | 'doc' | 'file' | 'link' | 'conversation_ref'.
    pub kind: String,
    pub uri: String,
    pub title: String,
    pub produced_by: String,
    pub created_at: TimestampMs,
}

/// Entrada de auditoría de un handoff/transición (modelo supervisor/ejecutor).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskHandoffLogRow {
    pub id: String,
    pub task_id: String,
    pub from_status: Option<String>,
    pub to_status: String,
    /// 'system' | 'hermes' | 'openclaw' | user_id.
    pub actor: String,
    /// 'manual' | 'dependency_satisfied' | 'agent_completed' | 'approval' | 'rejection'.
    pub trigger_kind: String,
    pub note: Option<String>,
    pub created_at: TimestampMs,
}
