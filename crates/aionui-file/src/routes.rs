#![allow(clippy::disallowed_types)]

use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Extension, Json, Multipart, Query, State};
use axum::routing::{get, post};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tower_http::limit::RequestBodyLimitLayer;

use aionui_api_types::{
    ApiResponse, BrowseDirectoryQuery, BrowseDirectoryResponse, CancelZipRequest, CopyFilesRequest, CopyFilesResponse,
    CreateTempFileRequest, DirOrFileResponse, FetchRemoteImageRequest, FileChangeInfoResponse, FileMetadataResponse,
    FileWatchRequest, GetFileMetadataRequest, GetFilesByDirRequest, GetImageBase64Request, ListWorkspaceFilesRequest,
    MkdirRequest, ReadFileBufferRequest, ReadFileRequest, RemoveEntryRequest, RenameRequest, RenameResponse,
    SnapshotBaselineRequest, SnapshotCompareResponse, SnapshotDiscardRequest, SnapshotInfoResponse,
    SnapshotStageRequest, SnapshotWorkspaceRequest, WorkspaceFlatFileResponse, WorkspaceOfficeWatchRequest,
    WriteFileRequest, ZipRequest,
};
use aionui_auth::CurrentUser;
use aionui_common::ApiError;
use aionui_common::constants::UPLOAD_MAX_SIZE;

use crate::browse;
use crate::error::FileError;
use crate::traits::{FileServiceRef, FileWatchServiceRef, SnapshotServiceRef};
use crate::types::{
    CompareResult, CopyResult, DirOrFile, FileChangeInfo, FileMetadata, SnapshotInfo, SnapshotMode, WorkspaceFlatFile,
    ZipEntry,
};

impl From<FileError> for ApiError {
    fn from(error: FileError) -> Self {
        match error {
            FileError::BadRequest(message) => ApiError::BadRequest(message),
            FileError::Forbidden(message) => ApiError::Forbidden(message),
            FileError::PathOutsideSandbox {
                message,
                field,
                operation,
            } => ApiError::PathOutsideSandbox {
                message,
                field,
                operation,
            },
            FileError::NotFound(message) => ApiError::NotFound(message),
            FileError::Internal(message) => ApiError::Internal(message),
        }
    }
}

// ---------------------------------------------------------------------------
// Router state
// ---------------------------------------------------------------------------

type BrowseRootsResolver = dyn Fn() -> Vec<PathBuf> + Send + Sync;

/// Lazily resolves roots for the shallow `/api/fs/browse` endpoint.
#[derive(Clone)]
pub struct BrowseRoots {
    roots: Arc<OnceLock<Vec<PathBuf>>>,
    resolver: Arc<BrowseRootsResolver>,
}

impl BrowseRoots {
    pub fn new() -> Self {
        Self {
            roots: Arc::new(OnceLock::new()),
            resolver: Arc::new(browse::default_browse_roots),
        }
    }

    #[cfg(test)]
    fn with_resolver(resolver: impl Fn() -> Vec<PathBuf> + Send + Sync + 'static) -> Self {
        Self {
            roots: Arc::new(OnceLock::new()),
            resolver: Arc::new(resolver),
        }
    }

    fn get(&self) -> Vec<PathBuf> {
        self.roots.get_or_init(|| (self.resolver)()).clone()
    }
}

