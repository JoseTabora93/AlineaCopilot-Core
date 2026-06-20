//! Motor de handoffs: la cascada de dependencias del modelo supervisor/ejecutor.
//!
//! Lógica de orquestación pura (sin HTTP): el router (slice 5) y el dispatch de
//! agentes (slice 7) lo invocan tras una transición de tarea.

use std::sync::Arc;

use aionui_db::{DbError, ITaskRepository, NewHandoff, TaskRow};

/// Resultado de procesar el cierre de una tarea.
#[derive(Debug, Default)]
pub struct HandoffOutcome {
    /// Tareas que quedaron destrabadas (blocked → todo) por esta cascada.
    pub unblocked: Vec<TaskRow>,
}

/// Motor de handoffs sobre el repositorio de tareas.
#[derive(Clone)]
pub struct HandoffEngine {
    task_repo: Arc<dyn ITaskRepository>,
}

impl HandoffEngine {
    pub fn new(task_repo: Arc<dyn ITaskRepository>) -> Self {
        Self { task_repo }
    }

    /// Procesa el cierre de `completed_task_id`: destraba en cascada las tareas
    /// dependientes cuyas dependencias YA están todas 'done', y registra los
    /// handoffs (`actor='system'`, `trigger='dependency_satisfied'`). Devuelve las
    /// tareas destrabadas (para notificar/broadcast).
    pub async fn on_task_completed(&self, completed_task_id: &str) -> Result<HandoffOutcome, DbError> {
        let dependents = self.task_repo.dependents_of(completed_task_id).await?;
        let mut outcome = HandoffOutcome::default();
        for dep in dependents {
            // Solo destrabamos tareas 'blocked' cuyas dependencias ya estén satisfechas.
            if dep.status == "blocked" && self.task_repo.dependencies_satisfied(&dep.id).await? {
                self.task_repo.set_status(&dep.id, "todo").await?;
                self.task_repo
                    .log_handoff(NewHandoff {
                        task_id: dep.id.clone(),
                        from_status: Some("blocked".to_string()),
                        to_status: "todo".to_string(),
                        actor: "system".to_string(),
                        trigger_kind: "dependency_satisfied".to_string(),
                        note: None,
                    })
                    .await?;
                if let Some(updated) = self.task_repo.get(&dep.id).await? {
                    outcome.unblocked.push(updated);
                }
            }
        }
        Ok(outcome)
    }

    /// Bloquea una tarea recién creada si tiene dependencias aún no satisfechas
    /// (para que el board la muestre 'blocked' y la cascada la destrabe después).
    /// Devuelve `true` si la bloqueó.
    pub async fn block_if_unsatisfied(&self, task_id: &str) -> Result<bool, DbError> {
        if !self.task_repo.dependencies_satisfied(task_id).await? {
            self.task_repo.set_status(task_id, "blocked").await?;
            self.task_repo
                .log_handoff(NewHandoff {
                    task_id: task_id.to_string(),
                    from_status: Some("todo".to_string()),
                    to_status: "blocked".to_string(),
                    actor: "system".to_string(),
                    trigger_kind: "dependency_satisfied".to_string(),
                    note: Some("created with unmet dependencies".to_string()),
                })
                .await?;
            return Ok(true);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aionui_db::{
        IProjectRepository, NewProject, NewTask, SqliteProjectRepository, SqliteTaskRepository, init_database_memory,
    };

    async fn setup() -> (HandoffEngine, Arc<dyn ITaskRepository>, String) {
        let db = init_database_memory().await.unwrap();
        let projects = SqliteProjectRepository::new(db.pool().clone());
        let p = projects
            .create(NewProject {
                name: "P".to_string(),
                description: None,
                project_type: "generico".to_string(),
                created_by: "u1".to_string(),
            })
            .await
            .unwrap();
        let task_repo: Arc<dyn ITaskRepository> = Arc::new(SqliteTaskRepository::new(db.pool().clone()));
        (HandoffEngine::new(task_repo.clone()), task_repo, p.id)
    }

    fn new_task(project_id: &str, title: &str, depends_on: Vec<String>) -> NewTask {
        NewTask {
            project_id: project_id.to_string(),
            parent_task_id: None,
            title: title.to_string(),
            instructions: None,
            assignee_kind: "human".to_string(),
            assignee_id: None,
            requires_human_review: true,
            produces_artifact: None,
            order_index: 0,
            created_by: "u1".to_string(),
            depends_on,
        }
    }

    #[tokio::test]
    async fn cascade_unblocks_dependent_when_dep_completes() {
        let (engine, repo, pid) = setup().await;
        let a = repo.create(new_task(&pid, "A", vec![])).await.unwrap();
        let b = repo.create(new_task(&pid, "B", vec![a.id.clone()])).await.unwrap();

        // B se bloquea porque A no está done.
        assert!(engine.block_if_unsatisfied(&b.id).await.unwrap());
        assert_eq!(repo.get(&b.id).await.unwrap().unwrap().status, "blocked");

        // Completamos A → la cascada destraba B.
        repo.set_status(&a.id, "done").await.unwrap();
        let outcome = engine.on_task_completed(&a.id).await.unwrap();
        assert_eq!(outcome.unblocked.len(), 1);
        assert_eq!(outcome.unblocked[0].id, b.id);
        assert_eq!(repo.get(&b.id).await.unwrap().unwrap().status, "todo");

        // El handoff quedó registrado en el log.
        let log = repo.list_handoffs(&b.id).await.unwrap();
        assert!(
            log.iter()
                .any(|h| h.to_status == "todo" && h.trigger_kind == "dependency_satisfied")
        );
    }

    #[tokio::test]
    async fn no_cascade_while_other_deps_pending() {
        let (engine, repo, pid) = setup().await;
        let a = repo.create(new_task(&pid, "A", vec![])).await.unwrap();
        let b = repo.create(new_task(&pid, "B", vec![])).await.unwrap();
        // C depende de A y B.
        let c = repo
            .create(new_task(&pid, "C", vec![a.id.clone(), b.id.clone()]))
            .await
            .unwrap();
        engine.block_if_unsatisfied(&c.id).await.unwrap();

        // Solo A done → C sigue bloqueada (falta B).
        repo.set_status(&a.id, "done").await.unwrap();
        let outcome = engine.on_task_completed(&a.id).await.unwrap();
        assert_eq!(outcome.unblocked.len(), 0);
        assert_eq!(repo.get(&c.id).await.unwrap().unwrap().status, "blocked");

        // Ahora B done → C se destraba.
        repo.set_status(&b.id, "done").await.unwrap();
        let outcome = engine.on_task_completed(&b.id).await.unwrap();
        assert_eq!(outcome.unblocked.len(), 1);
        assert_eq!(repo.get(&c.id).await.unwrap().unwrap().status, "todo");
    }
}
