#![allow(clippy::disallowed_types)] // ApiError es legítimo en handlers de rutas.
//! Endpoints del ledger de consumos $ (Alinea Fase 2 #3 — blueprint §12; ingest
//! externo + `profile_id` — Fase ledger tarea C1).
//!
//! - `GET  /api/usage/me`                    — "mi consumo" (cualquier usuario auth).
//! - `GET  /api/admin/usage`                 — consumo de todos (admin-only), con
//!   filtros opcionales `?profile_id=` y `?engine=`.
//! - `PUT  /api/admin/users/{id}/limit`       — fijar límite $ de un usuario (admin-only).
//! - `POST /api/usage/ingest`                — ingest de emisores externos (pipeline
//!   de preventa, Hermes). Auth por token de servicio, NUNCA sesión de usuario.
//! - `POST /api/admin/service-tokens`         — crea un token de servicio (admin-only);
//!   el token en claro se devuelve UNA sola vez.
//!
//! La ventana por defecto es **rolling 30 días** (override con `?since_ms=`).
//! El gate admin reusa `require_admin_middleware` (en local, `CurrentUser` ya trae
//! rol admin, así que funciona sin flag extra). Respuestas en `ApiResponse<T>`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::middleware::from_fn;
use axum::routing::{get, post};
use axum::{Extension, Router};
use serde::{Deserialize, Serialize};

use aionui_api_types::ApiResponse;
use aionui_auth::{CurrentUser, require_admin_middleware};
use aionui_common::{ApiError, TimestampMs};
use aionui_db::{
    IUsageRepository, UsageQueryFilters, hash_service_token,
    models::{CreatedServiceToken, IngestResult, IngestUsageEvent, UsageSummary, UserUsageLimit},
};

/// Ventana por defecto del ledger: 30 días en ms.
const DEFAULT_WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Clone)]
pub struct UsageRouterState {
    pub usage_repo: Arc<dyn IUsageRepository>,
}

#[derive(Debug, Deserialize)]
pub(super) struct WindowQuery {
    /// Inicio de la ventana (ms epoch). Default: hace 30 días.
    since_ms: Option<i64>,
    /// Filtra por perfil de agente (Fase ledger — C1). `None` = sin filtrar.
    profile_id: Option<String>,
    /// Filtra por motor (`copilot`/`openclaw`/`hermes`/`system:...`).
    engine: Option<String>,
}

fn window_start(q: &WindowQuery) -> TimestampMs {
    q.since_ms
        .unwrap_or_else(|| (aionui_common::now_ms() - DEFAULT_WINDOW_MS).max(0))
}

#[derive(Serialize)]
struct MyUsageResponse {
    usage: UsageSummary,
    limit: Option<UserUsageLimit>,
    since_ms: TimestampMs,
}

