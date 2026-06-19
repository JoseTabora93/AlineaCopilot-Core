//! Shared application services for dependency injection.

use std::path::PathBuf;
use std::sync::Arc;

use aionui_ai_agent::{
    AcpSessionSyncService, AcpSkillManager, AgentFactoryDeps, AgentRegistry, IWorkerTaskManager, WorkerTaskManagerImpl,
    build_agent_factory,
};
use aionui_api_types::GuideMcpConfig;
use aionui_auth::{
    CookieConfig, JwtService, QrTokenStore, RequestIdentityService, load_or_create_identity, resolve_jwt_secret,
};
use aionui_common::OnConversationDelete;
use aionui_conversation::{ConversationService, runtime_state::ConversationRuntimeStateService};
use aionui_db::{
    Database, IAcpSessionRepository, IAgentMetadataRepository, IConversationRepository, IMcpServerRepository,
    IUsageRepository, IUserRepository, SqliteAcpSessionRepository, SqliteAgentMetadataRepository,
    SqliteAssistantDefinitionRepository, SqliteUsageRepository,
    SqliteAssistantOverlayRepository, SqliteAssistantPreferenceRepository, SqliteConversationRepository,
    SqliteMcpServerRepository, SqliteProviderRepository, SqliteUserRepository,
};
use aionui_realtime::{BroadcastEventBus, WebSocketManager};
use aionui_team::GuideMcpServer;

use crate::config::{AppConfig, derive_encryption_key, derive_identity_kek};

pub struct AppServices {
    pub database: Database,
    pub jwt_service: Arc<JwtService>,
    pub user_repo: Arc<dyn IUserRepository>,
    /// Ledger de consumos $ (Fase 2 #3).
    pub usage_repo: Arc<dyn IUsageRepository>,
    pub cookie_config: Arc<CookieConfig>,
    pub qr_token_store: Arc<QrTokenStore>,
    pub ws_manager: Arc<WebSocketManager>,
    pub event_bus: Arc<BroadcastEventBus>,
    pub worker_task_manager: Arc<dyn IWorkerTaskManager>,
    pub conversation_runtime_state: Arc<ConversationRuntimeStateService>,
    pub conversation_service: ConversationService,
    /// Same instance as `worker_task_manager`, exposed through the
    /// `OnConversationDelete` trait so `ConversationService::with_delete_hook`
    /// can wire it up. Optional because tests construct `AppServices` with a
    /// mock `worker_task_manager` that does not implement the trait.
    pub task_manager_delete_hook: Option<Arc<dyn OnConversationDelete>>,
    pub agent_registry: Arc<AgentRegistry>,
    pub conversation_repo: Arc<dyn IConversationRepository>,
    pub acp_session_sync: Arc<AcpSessionSyncService>,
    /// Emisor de tokens de identidad firmados por request (Fase 2 #5). La semilla
    /// Ed25519 se carga cifrada del keystore (`<data_dir>/secrets/identity.enc`).
    pub request_identity: Arc<RequestIdentityService>,
    /// Raw JWT secret string, used to derive encryption keys.
    pub jwt_secret_raw: String,
    pub data_dir: PathBuf,
    pub work_dir: PathBuf,
    /// When `true`, skip JWT authentication and use a fixed default user.
    pub local: bool,
    /// When `true`, enforce per-user file segregation (Fase 2 #5). Derivado de
    /// `config.enforce_file_scope()` (multiusuario por defecto).
    pub enforce_file_scope: bool,
    pub app_version: String,
    /// Resolved skill paths. Shared with the `ConversationService` for
    /// snapshot resolution at create time.
    pub skill_paths: Arc<aionui_extension::SkillPaths>,
    /// Guide MCP server config (port, token, binary_path).
    /// `None` when the server failed to start (graceful degradation).
    pub guide_mcp_config: Option<GuideMcpConfig>,
    /// Guide MCP server instance kept alive for the app lifetime.
    pub(crate) _guide_server: Option<GuideMcpServer>,
}

