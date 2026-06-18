#![allow(clippy::disallowed_types)]
//! Endpoints de administración RBAC (Alinea Fase 2 #5): gestión de roles.
//!
//! Todas las rutas viven bajo `auth_middleware` (puebla `CurrentUser`) y un gate
//! [`require_admin`] propio. Cubren el panel de administración del SaaS:
//!
//! | Método | Path                                   | Acción                          |
//! |--------|----------------------------------------|---------------------------------|
//! | GET    | `/api/admin/users`                     | Listar usuarios con `roles[]`   |
//! | GET    | `/api/admin/roles`                     | Catálogo de roles (6 del seed)  |
//! | POST   | `/api/admin/users/{id}/roles`          | Asignar rol `{ "role": "..." }` |
//! | DELETE | `/api/admin/users/{id}/roles/{role}`   | Quitar rol                      |
//!
//! Modelo **multi-rol** (tabla `user_roles`, M:N). Las respuestas se envuelven en
//! `ApiResponse<T>` (`{ success, data }`), igual que el resto de la API.

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, Path, Request, State};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Router;
use serde::{Deserialize, Serialize};

use aionui_api_types::ApiResponse;
use aionui_common::{ApiError, TimestampMs};
use aionui_db::{DbError, IUserRepository, RoleRemoval, models::Role};

use crate::middleware::CurrentUser;

/// Rol con privilegios de administración del SaaS.
const ADMIN_ROLE: &str = "admin";

/// Estado compartido por los handlers admin.
#[derive(Clone)]
pub struct AdminRouterState {
    pub user_repo: Arc<dyn IUserRepository>,
    /// En `local` (desktop single-user) el gate admin se omite: el dueño de la
    /// máquina tiene control total. En SaaS (`local = false`) se exige rol `admin`.
    pub local: bool,
}

fn db_err(err: DbError) -> ApiError {
    match err {
        DbError::NotFound(msg) => ApiError::NotFound(msg),
        DbError::Conflict(msg) => ApiError::Conflict(msg),
        DbError::Query(e) => ApiError::Internal(format!("Database error: {e}")),
        DbError::Migration(e) => ApiError::Internal(format!("Migration error: {e}")),
        DbError::Init(msg) => ApiError::Internal(format!("Database init error: {msg}")),
    }
}

/// Usuario tal como lo ve el panel admin: datos básicos + sus roles asignados.
#[derive(Serialize)]
struct AdminUser {
    id: String,
    username: String,
    email: Option<String>,
    created_at: TimestampMs,
    last_login: Option<TimestampMs>,
    /// Roles asignados (RBAC eje 1). Vacío si no tiene ninguno.
    roles: Vec<String>,
}

#[derive(Deserialize)]
struct AssignRoleRequest {
    role: String,
}

/// Respuesta de asignar/quitar: el id del usuario y su set de roles ACTUALIZADO,
/// para que la UI refresque los chips sin un GET extra.
#[derive(Serialize)]
struct UserRolesResponse {
    id: String,
    roles: Vec<String>,
}

/// Gate de administrador. Corre DESPUÉS de `auth_middleware` (lee `CurrentUser`
/// de las extensiones). En `local` se permite siempre; en SaaS exige el rol
/// `admin`. Fail-closed: sin `CurrentUser` o sin rol admin → 403.
async fn require_admin(State(local): State<bool>, request: Request, next: Next) -> Result<Response, ApiError> {
    if local {
        return Ok(next.run(request).await);
    }
    let is_admin = request
        .extensions()
        .get::<CurrentUser>()
        .is_some_and(|u| u.roles.iter().any(|r| r == ADMIN_ROLE));
    if !is_admin {
        return Err(ApiError::Forbidden("Admin role required".into()));
    }
    Ok(next.run(request).await)
}