impl Default for BrowseRoots {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared state for all file-related route handlers.
#[derive(Clone)]
pub struct FileRouterState {
    pub file_service: FileServiceRef,
    pub watch_service: FileWatchServiceRef,
    pub snapshot_service: SnapshotServiceRef,
    pub allowed_roots: Vec<std::path::PathBuf>,
    /// Roots permitted by the shallow `/api/fs/browse` endpoint. This is
    /// typically wider than `allowed_roots` (it includes `cwd`, Windows
    /// drive letters, and `/` on Unix) because the WebUI host-file picker
    /// legitimately needs to reach outside any single workspace.
    pub browse_roots: BrowseRoots,
    /// Base directory under which each authenticated user gets a private
    /// sandbox at `{users_base_dir}/{user_id}`. The `/api/fs/browse` and
    /// `/api/fs/mkdir` endpoints scope all access to this per-user root,
    /// so users can never see or write outside their own folder.
    pub users_base_dir: PathBuf,
}

/// Subárbol de ficheros permitido para el usuario del request (Fase 2 #5).
///
/// Lo inyecta un middleware de `aionui-app` a partir de `CurrentUser` (que sabe
/// `local` + `work_dir`); así `aionui-file` no se acopla a auth. `None` = modo
/// local/single-user → sin restricción por-usuario (la sandbox global de
/// `allowed_roots` sigue aplicando). Los handlers lo pasan a `enforce_user_scope`.
#[derive(Clone, Debug, Default)]
pub struct UserFileScope(pub Option<std::path::PathBuf>);

/// Raíz del usuario desde el `Extension` opcional (ausente en tests → `None`).
fn scope_root(scope: &Option<axum::Extension<UserFileScope>>) -> Option<&std::path::Path> {
    scope.as_ref().and_then(|e| e.0.0.as_deref())
}

/// Rechaza un `file_path` con traversal (`..`) o NUL. Para campos relativos al
/// workspace (snapshot) que de otro modo podrían escapar del subárbol (Fase 2 #5).
fn enforce_no_traversal(file_path: &str) -> Result<(), ApiError> {
    if crate::path_safety::has_traversal(file_path) {
        return Err(ApiError::BadRequest(format!("invalid file_path '{file_path}'")));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Router builder
// ---------------------------------------------------------------------------

/// Build the file router with all `/api/fs/*` routes.
///
/// All routes require authentication (applied by the caller).
pub fn file_routes(state: FileRouterState) -> Router {
    // Upload route carries its own body-size limit (UPLOAD_MAX_SIZE, 30 MB).
    // We first disable the global `DefaultBodyLimit` that `aionui-app`
    // installs (otherwise the `Multipart` extractor would cap the body at
    // `BODY_LIMIT`), then apply `RequestBodyLimitLayer` as the sole hard
    // cap. The layers are added in outer->inner order via `.layer()`.
    let upload_router = Router::new()
        .route("/api/fs/upload", post(upload_file))
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(UPLOAD_MAX_SIZE))
        .with_state(state.clone());

    Router::new()
        // A. Core file operations
        .route("/api/fs/browse", get(browse_directory))
        .route("/api/fs/mkdir", post(mkdir_directory))
        .route("/api/fs/dir", post(get_files_by_dir))
        .route("/api/fs/list", post(list_workspace_files))
        .route("/api/fs/metadata", post(get_file_metadata))
        .route("/api/fs/read", post(read_file))
        .route("/api/fs/read-buffer", post(read_file_buffer))
        .route("/api/fs/write", post(write_file))
        .route("/api/fs/copy", post(copy_files))
        .route("/api/fs/remove", post(remove_entry))
        .route("/api/fs/rename", post(rename_entry))
        .route("/api/fs/temp", post(create_temp_file))
        .route("/api/fs/image-base64", post(get_image_base64))
        .route("/api/fs/fetch-remote-image", post(fetch_remote_image))
        .route("/api/fs/zip", post(create_zip))
        .route("/api/fs/zip/cancel", post(cancel_zip))
        // D. File watch
        .route("/api/fs/watch/start", post(start_watch))
        .route("/api/fs/watch/stop", post(stop_watch))
        .route("/api/fs/watch/stop-all", post(stop_all_watches))
        .route("/api/fs/office-watch/start", post(start_office_watch))
        .route("/api/fs/office-watch/stop", post(stop_office_watch))
        // E. Workspace snapshot
        .route("/api/fs/snapshot/init", post(snapshot_init))
        .route("/api/fs/snapshot/info", post(snapshot_info))
        .route("/api/fs/snapshot/compare", post(snapshot_compare))
        .route("/api/fs/snapshot/baseline", post(snapshot_baseline))
        .route("/api/fs/snapshot/stage", post(snapshot_stage_file))
        .route("/api/fs/snapshot/stage-all", post(snapshot_stage_all))
        .route("/api/fs/snapshot/unstage", post(snapshot_unstage_file))
        .route("/api/fs/snapshot/unstage-all", post(snapshot_unstage_all))
        .route("/api/fs/snapshot/discard", post(snapshot_discard))
        .route("/api/fs/snapshot/reset", post(snapshot_reset))
        .route("/api/fs/snapshot/branches", post(snapshot_branches))
        .route("/api/fs/snapshot/dispose", post(snapshot_dispose))
        .with_state(state)
        .merge(upload_router)
}

// ---------------------------------------------------------------------------
// A. Core file operations — handlers
// ---------------------------------------------------------------------------

/// `GET /api/fs/browse` — shallow directory listing scoped to the caller's
/// per-user sandbox (`{users_base_dir}/{user_id}`).
///
/// Security: the only browse root passed to [`browse::browse`] is the user's
/// own root, so `resolve_browse_path` rejects any path that canonicalizes
/// outside it (path traversal, symlink escape) with `PathOutsideSandbox`,
/// and `navigation_hints` reports `can_go_up = false` at the sandbox root.
/// An empty/absent `path` lists the user root itself. Runs the synchronous
/// filesystem work on the Tokio blocking pool.
async fn browse_directory(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    Query(query): Query<BrowseDirectoryQuery>,
) -> Result<Json<ApiResponse<BrowseDirectoryResponse>>, ApiError> {
    let show_files = matches!(query.show_files.as_deref(), Some("true") | Some("1"));
    let raw_path = query.path.clone();
    let browse_roots = state.browse_roots.clone();
    // En multiusuario el host-file picker se restringe al subárbol del usuario
    // (Fase 2 #5): deja de exponer `/`, home y rutas fuera del subárbol.
    let user_root = scope_root(&scope).map(std::path::Path::to_path_buf);

    let response = tokio::task::spawn_blocking(move || {
        let roots = match user_root {
            Some(root) => vec![root],
            None => browse_roots.get(),
        };
        browse::browse(raw_path.as_deref(), show_files, &roots)
    })
    .await
    .map_err(|e| ApiError::Internal(format!("browse task failed: {}", e)))??;

    Ok(Json(ApiResponse::ok(response)))
}

/// `POST /api/fs/mkdir` — create a directory inside the caller's per-user
/// sandbox.
///
/// `req.path` is resolved relative to `{users_base_dir}/{user_id}`. Security
/// is enforced in two layers:
/// 1. A fast `has_traversal` pre-check rejects any `..` component (or null
///    byte) before touching the filesystem.
/// 2. After creating the directory, the result is canonicalized and verified
///    to still live under the canonicalized user root — defense in depth
///    against symlink-based escapes.
async fn mkdir_directory(
    State(state): State<FileRouterState>,
    Extension(current_user): Extension<CurrentUser>,
    body: Result<Json<MkdirRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;

    let user_root = state.users_base_dir.join(&current_user.id);
    let req_path = req.path;

    tokio::task::spawn_blocking(move || create_user_directory(&user_root, &req_path))
        .await
        .map_err(|e| ApiError::Internal(format!("mkdir task failed: {}", e)))??;

    Ok(Json(ApiResponse::message("created")))
}

/// Create `relative_path` under `user_root`, enforcing the per-user sandbox.
///
/// Security layers:
/// 1. Reject empty paths and obvious traversal (`..`, null byte) before
///    touching the filesystem.
/// 2. After `create_dir_all`, canonicalize the result and confirm it still
///    lives under the canonicalized `user_root` — defense in depth against
///    symlink-based escapes.
///
/// Returns the canonicalized target on success. Does synchronous filesystem
/// I/O, so callers must run it off the async runtime (e.g. `spawn_blocking`).
fn create_user_directory(user_root: &Path, relative_path: &str) -> Result<PathBuf, ApiError> {
    let relative = relative_path.trim().trim_start_matches('/');
    if relative.is_empty() {
        return Err(ApiError::BadRequest("mkdir path must not be empty".to_owned()));
    }
    if crate::path_safety::has_traversal(relative) {
        return Err(ApiError::PathOutsideSandbox {
            message: format!("path '{}' attempts to escape the user sandbox", relative_path),
            field: Some("path"),
            operation: Some("mkdir"),
        });
    }

    // Ensure the sandbox root exists, then create the requested subtree.
    std::fs::create_dir_all(user_root).map_err(|e| ApiError::Internal(format!("failed to create user root: {}", e)))?;
    let target = user_root.join(relative);
    std::fs::create_dir_all(&target).map_err(|e| ApiError::Internal(format!("mkdir failed: {}", e)))?;

    // Defense in depth: canonicalize both sides and confirm containment.
    let canonical_root =
        std::fs::canonicalize(user_root).map_err(|e| ApiError::Internal(format!("cannot resolve user root: {}", e)))?;
    let canonical_target = std::fs::canonicalize(&target)
        .map_err(|e| ApiError::Internal(format!("cannot resolve mkdir target: {}", e)))?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(ApiError::PathOutsideSandbox {
            message: format!("path '{}' resolves outside the user sandbox", relative_path),
            field: Some("path"),
            operation: Some("mkdir"),
        });
    }
    Ok(canonical_target)
}

async fn get_files_by_dir(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<GetFilesByDirRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<Vec<DirOrFileResponse>>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.root, scope_root(&scope))?;
    let items = state.file_service.get_files_by_dir(&req.dir, &req.root).await?;
    let response: Vec<DirOrFileResponse> = items.into_iter().map(to_dir_or_file_response).collect();
    Ok(Json(ApiResponse::ok(response)))
}