impl AppServices {
    /// Replace the worker task manager after construction.
    ///
    /// Primarily used by tests to inject mock implementations.
    pub fn with_worker_task_manager(mut self, wtm: Arc<dyn IWorkerTaskManager>) -> Self {
        self.worker_task_manager = wtm;
        self.conversation_service = build_conversation_service(ConversationServiceDeps {
            database: &self.database,
            work_dir: self.work_dir.clone(),
            event_bus: self.event_bus.clone(),
            skill_paths: self.skill_paths.clone(),
            worker_task_manager: self.worker_task_manager.clone(),
            conversation_runtime_state: self.conversation_runtime_state.clone(),
            conversation_repo: self.conversation_repo.clone(),
            user_repo: self.user_repo.clone(),
            multiuser: self.enforce_file_scope,
            task_manager_delete_hook: self.task_manager_delete_hook.clone(),
        });
        self
    }

    /// Wire the TeamSessionService into the Guide MCP server so
    /// `aion_create_team` requests can call `service.create_team(...)`.
    /// Called from `create_router` after `build_module_states`.
    pub(crate) async fn inject_guide_service(&self, service: std::sync::Weak<aionui_team::TeamSessionService>) {
        if let Some(server) = &self._guide_server {
            server.set_service(service).await;
        }
    }

