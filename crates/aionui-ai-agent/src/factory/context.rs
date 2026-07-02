//! Workspace information shared across factory builders. The conversation
//! domain has already decoded raw DB state into typed context before this
//! layer sees it.

use crate::error::AgentError;
use crate::session_context::AgentSessionContext;

pub(super) struct FactoryContext {
    pub conversation_id: String,
    /// Dueño de la conversación. Usado para emitir el token de identidad
    /// firmado que viaja al agente (Fase 2 #5).
    pub user_id: String,
    /// Roles RBAC del usuario (eje 1), propagados al token de identidad.
    pub roles: Vec<String>,
    /// Proyecto de la conversación (Fase 2 #2). Va como `project_id` del token
    /// firmado; `None` si la conversación no pertenece a ningún proyecto.
    pub project_id: Option<String>,
    /// Perfil de agente de la sesión (Motor MULTI-PERFIL — tarea A2), por
    /// `name` de `agent_profiles`. `None` = comportamiento legado intacto.
    /// La factory ACP debe verificar acceso (gate `resource_acl`) ANTES de
    /// emitir el token cuando este campo está presente.
    pub profile_id: Option<String>,
    pub workspace: String,
    pub is_custom_workspace: bool,
}

impl FactoryContext {
    pub async fn resolve(context: &AgentSessionContext) -> Result<Self, AgentError> {
        Ok(Self {
            conversation_id: context.conversation.conversation_id.clone(),
            user_id: context.conversation.user_id.clone(),
            roles: context.conversation.roles.clone(),
            project_id: context.conversation.project_id.clone(),
            profile_id: context.conversation.profile_id.clone(),
            workspace: context.workspace.path.clone(),
            is_custom_workspace: context.workspace.is_custom,
        })
    }
}
