use crate::error::DbError;
use crate::models::{
    CreatedServiceToken, IngestResult, IngestUsageEvent, NewUsageEvent, ServiceTokenRow, UsageSummary, UserUsageLimit,
};

/// Filtros opcionales del panel admin (`GET /api/admin/usage`). `None` en un
/// campo = sin filtrar por esa dimensión. Agregación existente (por usuario)
/// se mantiene; estos filtros solo restringen las filas consideradas.
#[derive(Debug, Clone, Default)]
pub struct UsageQueryFilters {
    pub profile_id: Option<String>,
    pub engine: Option<String>,
}

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

    /// Consumo agregado de un PERFIL de agente desde `since_ms` (inclusive),
    /// sumado a través de TODOS los usuarios que lo usaron (Fase perfiles —
    /// tarea C4: enforcement de techo por perfil). `profile_id` es el `name`
    /// del perfil (mismo valor que viaja en `usage_events.profile_id` y en
    /// `ConversationContext::profile_id` — ver `crates/aionui-ai-agent/src/factory/acp.rs`),
    /// NO el `id` de la fila `agent_profiles`. El `user_id` del `UsageSummary`
    /// devuelto es el propio `profile_id` (mismo patrón de reuso que
    /// `summary_all_users_filtered` usa `user_id` como clave de agrupación).
    async fn summary_for_profile(&self, profile_id: &str, since_ms: i64) -> Result<UsageSummary, DbError>;

    /// Consumo agregado de TODOS los usuarios desde `since_ms` (panel admin),
    /// opcionalmente filtrado por `profile_id`/`engine` (Fase ledger — C1).
    async fn summary_all_users(&self, since_ms: i64) -> Result<Vec<UsageSummary>, DbError> {
        self.summary_all_users_filtered(since_ms, &UsageQueryFilters::default())
            .await
    }

    /// Igual que `summary_all_users` pero con filtros explícitos. Método base;
    /// `summary_all_users` delega aquí con filtros vacíos para no romper a los
    /// llamadores existentes.
    async fn summary_all_users_filtered(
        &self,
        since_ms: i64,
        filters: &UsageQueryFilters,
    ) -> Result<Vec<UsageSummary>, DbError>;

    /// Límite de gasto del usuario (`None` si no tiene configurado).
    async fn get_limit(&self, user_id: &str) -> Result<Option<UserUsageLimit>, DbError>;

    /// Crea/actualiza el límite de gasto del usuario (upsert). `None` en un
    /// umbral lo deja sin ese límite.
    async fn set_limit(&self, user_id: &str, soft_usd: Option<f64>, hard_usd: Option<f64>) -> Result<(), DbError>;

    // ---- Ingest de emisores externos (Fase ledger — tarea C1) ----

    /// Graba un evento recibido por `POST /api/usage/ingest`. Si trae
    /// `idempotency_key` y ya existe, NO duplica: devuelve el evento existente
    /// con `deduped: true`.
    async fn ingest_event(&self, event: IngestUsageEvent) -> Result<IngestResult, DbError>;

    // ---- Service tokens (autenticación de emisores externos) ----

    /// Crea un nuevo service token. Devuelve el token en claro (única vez que
    /// se expone) junto al hash ya persistido.
    async fn create_service_token(&self, name: &str) -> Result<CreatedServiceToken, DbError>;

    /// Busca un token activo cuyo hash coincida con `token_hash` (hex SHA-256
    /// del token en claro, calculado por el llamador). `None` si no hay match
    /// o el token está desactivado — fail-closed en el handler.
    async fn find_active_service_token_by_hash(&self, token_hash: &str) -> Result<Option<ServiceTokenRow>, DbError>;
}