async fn list_workspace_files(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<ListWorkspaceFilesRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<Vec<WorkspaceFlatFileResponse>>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.root, scope_root(&scope))?;
    let items = state.file_service.list_workspace_files(&req.root).await?;
    let response: Vec<WorkspaceFlatFileResponse> = items.into_iter().map(to_flat_file_response).collect();
    Ok(Json(ApiResponse::ok(response)))
}

async fn get_file_metadata(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<GetFileMetadataRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<FileMetadataResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.path, scope_root(&scope))?;
    let meta = state
        .file_service
        .get_file_metadata(&req.path, req.workspace.as_deref().map(Path::new))
        .await?;
    Ok(Json(ApiResponse::ok(to_metadata_response(meta))))
}

async fn read_file(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<ReadFileRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<Option<String>>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.path, scope_root(&scope))?;
    let content = state
        .file_service
        .read_file(&req.path, req.workspace.as_deref().map(Path::new))
        .await?;
    Ok(Json(ApiResponse::ok(content)))
}

async fn read_file_buffer(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<ReadFileBufferRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<Option<String>>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.path, scope_root(&scope))?;
    let data = state
        .file_service
        .read_file_buffer(&req.path, req.workspace.as_deref().map(Path::new))
        .await?;
    // Binary data is base64-encoded for JSON transport.
    let encoded = data.map(|bytes| {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    });
    Ok(Json(ApiResponse::ok(encoded)))
}