/// Fila del panel admin: consumo del usuario + su límite activo (para que el
/// admin vea el umbral antes de editar). `usage` se aplana, así que la respuesta
/// es retro-compatible con quien solo leía los campos de `UsageSummary`.
#[derive(Serialize)]
struct AdminUsageRow {
    #[serde(flatten)]
    usage: UsageSummary,
    limit: Option<UserUsageLimit>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SetLimitRequest {
    /// USD; `null` quita ese umbral.
    soft_usd: Option<f64>,
    hard_usd: Option<f64>,
}

fn db_err(err: aionui_db::DbError) -> ApiError {
    ApiError::Internal(format!("Database error: {err}"))
}

/// `GET /api/usage/me` — consumo del usuario autenticado + su límite.
async fn my_usage_handler(
    State(state): State<UsageRouterState>,
    Extension(current): Extension<CurrentUser>,
    Query(q): Query<WindowQuery>,
) -> Result<Json<ApiResponse<MyUsageResponse>>, ApiError> {
    let since = window_start(&q);
    let usage = state
        .usage_repo
        .summary_for_user(&current.id, since)
        .await
        .map_err(db_err)?;
    let limit = state.usage_repo.get_limit(&current.id).await.map_err(db_err)?;
    Ok(Json(ApiResponse::ok(MyUsageResponse {
        usage,
        limit,
        since_ms: since,
    })))
}

/// `GET /api/admin/usage` — consumo agregado de todos los usuarios (admin-only).
/// Filtros opcionales `?profile_id=` y `?engine=` (Fase ledger — C1); sin ellos
/// el comportamiento es idéntico al de antes (retro-compatible).
async fn admin_usage_handler(
    State(state): State<UsageRouterState>,
    Query(q): Query<WindowQuery>,
) -> Result<Json<ApiResponse<Vec<AdminUsageRow>>>, ApiError> {
    let since = window_start(&q);
    let filters = UsageQueryFilters {
        profile_id: q.profile_id.clone(),
        engine: q.engine.clone(),
    };
    let rows = state
        .usage_repo
        .summary_all_users_filtered(since, &filters)
        .await
        .map_err(db_err)?;
    // N+1 aceptable: el panel admin tiene pocos usuarios. Adjunta el límite activo
    // de cada uno para que la UI muestre el umbral y prellene el editor.
    let mut out = Vec::with_capacity(rows.len());
    for usage in rows {
        let limit = state.usage_repo.get_limit(&usage.user_id).await.map_err(db_err)?;
        out.push(AdminUsageRow { usage, limit });
    }
    Ok(Json(ApiResponse::ok(out)))
}

/// `GET /api/admin/users/{id}/limit` — límite activo de un usuario (admin-only).
/// `data: null` si no tiene. A diferencia de `/api/admin/usage` (que solo lista
/// usuarios con gasto), sirve para CUALQUIER usuario → el editor prellena el umbral.
async fn get_limit_handler(
    State(state): State<UsageRouterState>,
    Path(user_id): Path<String>,
) -> Result<Json<ApiResponse<Option<UserUsageLimit>>>, ApiError> {
    let limit = state.usage_repo.get_limit(&user_id).await.map_err(db_err)?;
    Ok(Json(ApiResponse::ok(limit)))
}

/// `PUT /api/admin/users/{id}/limit` — fija el límite $ de un usuario (admin-only).
async fn set_limit_handler(
    State(state): State<UsageRouterState>,
    Path(user_id): Path<String>,
    body: Result<Json<SetLimitRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<ApiResponse<UserUsageLimit>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    if let Some(h) = req.hard_usd
        && h < 0.0
    {
        return Err(ApiError::BadRequest("hard_usd must be >= 0".into()));
    }
    state
        .usage_repo
        .set_limit(&user_id, req.soft_usd, req.hard_usd)
        .await
        .map_err(db_err)?;
    let limit = state
        .usage_repo
        .get_limit(&user_id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::Internal("limit not found after set".into()))?;
    Ok(Json(ApiResponse::ok(limit)))
}

/// Router del ledger. El subconjunto admin trae su gate `require_admin` interno;
/// el llamador añade `auth_middleware` (capa externa) para poblar `CurrentUser`.
///
/// NO incluye `/api/usage/ingest` ni `POST /api/admin/service-tokens` (Fase
/// ledger — C1): el ingest se autentica con token de servicio, no sesión de
/// usuario, así que vive fuera de la capa `auth_middleware` (ver
/// `ingest_routes`); la creación de tokens sí es admin-only por sesión, pero
/// se mantiene junto al ingest para que ambos compartan `IUsageRepository`
/// sin duplicar wiring.
pub fn usage_routes(state: UsageRouterState) -> Router {
    let me = Router::new()
        .route("/api/usage/me", get(my_usage_handler))
        .with_state(state.clone());

    let admin = Router::new()
        .route("/api/admin/usage", get(admin_usage_handler))
        .route(
            "/api/admin/users/{id}/limit",
            get(get_limit_handler).put(set_limit_handler),
        )
        .route("/api/admin/service-tokens", post(create_service_token_handler))
        .with_state(state)
        .route_layer(from_fn(require_admin_middleware));

    me.merge(admin)
}

// ---------------------------------------------------------------------
// Ingest de emisores externos (Fase ledger — tarea C1)
// ---------------------------------------------------------------------

/// Body de `POST /api/usage/ingest`.
#[derive(Debug, Deserialize)]
pub(super) struct IngestRequest {
    engine: String,
    model: Option<String>,
    provider: Option<String>,
    user_id: Option<String>,
    profile_id: Option<String>,
    project_id: Option<String>,
    #[serde(default)]
    tokens_in: i64,
    #[serde(default)]
    tokens_out: i64,
    #[serde(default)]
    cache_read: i64,
    #[serde(default)]
    cache_write: i64,
    cost_usd: f64,
    ts_ms: TimestampMs,
    source: String,
    idempotency_key: Option<String>,
}

impl From<IngestRequest> for IngestUsageEvent {
    fn from(r: IngestRequest) -> Self {
        IngestUsageEvent {
            engine: r.engine,
            model: r.model,
            provider: r.provider,
            user_id: r.user_id,
            profile_id: r.profile_id,
            project_id: r.project_id,
            tokens_in: r.tokens_in,
            tokens_out: r.tokens_out,
            cache_read: r.cache_read,
            cache_write: r.cache_write,
            cost_usd: r.cost_usd,
            ts_ms: r.ts_ms,
            source: r.source,
            idempotency_key: r.idempotency_key,
        }
    }
}

/// Extrae el token `Bearer <token>` del header `Authorization`. Distinto del
/// extractor de sesión JWT (`aionui_auth::extract_token_from_headers`): este
/// endpoint NUNCA acepta cookies de sesión, solo el header explícito — un
/// service token no debe poder colarse vía sesión de navegador ni viceversa.
fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
}

