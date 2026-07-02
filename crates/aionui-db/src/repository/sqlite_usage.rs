use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::error::DbError;
use crate::models::{
    CreatedServiceToken, IngestResult, IngestUsageEvent, NewUsageEvent, ServiceTokenRow, UsageSummary, UserUsageLimit,
};
use crate::pricing;
use crate::repository::IUsageRepository;
use crate::repository::usage::UsageQueryFilters;

/// Longitud en bytes del token de servicio en claro (antes de hex-encode).
/// 32 bytes = 256 bits de entropía, igual orden de magnitud que una clave API.
const SERVICE_TOKEN_BYTES: usize = 32;

/// SHA-256 hex del token en claro. Determinista y rápido a propósito: un
/// service token es un secreto de alta entropía (no una contraseña humana),
/// así que no necesita el costo de bcrypt — el ingest se valida en cada
/// request del hot-path.
pub fn hash_service_token(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

fn generate_raw_service_token() -> String {
    let mut buf = [0u8; SERVICE_TOKEN_BYTES];
    getrandom::getrandom(&mut buf).expect("OS entropy source unavailable");
    hex::encode(buf)
}

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
        let cost = pricing::estimate_cost_usd(
            e.model.as_deref(),
            e.tokens_in,
            e.tokens_out,
            e.cache_read,
            e.cache_write,
        );
        sqlx::query(
            "INSERT INTO usage_events \
             (id, user_id, conversation_id, project_id, engine, provider, model, \
              tokens_in, tokens_out, cache_read, cache_write, cost_usd, created_at, profile_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(&e.profile_id)
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

    async fn summary_all_users_filtered(
        &self,
        since_ms: i64,
        filters: &UsageQueryFilters,
    ) -> Result<Vec<UsageSummary>, DbError> {
        // Filtros dinámicos simples: solo 2 columnas opcionales, así que un
        // query_builder manual basta (evita traer un query-builder crate solo
        // para esto). Los binds se aplican en el mismo orden que se agregan
        // los fragmentos SQL.
        let mut sql = String::from(
            "SELECT user_id, COALESCE(SUM(tokens_in), 0), COALESCE(SUM(tokens_out), 0), \
                    COALESCE(SUM(cache_read), 0), COALESCE(SUM(cache_write), 0), \
                    COALESCE(SUM(cost_usd), 0.0), COUNT(*) \
             FROM usage_events WHERE created_at >= ?",
        );
        if filters.profile_id.is_some() {
            sql.push_str(" AND profile_id = ?");
        }
        if filters.engine.is_some() {
            sql.push_str(" AND engine = ?");
        }
        sql.push_str(" GROUP BY user_id ORDER BY SUM(cost_usd) DESC");

        let mut query = sqlx::query_as::<_, (String, i64, i64, i64, i64, f64, i64)>(&sql).bind(since_ms);
        if let Some(profile_id) = &filters.profile_id {
            query = query.bind(profile_id);
        }
        if let Some(engine) = &filters.engine {
            query = query.bind(engine);
        }
        let rows = query.fetch_all(&self.pool).await?;
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

    async fn ingest_event(&self, e: IngestUsageEvent) -> Result<IngestResult, DbError> {
        // Dedupe primero: si la idempotency_key ya existe, devuelve el evento
        // existente sin tocar la tabla (fallback JSONL del emisor puede
        // reintentar el mismo evento tras una caída del Core sin duplicar $).
        if let Some(key) = &e.idempotency_key {
            let existing: Option<(String,)> =
                sqlx::query_as("SELECT usage_event_id FROM usage_ingest_idempotency WHERE idempotency_key = ?")
                    .bind(key)
                    .fetch_optional(&self.pool)
                    .await?;
            if let Some((event_id,)) = existing {
                return Ok(IngestResult {
                    event_id,
                    deduped: true,
                });
            }
        }

        let id = aionui_common::generate_prefixed_id("usage");
        // `usage_events.user_id` tiene FK a `users(id)`: sin user_id explícito
        // (p.ej. un run de pipeline no atribuible a una sesión humana) se
        // atribuye a `system_default_user`, el único usuario garantizado por
        // el seed inicial (`database.rs`) — un pseudo-usuario `'system'` suelto
        // violaría la FK porque nunca se siembra como fila real.
        let user_id = e.user_id.clone().unwrap_or_else(|| "system_default_user".to_string());

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO usage_events \
             (id, user_id, conversation_id, project_id, engine, provider, model, \
              tokens_in, tokens_out, cache_read, cache_write, cost_usd, created_at, profile_id) \
             VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&user_id)
        .bind(&e.project_id)
        .bind(&e.engine)
        .bind(&e.provider)
        .bind(&e.model)
        .bind(e.tokens_in)
        .bind(e.tokens_out)
        .bind(e.cache_read)
        .bind(e.cache_write)
        .bind(e.cost_usd)
        .bind(e.ts_ms)
        .bind(&e.profile_id)
        .execute(&mut *tx)
        .await?;

        if let Some(key) = &e.idempotency_key {
            let now = aionui_common::now_ms();
            sqlx::query(
                "INSERT INTO usage_ingest_idempotency (idempotency_key, usage_event_id, created_at) \
                 VALUES (?, ?, ?)",
            )
            .bind(key)
            .bind(&id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(IngestResult {
            event_id: id,
            deduped: false,
        })
    }

    async fn create_service_token(&self, name: &str) -> Result<CreatedServiceToken, DbError> {
        let id = aionui_common::generate_prefixed_id("svctok");
        let now = aionui_common::now_ms();
        let raw_token = generate_raw_service_token();
        let token_hash = hash_service_token(&raw_token);

        sqlx::query("INSERT INTO service_tokens (id, name, token_hash, is_active, created_at) VALUES (?, ?, ?, 1, ?)")
            .bind(&id)
            .bind(name)
            .bind(&token_hash)
            .bind(now)
            .execute(&self.pool)
            .await?;

        Ok(CreatedServiceToken {
            id,
            name: name.to_string(),
            token: raw_token,
            created_at: now,
        })
    }

    async fn find_active_service_token_by_hash(&self, token_hash: &str) -> Result<Option<ServiceTokenRow>, DbError> {
        let row =
            sqlx::query_as::<_, ServiceTokenRow>("SELECT * FROM service_tokens WHERE token_hash = ? AND is_active = 1")
                .bind(token_hash)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
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

    fn sample_ingest_event(engine: &str, profile_id: Option<&str>, idempotency_key: Option<&str>) -> IngestUsageEvent {
        IngestUsageEvent {
            engine: engine.to_string(),
            model: Some("glm-4.6".into()),
            provider: Some("zai".into()),
            user_id: None,
            profile_id: profile_id.map(|s| s.to_string()),
            project_id: None,
            tokens_in: 1000,
            tokens_out: 500,
            cache_read: 0,
            cache_write: 0,
            cost_usd: 1.23,
            ts_ms: aionui_common::now_ms(),
            source: "test_emitter".into(),
            idempotency_key: idempotency_key.map(|s| s.to_string()),
        }
    }

    #[tokio::test]
    async fn ingest_event_records_and_is_visible_in_summary() {
        let db = init_database_memory().await.unwrap();
        let repo = SqliteUsageRepository::new(db.pool().clone());

        let result = repo
            .ingest_event(sample_ingest_event("hermes", Some("servimec-tko"), None))
            .await
            .unwrap();
        assert!(!result.deduped);

        // Sin user_id -> se atribuye a 'system'.
        let summary = repo.summary_for_user("system_default_user", 0).await.unwrap();
        assert_eq!(summary.events, 1);
        assert!((summary.cost_usd - 1.23).abs() < 1e-9);
    }

    #[tokio::test]
    async fn ingest_event_with_repeated_idempotency_key_does_not_duplicate() {
        let db = init_database_memory().await.unwrap();
        let repo = SqliteUsageRepository::new(db.pool().clone());

        let first = repo
            .ingest_event(sample_ingest_event("openclaw", None, Some("run-42")))
            .await
            .unwrap();
        assert!(!first.deduped);

        let second = repo
            .ingest_event(sample_ingest_event("openclaw", None, Some("run-42")))
            .await
            .unwrap();
        assert!(second.deduped, "misma idempotency_key debe deduplicar");
        assert_eq!(second.event_id, first.event_id);

        let summary = repo.summary_for_user("system_default_user", 0).await.unwrap();
        assert_eq!(summary.events, 1, "no debe haber grabado un segundo evento");
    }

    #[tokio::test]
    async fn summary_all_users_filtered_by_profile_and_engine() {
        let db = init_database_memory().await.unwrap();
        let repo = SqliteUsageRepository::new(db.pool().clone());

        repo.ingest_event(sample_ingest_event("hermes", Some("servimec-tko"), None))
            .await
            .unwrap();
        repo.ingest_event(sample_ingest_event("openclaw", Some("preventa"), None))
            .await
            .unwrap();

        // Sin filtro: ambos eventos entran en el agregado de 'system'.
        let all = repo
            .summary_all_users_filtered(0, &UsageQueryFilters::default())
            .await
            .unwrap();
        assert_eq!(
            all.iter().find(|s| s.user_id == "system_default_user").unwrap().events,
            2
        );

        // Filtro por profile_id: solo el evento de 'servimec-tko'.
        let by_profile = repo
            .summary_all_users_filtered(
                0,
                &UsageQueryFilters {
                    profile_id: Some("servimec-tko".into()),
                    engine: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            by_profile
                .iter()
                .find(|s| s.user_id == "system_default_user")
                .unwrap()
                .events,
            1
        );

        // Filtro por engine: solo el evento 'openclaw'.
        let by_engine = repo
            .summary_all_users_filtered(
                0,
                &UsageQueryFilters {
                    profile_id: None,
                    engine: Some("openclaw".into()),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            by_engine
                .iter()
                .find(|s| s.user_id == "system_default_user")
                .unwrap()
                .events,
            1
        );
    }

    #[tokio::test]
    async fn service_token_create_and_lookup_roundtrip() {
        let db = init_database_memory().await.unwrap();
        let repo = SqliteUsageRepository::new(db.pool().clone());

        let created = repo.create_service_token("preventa-pipeline").await.unwrap();
        assert_eq!(created.name, "preventa-pipeline");
        assert!(!created.token.is_empty());

        let hash = hash_service_token(&created.token);
        let found = repo
            .find_active_service_token_by_hash(&hash)
            .await
            .unwrap()
            .expect("token recién creado debe encontrarse por su hash");
        assert_eq!(found.id, created.id);
        assert_eq!(found.name, "preventa-pipeline");
        assert!(found.is_active);

        // Un hash que no corresponde a ningún token no debe encontrar nada.
        let bogus_hash = hash_service_token("not-a-real-token");
        assert!(
            repo.find_active_service_token_by_hash(&bogus_hash)
                .await
                .unwrap()
                .is_none()
        );
    }
}
