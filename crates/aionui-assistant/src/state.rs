//! Router state carrying the assistant service for axum handlers.

use std::sync::Arc;

use aionui_db::{IAgentProfileRepository, IAssistantDefinitionRepository, IResourceAclRepository};

use crate::service::AssistantService;

/// Shared state injected into `/api/assistants/*` handlers.
#[derive(Clone)]
pub struct AssistantRouterState {
    pub service: Arc<AssistantService>,
    /// Perfiles de agente + ACL de recursos + definiciones de assistants —
    /// usados por el gate de visibilidad de assistants gestionados
    /// (`source='generated'`, motor `"copilot"`, tarea A8): replica
    /// `list_visible_profiles` (`crates/aionui-app/src/router/profiles.rs`)
    /// para el subconjunto de assistants materializados desde un perfil.
    /// `None` mantiene el comportamiento previo a A8 (sin filtrar) — usado
    /// por tests que no necesitan ejercer el gate.
    pub profile_visibility: Option<ProfileVisibilityGate>,
}

/// Dependencias del gate de visibilidad perfil→assistant.
#[derive(Clone)]
pub struct ProfileVisibilityGate {
    pub profile_repo: Arc<dyn IAgentProfileRepository>,
    pub acl_repo: Arc<dyn IResourceAclRepository>,
    pub definition_repo: Arc<dyn IAssistantDefinitionRepository>,
}
