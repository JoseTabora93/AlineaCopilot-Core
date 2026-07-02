//! Enforcement de techo de gasto POR PERFIL en el pre-flight del turno
//! (Fase perfiles — tarea C4, `turn_orchestrator::check_profile_cap`).
//!
//! Estrategia: `ConversationService` real con DB en memoria + un task manager
//! que REGISTRA si `get_or_build_task` fue invocado. Si el techo del perfil
//! bloquea, el agente NUNCA se construye (no se gasta); si no hay bloqueo, el
//! build sí se intenta (aquí falla con un error nop — el turno termina
//! `Failed` igual, pero la señal que importa es el flag del task manager).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aionui_ai_agent::{AgentError, IWorkerTaskManager};
use aionui_common::{AgentKillReason, TimestampMs, now_ms};
use aionui_conversation::skill_resolver::SkillResolver;
use aionui_conversation::{ConversationAgentTurnRequest, ConversationAgentTurnStatus, ConversationService};
use aionui_db::models::IngestUsageEvent;
use aionui_db::{
    IAgentProfileRepository, IUsageRepository, NewAgentProfile, SqliteAgentProfileRepository,
    SqliteConversationRepository, SqliteUsageRepository, init_database_memory,
};
use aionui_realtime::EventBroadcaster;
use serde_json::json;

// ── Infraestructura de test ───────────────────────────────────────────

struct SilentBroadcaster;

impl EventBroadcaster for SilentBroadcaster {
    fn broadcast(&self, _event: aionui_api_types::WebSocketMessage<serde_json::Value>) {}
}

struct EmptySkillResolver;

#[async_trait::async_trait]
impl SkillResolver for EmptySkillResolver {
    async fn auto_inject_names(&self) -> Vec<String> {
        Vec::new()
    }

    async fn resolve_skills(&self, _names: &[String]) -> Vec<aionui_extension::ResolvedAgentSkill> {
        Vec::new()
    }

    async fn link_workspace_skills(
        &self,
        _workspace: &std::path::Path,
        _rel_dirs: &[&str],
        _skills: &[aionui_extension::ResolvedAgentSkill],
    ) -> usize {
        0
    }
}

/// Task manager nop que registra si `get_or_build_task` llegó a invocarse —
/// la señal de "el pre-flight NO bloqueó" (el build en sí falla a propósito).
struct RecordingTaskManager {
    build_attempted: AtomicBool,
}

impl RecordingTaskManager {
    fn new() -> Self {
        Self {
            build_attempted: AtomicBool::new(false),
        }
    }

