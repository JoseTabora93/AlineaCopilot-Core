//! Repositorio de tareas/subtareas (Alinea Fase 2 #2 — slice 4).
//!
//! Persistencia pura. La validación de transiciones vive en `crate::task_state`
//! (lógica pura); el handoff engine (cascada por dependencias + dispatch a
//! agentes) llega en slices 5-7.

use crate::DbError;
use crate::models::TaskRow;

/// Parámetros para crear una tarea (o subtarea, si `parent_task_id` es `Some`).
pub struct NewTask {
    pub project_id: String,
    pub parent_task_id: Option<String>,
    pub title: String,
    pub instructions: Option<String>,
    pub assignee_kind: String,
    pub assignee_id: Option<String>,
    pub requires_human_review: bool,
    pub produces_artifact: Option<String>,
    pub order_index: i64,
    pub created_by: String,
    /// IDs de tareas de las que esta depende (se insertan en task_dependencies).
    pub depends_on: Vec<String>,
}

/// Actualización parcial de los campos editables de una tarea (NO el estado:
/// el estado solo cambia vía `set_status`, validado por la máquina de estados).
#[derive(Default)]
pub struct TaskUpdate {
    pub title: Option<String>,
    pub instructions: Option<Option<String>>,
    pub assignee_kind: Option<String>,
    pub assignee_id: Option<Option<String>>,
    pub requires_human_review: Option<bool>,
    pub order_index: Option<i64>,
}

#[async_trait::async_trait]
pub trait ITaskRepository: Send + Sync {
    /// Inserta la tarea + sus dependencias (atómico). Status inicial 'todo'.
    async fn create(&self, params: NewTask) -> Result<TaskRow, DbError>;

    async fn get(&self, id: &str) -> Result<Option<TaskRow>, DbError>;

    /// Todas las tareas del proyecto (para el board), ordenadas por
    /// (parent, order_index). Incluye subtareas.
    async fn list_by_project(&self, project_id: &str) -> Result<Vec<TaskRow>, DbError>;

    async fn list_subtasks(&self, parent_task_id: &str) -> Result<Vec<TaskRow>, DbError>;

    /// Actualización parcial de campos editables. `NotFound` si no existe.
    async fn update(&self, id: &str, update: TaskUpdate) -> Result<(), DbError>;

    /// Persiste el nuevo estado + timestamps (started_at al entrar en progreso,
    /// completed_at al terminar). `NotFound` si no existe.
    async fn set_status(&self, id: &str, status: &str) -> Result<(), DbError>;

    /// IDs de las tareas de las que `task_id` depende.
    async fn dependencies(&self, task_id: &str) -> Result<Vec<String>, DbError>;

    /// `true` si TODAS las dependencias de `task_id` están en estado 'done'
    /// (o no tiene dependencias).
    async fn dependencies_satisfied(&self, task_id: &str) -> Result<bool, DbError>;

    /// `(total, done)` de subtareas directas de `parent_task_id` (para rollup).
    async fn subtask_rollup(&self, parent_task_id: &str) -> Result<(i64, i64), DbError>;
}