/// `POST /api/usage/ingest` — graba un evento de un emisor externo (pipeline de
/// preventa, Hermes). Fail-closed: sin `Authorization: Bearer <service token>`
/// válido y activo → 401, no se graba nada. NUNCA acepta sesión de usuario.
async fn ingest_handler(
    State(state): State<UsageRouterState>,
    headers: HeaderMap,
    body: Result<Json<IngestRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<ApiResponse<IngestResult>>, ApiError> {
    let token = extract_bearer(&headers).ok_or_else(|| ApiError::Unauthorized("service token required".into()))?;
    let token_hash = hash_service_token(token);
    let service_token = state
        .usage_repo
        .find_active_service_token_by_hash(&token_hash)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::Unauthorized("invalid or inactive service token".into()))?;

    let Json(req) = body.map_err(ApiError::from)?;
    if req.engine.trim().is_empty() {
        return Err(ApiError::BadRequest("engine must not be empty".into()));
    }
    if req.cost_usd < 0.0 {
        return Err(ApiError::BadRequest("cost_usd must be >= 0".into()));
    }

    tracing::info!(
        service_token = %service_token.name,
        engine = %req.engine,
        source = %req.source,
        "usage ingest event accepted"
    );

    let result = state
        .usage_repo
        .ingest_event(IngestUsageEvent::from(req))
        .await
        .map_err(db_err)?;
    Ok(Json(ApiResponse::ok(result)))
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateServiceTokenRequest {
    name: String,
}

/// `POST /api/admin/service-tokens` — crea un service token para un emisor
/// externo (admin-only, sesión de usuario). El token en claro solo se devuelve
/// en ESTA respuesta; el repositorio únicamente persiste su hash.
async fn create_service_token_handler(
    State(state): State<UsageRouterState>,
    body: Result<Json<CreateServiceTokenRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<ApiResponse<CreatedServiceToken>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name must not be empty".into()));
    }
    let created = state.usage_repo.create_service_token(name).await.map_err(db_err)?;
    Ok(Json(ApiResponse::ok(created)))
}

/// Router del ingest — deliberadamente SIN `auth_middleware` de sesión: el
/// gate es el token de servicio, verificado dentro del handler (fail-closed).
/// Móntalo directo en el router raíz, no dentro de la capa autenticada.
pub fn ingest_routes(state: UsageRouterState) -> Router {
    Router::new()
        .route("/api/usage/ingest", post(ingest_handler))
        .with_state(state)
}