    fn build_was_attempted(&self) -> bool {
        self.build_attempted.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl IWorkerTaskManager for RecordingTaskManager {
    fn get_task(&self, _: &str) -> Option<aionui_ai_agent::AgentInstance> {
        None
    }
    async fn get_or_build_task(
        &self,
        _: &str,
        _: aionui_ai_agent::types::BuildTaskOptions,
    ) -> Result<aionui_ai_agent::AgentInstance, AgentError> {
        self.build_attempted.store(true, Ordering::SeqCst);
        Err(AgentError::internal("recording noop: build not implemented in test"))
    }
    fn kill(&self, _: &str, _: Option<AgentKillReason>) -> Result<(), AgentError> {
        Ok(())
    }
    fn kill_and_wait(
        &self,
        _: &str,
        _: Option<AgentKillReason>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(std::future::ready(()))
    }
    async fn clear(&self) {}
    fn active_count(&self) -> usize {
        0
    }
    fn collect_idle(&self, _: TimestampMs) -> Vec<String> {
        vec![]
    }
}

const USER_ID: &str = "system_default_user";

struct TestEnv {
    svc: ConversationService,
    task_mgr: Arc<RecordingTaskManager>,
    usage_repo: Arc<dyn IUsageRepository>,
    profile_repo: Arc<dyn IAgentProfileRepository>,
}

async fn setup() -> TestEnv {
    let db = init_database_memory().await.unwrap();
    let pool = db.pool().clone();
    let conversation_repo = Arc::new(SqliteConversationRepository::new(pool.clone()));
    let agent_metadata_repo: Arc<dyn aionui_db::IAgentMetadataRepository> =
        Arc::new(aionui_db::SqliteAgentMetadataRepository::new(pool.clone()));
    let acp_session_repo: Arc<dyn aionui_db::IAcpSessionRepository> =
        Arc::new(aionui_db::SqliteAcpSessionRepository::new(pool.clone()));
    let task_mgr = Arc::new(RecordingTaskManager::new());
    let usage_repo: Arc<dyn IUsageRepository> = Arc::new(SqliteUsageRepository::new(pool.clone()));
    let profile_repo: Arc<dyn IAgentProfileRepository> = Arc::new(SqliteAgentProfileRepository::new(pool.clone()));

    let svc = ConversationService::new(
        std::env::temp_dir(),
        Arc::new(SilentBroadcaster),
        Arc::new(EmptySkillResolver),
        task_mgr.clone() as Arc<dyn IWorkerTaskManager>,
        conversation_repo,
        agent_metadata_repo,
        acp_session_repo,
    );
    svc.with_usage_repo(usage_repo.clone());
    svc.with_profile_repo(profile_repo.clone());

    TestEnv {
        svc,
        task_mgr,
        usage_repo,
        profile_repo,
    }
}

fn workspace_path(label: &str) -> String {
    let workspace = std::env::temp_dir().join(format!("aionui-turn-profile-cap-{label}"));
    std::fs::create_dir_all(&workspace).unwrap();
    workspace.to_string_lossy().to_string()
}

/// Definition PERFIL v1 completa (pasa `validate_profile_definition`) con los
/// caps indicados.
fn profile_definition(name: &str, soft_usd: f64, hard_usd: f64) -> String {
    json!({
        "name": name,
        "label": "Test profile",
        "engines": ["openclaw"],
        "soul_md": "# Test",
        "model": { "primary": "zai/glm-5.1", "fallbacks": [] },
        "mcp_allowlist": [],
        "skills": [],
        "kb_scope": [],
        "channels": [{ "type": "web" }],
        "caps": { "soft_usd": soft_usd, "hard_usd": hard_usd, "period": "month" },
        "acl": { "etiqueta": "interno", "roles": ["admin"] }
    })
    .to_string()
}

async fn seed_profile(env: &TestEnv, name: &str, definition: String) {
    env.profile_repo
        .create(NewAgentProfile {
            name: name.to_string(),
            label: format!("Perfil {name}"),
            definition,
        })
        .await
        .unwrap();
}

/// Siembra gasto atribuido al perfil `profile_id` (vía ingest, como lo haría
/// el pipeline de preventa con su service token).
async fn seed_profile_spend(env: &TestEnv, profile_id: &str, cost_usd: f64) {
    env.usage_repo
        .ingest_event(IngestUsageEvent {
            engine: "openclaw".into(),
            model: Some("glm-5.1".into()),
            provider: Some("zai".into()),
            user_id: None,
            profile_id: Some(profile_id.to_string()),
            project_id: None,
            tokens_in: 1000,
            tokens_out: 500,
            cache_read: 0,
            cache_write: 0,
            cost_usd,
            ts_ms: now_ms(),
            source: "test_emitter".into(),
            idempotency_key: None,
        })
        .await
        .unwrap();
}

async fn create_conversation(env: &TestEnv, label: &str, profile_id: Option<&str>) -> String {
    let mut extra = json!({
        "workspace": workspace_path(label),
        "agent_id": "custom-agent-1",
        "backend": "claude",
        "agent_source": "custom"
    });
    if let Some(profile_id) = profile_id {
        extra["profile_id"] = json!(profile_id);
    }
    let req = serde_json::from_value(json!({ "type": "acp", "extra": extra })).unwrap();
    env.svc.create(USER_ID, req).await.unwrap().id
}

async fn run_turn(env: &TestEnv, conversation_id: &str) -> ConversationAgentTurnStatus {
    env.svc
        .run_agent_turn(ConversationAgentTurnRequest {
            user_id: USER_ID.to_string(),
            conversation_id: conversation_id.to_string(),
            content: "hola".to_string(),
            files: vec![],
            inject_skills: vec![],
            on_started: None,
        })
        .await
        .unwrap()
        .status
}

// ── Tests ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn profile_under_cap_proceeds_to_agent_build() {
    let env = setup().await;
    seed_profile(&env, "preventa", profile_definition("preventa", 4.0, 8.0)).await;
    seed_profile_spend(&env, "preventa", 2.0).await;
    let conv = create_conversation(&env, "under-cap", Some("preventa")).await;

    let status = run_turn(&env, &conv).await;

    // El pre-flight NO bloquea: el build del agente sí se intenta (y falla
    // con el nop del test — por eso el turno termina Failed igualmente).
    assert!(
        env.task_mgr.build_was_attempted(),
        "under cap: the agent build must be attempted"
    );
    assert_eq!(status, ConversationAgentTurnStatus::Failed);
}

#[tokio::test]
async fn profile_over_cap_blocks_without_building_agent() {
    let env = setup().await;
    seed_profile(&env, "preventa", profile_definition("preventa", 4.0, 8.0)).await;
    seed_profile_spend(&env, "preventa", 9.0).await;
    let conv = create_conversation(&env, "over-cap", Some("preventa")).await;

    let events_before = env.usage_repo.summary_for_profile("preventa", 0).await.unwrap().events;
    let status = run_turn(&env, &conv).await;

    assert_eq!(status, ConversationAgentTurnStatus::Failed);
    assert!(
        !env.task_mgr.build_was_attempted(),
        "over cap: the agent must NOT be built (no spend)"
    );
    // Sin gasto nuevo: el ledger del perfil no creció.
    let events_after = env.usage_repo.summary_for_profile("preventa", 0).await.unwrap().events;
    assert_eq!(events_before, events_after, "blocked turn must not add usage events");
}

#[tokio::test]
async fn spend_exactly_at_hard_cap_blocks() {
    let env = setup().await;
    seed_profile(&env, "preventa", profile_definition("preventa", 4.0, 8.0)).await;
    seed_profile_spend(&env, "preventa", 8.0).await;
    let conv = create_conversation(&env, "at-cap", Some("preventa")).await;

    let status = run_turn(&env, &conv).await;

    assert_eq!(status, ConversationAgentTurnStatus::Failed);
    assert!(
        !env.task_mgr.build_was_attempted(),
        "spent == hard must block (>= semantics, same as the user limit)"
    );
}

#[tokio::test]
async fn no_profile_id_keeps_existing_behavior() {
    let env = setup().await;
    // Gasto enorme de un perfil cualquiera: irrelevante sin profile_id en la sesión.
    seed_profile(&env, "preventa", profile_definition("preventa", 4.0, 8.0)).await;
    seed_profile_spend(&env, "preventa", 999.0).await;
    let conv = create_conversation(&env, "no-profile", None).await;

    let _status = run_turn(&env, &conv).await;

    assert!(
        env.task_mgr.build_was_attempted(),
        "session without profile_id must not be gated by any profile cap"
    );
}

#[tokio::test]
async fn profile_without_caps_has_no_profile_limit() {
    let env = setup().await;
    // Definition sin `caps` (el repo no valida el schema — la validación vive
    // en el handler HTTP; esto simula drift o un perfil legado).
    seed_profile(&env, "sin-caps", r#"{"name":"sin-caps","label":"X"}"#.to_string()).await;
    seed_profile_spend(&env, "sin-caps", 999.0).await;
    let conv = create_conversation(&env, "no-caps", Some("sin-caps")).await;

    let _status = run_turn(&env, &conv).await;

    assert!(
        env.task_mgr.build_was_attempted(),
        "profile without parseable caps must not be gated (only the user limit applies)"
    );
}

#[tokio::test]
async fn unknown_profile_does_not_block() {
    let env = setup().await;
    let conv = create_conversation(&env, "unknown-profile", Some("no-existe")).await;

    let _status = run_turn(&env, &conv).await;

    assert!(
        env.task_mgr.build_was_attempted(),
        "unknown profile must not block the turn (factory-level ACL will handle access)"
    );
}
