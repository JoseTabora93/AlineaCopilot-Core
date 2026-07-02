#![allow(clippy::disallowed_types)] // ApiError es legítimo en handlers de rutas.
//! Endpoints de Perfiles de agente (Motor MULTI-PERFIL — plan hermes-alinea, tarea A1).
//!
//! - `GET/POST   /api/admin/profiles`      — listar todos (incl. inactivos) / crear (admin-only).
//! - `GET/PATCH/DELETE /api/admin/profiles/{id}` — detalle / editar / borrar (admin-only).
//! - `GET        /api/profiles`            — lista SOLO los perfiles a los que el usuario
//!   autenticado tiene acceso, vía `resource_acl` (resource_type='agent_profile'), por
//!   grant directo de usuario O por cualquiera de sus roles.
//!
//! 🔒 Gate FAIL-CLOSED (replica `ConversationService::update`,
//! `crates/aionui-conversation/src/service.rs:1623-1647`): sin `resource_acl`
//! wireado, o sin ninguna entrada de membresía (user O role), el usuario ve
//! una lista vacía — nunca se listan perfiles "por defecto".
//!
//! El JSON `definition` se valida contra el esquema PERFIL v1
//! (`aionui_db::profile_schema::validate_profile_definition`,
//! `PROFILE_SCHEMA.md` en la raíz del repo) al crear y al actualizar.

use std::collections::HashSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::middleware::from_fn;
use axum::routing::get;
use axum::{Extension, Router};
use serde::{Deserialize, Serialize};

use aionui_api_types::ApiResponse;
use aionui_auth::{CurrentUser, require_admin_middleware};
use aionui_common::{ApiError, TimestampMs};
use aionui_db::models::{AgentProfileRow, UsageSummary};
use aionui_db::profile_schema::{ProfileCapsView, extract_caps, validate_profile_definition};
use aionui_db::{
    AgentProfileUpdate, DbError, IAgentProfileRepository, IResourceAclRepository, IUsageRepository, NewAgentProfile,
};

/// Ventana por defecto del gasto por perfil: rolling 30 días en ms — misma
/// semántica que el ledger de usuarios (`super::usage`) y que el pre-flight
/// de `turn_orchestrator` (`caps.period: "month"` = rolling 30 días).
const DEFAULT_WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Clone)]
pub struct ProfileRouterState {
    pub profile_repo: Arc<dyn IAgentProfileRepository>,
    pub acl_repo: Arc<dyn IResourceAclRepository>,
    /// Ledger de consumos — gasto agregado por perfil para el dashboard
    /// (`GET /api/admin/profiles/{id}/usage`, tarea C4).
    pub usage_repo: Arc<dyn IUsageRepository>,
}

fn db_err(err: DbError) -> ApiError {
    match err {
        DbError::NotFound(msg) => ApiError::NotFound(msg),
        DbError::Conflict(msg) => ApiError::Conflict(msg),
        other => ApiError::Internal(format!("Database error: {other}")),
    }
}

fn schema_err(err: aionui_db::ProfileSchemaError) -> ApiError {
    ApiError::BadRequest(err.0)
}

// ── Admin: CRUD completo ──────────────────────────────────────────────

async fn admin_list_profiles(
    State(state): State<ProfileRouterState>,
) -> Result<Json<ApiResponse<Vec<AgentProfileRow>>>, ApiError> {
    let rows = state.profile_repo.list_all(true).await.map_err(db_err)?;
    Ok(Json(ApiResponse::ok(rows)))
}

#[derive(Debug, Deserialize)]
struct CreateProfileRequest {
    name: String,
    label: String,
    /// JSON crudo de PERFIL v1 (objeto completo, no serializado a string por
    /// el cliente — se re-serializa aquí para persistirlo como TEXT).
    definition: serde_json::Value,
}

async fn admin_create_profile(
    State(state): State<ProfileRouterState>,
    body: Result<Json<CreateProfileRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<AgentProfileRow>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let name = req.name.trim().to_string();
    let label = req.label.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    if label.is_empty() {
        return Err(ApiError::BadRequest("label is required".into()));
    }
    let definition = serde_json::to_string(&req.definition)
        .map_err(|e| ApiError::BadRequest(format!("definition must be valid JSON: {e}")))?;
    validate_profile_definition(&definition, &name).map_err(schema_err)?;

    let profile = state
        .profile_repo
        .create(NewAgentProfile {
            name,
            label,
            definition,
        })
        .await
        .map_err(db_err)?;
    Ok(Json(ApiResponse::ok(profile)))
}

