use aionui_common::TimestampMs;
use serde::{Deserialize, Serialize};

/// Row de `usage_events` (Alinea Fase 2 #3 — ledger de consumos, blueprint §12).
///
/// Un evento por llamada LLM (o bucket de sistema). El costo en USD se calcula
/// en el Core con la tabla de precios por modelo antes de insertar.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UsageEvent {
    pub id: String,
    pub user_id: String,
    pub conversation_id: Option<String>,
    pub project_id: Option<String>,
    /// 'copilot' | 'openclaw' | 'hermes' | 'system:...'.
    pub engine: String,
    /// 'anthropic' | 'zai' | 'minimax' | 'openrouter' | …
    pub provider: Option<String>,
    pub model: Option<String>,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub cost_usd: f64,
    pub created_at: TimestampMs,
    /// Perfil de agente que originó el costo (Fase ledger — tarea C1). `None`
    /// para eventos del hot-path que no tienen perfil asociado.
    pub profile_id: Option<String>,
}

/// Parámetros para registrar un evento de consumo (el `id`/`created_at` los pone
/// el repositorio).
#[derive(Debug, Clone, Default)]
pub struct NewUsageEvent {
    pub user_id: String,
    pub conversation_id: Option<String>,
    pub project_id: Option<String>,
    pub engine: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub profile_id: Option<String>,
}

/// Evento de consumo recibido por `POST /api/usage/ingest` (emisores externos:
/// pipeline de preventa, Hermes). A diferencia de `NewUsageEvent` (hot-path
/// interno, costo calculado por `pricing::estimate_cost_usd`), el emisor externo
/// manda su propio `cost_usd` ya calculado y controla `created_at`/idempotencia.
#[derive(Debug, Clone, Deserialize)]
pub struct IngestUsageEvent {
    pub engine: String,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub user_id: Option<String>,
    pub profile_id: Option<String>,
    pub project_id: Option<String>,
    #[serde(default)]
    pub tokens_in: i64,
    #[serde(default)]
    pub tokens_out: i64,
    #[serde(default)]
    pub cache_read: i64,
    #[serde(default)]
    pub cache_write: i64,
    pub cost_usd: f64,
    pub ts_ms: TimestampMs,
    /// Identificador libre del emisor (p.ej. `"preventa_cost"`, `"hermes_export"`).
    pub source: String,
    /// Si viene y ya existe, el ingest no duplica el evento (`deduped: true`).
    pub idempotency_key: Option<String>,
}

/// Resultado de un ingest: el evento grabado (o el ya existente si se dedujo).
#[derive(Debug, Clone, Serialize)]
pub struct IngestResult {
    pub event_id: String,
    pub deduped: bool,
}

/// Row de `service_tokens`: credencial de un emisor externo autorizado a postear
/// a `/api/usage/ingest`. El token en claro nunca se persiste — solo su hash.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ServiceTokenRow {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing)]
    pub token_hash: String,
    pub is_active: bool,
    pub created_at: TimestampMs,
}

/// Respuesta de `POST /api/admin/service-tokens`: el token en claro se expone
/// UNA sola vez, en el momento de creación.
#[derive(Debug, Clone, Serialize)]
pub struct CreatedServiceToken {
    pub id: String,
    pub name: String,
    pub token: String,
    pub created_at: TimestampMs,
}

/// Resumen agregado de consumo de un usuario en una ventana temporal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageSummary {
    pub user_id: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub cost_usd: f64,
    pub events: i64,
}

/// Row de `user_usage_limit`: límite de gasto por usuario (pre-flight).
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserUsageLimit {
    pub user_id: String,
    /// Umbral USD del período. `None` = sin límite de ese tipo.
    pub soft_usd: Option<f64>,
    pub hard_usd: Option<f64>,
    /// Ventana de acumulación (por ahora solo `"monthly"`).
    pub period: String,
    pub updated_at: TimestampMs,
}