    pub async fn from_config(database: Database, config: &AppConfig) -> anyhow::Result<Self> {
        let data_dir = config.data_dir.clone();
        let work_dir = config.work_dir.clone();
        let local = config.local;
        let enforce_file_scope = config.enforce_file_scope();
        let app_version = config.app_version.clone();
        let user_repo: Arc<dyn IUserRepository> = Arc::new(SqliteUserRepository::new(database.pool().clone()));
        let usage_repo: Arc<dyn IUsageRepository> = Arc::new(SqliteUsageRepository::new(database.pool().clone()));

        // Resolve JWT secret: env var → system user db field → random generation
        let env_secret = std::env::var("JWT_SECRET").ok();
        let system_user = user_repo
            .get_system_user()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get system user: {e}"))?;

        let db_secret = system_user
            .as_ref()
            .and_then(|u| u.jwt_secret.as_deref())
            .filter(|s| !s.is_empty());

        let (secret, is_new) = resolve_jwt_secret(env_secret.as_deref(), db_secret);

        // Persist newly generated secret to database
        if is_new && let Some(user) = &system_user {
            user_repo
                .update_jwt_secret(&user.id, &secret)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to persist JWT secret: {e}"))?;
            tracing::info!("Generated and persisted new JWT secret");
        }

        let encryption_key = derive_encryption_key(&secret);

        // Identidad firmada por request (Fase 2 #5). El KEK usa dominio separado
        // del de providers; su raíz es `ALINEA_MASTER_KEY` si está en el entorno
        // (aísla la identidad del JWT), o el mismo secreto raíz del JWT en su
        // defecto. La semilla Ed25519 se carga/genera cifrada en reposo (0600).
        let identity_root = std::env::var("ALINEA_MASTER_KEY")
            .ok()
            .unwrap_or_else(|| secret.clone());
        let identity_kek = derive_identity_kek(&identity_root);
        let request_identity = Arc::new(
            load_or_create_identity(&data_dir, &identity_kek)
                .map_err(|e| anyhow::anyhow!("Failed to load identity keystore: {e}"))?,
        );
        tracing::info!(pubkey = %request_identity.public_key_base64(), "Request-identity keystore ready");

        let provider_repo = Arc::new(SqliteProviderRepository::new(database.pool().clone()));
        let event_bus = Arc::new(BroadcastEventBus::new(256));
        // User-configured MCP servers — injected into ACP `session/new`
        // so the agent gets the operator's tools (ELECTRON-1JG fix).
        let mcp_server_repo: Arc<dyn IMcpServerRepository> =
            Arc::new(SqliteMcpServerRepository::new(database.pool().clone()));

        let agent_metadata_repo: Arc<dyn IAgentMetadataRepository> =
            Arc::new(SqliteAgentMetadataRepository::new(database.pool().clone()));
        let agent_registry = AgentRegistry::new(agent_metadata_repo);
        agent_registry
            .hydrate()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to hydrate agent registry: {e}"))?;

        let acp_session_repo: Arc<dyn IAcpSessionRepository> =
            Arc::new(SqliteAcpSessionRepository::new(database.pool().clone()));
        let acp_agent_service = AcpSessionSyncService::new(acp_session_repo.clone());

        let conversation_repo: Arc<dyn IConversationRepository> =
            Arc::new(SqliteConversationRepository::new(database.pool().clone()));

        // Skill paths need app resource dir (for builtin rules) + data dir
        // (for user skills + materialized views). AcpSkillManager uses these
        // for first-message skill index/body loading.
        let app_resource_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.canonicalize().ok())
            .and_then(|p| p.parent().map(|pp| pp.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let skill_paths = Arc::new(aionui_extension::resolve_skill_paths(&app_resource_dir, &data_dir));

        // Absolute path to this process's binary. Reused as the `command` for
        // the stdio MCP bridge spawned by ACP CLIs when a team session is
        // attached to a conversation (phase1 mcp.md §4.6 single-binary model).
        let backend_binary_path =
            Arc::new(std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("aioncore")));

        // Start Guide MCP server. Failure is non-fatal: solo agents simply
        // won't get the `aion_create_team` tool.
        let (guide_server, guide_mcp_config) = match GuideMcpServer::start().await {
            Ok(srv) => {
                let config = GuideMcpConfig {
                    port: srv.http_port(),
                    token: srv.auth_token().to_owned(),
                    binary_path: backend_binary_path.to_string_lossy().to_string(),
                };
                tracing::info!(port = config.port, "Guide MCP server started");
                (Some(srv), Some(config))
            }
            Err(e) => {
                tracing::warn!(
                    code = "BOOTSTRAP_DEGRADED_GUIDE_MCP",
                    stage = "guide_mcp.start",
                    error = %e,
                    "Guide MCP server failed to start; solo create-team disabled"
                );
                (None, None)
            }
        };

        let factory = build_agent_factory(AgentFactoryDeps {
            skill_manager: AcpSkillManager::new(skill_paths.clone()),
            provider_repo,
            encryption_key,
            agent_registry: agent_registry.clone(),
            acp_agent_service: acp_agent_service.clone(),
            data_dir: data_dir.clone(),
            broadcaster: event_bus.clone(),
            backend_binary_path: backend_binary_path.clone(),
            guide_mcp_config: guide_mcp_config.clone(),
            mcp_server_repo: Some(mcp_server_repo),
            request_identity: Some(request_identity.clone()),
        });

        // Agent factory is now wired. Future extension/custom agents
        // that get written to `agent_metadata` will show up after the
        // relevant service calls `AgentRegistry::hydrate`.
        let task_manager_concrete = Arc::new(WorkerTaskManagerImpl::new(factory));
        let worker_task_manager: Arc<dyn IWorkerTaskManager> = task_manager_concrete.clone();
        let task_manager_delete_hook: Arc<dyn OnConversationDelete> = task_manager_concrete;
        let conversation_runtime_state = Arc::new(ConversationRuntimeStateService::default());
        let conversation_service = build_conversation_service(ConversationServiceDeps {
            database: &database,
            work_dir: work_dir.clone(),
            event_bus: event_bus.clone(),
            skill_paths: skill_paths.clone(),
            worker_task_manager: worker_task_manager.clone(),
            conversation_runtime_state: conversation_runtime_state.clone(),
            conversation_repo: conversation_repo.clone(),
            user_repo: user_repo.clone(),
            multiuser: enforce_file_scope,
            task_manager_delete_hook: Some(task_manager_delete_hook.clone()),
        });

        Ok(Self {
            database,
            jwt_service: Arc::new(JwtService::new(secret.clone())),
            user_repo,
            usage_repo,
            cookie_config: Arc::new(CookieConfig::from_env()),
            qr_token_store: Arc::new(QrTokenStore::new()),
            ws_manager: Arc::new(WebSocketManager::new()),
            event_bus,
            worker_task_manager,
            conversation_runtime_state,
            conversation_service,
            task_manager_delete_hook: Some(task_manager_delete_hook),
            agent_registry,
            conversation_repo,
            acp_session_sync: acp_agent_service,
            request_identity,
            jwt_secret_raw: secret,
            data_dir,
            work_dir,
            local,
            enforce_file_scope,
            app_version,
            skill_paths,
            guide_mcp_config: guide_mcp_config.clone(),
            _guide_server: guide_server,
        })
    }
}