async fn admin_get_profile(
    State(state): State<ProfileRouterState>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<AgentProfileRow>>, ApiError> {
    let profile = state
        .profile_repo
        .get(&id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::NotFound(format!("Agent profile {id} not found")))?;
    Ok(Json(ApiResponse::ok(profile)))
}

#[derive(Debug, Deserialize)]
struct UpdateProfileRequest {
    label: Option<String>,
    /// Si se manda, reemplaza `definition` completo (se re-valida contra el
    /// esquema con el `name` YA persistido de la fila — `name` es inmutable).
    definition: Option<serde_json::Value>,
    is_active: Option<bool>,
}

async fn admin_update_profile(
    State(state): State<ProfileRouterState>,
    Path(id): Path<String>,
    body: Result<Json<UpdateProfileRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<AgentProfileRow>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let existing = state
        .profile_repo
        .get(&id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::NotFound(format!("Agent profile {id} not found")))?;

    let definition = req
        .definition
        .map(|v| {
            let raw = serde_json::to_string(&v)
                .map_err(|e| ApiError::BadRequest(format!("definition must be valid JSON: {e}")))?;
            validate_profile_definition(&raw, &existing.name).map_err(schema_err)?;
            Ok::<String, ApiError>(raw)
        })
        .transpose()?;

    if let Some(ref label) = req.label
        && label.trim().is_empty()
    {
        return Err(ApiError::BadRequest("label must not be empty".into()));
    }

    state
        .profile_repo
        .update(
            &id,
            AgentProfileUpdate {
                label: req.label.map(|l| l.trim().to_string()),
                definition,
                is_active: req.is_active,
            },
        )
        .await
        .map_err(db_err)?;

    let profile = state
        .profile_repo
        .get(&id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::NotFound(format!("Agent profile {id} not found")))?;
    Ok(Json(ApiResponse::ok(profile)))
}

async fn admin_delete_profile(
    State(state): State<ProfileRouterState>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    state.profile_repo.delete(&id).await.map_err(db_err)?;
    Ok(Json(ApiResponse::ok(())))
}

// ── Admin: gasto por perfil (tarea C4) ───────────────────────────────

#[derive(Debug, Deserialize)]
struct ProfileUsageQuery {
    /// Inicio de la ventana (ms epoch). Default: hace 30 días (rolling).
    since_ms: Option<i64>,
}

/// Respuesta de `GET /api/admin/profiles/{id}/usage`: gasto agregado del
/// perfil en la ventana + su `caps` (para que el dashboard muestre gasto vs
/// techo sin re-parsear `definition`). `usage.user_id` trae el `name` del
/// perfil (clave de agregación reutilizada — ver
/// `IUsageRepository::summary_for_profile`).
#[derive(Serialize)]
struct ProfileUsageResponse {
    profile_id: String,
    /// `name` del perfil — el valor que viaja en `usage_events.profile_id`.
    profile_name: String,
    usage: UsageSummary,
    /// `None` si el `definition` no trae `caps` parseables (sin techo de perfil).
    caps: Option<ProfileCapsView>,
    since_ms: TimestampMs,
}

/// `GET /api/admin/profiles/{id}/usage` — gasto del perfil en la ventana
/// rolling (30 días por defecto) + su cap, para el dashboard admin.
async fn admin_profile_usage(
    State(state): State<ProfileRouterState>,
    Path(id): Path<String>,
    Query(q): Query<ProfileUsageQuery>,
) -> Result<Json<ApiResponse<ProfileUsageResponse>>, ApiError> {
    let profile = state
        .profile_repo
        .get(&id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::NotFound(format!("Agent profile {id} not found")))?;

    let since = q
        .since_ms
        .unwrap_or_else(|| (aionui_common::now_ms() - DEFAULT_WINDOW_MS).max(0));
    // El ledger atribuye por `name` (ver `usage_events.profile_id`), no por
    // el `id` de la fila — mismo valor que emite el pipeline/ingest.
    let usage = state
        .usage_repo
        .summary_for_profile(&profile.name, since)
        .await
        .map_err(db_err)?;
    let caps = extract_caps(&profile.definition);

    Ok(Json(ApiResponse::ok(ProfileUsageResponse {
        profile_id: profile.id,
        profile_name: profile.name,
        usage,
        caps,
        since_ms: since,
    })))
}

// ── Usuario: visibilidad por ACL (fail-closed) ───────────────────────

/// `GET /api/profiles` — perfiles ACTIVOS a los que el usuario autenticado
/// tiene acceso: grant directo (`principal_type='user'`) O grant a
/// cualquiera de sus roles (`principal_type='role'`) en `resource_acl` con
/// `resource_type='agent_profile'`.
///
/// 🔒 FAIL-CLOSED: si el usuario no tiene NINGUNA entrada (ni por user ni por
/// rol), la respuesta es una lista vacía — nunca se asume acceso por defecto.
/// Mismo espíritu que el gate de `ConversationService::update` (línea 1623 de
/// `crates/aionui-conversation/src/service.rs`): sin membresía verificable,
/// no hay acceso.
/// Fila de `GET /api/profiles`: el row completo + `caps` aplanado como campo
/// top-level (tarea C4). Consumidores externos (p.ej. el pipeline de preventa
/// leyendo `caps.hard_usd` del perfil `preventa` para su `PREVENTA_CAP_USD`)
/// no necesitan re-parsear el JSON `definition`. Retro-compatible: el
/// `flatten` conserva todos los campos que ya leían los clientes existentes.
#[derive(Serialize)]
struct ProfileListItem {
    #[serde(flatten)]
    profile: AgentProfileRow,
    /// `None` si el `definition` no trae `caps` parseables.
    caps: Option<ProfileCapsView>,
}

impl From<AgentProfileRow> for ProfileListItem {
    fn from(profile: AgentProfileRow) -> Self {
        let caps = extract_caps(&profile.definition);
        Self { profile, caps }
    }
}

async fn list_visible_profiles(
    State(state): State<ProfileRouterState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<ApiResponse<Vec<ProfileListItem>>>, ApiError> {
    // Admins ven todo el catálogo activo sin depender de grants explícitos —
    // consistente con el resto del panel admin (evita que un admin recién
    // creado se quede sin ver perfiles hasta que alguien le otorgue ACL).
    if current.is_admin() {
        let rows = state.profile_repo.list_all(false).await.map_err(db_err)?;
        return Ok(Json(ApiResponse::ok(
            rows.into_iter().map(ProfileListItem::from).collect(),
        )));
    }

    let mut profile_ids: HashSet<String> = HashSet::new();

    // `IResourceAclRepository::list_principals` está indexado por
    // (resource_type, resource_id), no al revés — no sirve directamente para
    // "¿a qué perfiles tiene acceso este usuario?". En su lugar, se resuelve
    // por perfil: se listan TODOS los perfiles activos y se filtra por
    // membresía (user directo O alguno de sus roles). El catálogo de
    // perfiles es pequeño (6-10 role-packs, nunca miles — decisión de
    // arquitectura del plan), así que el costo de esta verificación por
    // perfil es aceptable y evita una tabla/índice nuevos.
    let all_active = state.profile_repo.list_all(false).await.map_err(db_err)?;
    for profile in &all_active {
        let is_member = state
            .acl_repo
            .is_member("agent_profile", &profile.id, &current.id)
            .await
            .map_err(db_err)?;
        if is_member {
            profile_ids.insert(profile.id.clone());
            continue;
        }
        for role in &current.roles {
            if state
                .acl_repo
                .is_member("agent_profile", &profile.id, role)
                .await
                .map_err(db_err)?
            {
                profile_ids.insert(profile.id.clone());
                break;
            }
        }
    }

    let visible: Vec<ProfileListItem> = all_active
        .into_iter()
        .filter(|p| profile_ids.contains(&p.id))
        .map(ProfileListItem::from)
        .collect();
    Ok(Json(ApiResponse::ok(visible)))
}

/// Router de Perfiles. El llamador añade `auth_middleware` (capa externa) para
/// poblar `CurrentUser` en TODAS las rutas (incluida `/api/admin/profiles/*`,
/// que además exige `require_admin_middleware`).
pub fn profile_routes(state: ProfileRouterState) -> Router {
    let user = Router::new()
        .route("/api/profiles", get(list_visible_profiles))
        .with_state(state.clone());

    let admin = Router::new()
        .route(
            "/api/admin/profiles",
            get(admin_list_profiles).post(admin_create_profile),
        )
        .route(
            "/api/admin/profiles/{id}",
            get(admin_get_profile)
                .patch(admin_update_profile)
                .delete(admin_delete_profile),
        )
        .route("/api/admin/profiles/{id}/usage", get(admin_profile_usage))
        .with_state(state)
        .route_layer(from_fn(require_admin_middleware));

    user.merge(admin)
}
