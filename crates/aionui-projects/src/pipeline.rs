//! Instanciación de pipelines desde plantillas (Alinea Fase 2 #2 — slice 6).
//!
//! Toma la definición JSON de una `pipeline_template` y crea el árbol de tareas
//! con sus dependencias, dejando 'blocked' las que dependen de otras. Es lo que
//! Hermes invoca (vía REST/MCP) cuando "arma el pipeline" de un proyecto.

use std::collections::HashMap;
use std::sync::Arc;

use aionui_db::models::TaskRow;
use aionui_db::{DbError, IProjectRepository, ITaskRepository, NewTask};
use serde::Deserialize;

use crate::HandoffEngine;

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("template '{0}' not found")]
    TemplateNotFound(String),
    #[error("invalid template definition: {0}")]
    InvalidTemplate(String),
}

/// Spec de una tarea dentro de la definición JSON de una plantilla.
#[derive(Debug, Deserialize)]
struct TaskSpec {
    key: String,
    title: String,
    #[serde(default = "default_human")]
    assignee_kind: String,
    /// 'openclaw' u otro id de agente → `assignee_id` de la tarea de agente.
    #[serde(default)]
    agent: Option<String>,
    #[serde(default = "default_true")]
    requires_human_review: bool,
    #[serde(default)]
    produces_artifact: Option<String>,
    /// KEYS (no ids) de las tareas de las que depende.
    #[serde(default)]
    depends_on: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct TemplateDef {
    tasks: Vec<TaskSpec>,
}

fn default_human() -> String {
    "human".to_string()
}
fn default_true() -> bool {
    true
}

/// Instancia pipelines desde plantillas: JSON → árbol de tareas + dependencias.
#[derive(Clone)]
pub struct PipelineService {
    project_repo: Arc<dyn IProjectRepository>,
    task_repo: Arc<dyn ITaskRepository>,
    handoff: HandoffEngine,
}

impl PipelineService {
    pub fn new(
        project_repo: Arc<dyn IProjectRepository>,
        task_repo: Arc<dyn ITaskRepository>,
        handoff: HandoffEngine,
    ) -> Self {
        Self {
            project_repo,
            task_repo,
            handoff,
        }
    }

    /// Instancia el pipeline de `template_id` en `project_id`. Devuelve las tareas
    /// creadas ya con su estado: 'todo' las iniciales, 'blocked' las que dependen
    /// de otras (la cascada del handoff engine las destrabará al completarse).
    pub async fn instantiate(
        &self,
        project_id: &str,
        template_id: &str,
        created_by: &str,
    ) -> Result<Vec<TaskRow>, PipelineError> {
        let template = self
            .project_repo
            .get_template(template_id)
            .await?
            .ok_or_else(|| PipelineError::TemplateNotFound(template_id.to_string()))?;
        let def: TemplateDef =
            serde_json::from_str(&template.definition).map_err(|e| PipelineError::InvalidTemplate(e.to_string()))?;

        // Paso 1: crear todas las tareas (sin deps todavía), mapear key→id.
        let mut key_to_id: HashMap<String, String> = HashMap::new();
        for (i, spec) in def.tasks.iter().enumerate() {
            let assignee_id = if spec.assignee_kind == "agent" {
                spec.agent.clone()
            } else {
                None
            };
            let task = self
                .task_repo
                .create(NewTask {
                    project_id: project_id.to_string(),
                    parent_task_id: None,
                    title: spec.title.clone(),
                    instructions: None,
                    assignee_kind: spec.assignee_kind.clone(),
                    assignee_id,
                    requires_human_review: spec.requires_human_review,
                    produces_artifact: spec.produces_artifact.clone(),
                    order_index: i as i64,
                    created_by: created_by.to_string(),
                    depends_on: vec![],
                })
                .await?;
            key_to_id.insert(spec.key.clone(), task.id);
        }

        // Paso 2: cablear dependencias (resolver keys → ids).
        for spec in &def.tasks {
            let Some(task_id) = key_to_id.get(&spec.key) else {
                continue;
            };
            for dep_key in &spec.depends_on {
                // Una key inexistente se ignora (plantilla mal formada, no fatal).
                if let Some(dep_id) = key_to_id.get(dep_key) {
                    self.task_repo.add_dependency(task_id, dep_id).await?;
                }
            }
        }

        // Paso 3: bloquear las que nacen con dependencias no satisfechas.
        let mut created = Vec::with_capacity(def.tasks.len());
        for spec in &def.tasks {
            if let Some(task_id) = key_to_id.get(&spec.key) {
                self.handoff.block_if_unsatisfied(task_id).await?;
                if let Some(t) = self.task_repo.get(task_id).await? {
                    created.push(t);
                }
            }
        }
        Ok(created)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aionui_db::{NewProject, SqliteProjectRepository, SqliteTaskRepository, init_database_memory};

    #[tokio::test]
    async fn instantiate_preventa_template() {
        let db = init_database_memory().await.unwrap();
        let project_repo: Arc<dyn IProjectRepository> = Arc::new(SqliteProjectRepository::new(db.pool().clone()));
        let task_repo: Arc<dyn ITaskRepository> = Arc::new(SqliteTaskRepository::new(db.pool().clone()));
        let handoff = HandoffEngine::new(task_repo.clone());
        let svc = PipelineService::new(project_repo.clone(), task_repo.clone(), handoff);

        let project = project_repo
            .create(NewProject {
                name: "Preventa SPS".to_string(),
                description: None,
                project_type: "preventa_mep".to_string(),
                created_by: "u1".to_string(),
            })
            .await
            .unwrap();

        let tasks = svc.instantiate(&project.id, "tpl-preventa-mep", "u1").await.unwrap();

        // La plantilla Preventa tiene 6 tareas.
        assert_eq!(tasks.len(), 6);
        // La primera (levantamiento, sin deps) queda 'todo'; las demás 'blocked'.
        assert_eq!(tasks[0].status, "todo");
        assert!(tasks[1..].iter().all(|t| t.status == "blocked"));
        // Existen las tareas de agente que producen BOM y alcances de obra.
        assert!(
            tasks
                .iter()
                .any(|t| t.produces_artifact.as_deref() == Some("bom") && t.assignee_kind == "agent")
        );
        assert!(
            tasks
                .iter()
                .any(|t| t.produces_artifact.as_deref() == Some("alcances_obra"))
        );
        // El cálculo HVAC es de agente y va a OpenClaw.
        assert!(
            tasks
                .iter()
                .any(|t| t.assignee_kind == "agent" && t.assignee_id.as_deref() == Some("openclaw"))
        );
    }

    #[tokio::test]
    async fn unknown_template_errors() {
        let db = init_database_memory().await.unwrap();
        let project_repo: Arc<dyn IProjectRepository> = Arc::new(SqliteProjectRepository::new(db.pool().clone()));
        let task_repo: Arc<dyn ITaskRepository> = Arc::new(SqliteTaskRepository::new(db.pool().clone()));
        let handoff = HandoffEngine::new(task_repo.clone());
        let svc = PipelineService::new(project_repo, task_repo, handoff);
        let err = svc.instantiate("p1", "nope", "u1").await.unwrap_err();
        assert!(matches!(err, PipelineError::TemplateNotFound(_)));
    }
}
