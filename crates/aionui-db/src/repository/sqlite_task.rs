//! Implementación SQLite de `ITaskRepository`.

use aionui_common::{generate_id, now_ms};
use sqlx::SqlitePool;

use crate::DbError;
use crate::models::TaskRow;
use crate::repository::task::{ITaskRepository, NewTask, TaskUpdate};

pub struct SqliteTaskRepository {
    pool: SqlitePool,
}

impl SqliteTaskRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ITaskRepository for SqliteTaskRepository {
    async fn create(&self, params: NewTask) -> Result<TaskRow, DbError> {
        let id = generate_id();
        let now = now_ms();

        // Atómico: tarea + dependencias en una transacción.
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO tasks \
               (id, project_id, parent_task_id, title, instructions, assignee_kind, assignee_id, \
                status, requires_human_review, produces_artifact, order_index, created_by, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, 'todo', ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&params.project_id)
        .bind(&params.parent_task_id)
        .bind(&params.title)
        .bind(&params.instructions)
        .bind(&params.assignee_kind)
        .bind(&params.assignee_id)
        .bind(params.requires_human_review)
        .bind(&params.produces_artifact)
        .bind(params.order_index)
        .bind(&params.created_by)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        for dep in &params.depends_on {
            sqlx::query("INSERT OR IGNORE INTO task_dependencies (task_id, depends_on_task_id) VALUES (?, ?)")
                .bind(&id)
                .bind(dep)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;

        self.get(&id)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("Task '{id}' not found after insert")))
    }

    async fn get(&self, id: &str) -> Result<Option<TaskRow>, DbError> {
        let row = sqlx::query_as::<_, TaskRow>("SELECT * FROM tasks WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    async fn list_by_project(&self, project_id: &str) -> Result<Vec<TaskRow>, DbError> {
        let rows = sqlx::query_as::<_, TaskRow>(
            "SELECT * FROM tasks WHERE project_id = ? \
             ORDER BY COALESCE(parent_task_id, id), order_index ASC, created_at ASC",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn list_subtasks(&self, parent_task_id: &str) -> Result<Vec<TaskRow>, DbError> {
        let rows = sqlx::query_as::<_, TaskRow>(
            "SELECT * FROM tasks WHERE parent_task_id = ? ORDER BY order_index ASC, created_at ASC",
        )
        .bind(parent_task_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn update(&self, id: &str, update: TaskUpdate) -> Result<(), DbError> {
        let now = now_ms();
        let mut clauses: Vec<&str> = Vec::new();
        if update.title.is_some() {
            clauses.push("title = ?");
        }
        if update.instructions.is_some() {
            clauses.push("instructions = ?");
        }
        if update.assignee_kind.is_some() {
            clauses.push("assignee_kind = ?");
        }
        if update.assignee_id.is_some() {
            clauses.push("assignee_id = ?");
        }
        if update.requires_human_review.is_some() {
            clauses.push("requires_human_review = ?");
        }
        if update.order_index.is_some() {
            clauses.push("order_index = ?");
        }
        if clauses.is_empty() {
            return Ok(());
        }
        let sql = format!("UPDATE tasks SET {}, updated_at = ? WHERE id = ?", clauses.join(", "));
        let mut q = sqlx::query(&sql);
        if let Some(title) = update.title {
            q = q.bind(title);
        }
        if let Some(instructions) = update.instructions {
            q = q.bind(instructions);
        }
        if let Some(assignee_kind) = update.assignee_kind {
            q = q.bind(assignee_kind);
        }
        if let Some(assignee_id) = update.assignee_id {
            q = q.bind(assignee_id);
        }
        if let Some(requires_human_review) = update.requires_human_review {
            q = q.bind(requires_human_review);
        }
        if let Some(order_index) = update.order_index {
            q = q.bind(order_index);
        }
        q = q.bind(now).bind(id);
        let res = q.execute(&self.pool).await?;
        if res.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("Task '{id}' not found")));
        }
        Ok(())
    }

    async fn set_status(&self, id: &str, status: &str) -> Result<(), DbError> {
        let now = now_ms();
        // started_at se fija la primera vez que entra en progreso; completed_at al
        // terminar (done/rejected). Idempotente respecto a started_at.
        let res = sqlx::query(
            "UPDATE tasks SET status = ?, updated_at = ?, \
                started_at = CASE WHEN ? = 'in_progress' AND started_at IS NULL THEN ? ELSE started_at END, \
                completed_at = CASE WHEN ? IN ('done','rejected') THEN ? ELSE completed_at END \
             WHERE id = ?",
        )
        .bind(status)
        .bind(now)
        .bind(status)
        .bind(now)
        .bind(status)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("Task '{id}' not found")));
        }
        Ok(())
    }

    async fn dependencies(&self, task_id: &str) -> Result<Vec<String>, DbError> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT depends_on_task_id FROM task_dependencies WHERE task_id = ?")
            .bind(task_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(d,)| d).collect())
    }

    async fn dependencies_satisfied(&self, task_id: &str) -> Result<bool, DbError> {
        // Cuenta dependencias que aún NO están 'done'. 0 = satisfechas.
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM task_dependencies d \
             JOIN tasks t ON t.id = d.depends_on_task_id \
             WHERE d.task_id = ? AND t.status != 'done'",
        )
        .bind(task_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0 == 0)
    }

    async fn subtask_rollup(&self, parent_task_id: &str) -> Result<(i64, i64), DbError> {
        let row: (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(CASE WHEN status = 'done' THEN 1 ELSE 0 END), 0) \
             FROM tasks WHERE parent_task_id = ?",
        )
        .bind(parent_task_id)
        .fetch_one(&self.pool)
        .await?;
        Ok((row.0, row.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::project::{IProjectRepository, NewProject};
    use crate::repository::sqlite_project::SqliteProjectRepository;
    use crate::{TaskUpdate, init_database_memory};

    async fn setup() -> (SqliteTaskRepository, String) {
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
        (SqliteTaskRepository::new(db.pool().clone()), p.id)
    }

    fn new_task(project_id: &str, title: &str) -> NewTask {
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
            depends_on: vec![],
        }
    }

    #[tokio::test]
    async fn create_get_and_list() {
        let (repo, pid) = setup().await;
        let t = repo.create(new_task(&pid, "Tarea 1")).await.unwrap();
        assert_eq!(t.status, "todo");
        assert_eq!(t.title, "Tarea 1");
        assert_eq!(repo.get(&t.id).await.unwrap().unwrap().id, t.id);
        assert_eq!(repo.list_by_project(&pid).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn subtasks_and_rollup() {
        let (repo, pid) = setup().await;
        let parent = repo.create(new_task(&pid, "Padre")).await.unwrap();
        let mut s1 = new_task(&pid, "Sub 1");
        s1.parent_task_id = Some(parent.id.clone());
        let mut s2 = new_task(&pid, "Sub 2");
        s2.parent_task_id = Some(parent.id.clone());
        let c1 = repo.create(s1).await.unwrap();
        let c2 = repo.create(s2).await.unwrap();
        assert_eq!(repo.list_subtasks(&parent.id).await.unwrap().len(), 2);
        assert_eq!(repo.subtask_rollup(&parent.id).await.unwrap(), (2, 0));
        repo.set_status(&c1.id, "done").await.unwrap();
        assert_eq!(repo.subtask_rollup(&parent.id).await.unwrap(), (2, 1));
        repo.set_status(&c2.id, "done").await.unwrap();
        assert_eq!(repo.subtask_rollup(&parent.id).await.unwrap(), (2, 2));
    }

    #[tokio::test]
    async fn dependencies_gating() {
        let (repo, pid) = setup().await;
        let dep = repo.create(new_task(&pid, "Dep")).await.unwrap();
        let mut t = new_task(&pid, "Dependiente");
        t.depends_on = vec![dep.id.clone()];
        let task = repo.create(t).await.unwrap();
        assert_eq!(repo.dependencies(&task.id).await.unwrap(), vec![dep.id.clone()]);
        // Dependencia no 'done' → no satisfecha.
        assert!(!repo.dependencies_satisfied(&task.id).await.unwrap());
        repo.set_status(&dep.id, "done").await.unwrap();
        assert!(repo.dependencies_satisfied(&task.id).await.unwrap());
        // Tarea sin dependencias → satisfecha trivialmente.
        assert!(repo.dependencies_satisfied(&dep.id).await.unwrap());
    }

    #[tokio::test]
    async fn set_status_manages_timestamps() {
        let (repo, pid) = setup().await;
        let t = repo.create(new_task(&pid, "T")).await.unwrap();
        assert!(t.started_at.is_none() && t.completed_at.is_none());
        repo.set_status(&t.id, "in_progress").await.unwrap();
        let t2 = repo.get(&t.id).await.unwrap().unwrap();
        assert!(t2.started_at.is_some() && t2.completed_at.is_none());
        repo.set_status(&t.id, "done").await.unwrap();
        let t3 = repo.get(&t.id).await.unwrap().unwrap();
        assert!(t3.completed_at.is_some());
        // started_at no se sobreescribe en transiciones posteriores.
        assert_eq!(t2.started_at, t3.started_at);
    }

    #[tokio::test]
    async fn update_fields_and_not_found() {
        let (repo, pid) = setup().await;
        let t = repo.create(new_task(&pid, "Old")).await.unwrap();
        repo.update(
            &t.id,
            TaskUpdate {
                title: Some("New".to_string()),
                requires_human_review: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let got = repo.get(&t.id).await.unwrap().unwrap();
        assert_eq!(got.title, "New");
        assert!(!got.requires_human_review);

        let err = repo
            .update(
                "nope",
                TaskUpdate {
                    title: Some("X".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }
}
