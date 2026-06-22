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