async fn write_file(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<WriteFileRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<bool>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.path, scope_root(&scope))?;
    let workspace = req.workspace.unwrap_or_else(|| {
        std::path::Path::new(&req.path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let ok = state
        .file_service
        .write_file(&req.path, req.data.as_bytes(), &workspace)
        .await?;
    Ok(Json(ApiResponse::ok(ok)))
}

async fn copy_files(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<CopyFilesRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<CopyFilesResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    // Destino Y fuentes deben estar en el subárbol del usuario: si no, un usuario
    // podría copiar ficheros de otro (`users/{B}/...`) a su workspace y leerlos.
    let user_root = scope_root(&scope);
    crate::path_safety::enforce_user_scope(&req.workspace, user_root)?;
    for src in &req.file_paths {
        crate::path_safety::enforce_user_scope(src, user_root)?;
    }
    if let Some(src_root) = req.source_root.as_deref() {
        crate::path_safety::enforce_user_scope(src_root, user_root)?;
    }
    let result = state
        .file_service
        .copy_files_to_workspace(&req.file_paths, &req.workspace, req.source_root.as_deref())
        .await?;
    Ok(Json(ApiResponse::ok(to_copy_response(result))))
}

async fn remove_entry(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<RemoveEntryRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.path, scope_root(&scope))?;
    let workspace = req.workspace.unwrap_or_else(|| {
        std::path::Path::new(&req.path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    state.file_service.remove_entry(&req.path, &workspace).await?;
    Ok(Json(ApiResponse::success()))
}

async fn rename_entry(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<RenameRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<RenameResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.path, scope_root(&scope))?;
    let new_path = state.file_service.rename_entry(&req.path, &req.new_name).await?;
    Ok(Json(ApiResponse::ok(RenameResponse { new_path })))
}

async fn create_temp_file(
    State(state): State<FileRouterState>,
    body: Result<Json<CreateTempFileRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<String>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    // Sin guard de scope: solo recibe `file_name` (no un path), pero escribe en un
    // namespace temp COMPARTIDO entre usuarios (no `users/{id}`). No expone paths
    // ajenos; el aislamiento real del temp por usuario es follow-up (Fase 2 #5).
    let path = state.file_service.create_temp_file(&req.file_name).await?;
    Ok(Json(ApiResponse::ok(path)))
}

/// Fields extracted from a `/api/fs/upload` multipart request.
struct UploadMultipartFields {
    file_data: Vec<u8>,
    file_name: Option<String>,
    dispo_file_name: Option<String>,
    conversation_id: Option<String>,
}

/// Strip any directory component from a file name and reject empty results.
/// The returned name is guaranteed not to contain path separators; deeper
/// traversal validation happens in [`IFileService::create_upload_file`].
fn sanitize_upload_filename(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let last = trimmed.rsplit(['/', '\\']).next().unwrap_or("");
    let last = last.trim();
    if last.is_empty() { None } else { Some(last.to_owned()) }
}

async fn extract_upload_multipart(mut multipart: Multipart) -> Result<UploadMultipartFields, ApiError> {
    let mut file_data: Option<Vec<u8>> = None;
    let mut file_name: Option<String> = None;
    let mut dispo_file_name: Option<String> = None;
    let mut conversation_id: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_owned();
        match name.as_str() {
            "file" => {
                // Capture the Content-Disposition filename (if any) before
                // consuming the field body — `field.file_name()` is only
                // available on the field metadata, not on the Bytes below.
                dispo_file_name = field.file_name().and_then(sanitize_upload_filename);
                file_data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| ApiError::BadRequest(format!("failed to read file: {e}")))?
                        .to_vec(),
                );
            }
            "file_name" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("failed to read file_name: {e}")))?;
                if let Some(name) = sanitize_upload_filename(&text) {
                    file_name = Some(name);
                }
            }
            "conversation_id" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("failed to read conversation_id: {e}")))?;
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    conversation_id = Some(trimmed.to_owned());
                }
            }
            _ => {}
        }
    }

    let file_data = file_data.ok_or_else(|| ApiError::BadRequest("missing 'file' field".to_owned()))?;

    Ok(UploadMultipartFields {
        file_data,
        file_name,
        dispo_file_name,
        conversation_id,
    })
}

async fn upload_file(
    State(state): State<FileRouterState>,
    multipart: Multipart,
) -> Result<Json<ApiResponse<String>>, ApiError> {
    let fields = extract_upload_multipart(multipart).await?;

    let file_name = fields.file_name.or(fields.dispo_file_name).ok_or_else(|| {
        ApiError::BadRequest("missing file name: provide 'file_name' or a multipart filename".to_owned())
    })?;

    let path = state
        .file_service
        .create_upload_file(&file_name, &fields.file_data, fields.conversation_id.as_deref())
        .await?;
    Ok(Json(ApiResponse::ok(path)))
}

async fn get_image_base64(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<GetImageBase64Request>, JsonRejection>,
) -> Result<Json<ApiResponse<String>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.path, scope_root(&scope))?;
    let data_url = state
        .file_service
        .get_image_base64(&req.path, req.workspace.as_deref().map(Path::new))
        .await?;
    Ok(Json(ApiResponse::ok(data_url)))
}

async fn fetch_remote_image(
    State(state): State<FileRouterState>,
    body: Result<Json<FetchRemoteImageRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<String>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let data_url = state.file_service.fetch_remote_image(&req.url).await;
    Ok(Json(ApiResponse::ok(data_url)))
}

async fn create_zip(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<ZipRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<bool>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    // Salida Y fuentes en disco deben estar en el subárbol del usuario: si no, un
    // usuario podría zipear ficheros de otro (lectura) o escribir en su subárbol.
    let user_root = scope_root(&scope);
    crate::path_safety::enforce_user_scope(&req.path, user_root)?;
    let entries: Vec<ZipEntry> = req.files.into_iter().map(to_zip_entry).collect();
    for entry in &entries {
        if let ZipEntry::Disk { file_path, .. } = entry {
            crate::path_safety::enforce_user_scope(file_path, user_root)?;
        }
    }
    let ok = state
        .file_service
        .create_zip(&req.path, entries, req.request_id)
        .await?;
    Ok(Json(ApiResponse::ok(ok)))
}

async fn cancel_zip(
    State(state): State<FileRouterState>,
    body: Result<Json<CancelZipRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<bool>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    let ok = state.file_service.cancel_zip(&req.request_id).await;
    Ok(Json(ApiResponse::ok(ok)))
}

// ---------------------------------------------------------------------------
// D. File watch — handlers
// ---------------------------------------------------------------------------

