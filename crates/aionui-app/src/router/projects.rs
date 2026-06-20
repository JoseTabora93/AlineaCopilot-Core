#![allow(clippy::disallowed_types)] // ApiError es legítimo en handlers de rutas.
//! Endpoints REST del módulo Proyectos (Alinea Fase 2 #2 — slice 3).
//!
//! - `GET/POST   /api/projects`                     — listar (filtrado por membresía) / crear
//! - `GET/PATCH  /api/projects/{id}`                — detalle (miembro) / editar (owner)
//! - `GET/POST   /api/projects/{id}/members`        — listar (miembro) / añadir (owner)
//! - `DELETE     /api/projects/{id}/members/{uid}`  — quitar miembro (owner, anti-lockout)
//! - `GET        /api/pipeline-templates`           — catálogo de plantillas (cualquier auth)
//!
//! 🔒 Membresía FAIL-CLOSED: todo acceso a un proyecto exige una entrada en
//! `resource_acl` (resource_type='project'). El que crea es 'owner'.

use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::routing::{delete, get};
use axum::{Extension, Router};
use serde::Deserialize;

use aionui_api_types::ApiResponse;
use aionui_auth::CurrentUser;
use aionui_common::ApiError;
use aionui_db::models::{PipelineTemplateRow, ProjectRow, ResourceAclRow};
use aionui_db::{IProjectRepository, IResourceAclRepository, IUserRepository, MemberRevoke, NewProject, ProjectUpdate};

#[derive(Clone)]
pub struct ProjectRouterState {
    pub project_repo: Arc<dyn IProjectRepository>,
    pub acl_repo: Arc<dyn IResourceAclRepository>,
    /// Para validar que el `user_id` objetivo de `add_member` existe.
    pub user_repo: Arc<dyn IUserRepository>,
}

fn db_err(err: aionui_db::DbError) -> ApiError {
    ApiError::Internal(format!("Database error: {err}"))
}

/// Devuelve el `perm` del usuario sobre el proyecto, o 403 si no es miembro.
async fn require_member(state: &ProjectRouterState, project_id: &str, user_id: &str) -> Result<String, ApiError> {
    match state
        .acl_repo
        .get_perm("project", project_id, user_id)
        .await
        .map_err(db_err)?
    {
        Some(perm) => Ok(perm),
        None => Err(ApiError::Forbidden("Not a member of this project".into())),
    }
}

/// 403 si el usuario no es `owner` del proyecto.
async fn require_owner(state: &ProjectRouterState, project_id: &str, user_id: &str) -> Result<(), ApiError> {
    if require_member(state, project_id, user_id).await? != "owner" {
        return Err(ApiError::Forbidden("Owner permission required".into()));
    }
    Ok(())
}

// ── Proyectos ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ListQuery {
    include_archived: Option<bool>,
}

async fn list_projects(
    State(state): State<ProjectRouterState>,
    Extension(current): Extension<CurrentUser>,
    Query(q): Query<ListQuery>,
) -> Result<Json<ApiResponse<Vec<ProjectRow>>>, ApiError> {
    let rows = state
        .project_repo
        .list_for_user(&current.id, q.include_archived.unwrap_or(false))
        .await
        .map_err(db_err)?;
    Ok(Json(ApiResponse::ok(rows)))
}

#[derive(Debug, Deserialize)]
struct CreateProjectRequest {
    name: String,
    description: Option<String>,
    project_type: Option<String>,
}

async fn create_project(
    State(state): State<ProjectRouterState>,
    Extension(current): Extension<CurrentUser>,
    body: Result<Json<CreateProjectRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<ProjectRow>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    let project = state
        .project_repo
        .create(NewProject {
            name: req.name.trim().to_string(),
            description: req.description,
            project_type: req.project_type.unwrap_or_else(|| "generico".to_string()),
            created_by: current.id.clone(),
        })
        .await
        .map_err(db_err)?;
    // El creador es OWNER (membresía en resource_acl).
    state
        .acl_repo
        .grant("project", &project.id, &current.id, "owner")
        .await
        .map_err(db_err)?;
    Ok(Json(ApiResponse::ok(project)))
}