/// `GET /api/admin/users` — usuarios con su `roles[]`.
async fn list_users(State(state): State<AdminRouterState>) -> Result<Json<ApiResponse<Vec<AdminUser>>>, ApiError> {
    let users = state.user_repo.list_users().await.map_err(db_err)?;
    let mut out = Vec::with_capacity(users.len());
    for u in users {
        // N+1 aceptable: el panel admin tiene pocos usuarios y no es hot-path.
        let roles = state.user_repo.get_user_roles(&u.id).await.map_err(db_err)?;
        out.push(AdminUser {
            id: u.id,
            username: u.username,
            email: u.email,
            created_at: u.created_at,
            last_login: u.last_login,
            roles,
        });
    }
    Ok(Json(ApiResponse::ok(out)))
}

/// `GET /api/admin/roles` — catálogo de roles (fuente de verdad para la UI).
async fn list_roles(State(state): State<AdminRouterState>) -> Result<Json<ApiResponse<Vec<Role>>>, ApiError> {
    let roles = state.user_repo.list_roles().await.map_err(db_err)?;
    Ok(Json(ApiResponse::ok(roles)))
}

/// `POST /api/admin/users/{id}/roles` con `{ "role": "gerencia" }`.
/// Idempotente. 404 si el usuario no existe; 400 si el rol no está en el catálogo.
async fn assign_role(
    State(state): State<AdminRouterState>,
    Path(user_id): Path<String>,
    body: Result<Json<AssignRoleRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<UserRolesResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let role = req.role.trim();
    if role.is_empty() {
        return Err(ApiError::BadRequest("role must not be empty".into()));
    }
    ensure_user_exists(&state, &user_id).await?;
    ensure_known_role(&state, role).await?;
    state.user_repo.assign_role(&user_id, role).await.map_err(db_err)?;
    respond_with_roles(&state, user_id).await
}

/// `DELETE /api/admin/users/{id}/roles/{role}`. Idempotente. 404 si el usuario no
/// existe. Invariante anti-lockout (atómica en el repo): quitar el rol admin del
/// último administrador se bloquea con 409 — nunca deja el sistema sin admins.
async fn remove_role(
    State(state): State<AdminRouterState>,
    Path((user_id, role)): Path<(String, String)>,
) -> Result<Json<ApiResponse<UserRolesResponse>>, ApiError> {
    ensure_user_exists(&state, &user_id).await?;

    match state.user_repo.remove_role(&user_id, &role).await.map_err(db_err)? {
        RoleRemoval::WouldLeaveNoAdmins => Err(ApiError::Conflict(
            "No puedes quitar el último rol admin del sistema".into(),
        )),
        RoleRemoval::Removed => respond_with_roles(&state, user_id).await,
    }
}

async fn ensure_user_exists(state: &AdminRouterState, user_id: &str) -> Result<(), ApiError> {
    if state.user_repo.find_by_id(user_id).await.map_err(db_err)?.is_none() {
        return Err(ApiError::NotFound(format!("User '{user_id}' not found")));
    }
    Ok(())
}

async fn ensure_known_role(state: &AdminRouterState, role: &str) -> Result<(), ApiError> {
    let known = state.user_repo.list_roles().await.map_err(db_err)?;
    if !known.iter().any(|r| r.id == role) {
        return Err(ApiError::BadRequest(format!("Unknown role '{role}'")));
    }
    Ok(())
}

async fn respond_with_roles(
    state: &AdminRouterState,
    user_id: String,
) -> Result<Json<ApiResponse<UserRolesResponse>>, ApiError> {
    let roles = state.user_repo.get_user_roles(&user_id).await.map_err(db_err)?;
    Ok(Json(ApiResponse::ok(UserRolesResponse { id: user_id, roles })))
}

/// Construye el sub-router admin con su gate `require_admin` ya aplicado.
/// El llamador debe envolverlo además con `auth_middleware` (capa externa) para
/// que `CurrentUser` esté poblado cuando corra el gate.
pub fn admin_routes(state: AdminRouterState) -> Router {
    let local = state.local;
    Router::new()
        .route("/api/admin/users", get(list_users))
        .route("/api/admin/roles", get(list_roles))
        .route("/api/admin/users/{id}/roles", post(assign_role))
        .route("/api/admin/users/{id}/roles/{role}", delete(remove_role))
        .with_state(state)
        .route_layer(from_fn_with_state(local, require_admin))
}
