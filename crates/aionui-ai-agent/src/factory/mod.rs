pub mod acp_assembler;

mod acp;
pub(crate) mod aionrs;
mod context;

use std::path::PathBuf;
use std::sync::Arc;

use aionui_api_types::GuideMcpConfig;
use aionui_auth::RequestIdentityService;
use aionui_db::{IAgentProfileRepository, IMcpServerRepository, IProviderRepository, IResourceAclRepository};
use aionui_realtime::EventBroadcaster;
use futures_util::FutureExt;

use crate::agent_task::AgentInstance;
use crate::capability::skill_manager::AcpSkillManager;
use crate::error::AgentError;
use crate::factory::context::FactoryContext;
use crate::persistence::AcpSessionSyncService;
use crate::registry::AgentRegistry;
use crate::session_context::AgentSessionKind;
use crate::task_manager::AgentFactory;
use crate::types::BuildTaskOptions;

/// Dependencies needed by the agent factory to construct agents.
pub struct AgentFactoryDeps {
    pub skill_manager: Arc<AcpSkillManager>,
    pub provider_repo: Arc<dyn IProviderRepository>,
    pub encryption_key: [u8; 32],
    pub agent_registry: Arc<AgentRegistry>,
    pub acp_agent_service: Arc<AcpSessionSyncService>,
    pub data_dir: PathBuf,
    pub broadcaster: Arc<dyn EventBroadcaster>,
    /// Absolute path to the backend binary, reused as the `command` of the
    /// stdio MCP bridge injected into ACP `session/new` for team sessions.
    /// Captured once at app startup (`std::env::current_exe()`).
    pub backend_binary_path: Arc<PathBuf>,
    /// Guide MCP server config. When `Some`, injected into solo (non-team)
    /// ACP agent sessions so the agent gets the `aion_create_team` tool.
    /// `None` when the Guide server failed to start (graceful degradation).
    pub guide_mcp_config: Option<GuideMcpConfig>,
    /// User-configured MCP servers repository. Used by ACP factory to
    /// inject enabled servers into `session/new` (ELECTRON-1JG fix).
    /// `None` for tests/composition paths that do not need MCP injection.
    pub mcp_server_repo: Option<Arc<dyn IMcpServerRepository>>,
    /// Emisor de tokens de identidad firmados (Fase 2 #5). Cuando `Some`, la
    /// factory ACP inyecta `AION_IDENTITY_TOKEN`/`AION_IDENTITY_PUBKEY` en el
    /// env del proceso del agente. `None` en tests/paths sin identidad.
    pub request_identity: Option<Arc<RequestIdentityService>>,
    /// Perfiles de agente (Motor MULTI-PERFIL — tarea A2). Cuando la sesión
    /// trae `profile_id`, la factory ACP lo resuelve por `name` aquí para
    /// poblar `scopes` desde `definition.mcp_allowlist`. `None` en
    /// tests/paths que no ejercitan perfiles — en ese caso una sesión con
    /// `profile_id` falla cerrado (ver `resolve_profile_scopes`).
    pub profile_repo: Option<Arc<dyn IAgentProfileRepository>>,
    /// ACL de recursos (Motor MULTI-PERFIL — tarea A2). Gate fail-closed:
    /// antes de emitir el token con `profile_id`, se verifica que el usuario
    /// (o alguno de sus roles) tenga membresía `resource_acl` con
    /// `resource_type='agent_profile'` sobre ese perfil (mismo patrón que
    /// `GET /api/profiles`, `crates/aionui-app/src/router/profiles.rs`). Los
    /// admins (`roles` contiene `"admin"`) pasan sin grant explícito,
    /// consistente con `list_visible_profiles`. `None` → mismo fail-closed
    /// que `profile_repo: None`.
    pub resource_acl_repo: Option<Arc<dyn IResourceAclRepository>>,
}

/// Build a production agent factory that dispatches to concrete agent types.
///
/// [`AgentFactory`] is async: the returned `BoxFuture` is driven by
/// [`crate::task_manager::IWorkerTaskManager::get_or_build_task`] on whatever
/// runtime is currently polling it. This lets us spawn CLI processes and
/// await ACP handshakes directly, without the scoped-thread + `block_on`
/// bridge the old sync-factory version needed.
pub fn build_agent_factory(deps: AgentFactoryDeps) -> AgentFactory {
    let deps = Arc::new(deps);

    Arc::new(move |options: BuildTaskOptions| {
        let deps = deps.clone();
        async move { build_agent(deps, options).await }.boxed()
    })
}

async fn build_agent(deps: Arc<AgentFactoryDeps>, options: BuildTaskOptions) -> Result<AgentInstance, AgentError> {
    let context = options.context;
    let ctx = FactoryContext::resolve(&context).await?;
    let model = context.model.clone();
    match context.kind {
        AgentSessionKind::Acp(acp_context) => acp::build(deps, *acp_context, ctx).await,
        AgentSessionKind::Aionrs(aionrs_context) => aionrs::build(deps, *aionrs_context, model, ctx).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_deps_can_be_constructed() {
        // Verify types compile — actual construction requires DB
        let _: fn() -> AgentFactoryDeps = || {
            panic!("compile-time check only");
        };
    }
}
