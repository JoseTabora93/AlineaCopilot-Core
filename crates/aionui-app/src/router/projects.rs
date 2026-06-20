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
use axum::routing::{delete, get, post};
use axum::{Extension, Router};
use serde::Deserialize;

use aionui_api_types::ApiResponse;
use aionui_auth::CurrentUser;
use aionui_common::ApiError;
use aionui_db::models::{PipelineTemplateRow, ProjectRow, ResourceAclRow, TaskArtifactRow, TaskHandoffLogRow, TaskRow};
use aionui_db::task_state::{TaskAction, TaskStatus, next_status};
use aionui_db::{
    IProjectRepository, IResourceAclRepository, ITaskRepository, IUserRepository, MemberRevoke, NewArtifact,
    NewHandoff, NewProject, NewTask, ProjectUpdate, TaskUpdate,
};
use aionui_projects::HandoffEngine;

#[derive(Clone)]
pub struct ProjectRouterState {
    pub project_repo: Arc<dyn IProjectRepository>,
    pub acl_repo: Arc<dyn IResourceAclRepository>,
    /// Para validar que el `user_id` objetivo de `add_member` existe.
    pub user_repo: Arc<dyn IUserRepository>,
    /// Tareas/subtareas del proyecto (slice 4).
    pub task_repo: Arc<dyn ITaskRepository>,
    /// Motor de handoffs: cascada de dependencias + bloqueo (slice 5).
    pub handoff: HandoffEngine,
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

// ── Tareas / subtareas (slice 4) ─────────────────────────────────────

/// Obtiene la tarea y exige que el usuario sea miembro de su proyecto (403/404).
async fn require_task_member(state: &ProjectRouterState, task_id: &str, user_id: &str) -> Result<TaskRow, ApiError> {
    let task = state
        .task_repo
        .get(task_id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::NotFound(format!("Task {task_id} not found")))?;
    require_member(state, &task.project_id, user_id).await?;
    Ok(task)
}

fn default_assignee_kind() -> String {
    "human".to_string()
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct CreateTaskRequest {
    title: String,
    instructions: Option<String>,
    parent_task_id: Option<String>,
    #[serde(default = "default_assignee_kind")]
    assignee_kind: String,
    assignee_id: Option<String>,
    #[serde(default = "default_true")]
    requires_human_review: bool,
    produces_artifact: Option<String>,
    #[serde(default)]
    order_index: i64,
    #[serde(default)]
    depends_on: Vec<String>,
}

async fn create_task(
    State(state): State<ProjectRouterState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<String>,
    body: Result<Json<CreateTaskRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<TaskRow>>, ApiError> {
    require_member(&state, &project_id, &current.id).await?;
    let Json(req) = body.map_err(ApiError::from)?;
    if req.title.trim().is_empty() {
        return Err(ApiError::BadRequest("title is required".into()));
    }
    if !matches!(req.assignee_kind.as_str(), "human" | "agent") {
        return Err(ApiError::BadRequest("assignee_kind must be 'human' or 'agent'".into()));
    }
    // Una subtarea debe pertenecer al MISMO proyecto que su padre.
    if let Some(parent_id) = &req.parent_task_id {
        let parent = state
            .task_repo
            .get(parent_id)
            .await
            .map_err(db_err)?
            .ok_or_else(|| ApiError::BadRequest("parent_task_id not found".into()))?;
        if parent.project_id != project_id {
            return Err(ApiError::BadRequest("parent task belongs to another project".into()));
        }
    }
    let task = state
        .task_repo
        .create(NewTask {
            project_id: project_id.clone(),
            parent_task_id: req.parent_task_id,
            title: req.title.trim().to_string(),
            instructions: req.instructions,
            assignee_kind: req.assignee_kind,
            assignee_id: req.assignee_id,
            requires_human_review: req.requires_human_review,
            produces_artifact: req.produces_artifact,
            order_index: req.order_index,
            created_by: current.id.clone(),
            depends_on: req.depends_on,
        })
        .await
        .map_err(db_err)?;
    // Si nace con dependencias no satisfechas, el engine la deja 'blocked'.
    if state.handoff.block_if_unsatisfied(&task.id).await.map_err(db_err)? {
        let blocked = state
            .task_repo
            .get(&task.id)
            .await
            .map_err(db_err)?
            .ok_or_else(|| ApiError::NotFound(format!("Task {} not found", task.id)))?;
        return Ok(Json(ApiResponse::ok(blocked)));
    }
    Ok(Json(ApiResponse::ok(task)))
}

async fn list_tasks(
    State(state): State<ProjectRouterState>,
    Extension(current): Extension<CurrentUser>,
    Path(project_id): Path<String>,
) -> Result<Json<ApiResponse<Vec<TaskRow>>>, ApiError> {
    require_member(&state, &project_id, &current.id).await?;
    let tasks = state.task_repo.list_by_project(&project_id).await.map_err(db_err)?;
    Ok(Json(ApiResponse::ok(tasks)))
}

async fn get_task(
    State(state): State<ProjectRouterState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<TaskRow>>, ApiError> {
    let task = require_task_member(&state, &id, &current.id).await?;
    Ok(Json(ApiResponse::ok(task)))
}

#[derive(Debug, Deserialize)]
struct UpdateTaskRequest {
    title: Option<String>,
    instructions: Option<Option<String>>,
    assignee_kind: Option<String>,
    assignee_id: Option<Option<String>>,
    requires_human_review: Option<bool>,
    order_index: Option<i64>,
}

async fn update_task(
    State(state): State<ProjectRouterState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
    body: Result<Json<UpdateTaskRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<TaskRow>>, ApiError> {
    require_task_member(&state, &id, &current.id).await?;
    let Json(req) = body.map_err(ApiError::from)?;
    if let Some(ref k) = req.assignee_kind
        && !matches!(k.as_str(), "human" | "agent")
    {
        return Err(ApiError::BadRequest("assignee_kind must be 'human' or 'agent'".into()));
    }
    state
        .task_repo
        .update(
            &id,
            TaskUpdate {
                title: req.title,
                instructions: req.instructions,
                assignee_kind: req.assignee_kind,
                assignee_id: req.assignee_id,
                requires_human_review: req.requires_human_review,
                order_index: req.order_index,
            },
        )
        .await
        .map_err(db_err)?;
    let task = state
        .task_repo
        .get(&id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::NotFound(format!("Task {id} not found")))?;
    Ok(Json(ApiResponse::ok(task)))
}

#[derive(Debug, Deserialize)]
struct TransitionRequest {
    /// 'start' | 'submit' | 'approve' | 'reject' | 'reopen'.
    action: String,
}

async fn transition_task(
    State(state): State<ProjectRouterState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
    body: Result<Json<TransitionRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<TaskRow>>, ApiError> {
    let task = require_task_member(&state, &id, &current.id).await?;
    // NOTA (precondición slice 7): hoy cualquier miembro puede transicionar
    // cualquier tarea (board 100% humano). Cuando OpenClaw ejecute tareas de
    // agente, `approve`/`reject` NO debe poder hacerlo el propio agente ni el
    // assignee de la tarea de agente — debe ser un humano revisor distinto.
    let Json(req) = body.map_err(ApiError::from)?;
    let action = TaskAction::parse(&req.action).ok_or_else(|| ApiError::BadRequest("invalid action".into()))?;
    let from = TaskStatus::parse(&task.status).ok_or_else(|| ApiError::Internal("corrupt task status".into()))?;
    let to = next_status(from, action, task.requires_human_review).ok_or_else(|| {
        ApiError::BadRequest(format!(
            "illegal transition from '{}' via '{}'",
            task.status, req.action
        ))
    })?;
    // 'start' exige que TODAS las dependencias estén 'done'.
    if action == TaskAction::Start && !state.task_repo.dependencies_satisfied(&id).await.map_err(db_err)? {
        return Err(ApiError::BadRequest("task dependencies are not all done".into()));
    }
    state.task_repo.set_status(&id, to.as_str()).await.map_err(db_err)?;
    // Auditoría del handoff (manual / approval / rejection).
    let trigger_kind = match action {
        TaskAction::Approve => "approval",
        TaskAction::Reject => "rejection",
        _ => "manual",
    };
    state
        .task_repo
        .log_handoff(NewHandoff {
            task_id: id.clone(),
            from_status: Some(from.as_str().to_string()),
            to_status: to.as_str().to_string(),
            actor: current.id.clone(),
            trigger_kind: trigger_kind.to_string(),
            note: None,
        })
        .await
        .map_err(db_err)?;
    // Al cerrar una tarea: cascada de dependencias (destraba dependientes) +
    // rollup del padre (que a su vez cascada a SUS dependientes).
    if to == TaskStatus::Done {
        state.handoff.on_task_completed(&id).await.map_err(db_err)?;
        if let Some(parent_id) = task.parent_task_id.as_deref() {
            let (total, done) = state.task_repo.subtask_rollup(parent_id).await.map_err(db_err)?;
            if total > 0 && total == done {
                state.task_repo.set_status(parent_id, "done").await.map_err(db_err)?;
                state
                    .task_repo
                    .log_handoff(NewHandoff {
                        task_id: parent_id.to_string(),
                        from_status: None,
                        to_status: "done".to_string(),
                        actor: "system".to_string(),
                        trigger_kind: "dependency_satisfied".to_string(),
                        note: Some("all subtasks done".to_string()),
                    })
                    .await
                    .map_err(db_err)?;
                state.handoff.on_task_completed(parent_id).await.map_err(db_err)?;
            }
        }
    }
    let updated = state
        .task_repo
        .get(&id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::NotFound(format!("Task {id} not found")))?;
    Ok(Json(ApiResponse::ok(updated)))
}

// ── Artefactos + log de handoffs (slice 5) ───────────────────────────

#[derive(Debug, Deserialize)]
struct AddArtifactRequest {
    /// 'bom' | 'alcances_obra' | 'doc' | 'file' | 'link' | 'conversation_ref'.
    kind: String,
    uri: String,
    title: String,
}

async fn add_artifact(
    State(state): State<ProjectRouterState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
    body: Result<Json<AddArtifactRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<TaskArtifactRow>>, ApiError> {
    require_task_member(&state, &id, &current.id).await?;
    let Json(req) = body.map_err(ApiError::from)?;
    if req.uri.trim().is_empty() || req.title.trim().is_empty() {
        return Err(ApiError::BadRequest("uri and title are required".into()));
    }
    let artifact = state
        .task_repo
        .add_artifact(NewArtifact {
            task_id: id.clone(),
            kind: req.kind,
            uri: req.uri,
            title: req.title,
            produced_by: current.id.clone(),
        })
        .await
        .map_err(db_err)?;
    Ok(Json(ApiResponse::ok(artifact)))
}

async fn list_artifacts(
    State(state): State<ProjectRouterState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<TaskArtifactRow>>>, ApiError> {
    require_task_member(&state, &id, &current.id).await?;
    let artifacts = state.task_repo.list_artifacts(&id).await.map_err(db_err)?;
    Ok(Json(ApiResponse::ok(artifacts)))
}

async fn list_handoffs(
    State(state): State<ProjectRouterState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<TaskHandoffLogRow>>>, ApiError> {
    require_task_member(&state, &id, &current.id).await?;
    let log = state.task_repo.list_handoffs(&id).await.map_err(db_err)?;
    Ok(Json(ApiResponse::ok(log)))
}

/// Router de Proyectos. El llamador añade `auth_middleware` (capa externa) para
/// poblar `CurrentUser`; la autorización fina (membresía/owner) vive en cada handler.
pub fn project_routes(state: ProjectRouterState) -> Router {
    Router::new()
        .route("/api/projects", get(list_projects).post(create_project))
        .route("/api/projects/{id}", get(get_project).patch(update_project))
        .route("/api/projects/{id}/members", get(list_members).post(add_member))
        .route("/api/projects/{id}/members/{uid}", delete(remove_member))
        .route("/api/projects/{id}/tasks", get(list_tasks).post(create_task))
        .route("/api/tasks/{id}", get(get_task).patch(update_task))
        .route("/api/tasks/{id}/transition", post(transition_task))
        .route("/api/tasks/{id}/artifacts", get(list_artifacts).post(add_artifact))
        .route("/api/tasks/{id}/handoffs", get(list_handoffs))
        .route("/api/pipeline-templates", get(list_templates))
        .with_state(state)
}
