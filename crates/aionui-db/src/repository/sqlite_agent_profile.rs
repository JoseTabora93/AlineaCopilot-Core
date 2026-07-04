//! Implementación SQLite de `IAgentProfileRepository`.

use aionui_common::{generate_id, now_ms};
use sqlx::SqlitePool;

use crate::DbError;
use crate::models::AgentProfileRow;
use crate::repository::agent_profile::{AgentProfileUpdate, IAgentProfileRepository, NewAgentProfile};

pub struct SqliteAgentProfileRepository {
    pool: SqlitePool,
}

impl SqliteAgentProfileRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

/// Checks if a SQLite database error is a UNIQUE constraint violation.
/// (Mismo criterio que `sqlite_user.rs`: SQLite error code 2067 = SQLITE_CONSTRAINT_UNIQUE.)
fn is_unique_violation(err: &dyn sqlx::error::DatabaseError) -> bool {
    err.code().is_some_and(|c| c == "2067")
}

#[async_trait::async_trait]
impl IAgentProfileRepository for SqliteAgentProfileRepository {
    async fn create(&self, params: NewAgentProfile) -> Result<AgentProfileRow, DbError> {
        let id = generate_id();
        let now = now_ms();
        sqlx::query(
            "INSERT INTO agent_profiles (id, name, label, definition, is_active, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(&id)
        .bind(&params.name)
        .bind(&params.label)
        .bind(&params.definition)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db_err) if is_unique_violation(db_err.as_ref()) => {
                DbError::Conflict(format!("Agent profile with name '{}' already exists", params.name))
            }
            _ => DbError::Query(e),
        })?;

        Ok(AgentProfileRow {
            id,
            name: params.name,
            label: params.label,
            definition: params.definition,
            is_active: true,
            created_at: now,
            updated_at: now,
        })
    }

    async fn get(&self, id: &str) -> Result<Option<AgentProfileRow>, DbError> {
        let row = sqlx::query_as::<_, AgentProfileRow>("SELECT * FROM agent_profiles WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    async fn get_by_name(&self, name: &str) -> Result<Option<AgentProfileRow>, DbError> {
        let row = sqlx::query_as::<_, AgentProfileRow>("SELECT * FROM agent_profiles WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    async fn list_all(&self, include_inactive: bool) -> Result<Vec<AgentProfileRow>, DbError> {
        let rows = sqlx::query_as::<_, AgentProfileRow>(
            "SELECT * FROM agent_profiles WHERE (? = 1 OR is_active = 1) ORDER BY name ASC",
        )
        .bind(i64::from(include_inactive))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn list_by_ids(&self, ids: &[String], include_inactive: bool) -> Result<Vec<AgentProfileRow>, DbError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // sqlx no soporta `IN (?)` con un Vec directamente sobre SQLite sin
        // query builder dinámico; con N pequeño (perfiles = 6-10 role-packs,
        // nunca miles) un placeholder por id es simple y suficientemente
        // rápido. Ver aionui-db/src/repository/sqlite_project.rs para el
        // mismo patrón de `update` con SET dinámico.
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            "SELECT * FROM agent_profiles WHERE id IN ({placeholders}) AND (? = 1 OR is_active = 1) ORDER BY name ASC"
        );
        let mut query = sqlx::query_as::<_, AgentProfileRow>(&sql);
        for id in ids {
            query = query.bind(id);
        }
        query = query.bind(i64::from(include_inactive));
        let rows = query.fetch_all(&self.pool).await?;
        Ok(rows)
    }

    async fn update(&self, id: &str, update: AgentProfileUpdate) -> Result<(), DbError> {
        let mut set_parts: Vec<String> = Vec::new();
        let mut binds: Vec<String> = Vec::new();
        let mut is_active_bind: Option<bool> = None;

        if let Some(label) = update.label {
            set_parts.push("label = ?".to_string());
            binds.push(label);
        }
        if let Some(definition) = update.definition {
            set_parts.push("definition = ?".to_string());
            binds.push(definition);
        }
        if let Some(is_active) = update.is_active {
            set_parts.push("is_active = ?".to_string());
            is_active_bind = Some(is_active);
        }

        if set_parts.is_empty() {
            // Nada que actualizar: no-op, pero se sigue exigiendo que el id exista.
            let exists = self.get(id).await?;
            return if exists.is_some() {
                Ok(())
            } else {
                Err(DbError::NotFound(format!("Agent profile '{id}' not found")))
            };
        }
        set_parts.push("updated_at = ?".to_string());

        let sql = format!("UPDATE agent_profiles SET {} WHERE id = ?", set_parts.join(", "));
        let mut query = sqlx::query(&sql);
        for bind in &binds {
            query = query.bind(bind);
        }
        if let Some(is_active) = is_active_bind {
            query = query.bind(is_active);
        }
        query = query.bind(now_ms());
        query = query.bind(id);

        let result = query.execute(&self.pool).await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("Agent profile '{id}' not found")));
        }
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<(), DbError> {
        let result = sqlx::query("DELETE FROM agent_profiles WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("Agent profile '{id}' not found")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init_database_memory;

    async fn setup() -> SqliteAgentProfileRepository {
        let db = init_database_memory().await.unwrap();
        SqliteAgentProfileRepository::new(db.pool().clone())
    }

    fn new_profile(name: &str) -> NewAgentProfile {
        NewAgentProfile {
            name: name.to_string(),
            label: format!("Label for {name}"),
            definition: format!(r#"{{"name":"{name}"}}"#),
        }
    }

    #[tokio::test]
    async fn create_and_get() {
        let repo = setup().await;
        let p = repo.create(new_profile("test-ingenieria")).await.unwrap();
        assert_eq!(p.name, "test-ingenieria");
        assert!(p.is_active);
        let got = repo.get(&p.id).await.unwrap().unwrap();
        assert_eq!(got.id, p.id);
        assert_eq!(got.label, "Label for test-ingenieria");
    }

    #[tokio::test]
    async fn get_by_name_roundtrip() {
        let repo = setup().await;
        let p = repo.create(new_profile("test-preventa")).await.unwrap();
        let got = repo.get_by_name("test-preventa").await.unwrap().unwrap();
        assert_eq!(got.id, p.id);
        assert!(repo.get_by_name("no-existe").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn create_enforces_unique_name() {
        let repo = setup().await;
        repo.create(new_profile("test-ingenieria")).await.unwrap();
        let err = repo.create(new_profile("test-ingenieria")).await.unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)), "expected Conflict, got: {err:?}");
    }

    #[tokio::test]
    async fn list_all_filters_inactive() {
        let repo = setup().await;
        let p1 = repo.create(new_profile("test-a")).await.unwrap();
        let _p2 = repo.create(new_profile("test-b")).await.unwrap();
        repo.update(
            &p1.id,
            AgentProfileUpdate {
                is_active: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // NB: init_database_memory() siembra los 6 role-packs de la migración
        // 022 (todos is_active=1), así que list_all() ya no arranca en 0 --
        // se filtra a los fixtures de este test (nombres "test-*") en vez de
        // asumir un conteo absoluto.
        let active_test_profiles: Vec<_> = repo
            .list_all(false)
            .await
            .unwrap()
            .into_iter()
            .filter(|p| p.name.starts_with("test-"))
            .collect();
        assert_eq!(active_test_profiles.len(), 1);

        let all_test_profiles: Vec<_> = repo
            .list_all(true)
            .await
            .unwrap()
            .into_iter()
            .filter(|p| p.name.starts_with("test-"))
            .collect();
        assert_eq!(all_test_profiles.len(), 2);
    }

    #[tokio::test]
    async fn list_by_ids_returns_only_requested_and_active() {
        let repo = setup().await;
        let p1 = repo.create(new_profile("test-a")).await.unwrap();
        let p2 = repo.create(new_profile("test-b")).await.unwrap();
        let _p3 = repo.create(new_profile("test-c")).await.unwrap();
        repo.update(
            &p2.id,
            AgentProfileUpdate {
                is_active: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let ids = vec![p1.id.clone(), p2.id.clone()];
        let active_only = repo.list_by_ids(&ids, false).await.unwrap();
        assert_eq!(active_only.len(), 1);
        assert_eq!(active_only[0].id, p1.id);

        let with_inactive = repo.list_by_ids(&ids, true).await.unwrap();
        assert_eq!(with_inactive.len(), 2);

        assert!(repo.list_by_ids(&[], false).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn update_partial_and_not_found() {
        let repo = setup().await;
        let p = repo.create(new_profile("test-ingenieria")).await.unwrap();
        repo.update(
            &p.id,
            AgentProfileUpdate {
                label: Some("Nuevo label".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let got = repo.get(&p.id).await.unwrap().unwrap();
        assert_eq!(got.label, "Nuevo label");
        assert!(got.updated_at >= got.created_at);

        let err = repo
            .update(
                "nope",
                AgentProfileUpdate {
                    label: Some("x".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }

    #[tokio::test]
    async fn update_definition_and_is_active() {
        let repo = setup().await;
        let p = repo.create(new_profile("test-preventa")).await.unwrap();
        repo.update(
            &p.id,
            AgentProfileUpdate {
                definition: Some(r#"{"name":"preventa","changed":true}"#.to_string()),
                is_active: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let got = repo.get(&p.id).await.unwrap().unwrap();
        assert!(got.definition.contains("changed"));
        assert!(!got.is_active);
    }

    #[tokio::test]
    async fn delete_removes_row() {
        let repo = setup().await;
        let p = repo.create(new_profile("test-ingenieria")).await.unwrap();
        repo.delete(&p.id).await.unwrap();
        assert!(repo.get(&p.id).await.unwrap().is_none());

        let err = repo.delete("nope").await.unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }
}
