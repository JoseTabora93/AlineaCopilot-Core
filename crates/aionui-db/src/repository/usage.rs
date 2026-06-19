use crate::error::DbError;
use crate::models::{NewUsageEvent, UsageSummary, UserUsageLimit};

/// Acceso al ledger de consumos (Alinea Fase 2 #3 — blueprint §12).
///
/// Object-safe vía `async_trait` para soportar `Arc<dyn IUsageRepository>`.
#[async_trait::async_trait]
pub trait IUsageRepository: Send + Sync {
    /// Registra un evento de consumo. El costo USD se calcula con la tabla de
    /// precios (`crate::pricing`) a partir del modelo + tokens.
    async fn record_event(&self, event: NewUsageEvent) -> Result<(), DbError>;

    /// Consumo agregado de un usuario desde `since_ms` (inclusive).
    async fn summary_for_user(&self, user_id: &str, since_ms: i64) -> Result<UsageSummary, DbError>;

    /// Consumo agregado de TODOS los usuarios desde `since_ms` (panel admin).
    async fn summary_all_users(&self, since_ms: i64) -> Result<Vec<UsageSummary>, DbError>;

    /// Límite de gasto del usuario (`None` si no tiene configurado).
    async fn get_limit(&self, user_id: &str) -> Result<Option<UserUsageLimit>, DbError>;

    /// Crea/actualiza el límite de gasto del usuario (upsert). `None` en un
    /// umbral lo deja sin ese límite.
    async fn set_limit(&self, user_id: &str, soft_usd: Option<f64>, hard_usd: Option<f64>) -> Result<(), DbError>;
}
