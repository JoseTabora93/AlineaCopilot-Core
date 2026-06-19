use sqlx::SqlitePool;

use crate::error::DbError;
use crate::models::{NewUsageEvent, UsageSummary, UserUsageLimit};
use crate::pricing;
use crate::repository::IUsageRepository;

/// Implementación SQLite de [`IUsageRepository`].
#[derive(Clone, Debug)]
pub struct SqliteUsageRepository {
    pool: SqlitePool,
}

impl SqliteUsageRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl IUsageRepository for SqliteUsageRepository {
    async fn record_event(&self, e: NewUsageEvent) -> Result<(), DbError> {
        let id = aionui_common::generate_prefixed_id("usage");
        let now = aionui_common::now_ms();
        let cost = pricing::estimate_cost_usd(e.model.as_deref(), e.tokens_in, e.tokens_out, e.cache_read, e.cache_write);
        sqlx::query(
            "INSERT INTO usage_events \
             (id, user_id, conversation_id, project_id, engine, provider, model, \
              tokens_in, tokens_out, cache_read, cache_write, cost_usd, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&e.user_id)
        .bind(&e.conversation_id)
        .bind(&e.project_id)
        .bind(&e.engine)
        .bind(&e.provider)
        .bind(&e.model)
        .bind(e.tokens_in)
        .bind(e.tokens_out)
        .bind(e.cache_read)
        .bind(e.cache_write)
        .bind(cost)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn summary_for_user(&self, user_id: &str, since_ms: i64) -> Result<UsageSummary, DbError> {
        let row: (i64, i64, i64, i64, f64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(tokens_in), 0), COALESCE(SUM(tokens_out), 0), \
                    COALESCE(SUM(cache_read), 0), COALESCE(SUM(cache_write), 0), \
                    COALESCE(SUM(cost_usd), 0.0), COUNT(*) \
             FROM usage_events WHERE user_id = ? AND created_at >= ?",
        )
        .bind(user_id)
        .bind(since_ms)
        .fetch_one(&self.pool)
        .await?;
        Ok(UsageSummary {
            user_id: user_id.to_string(),
            tokens_in: row.0,
            tokens_out: row.1,
            cache_read: row.2,
            cache_write: row.3,
            cost_usd: row.4,
            events: row.5,
        })
    }

    async fn summary_all_users(&self, since_ms: i64) -> Result<Vec<UsageSummary>, DbError> {
        let rows: Vec<(String, i64, i64, i64, i64, f64, i64)> = sqlx::query_as(
            "SELECT user_id, COALESCE(SUM(tokens_in), 0), COALESCE(SUM(tokens_out), 0), \
                    COALESCE(SUM(cache_read), 0), COALESCE(SUM(cache_write), 0), \
                    COALESCE(SUM(cost_usd), 0.0), COUNT(*) \
             FROM usage_events WHERE created_at >= ? \
             GROUP BY user_id ORDER BY SUM(cost_usd) DESC",
        )
        .bind(since_ms)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| UsageSummary {
                user_id: r.0,
                tokens_in: r.1,
                tokens_out: r.2,
                cache_read: r.3,
                cache_write: r.4,
                cost_usd: r.5,
                events: r.6,
            })
            .collect())
    }

    async fn get_limit(&self, user_id: &str) -> Result<Option<UserUsageLimit>, DbError> {
        let lim = sqlx::query_as::<_, UserUsageLimit>("SELECT * FROM user_usage_limit WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(lim)
    }

    async fn set_limit(&self, user_id: &str, soft_usd: Option<f64>, hard_usd: Option<f64>) -> Result<(), DbError> {
        let now = aionui_common::now_ms();
        sqlx::query(
            "INSERT INTO user_usage_limit (user_id, soft_usd, hard_usd, period, updated_at) \
             VALUES (?, ?, ?, 'monthly', ?) \
             ON CONFLICT(user_id) DO UPDATE SET \
                soft_usd = excluded.soft_usd, hard_usd = excluded.hard_usd, updated_at = excluded.updated_at",
        )
        .bind(user_id)
        .bind(soft_usd)
        .bind(hard_usd)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init_database_memory;
    use crate::repository::{IUserRepository, SqliteUserRepository};

    #[tokio::test]
    async fn record_and_summarize() {
        let db = init_database_memory().await.unwrap();
        let users = SqliteUserRepository::new(db.pool().clone());
        let user = users.create_user("ana", "h").await.unwrap();
        let repo = SqliteUsageRepository::new(db.pool().clone());

        repo.record_event(NewUsageEvent {
            user_id: user.id.clone(),
            engine: "copilot".into(),
            model: Some("claude-sonnet".into()),
            tokens_in: 1_000_000,
            tokens_out: 1_000_000,
            ..Default::default()
        })
        .await
        .unwrap();

        let s = repo.summary_for_user(&user.id, 0).await.unwrap();
        assert_eq!(s.events, 1);
        assert_eq!(s.tokens_in, 1_000_000);
        // 1M in @3 + 1M out @15 = 18.0
        assert!((s.cost_usd - 18.0).abs() < 1e-6, "cost {}", s.cost_usd);
    }

    #[tokio::test]
    async fn limit_upsert_roundtrip() {
        let db = init_database_memory().await.unwrap();
        let users = SqliteUserRepository::new(db.pool().clone());
        let user = users.create_user("bob", "h").await.unwrap();
        let repo = SqliteUsageRepository::new(db.pool().clone());

        assert!(repo.get_limit(&user.id).await.unwrap().is_none());
        repo.set_limit(&user.id, Some(5.0), Some(20.0)).await.unwrap();
        let lim = repo.get_limit(&user.id).await.unwrap().unwrap();
        assert_eq!(lim.soft_usd, Some(5.0));
        assert_eq!(lim.hard_usd, Some(20.0));
        // upsert: actualiza
        repo.set_limit(&user.id, None, Some(50.0)).await.unwrap();
        let lim = repo.get_limit(&user.id).await.unwrap().unwrap();
        assert_eq!(lim.soft_usd, None);
        assert_eq!(lim.hard_usd, Some(50.0));
    }
}