async fn start_watch(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<FileWatchRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.file_path, scope_root(&scope))?;
    state.watch_service.start_watch(&req.file_path).await?;
    Ok(Json(ApiResponse::success()))
}

async fn stop_watch(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<FileWatchRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.file_path, scope_root(&scope))?;
    state.watch_service.stop_watch(&req.file_path).await?;
    Ok(Json(ApiResponse::success()))
}

async fn stop_all_watches(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    // En multiusuario limpia solo los watchers del subárbol del usuario, no los de
    // todos (cierra el DoS cross-usuario; Fase 2 #5).
    state.watch_service.stop_all_watches(scope_root(&scope)).await?;
    Ok(Json(ApiResponse::success()))
}

async fn start_office_watch(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<WorkspaceOfficeWatchRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.workspace, scope_root(&scope))?;
    let allowed_roots: Vec<&Path> = state.allowed_roots.iter().map(std::path::PathBuf::as_path).collect();
    crate::path_safety::validate_path_with_extra_root(&req.workspace, &allowed_roots, Some(Path::new(&req.workspace)))?;
    state.watch_service.start_office_watch(&req.workspace).await?;
    Ok(Json(ApiResponse::success()))
}

async fn stop_office_watch(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<WorkspaceOfficeWatchRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.workspace, scope_root(&scope))?;
    state.watch_service.stop_office_watch(&req.workspace).await?;
    Ok(Json(ApiResponse::success()))
}

// ---------------------------------------------------------------------------
// E. Workspace snapshot — handlers
// ---------------------------------------------------------------------------

async fn snapshot_init(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<SnapshotWorkspaceRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<SnapshotInfoResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.workspace, scope_root(&scope))?;
    let info = state.snapshot_service.init(&req.workspace).await?;
    Ok(Json(ApiResponse::ok(to_snapshot_info_response(info))))
}

async fn snapshot_info(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<SnapshotWorkspaceRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<SnapshotInfoResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.workspace, scope_root(&scope))?;
    let info = state.snapshot_service.get_info(&req.workspace).await?;
    Ok(Json(ApiResponse::ok(to_snapshot_info_response(info))))
}

async fn snapshot_compare(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<SnapshotWorkspaceRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<SnapshotCompareResponse>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.workspace, scope_root(&scope))?;
    let result = state.snapshot_service.compare(&req.workspace).await?;
    Ok(Json(ApiResponse::ok(to_compare_response(result))))
}

async fn snapshot_baseline(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<SnapshotBaselineRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<Option<String>>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.workspace, scope_root(&scope))?;
    enforce_no_traversal(&req.file_path)?;
    let content = state
        .snapshot_service
        .get_baseline_content(&req.workspace, &req.file_path)
        .await?;
    Ok(Json(ApiResponse::ok(content)))
}

async fn snapshot_stage_file(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<SnapshotStageRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.workspace, scope_root(&scope))?;
    enforce_no_traversal(&req.file_path)?;
    state
        .snapshot_service
        .stage_file(&req.workspace, &req.file_path)
        .await?;
    Ok(Json(ApiResponse::success()))
}

async fn snapshot_stage_all(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<SnapshotWorkspaceRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.workspace, scope_root(&scope))?;
    state.snapshot_service.stage_all(&req.workspace).await?;
    Ok(Json(ApiResponse::success()))
}

async fn snapshot_unstage_file(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<SnapshotStageRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.workspace, scope_root(&scope))?;
    enforce_no_traversal(&req.file_path)?;
    state
        .snapshot_service
        .unstage_file(&req.workspace, &req.file_path)
        .await?;
    Ok(Json(ApiResponse::success()))
}

async fn snapshot_unstage_all(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<SnapshotWorkspaceRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.workspace, scope_root(&scope))?;
    state.snapshot_service.unstage_all(&req.workspace).await?;
    Ok(Json(ApiResponse::success()))
}

async fn snapshot_discard(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<SnapshotDiscardRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.workspace, scope_root(&scope))?;
    enforce_no_traversal(&req.file_path)?;
    state
        .snapshot_service
        .discard_file(&req.workspace, &req.file_path, req.operation)
        .await?;
    Ok(Json(ApiResponse::success()))
}

async fn snapshot_reset(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<SnapshotDiscardRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.workspace, scope_root(&scope))?;
    enforce_no_traversal(&req.file_path)?;
    state
        .snapshot_service
        .reset_file(&req.workspace, &req.file_path, req.operation)
        .await?;
    Ok(Json(ApiResponse::success()))
}

async fn snapshot_branches(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<SnapshotWorkspaceRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<Vec<String>>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.workspace, scope_root(&scope))?;
    let branches = state.snapshot_service.get_branches(&req.workspace).await?;
    Ok(Json(ApiResponse::ok(branches)))
}

async fn snapshot_dispose(
    State(state): State<FileRouterState>,
    scope: Option<axum::Extension<UserFileScope>>,
    body: Result<Json<SnapshotWorkspaceRequest>, JsonRejection>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let Json(req) = body.map_err(ApiError::from)?;
    crate::path_safety::enforce_user_scope(&req.workspace, scope_root(&scope))?;
    state.snapshot_service.dispose(&req.workspace).await?;
    Ok(Json(ApiResponse::success()))
}

// ---------------------------------------------------------------------------
// Domain → DTO conversions
// ---------------------------------------------------------------------------