async fn get_project(
    State(state): State<ProjectRouterState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<ProjectRow>>, ApiError> {
    require_member(&state, &id, &current.id).await?;
    let project = state
        .project_repo
        .get(&id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::NotFound(format!("Project {id} not found")))?;
    Ok(Json(ApiResponse::ok(project)))
}

#[derive(Debug, Deserialize)]
struct UpdateProjectRequest {
    name: Option<String>,
    /// `Some(None)` limpia la descripción; ausente = sin cambio.
    description: Option<Option<String>>,
    status: Option<String>,
}

async fn update_project(
    State(state): State<ProjectRouterState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
    body: Result<Json<UpdateProjectRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<ProjectRow>>, ApiError> {
    require_owner(&state, &id, &current.id).await?;
    let Json(req) = body.map_err(ApiError::from)?;
    if let Some(ref s) = req.status
        && s != "active"
        && s != "archived"
    {
        return Err(ApiError::BadRequest("status must be 'active' or 'archived'".into()));
    }
    state
        .project_repo
        .update(
            &id,
            ProjectUpdate {
                name: req.name,
                description: req.description,
                status: req.status,
            },
        )
        .await
        .map_err(db_err)?;
    let project = state
        .project_repo
        .get(&id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::NotFound(format!("Project {id} not found")))?;
    Ok(Json(ApiResponse::ok(project)))
}

// ── Miembros (sobre resource_acl) ────────────────────────────────────

async fn list_members(
    State(state): State<ProjectRouterState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<ResourceAclRow>>>, ApiError> {
    require_member(&state, &id, &current.id).await?;
    let members = state.acl_repo.list_principals("project", &id).await.map_err(db_err)?;
    Ok(Json(ApiResponse::ok(members)))
}

#[derive(Debug, Deserialize)]
struct AddMemberRequest {
    user_id: String,
    /// 'read' | 'write' | 'owner'. Default 'read'.
    perm: Option<String>,
}

async fn add_member(
    State(state): State<ProjectRouterState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
    body: Result<Json<AddMemberRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<Vec<ResourceAclRow>>>, ApiError> {
    require_owner(&state, &id, &current.id).await?;
    let Json(req) = body.map_err(ApiError::from)?;
    let target = req.user_id.trim();
    if target.is_empty() {
        return Err(ApiError::BadRequest("user_id is required".into()));
    }
    let perm = req.perm.unwrap_or_else(|| "read".to_string());
    if !matches!(perm.as_str(), "read" | "write" | "owner") {
        return Err(ApiError::BadRequest("perm must be 'read', 'write' or 'owner'".into()));
    }
    // El usuario objetivo debe existir (evita ACLs fantasma con IDs inexistentes).
    if state.user_repo.find_by_id(target).await.map_err(db_err)?.is_none() {
        return Err(ApiError::BadRequest("target user does not exist".into()));
    }
    // Anti-lockout: no degradar al último owner (incluido a sí mismo) a un perm menor.
    if perm != "owner" {
        let members = state.acl_repo.list_principals("project", &id).await.map_err(db_err)?;
        let owner_count = members
            .iter()
            .filter(|m| m.principal_type == "user" && m.perm == "owner")
            .count();
        let target_is_owner = members
            .iter()
            .any(|m| m.principal_type == "user" && m.principal_id == target && m.perm == "owner");
        if target_is_owner && owner_count <= 1 {
            return Err(ApiError::BadRequest(
                "Cannot demote the last owner of the project".into(),
            ));
        }
    }
    state
        .acl_repo
        .grant("project", &id, target, &perm)
        .await
        .map_err(db_err)?;
    let members = state.acl_repo.list_principals("project", &id).await.map_err(db_err)?;
    Ok(Json(ApiResponse::ok(members)))
}

async fn remove_member(
    State(state): State<ProjectRouterState>,
    Extension(current): Extension<CurrentUser>,
    Path((id, uid)): Path<(String, String)>,
) -> Result<Json<ApiResponse<Vec<ResourceAclRow>>>, ApiError> {
    require_owner(&state, &id, &current.id).await?;
    // Anti-lockout ATÓMICO: el repo no borra si es el último owner (sin TOCTOU).
    match state
        .acl_repo
        .try_revoke_project_member(&id, &uid)
        .await
        .map_err(db_err)?
    {
        MemberRevoke::Revoked => {}
        MemberRevoke::WouldLeaveNoOwner => {
            return Err(ApiError::BadRequest(
                "Cannot remove the last owner of the project".into(),
            ));
        }
    }
    let members = state.acl_repo.list_principals("project", &id).await.map_err(db_err)?;
    Ok(Json(ApiResponse::ok(members)))
}

// ── Plantillas (catálogo; cualquier usuario autenticado) ─────────────

#[derive(Debug, Deserialize)]
struct TemplatesQuery {
    project_type: Option<String>,
}

async fn list_templates(
    State(state): State<ProjectRouterState>,
    Query(q): Query<TemplatesQuery>,
) -> Result<Json<ApiResponse<Vec<PipelineTemplateRow>>>, ApiError> {
    let rows = state
        .project_repo
        .list_templates(q.project_type.as_deref())
        .await
        .map_err(db_err)?;
    Ok(Json(ApiResponse::ok(rows)))
}

/// Router de Proyectos. El llamador añade `auth_middleware` (capa externa) para
/// poblar `CurrentUser`; la autorización fina (membresía/owner) vive en cada handler.
pub fn project_routes(state: ProjectRouterState) -> Router {
    Router::new()
        .route("/api/projects", get(list_projects).post(create_project))
        .route("/api/projects/{id}", get(get_project).patch(update_project))
        .route("/api/projects/{id}/members", get(list_members).post(add_member))
        .route("/api/projects/{id}/members/{uid}", delete(remove_member))
        .route("/api/pipeline-templates", get(list_templates))
        .with_state(state)
}