struct ConversationServiceDeps<'a> {
    database: &'a Database,
    work_dir: PathBuf,
    event_bus: Arc<BroadcastEventBus>,
    skill_paths: Arc<aionui_extension::SkillPaths>,
    worker_task_manager: Arc<dyn IWorkerTaskManager>,
    conversation_runtime_state: Arc<ConversationRuntimeStateService>,
    conversation_repo: Arc<dyn IConversationRepository>,
    user_repo: Arc<dyn IUserRepository>,
    multiuser: bool,
    task_manager_delete_hook: Option<Arc<dyn OnConversationDelete>>,
}

fn build_conversation_service(deps: ConversationServiceDeps<'_>) -> ConversationService {
    let skill_resolver = Arc::new(aionui_conversation::skill_resolver::ExtensionSkillResolver::new(
        deps.skill_paths,
    ));
    let service = ConversationService::new(
        deps.work_dir,
        deps.event_bus,
        skill_resolver,
        deps.worker_task_manager,
        deps.conversation_repo,
        Arc::new(SqliteAgentMetadataRepository::new(deps.database.pool().clone())),
        Arc::new(SqliteAcpSessionRepository::new(deps.database.pool().clone())),
    )
    .with_runtime_state(deps.conversation_runtime_state)
    .with_multiuser(deps.multiuser);
    service.with_user_repo(deps.user_repo);
    service.with_mcp_server_repo(Arc::new(SqliteMcpServerRepository::new(deps.database.pool().clone())));
    service.with_assistant_definition_repo(Arc::new(SqliteAssistantDefinitionRepository::new(
        deps.database.pool().clone(),
    )));
    service.with_assistant_state_repo(Arc::new(SqliteAssistantOverlayRepository::new(
        deps.database.pool().clone(),
    )));
    service.with_assistant_preference_repo(Arc::new(SqliteAssistantPreferenceRepository::new(
        deps.database.pool().clone(),
    )));
    if let Some(hook) = deps.task_manager_delete_hook {
        service.with_delete_hook(hook);
    }
    service
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Config de test con `data_dir` temporal aislado por test. La DB en memoria
    /// regenera el secreto JWT en cada arranque (y con él el KEK de identidad),
    /// así que un `identity.enc` persistido por otra corrida/test impediría el
    /// boot; un dir único —limpiado al entrar— da a cada test su propio keystore.
    fn isolated_config(label: &str) -> AppConfig {
        let dir = std::env::temp_dir().join(format!("alinea-app-{}-{}", std::process::id(), label));
        let _ = std::fs::remove_dir_all(&dir);
        AppConfig {
            data_dir: dir,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_app_services_from_memory_db() {
        let db = aionui_db::init_database_memory().await.unwrap();
        let services = AppServices::from_config(db, &isolated_config("memdb")).await.unwrap();

        // JWT service should be functional
        let token = services.jwt_service.sign("test_user", "testuser").unwrap();
        let payload = services.jwt_service.verify(&token).unwrap();
        assert_eq!(payload.user_id, "test_user");

        // User repo should have system user
        let has_users = services.user_repo.has_users().await.unwrap();
        assert!(!has_users); // system user has empty password → not counted

        services.database.close().await;
    }

    #[tokio::test]
    async fn test_jwt_secret_persisted_to_db() {
        let db = aionui_db::init_database_memory().await.unwrap();
        let services = AppServices::from_config(db, &isolated_config("jwtpersist"))
            .await
            .unwrap();

        // System user should now have a jwt_secret persisted
        let system_user = services.user_repo.get_system_user().await.unwrap();
        let jwt_secret = system_user.unwrap().jwt_secret;
        assert!(jwt_secret.is_some());
        assert!(!jwt_secret.unwrap().is_empty());

        services.database.close().await;
    }

    #[tokio::test]
    async fn test_app_services_uses_supplied_app_version() {
        let db = aionui_db::init_database_memory().await.unwrap();
        let config = AppConfig {
            app_version: "9.9.9".to_string(),
            ..isolated_config("appver")
        };
        let services = AppServices::from_config(db, &config).await.unwrap();

        assert_eq!(services.app_version, "9.9.9");

        services.database.close().await;
    }
}
