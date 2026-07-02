#![warn(clippy::disallowed_types)]

//! SQLite database layer: init, migrations, repository traits, and implementations.
mod database;
mod error;
pub mod models;
pub mod pricing;
mod repository;
pub mod task_state;

pub use database::{
    Database, DatabaseInitError, init_database, init_database_memory, init_database_staged, maybe_copy_legacy_database,
};
pub use error::DbError;
pub use models::{
    AgentMetadataRow, AssistantDefinitionRow, AssistantOverlayRow, AssistantOverrideRow, AssistantPreferenceRow,
    AssistantRow, ConversationArtifactRow, ConversationAssistantSnapshotRow, CreateAssistantParams,
    CreatedServiceToken, IngestResult, IngestUsageEvent, PipelineTemplateRow, ProjectRow, ResourceAclRow,
    ServiceTokenRow, TaskArtifactRow, TaskHandoffLogRow, TaskRow, UpdateAgentHandshakeParams, UpdateAssistantParams,
    UpsertAgentMetadataParams, UpsertAssistantDefinitionParams, UpsertAssistantOverlayParams,
    UpsertAssistantPreferenceParams, UpsertConversationAssistantSnapshotParams, UpsertOverrideParams,
};
pub use repository::channel::UpdatePluginStatusParams;
pub use repository::conversation::{
    ConversationFilters, ConversationRowUpdate, MessageRowUpdate, MessageSearchRow, SortOrder,
};
pub use repository::cron::UpdateCronJobParams;
pub use repository::mcp_server::{CreateMcpServerParams, UpdateMcpServerParams};
pub use repository::oauth_token::UpsertOAuthTokenParams;
pub use repository::provider::{CreateProviderParams, UpdateProviderParams};
pub use repository::remote_agent::{CreateRemoteAgentParams, UpdateRemoteAgentParams};
pub use repository::team::{UpdateTaskParams, UpdateTeamParams};
pub use repository::{
    CreateAcpSessionParams, IAcpSessionRepository, IAgentMetadataRepository, IAssistantDefinitionRepository,
    IAssistantOverlayRepository, IAssistantOverrideRepository, IAssistantPreferenceRepository, IAssistantRepository,
    IChannelRepository, IClientPreferenceRepository, IConversationRepository, ICronRepository, IMcpServerRepository,
    IOAuthTokenRepository, IProjectRepository, IProviderRepository, IRemoteAgentRepository, IResourceAclRepository,
    ISettingsRepository, ITaskRepository, ITeamRepository, IUsageRepository, IUserRepository, MemberRevoke,
    NewArtifact, NewHandoff, NewProject, NewTask, PersistedSessionState, ProjectUpdate, RoleRemoval,
    SaveRuntimeStateParams, SqliteAcpSessionRepository, SqliteAgentMetadataRepository,
    SqliteAssistantDefinitionRepository, SqliteAssistantOverlayRepository, SqliteAssistantOverrideRepository,
    SqliteAssistantPreferenceRepository, SqliteAssistantRepository, SqliteChannelRepository,
    SqliteClientPreferenceRepository, SqliteConversationRepository, SqliteCronRepository, SqliteMcpServerRepository,
    SqliteOAuthTokenRepository, SqliteProjectRepository, SqliteProviderRepository, SqliteRemoteAgentRepository,
    SqliteResourceAclRepository, SqliteSettingsRepository, SqliteTaskRepository, SqliteTeamRepository,
    SqliteUsageRepository, SqliteUserRepository, TaskUpdate, UsageQueryFilters, rebuild_legacy_assistant_mirror,
};

// Ingest de emisores externos (Fase ledger — tarea C1): hash de service tokens,
// reusado por el router para validar el header `Authorization: Bearer`.
pub use repository::sqlite_usage::hash_service_token;

// Re-export sqlx pool type for downstream crates
pub use sqlx::SqlitePool;