fn to_dir_or_file_response(d: DirOrFile) -> DirOrFileResponse {
    let children = if d.is_dir {
        Some(d.children.into_iter().map(to_dir_or_file_response).collect())
    } else {
        None
    };
    DirOrFileResponse {
        name: d.name,
        full_path: d.full_path,
        relative_path: d.relative_path,
        is_dir: d.is_dir,
        is_file: !d.is_dir,
        children,
    }
}

fn to_flat_file_response(f: WorkspaceFlatFile) -> WorkspaceFlatFileResponse {
    WorkspaceFlatFileResponse {
        name: f.name,
        full_path: f.full_path,
        relative_path: f.relative_path,
    }
}

fn to_metadata_response(m: FileMetadata) -> FileMetadataResponse {
    FileMetadataResponse {
        name: m.name,
        path: m.path,
        size: m.size,
        mime_type: m.mime_type,
        last_modified: m.last_modified,
        is_directory: if m.is_directory { Some(true) } else { None },
    }
}

fn to_copy_response(r: CopyResult) -> CopyFilesResponse {
    CopyFilesResponse {
        copied_files: r.copied_files,
        failed_files: r.failed_files,
    }
}

fn to_zip_entry(e: aionui_api_types::ZipFileEntry) -> ZipEntry {
    if let Some(content) = e.content {
        ZipEntry::Text { name: e.name, content }
    } else if let Some(file_path) = e.file_path {
        ZipEntry::Disk {
            name: e.name,
            file_path,
        }
    } else {
        // Fallback: treat as empty text entry
        ZipEntry::Text {
            name: e.name,
            content: String::new(),
        }
    }
}

fn to_snapshot_info_response(info: SnapshotInfo) -> SnapshotInfoResponse {
    let mode = match info.mode {
        SnapshotMode::GitRepo => aionui_api_types::SnapshotMode::GitRepo,
        SnapshotMode::Snapshot => aionui_api_types::SnapshotMode::Snapshot,
    };
    SnapshotInfoResponse {
        mode,
        branch: info.branch,
    }
}

fn to_file_change_response(c: FileChangeInfo) -> FileChangeInfoResponse {
    FileChangeInfoResponse {
        file_path: c.file_path,
        relative_path: c.relative_path,
        operation: c.operation,
    }
}

