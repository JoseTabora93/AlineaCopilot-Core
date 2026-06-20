//! Implementación SQLite de `IProjectRepository`.

use aionui_common::{generate_id, now_ms};
use sqlx::SqlitePool;

use crate::DbError;
use crate::models::{PipelineTemplateRow, ProjectRow};
use crate::repository::project::{IProjectRepository, NewProject, ProjectUpdate};

pub struct SqliteProjectRepository {
    pool: SqlitePool,
}

impl SqliteProjectRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl IProjectRepository for SqliteProjectRepository {
    async fn create(&self, params: NewProject) -> Result<ProjectRow, DbError> {
        let id = generate_id();
        let now = now_ms();
        sqlx::query(
            "INSERT INTO projects (id, name, description, project_type, status, created_by, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'active', ?, ?, ?)",
        )
        .bind(&id)
        .bind(&params.name)
        .bind(&params.description)
        .bind(&params.project_type)
        .bind(&params.created_by)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(ProjectRow {
            id,
            name: params.name,
            description: params.description,
            project_type: params.project_type,
            status: "active".to_string(),
            created_by: params.created_by,
            created_at: now,
            updated_at: now,
        })
    }

    async fn get(&self, id: &str) -> Result<Option<ProjectRow>, DbError> {
        let row = sqlx::query_as::<_, ProjectRow>("SELECT * FROM projects WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    async fn list_for_user(&self, user_id: &str, include_archived: bool) -> Result<Vec<ProjectRow>, DbError> {
        let rows = sqlx::query_as::<_, ProjectRow>(
            "SELECT p.id, p.name, p.description, p.project_type, p.status, p.created_by, p.created_at, p.updated_at \
             FROM projects p \
             JOIN resource_acl a \
               ON a.resource_type = 'project' AND a.resource_id = p.id \
              AND a.principal_type = 'user'    AND a.principal_id = ? \
             WHERE (? = 1 OR p.status = 'active') \
             ORDER BY p.updated_at DESC",
        )
        .bind(user_id)
        .bind(i64::from(include_archived))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn update(&self, id: &str, update: ProjectUpdate) -> Result<(), DbError> {
        let mut set_parts: Vec<String> = Vec::new();
        let mut binds: Vec<Option<String>> = Vec::new();

        if let Some(name) = update.name {
            set_parts.push("name = ?".to_string());
            binds.push(Some(name));
        }
        if let Some(description) = update.description {
            set_parts.push("description = ?".to_string());
            binds.push(description);
        }
        if let Some(status) = update.status {
            set_parts.push("status = ?".to_string());
            binds.push(Some(status));
        }

        if set_parts.is_empty() {
            return Ok(());
        }
        set_parts.push("updated_at = ?".to_string());

        let sql = format!("UPDATE projects SET {} WHERE id = ?", set_parts.join(", "));
        let mut query = sqlx::query(&sql);
        for bind in binds {
            query = query.bind(bind);
        }
        query = query.bind(now_ms());
        query = query.bind(id);

        let result = query.execute(&self.pool).await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("Project '{id}' not found")));
        }
        Ok(())
    }

    async fn list_templates(&self, project_type: Option<&str>) -> Result<Vec<PipelineTemplateRow>, DbError> {
        let rows = match project_type {
            Some(pt) => {
                sqlx::query_as::<_, PipelineTemplateRow>(
                    "SELECT * FROM pipeline_templates WHERE is_active = 1 AND project_type = ? ORDER BY name ASC",
                )
                .bind(pt)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as::<_, PipelineTemplateRow>(
                    "SELECT * FROM pipeline_templates WHERE is_active = 1 ORDER BY name ASC",
                )
                .fetch_all(&self.pool)
                .await?
            }
        };
        Ok(rows)
    }

    async fn get_template(&self, id: &str) -> Result<Option<PipelineTemplateRow>, DbError> {
        let row = sqlx::query_as::<_, PipelineTemplateRow>("SELECT * FROM pipeline_templates WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init_database_memory;
    use crate::repository::resource_acl::IResourceAclRepository;
    use crate::repository::sqlite_resource_acl::SqliteResourceAclRepository;

    async fn setup() -> (SqliteProjectRepository, SqliteResourceAclRepository) {
        let db = init_database_memory().await.unwrap();
        (
            SqliteProjectRepository::new(db.pool().clone()),
            SqliteResourceAclRepository::new(db.pool().clone()),
        )
    }

    fn new_project(name: &str, project_type: &str) -> NewProject {
        NewProject {
            name: name.to_string(),
            description: None,
            project_type: project_type.to_string(),
            created_by: "u1".to_string(),
        }
    }

    #[tokio::test]
    async fn create_and_get() {
        let (repo, _) = setup().await;
        let p = repo.create(new_project("Torre A", "preventa_mep")).await.unwrap();
        assert_eq!(p.name, "Torre A");
        assert_eq!(p.status, "active");
        let got = repo.get(&p.id).await.unwrap().unwrap();
        assert_eq!(got.id, p.id);
        assert_eq!(got.project_type, "preventa_mep");
        assert_eq!(got.created_by, "u1");
    }

    #[tokio::test]
    async fn list_for_user_filters_by_membership() {
        let (repo, acl) = setup().await;
        let p = repo.create(new_project("P", "generico")).await.unwrap();
        // Sin membresía: nadie lo ve (fail-closed).
        assert_eq!(repo.list_for_user("u1", false).await.unwrap().len(), 0);
        acl.grant("project", &p.id, "u1", "owner").await.unwrap();
        assert_eq!(repo.list_for_user("u1", false).await.unwrap().len(), 1);
        // Otro usuario sigue sin verlo.
        assert_eq!(repo.list_for_user("u2", false).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn list_for_user_archived_filter() {
        let (repo, acl) = setup().await;
        let p = repo.create(new_project("P", "generico")).await.unwrap();
        acl.grant("project", &p.id, "u1", "owner").await.unwrap();
        repo.update(
            &p.id,
            ProjectUpdate {
                status: Some("archived".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(repo.list_for_user("u1", false).await.unwrap().len(), 0);
        assert_eq!(repo.list_for_user("u1", true).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn update_partial_and_not_found() {
        let (repo, _) = setup().await;
        let p = repo.create(new_project("Old", "generico")).await.unwrap();
        repo.update(
            &p.id,
            ProjectUpdate {
                name: Some("New".to_string()),
                description: Some(None),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let got = repo.get(&p.id).await.unwrap().unwrap();
        assert_eq!(got.name, "New");
        assert!(got.description.is_none());

        let err = repo
            .update(
                "nope",
                ProjectUpdate {
                    name: Some("X".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[tokio::test]
    async fn templates_seeded_with_terminal_artifacts() {
        let (repo, _) = setup().await;
        assert!(repo.list_templates(None).await.unwrap().len() >= 2);
        let preventa = repo.list_templates(Some("preventa_mep")).await.unwrap();
        assert_eq!(preventa.len(), 1);
        assert_eq!(preventa[0].name, "Preventa MEP");
        // La preventa termina en BOM + Alcances de obra.
        assert!(preventa[0].definition.contains("\"bom\""));
        assert!(preventa[0].definition.contains("alcances_obra"));
    }
}
