use aionui_api_types::{AcpBuildExtra, AionrsBuildExtra, TeamSessionBinding};
use aionui_common::{AgentType, ProviderWithModel};

use crate::shared_kernel::PersistedSessionState;

/// Typed runtime-build input for creating or resuming an agent task.
///
/// This is the boundary after `conversation.extra` has been decoded by the
/// conversation domain. Agent factories should consume this typed shape rather
/// than re-parsing raw JSON from the DB envelope.
#[derive(Debug, Clone)]
pub struct AgentSessionContext {
    pub conversation: ConversationContext,
    pub workspace: WorkspaceContext,
    pub model: ProviderWithModel,
    pub skills: Vec<String>,
    pub team: Option<TeamSessionBinding>,
    pub kind: AgentSessionKind,
}

#[derive(Debug, Clone)]
pub struct ConversationContext {
    pub conversation_id: String,
    pub user_id: String,
    /// Roles RBAC del usuario (eje 1), resueltos al construir el contexto.
    /// Viajan al agente para emitir el token de identidad y aplicar scope.
    pub roles: Vec<String>,
    /// Proyecto de la conversación (Fase 2 #2). Viaja al agente en el token de
    /// identidad firmado como `project_id` para acotar el scope (RAG por proyecto).
    pub project_id: Option<String>,
    /// Perfil de agente de la sesión (Motor MULTI-PERFIL — tarea A2), por
    /// `name` de `agent_profiles`. `None` = sin perfil (comportamiento legado
    /// intacto). Cuando está presente, la factory ACP debe verificar acceso
    /// (`resource_acl` `agent_profile`, fail-closed) ANTES de emitir el token
    /// de identidad, y poblar `scopes` desde `definition.mcp_allowlist`.
    pub profile_id: Option<String>,
    pub agent_type: AgentType,
    pub source: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceContext {
    /// Workspace path used by the runtime.
    pub path: String,
    /// Workspace path already persisted in `conversation.extra.workspace`.
    /// Empty when this is a legacy row without a stored workspace.
    pub stored_path: String,
    /// Whether the user supplied this workspace explicitly.
    pub is_custom: bool,
}

#[derive(Debug, Clone)]
pub enum AgentSessionKind {
    Acp(Box<AcpSessionBuildContext>),
    Aionrs(Box<AionrsSessionBuildContext>),
}

#[derive(Debug, Clone)]
pub struct AcpSessionBuildContext {
    pub config: AcpBuildExtra,
    pub team: Option<TeamSessionBinding>,
    pub belongs_to_team: bool,
    pub session_id: Option<String>,
    pub session_snapshot: Option<PersistedSessionState>,
}

#[derive(Debug, Clone)]
pub struct AionrsSessionBuildContext {
    pub config: AionrsBuildExtra,
    pub team: Option<TeamSessionBinding>,
    pub belongs_to_team: bool,
}

impl AgentSessionContext {
    pub fn conversation_id(&self) -> &str {
        &self.conversation.conversation_id
    }

    pub fn agent_type(&self) -> AgentType {
        self.conversation.agent_type
    }
}
