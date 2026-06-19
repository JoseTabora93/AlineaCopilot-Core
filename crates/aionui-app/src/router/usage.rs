#![allow(clippy::disallowed_types)] // ApiError es legítimo en handlers de rutas.
//! Endpoints del ledger de consumos $ (Alinea Fase 2 #3 — blueprint §12).
//!
//! - `GET /api/usage/me`                  — "mi consumo" (cualquier usuario auth).
//! - `GET /api/admin/usage`               — consumo de todos (admin-only).
//! - `PUT /api/admin/users/{id}/limit`    — fijar límite $ de un usuario (admin-only).
//!
//! La ventana por defecto es **rolling 30 días** (override con `?since_ms=`).
//! El gate admin reusa `require_admin_middleware` (en local, `CurrentUser` ya trae
//! rol admin, así que funciona sin flag extra). Respuestas en `ApiResponse<T>`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::middleware::from_fn;
use axum::routing::get;
use axum::{Extension, Router};
use serde::{Deserialize, Serialize};

use aionui_api_types::ApiResponse;
use aionui_auth::{CurrentUser, require_admin_middleware};
use aionui_common::{ApiError, TimestampMs};
use aionui_db::{
    IUsageRepository,
    models::{UsageSummary, UserUsageLimit},
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
}

fn window_start(q: &WindowQuery) -> TimestampMs {
    q.since_ms.unwrap_or_else(|| (aionui_common::now_ms() - DEFAULT_WINDOW_MS).max(0))
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
    let usage = state.usage_repo.summary_for_user(&current.id, since).await.map_err(db_err)?;
    let limit = state.usage_repo.get_limit(&current.id).await.map_err(db_err)?;
    Ok(Json(ApiResponse::ok(MyUsageResponse { usage, limit, since_ms: since })))
}

/// `GET /api/admin/usage` — consumo agregado de todos los usuarios (admin-only).
async fn admin_usage_handler(
    State(state): State<UsageRouterState>,
    Query(q): Query<WindowQuery>,
) -> Result<Json<ApiResponse<Vec<AdminUsageRow>>>, ApiError> {
    let since = window_start(&q);
    let rows = state.usage_repo.summary_all_users(since).await.map_err(db_err)?;
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
        .with_state(state)
        .route_layer(from_fn(require_admin_middleware));

    me.merge(admin)
}