fn to_compare_response(r: CompareResult) -> SnapshotCompareResponse {
    SnapshotCompareResponse {
        staged: r.staged.into_iter().map(to_file_change_response).collect(),
        unstaged: r.unstaged.into_iter().map(to_file_change_response).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // -----------------------------------------------------------------------
    // Per-user sandbox — security tests
    //
    // These exercise the two security-critical primitives the `browse` and
    // `mkdir` handlers delegate to:
    //   * `browse::browse(path, show_files, &[user_root])` — host-file picker
    //     scoped to a single per-user root.
    //   * `create_user_directory(user_root, relative)` — sandboxed mkdir.
    // The handlers themselves are thin plumbing over these (extract the
    // `CurrentUser`, build `user_root = users_base_dir/{id}`, call through),
    // so testing the primitives directly covers the isolation guarantees
    // without standing up the full axum stack.
    // -----------------------------------------------------------------------

    /// Create `{base}/users/{user_id}` and return it.
    fn make_user_root(base: &Path, user_id: &str) -> PathBuf {
        let root = base.join("users").join(user_id);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn browse_empty_path_lists_user_root_and_cannot_go_up() {
        let base = tempfile::tempdir().unwrap();
        let user_root = make_user_root(base.path(), "alice");
        std::fs::create_dir_all(user_root.join("proyectos")).unwrap();
        std::fs::write(user_root.join("nota.txt"), "hi").unwrap();

        // Handler maps an empty/absent path to the user root itself.
        let path = user_root.to_string_lossy().into_owned();
        let resp = browse::browse(Some(&path), true, &[user_root]).unwrap();

        // The sandbox root is the top of the tree: the up-arrow is hidden.
        assert!(!resp.can_go_up, "user must not be able to navigate above their root");
        // Listing shows the user's own contents.
        let names: Vec<&str> = resp.items.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"proyectos"));
        assert!(names.contains(&"nota.txt"));
    }

    #[test]
    fn browse_absolute_path_outside_root_is_forbidden() {
        let base = tempfile::tempdir().unwrap();
        let user_root = make_user_root(base.path(), "alice");

        // A real directory that exists but lives outside the user's sandbox.
        let outside = tempfile::tempdir().unwrap();
        let result = browse::browse(Some(outside.path().to_str().unwrap()), false, &[user_root]);

        match result {
            Err(FileError::PathOutsideSandbox { .. }) => {}
            other => panic!("expected PathOutsideSandbox, got {other:?}"),
        }
    }

    #[test]
    fn browse_parent_traversal_is_forbidden() {
        let base = tempfile::tempdir().unwrap();
        let user_root = make_user_root(base.path(), "alice");

        // `{user_root}/../../` canonicalizes above the sandbox and must be
        // rejected even though every component exists on disk.
        let escape = format!("{}/../../", user_root.to_string_lossy());
        let result = browse::browse(Some(&escape), false, &[user_root]);

        match result {
            Err(FileError::PathOutsideSandbox { .. }) => {}
            other => panic!("expected PathOutsideSandbox for traversal, got {other:?}"),
        }
    }

    #[test]
    fn mkdir_creates_nested_dir_under_user_root() {
        let base = tempfile::tempdir().unwrap();
        let user_root = make_user_root(base.path(), "alice");

        let created = create_user_directory(&user_root, "proyectos/x").expect("mkdir should succeed");

        // The returned path is the canonical target and the dir exists.
        assert!(created.is_dir());
        assert!(created.ends_with("proyectos/x"));
        assert!(user_root.join("proyectos").join("x").is_dir());
        // And it is genuinely inside the (canonicalized) user root.
        let canonical_root = std::fs::canonicalize(&user_root).unwrap();
        assert!(created.starts_with(&canonical_root));
    }

    #[test]
    fn mkdir_leading_slash_is_treated_as_relative() {
        let base = tempfile::tempdir().unwrap();
        let user_root = make_user_root(base.path(), "alice");

        // A leading `/` must NOT escape to the filesystem root — it is
        // stripped and the path stays inside the sandbox.
        let created = create_user_directory(&user_root, "/docs").expect("mkdir should succeed");
        let canonical_root = std::fs::canonicalize(&user_root).unwrap();
        assert!(created.starts_with(&canonical_root));
        assert!(user_root.join("docs").is_dir());
    }

    #[test]
    fn mkdir_parent_escape_is_rejected() {
        let base = tempfile::tempdir().unwrap();
        let user_root = make_user_root(base.path(), "alice");

        let result = create_user_directory(&user_root, "../escape");
        match result {
            Err(ApiError::PathOutsideSandbox {
                operation: Some("mkdir"),
                ..
            }) => {}
            other => panic!("expected PathOutsideSandbox(mkdir), got {other:?}"),
        }
        // The escape directory must NOT have been created next to the sandbox.
        assert!(!user_root.parent().unwrap().join("escape").exists());
    }

    #[test]
    fn mkdir_empty_path_is_rejected() {
        let base = tempfile::tempdir().unwrap();
        let user_root = make_user_root(base.path(), "alice");

        assert!(matches!(
            create_user_directory(&user_root, "   "),
            Err(ApiError::BadRequest(_))
        ));
    }

    #[test]
    fn two_users_get_isolated_roots() {
        let base = tempfile::tempdir().unwrap();
        let alice_root = make_user_root(base.path(), "alice");
        let bob_root = make_user_root(base.path(), "bob");

        // Alice creates a private project; Bob creates his own.
        create_user_directory(&alice_root, "secret-alice").unwrap();
        create_user_directory(&bob_root, "secret-bob").unwrap();

        // Each user's browse is scoped to their own root only.
        let alice_listing = browse::browse(Some(alice_root.to_str().unwrap()), false, &[alice_root.clone()]).unwrap();
        let bob_listing = browse::browse(Some(bob_root.to_str().unwrap()), false, &[bob_root.clone()]).unwrap();

        let alice_names: Vec<&str> = alice_listing.items.iter().map(|e| e.name.as_str()).collect();
        let bob_names: Vec<&str> = bob_listing.items.iter().map(|e| e.name.as_str()).collect();

        assert!(alice_names.contains(&"secret-alice"));
        assert!(!alice_names.contains(&"secret-bob"), "alice must not see bob's folder");
        assert!(bob_names.contains(&"secret-bob"));
        assert!(!bob_names.contains(&"secret-alice"), "bob must not see alice's folder");

        // Bob cannot browse into Alice's root: it is outside his sandbox.
        let cross = browse::browse(Some(alice_root.to_str().unwrap()), false, &[bob_root]);
        match cross {
            Err(FileError::PathOutsideSandbox { .. }) => {}
            other => panic!("bob reaching into alice's root must be forbidden, got {other:?}"),
        }
    }

    #[test]
    fn file_path_outside_sandbox_maps_to_explicit_api_code() {
        let api_err = ApiError::from(FileError::PathOutsideSandbox {
            message: "path '/tmp/x' is outside the allowed sandbox".into(),
            field: Some("path"),
            operation: Some("access"),
        });
        assert_eq!(api_err.error_code(), "PATH_OUTSIDE_SANDBOX");
        assert_eq!(api_err.error_details().unwrap()["field"], "path");
        assert_eq!(api_err.error_details().unwrap()["operation"], "access");
    }

    #[test]
    fn browse_roots_are_resolved_lazily() {
        let calls = Arc::new(AtomicUsize::new(0));
        let roots = BrowseRoots::with_resolver({
            let calls = calls.clone();
            move || {
                calls.fetch_add(1, Ordering::SeqCst);
                vec![std::env::current_dir().unwrap()]
            }
        });

        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let first = roots.get();
        let second = roots.get();

        assert!(!first.is_empty());
        assert_eq!(first, second);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dir_or_file_response_conversion_file() {
        let d = DirOrFile {
            name: "test.txt".into(),
            full_path: "/ws/test.txt".into(),
            relative_path: "test.txt".into(),
            is_dir: false,
            children: vec![],
        };
        let r = to_dir_or_file_response(d);
        assert_eq!(r.name, "test.txt");
        assert!(!r.is_dir);
        assert!(r.is_file);
        assert!(r.children.is_none());
    }

    #[test]
    fn dir_or_file_response_conversion_dir_with_children() {
        let d = DirOrFile {
            name: "src".into(),
            full_path: "/ws/src".into(),
            relative_path: "src".into(),
            is_dir: true,
            children: vec![DirOrFile {
                name: "main.rs".into(),
                full_path: "/ws/src/main.rs".into(),
                relative_path: "src/main.rs".into(),
                is_dir: false,
                children: vec![],
            }],
        };
        let r = to_dir_or_file_response(d);
        assert!(r.is_dir);
        assert!(!r.is_file);
        let children = r.children.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "main.rs");
    }

    #[test]
    fn flat_file_response_conversion() {
        let f = WorkspaceFlatFile {
            name: "lib.rs".into(),
            full_path: "/ws/src/lib.rs".into(),
            relative_path: "src/lib.rs".into(),
        };
        let r = to_flat_file_response(f);
        assert_eq!(r.name, "lib.rs");
        assert_eq!(r.full_path, "/ws/src/lib.rs");
        assert_eq!(r.relative_path, "src/lib.rs");
    }

    #[test]
    fn metadata_response_conversion_file() {
        let m = FileMetadata {
            name: "readme.md".into(),
            path: "/ws/readme.md".into(),
            size: 1024,
            mime_type: "text/markdown".into(),
            last_modified: 1700000000000,
            is_directory: false,
        };
        let r = to_metadata_response(m);
        assert_eq!(r.name, "readme.md");
        assert_eq!(r.size, 1024);
        assert!(r.is_directory.is_none());
    }

    #[test]
    fn metadata_response_conversion_directory() {
        let m = FileMetadata {
            name: "src".into(),
            path: "/ws/src".into(),
            size: 0,
            mime_type: "".into(),
            last_modified: 1700000000000,
            is_directory: true,
        };
        let r = to_metadata_response(m);
        assert_eq!(r.is_directory, Some(true));
    }

    #[test]
    fn zip_entry_conversion_text() {
        let e = aionui_api_types::ZipFileEntry {
            name: "a.txt".into(),
            content: Some("hello".into()),
            file_path: None,
        };
        let z = to_zip_entry(e);
        match z {
            ZipEntry::Text { name, content } => {
                assert_eq!(name, "a.txt");
                assert_eq!(content, "hello");
            }
            _ => panic!("expected Text variant"),
        }
    }

    #[test]
    fn zip_entry_conversion_disk() {
        let e = aionui_api_types::ZipFileEntry {
            name: "b.bin".into(),
            content: None,
            file_path: Some("/src/b.bin".into()),
        };
        let z = to_zip_entry(e);
        match z {
            ZipEntry::Disk { name, file_path } => {
                assert_eq!(name, "b.bin");
                assert_eq!(file_path, "/src/b.bin");
            }
            _ => panic!("expected Disk variant"),
        }
    }

    #[test]
    fn zip_entry_conversion_empty_fallback() {
        let e = aionui_api_types::ZipFileEntry {
            name: "empty.txt".into(),
            content: None,
            file_path: None,
        };
        let z = to_zip_entry(e);
        match z {
            ZipEntry::Text { name, content } => {
                assert_eq!(name, "empty.txt");
                assert!(content.is_empty());
            }
            _ => panic!("expected Text variant"),
        }
    }

    #[test]
    fn snapshot_info_response_git_repo() {
        let info = SnapshotInfo {
            mode: SnapshotMode::GitRepo,
            branch: Some("main".into()),
        };
        let r = to_snapshot_info_response(info);
        assert_eq!(r.mode, aionui_api_types::SnapshotMode::GitRepo);
        assert_eq!(r.branch, Some("main".into()));
    }

    #[test]
    fn snapshot_info_response_snapshot_mode() {
        let info = SnapshotInfo {
            mode: SnapshotMode::Snapshot,
            branch: None,
        };
        let r = to_snapshot_info_response(info);
        assert_eq!(r.mode, aionui_api_types::SnapshotMode::Snapshot);
        assert!(r.branch.is_none());
    }

    #[test]
    fn compare_response_conversion() {
        use aionui_common::FileChangeOperation;
        let result = CompareResult {
            staged: vec![FileChangeInfo {
                file_path: "/ws/a.txt".into(),
                relative_path: "a.txt".into(),
                operation: FileChangeOperation::Create,
            }],
            unstaged: vec![FileChangeInfo {
                file_path: "/ws/b.txt".into(),
                relative_path: "b.txt".into(),
                operation: FileChangeOperation::Modify,
            }],
        };
        let r = to_compare_response(result);
        assert_eq!(r.staged.len(), 1);
        assert_eq!(r.staged[0].file_path, "/ws/a.txt");
        assert_eq!(r.staged[0].operation, FileChangeOperation::Create);
        assert_eq!(r.unstaged.len(), 1);
        assert_eq!(r.unstaged[0].operation, FileChangeOperation::Modify);
    }

    // ---- sanitize_upload_filename -----------------------------------------

    #[test]
    fn sanitize_upload_filename_strips_directory_components() {
        assert_eq!(sanitize_upload_filename("a/b/c.png").as_deref(), Some("c.png"));
        assert_eq!(sanitize_upload_filename("C:\\tmp\\d.jpg").as_deref(), Some("d.jpg"));
        assert_eq!(
            sanitize_upload_filename("  spaced.txt  ").as_deref(),
            Some("spaced.txt")
        );
    }

    #[test]
    fn sanitize_upload_filename_rejects_empty() {
        assert_eq!(sanitize_upload_filename(""), None);
        assert_eq!(sanitize_upload_filename("   "), None);
        assert_eq!(sanitize_upload_filename("/"), None);
        assert_eq!(sanitize_upload_filename("a/b/"), None);
    }

    #[test]
    fn sanitize_upload_filename_plain_passthrough() {
        assert_eq!(sanitize_upload_filename("image.png").as_deref(), Some("image.png"));
    }
}
