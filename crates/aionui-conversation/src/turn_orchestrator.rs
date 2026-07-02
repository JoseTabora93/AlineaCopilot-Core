use std::sync::Arc;

use aionui_ai_agent::types::{BuildTaskOptions, SendMessageData};
use aionui_ai_agent::{AgentSendError, IWorkerTaskManager};
use aionui_common::{ConversationStatus, ErrorChain, now_ms};
use aionui_db::models::ConversationRow;
use tokio::sync::oneshot;
use tracing::{debug, error, info, warn};

use crate::agent_health_policy::{AgentHealthAction, AgentHealthPolicy};
use crate::runtime_state::TurnClaim;
use crate::service::{
    ConversationService, MAX_CRON_CONTINUATIONS_PER_TURN, agent_error_top_level_code, persist_session_key,
};
use crate::stream_relay::StreamRelay;
use crate::turn_continuation_policy::{ContinuationDecision, TurnContinuationPolicy};
use aionui_api_types::{AgentErrorCode, AgentErrorOwnership, SendMessageRequest};

pub(crate) struct TurnStartInput {
    pub user_id: String,
    pub conversation: ConversationRow,
    pub request: SendMessageRequest,
    pub build_options: BuildTaskOptions,
    pub stored_workspace: String,
    pub turn_id: String,
    pub turn_claim: TurnClaim,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConversationTurnStatus {
    Completed,
    Failed,
}

pub(crate) struct ConversationTurnResult {
    pub status: ConversationTurnStatus,
}

pub(crate) struct ConversationTurnOrchestrator {
    service: ConversationService,
    task_manager: Arc<dyn IWorkerTaskManager>,
}

impl ConversationTurnOrchestrator {
    pub fn new(service: ConversationService, task_manager: Arc<dyn IWorkerTaskManager>) -> Self {
        Self { service, task_manager }
    }

    pub fn spawn_user_turn(self, input: TurnStartInput) {
        tokio::spawn(async move {
            let _ = self.run_user_turn(input).await;
        });
    }

    /// Pre-flight del techo de gasto por PERFIL (tarea C4). Devuelve
    /// `Some(result)` (siempre `Failed`) si el turno debe bloquearse SIN
    /// construir el agente ni gastar; `None` para continuar normalmente
    /// (incluye: sin `profile_id`, perfil no encontrado/sin repo wireado,
    /// `caps` ausente/no parseable, o gasto bajo `hard_usd`).
    async fn check_profile_cap(
        &self,
        build_options: &BuildTaskOptions,
        conv_id: &str,
        turn_id: &str,
    ) -> Option<ConversationTurnResult> {
        let profile_name = build_options.context.conversation.profile_id.as_deref()?;
        let profile_repo = self.service.profile_repo()?;
        let usage_repo = self.service.usage_repo()?;

        let profile = match profile_repo.get_by_name(profile_name).await {
            Ok(Some(profile)) => profile,
            Ok(None) => {
                debug!(
                    conversation_id = %conv_id,
                    turn_id = %turn_id,
                    profile_id = %profile_name,
                    "ledger: perfil de la sesión no encontrado; sin enforcement de techo de perfil"
                );
                return None;
            }
            Err(err) => {
                warn!(
                    conversation_id = %conv_id,
                    turn_id = %turn_id,
                    profile_id = %profile_name,
                    error = %ErrorChain(&err),
                    "ledger: fallo al resolver el perfil de la sesión; sin enforcement de techo de perfil"
                );
                return None;
            }
        };

        // Perfil sin `caps` parseables = sin límite de perfil (solo usuario).
        let caps = aionui_db::extract_caps(&profile.definition)?;

        let since = now_ms() - 30 * 24 * 60 * 60 * 1000;
        let spent = match usage_repo.summary_for_profile(profile_name, since).await {
            Ok(summary) => summary.cost_usd,
            Err(err) => {
                warn!(
                    conversation_id = %conv_id,
                    turn_id = %turn_id,
                    profile_id = %profile_name,
                    error = %ErrorChain(&err),
                    "ledger: fallo al leer el gasto del perfil; sin enforcement de techo de perfil"
                );
                return None;
            }
        };

        if spent >= caps.soft_usd && spent < caps.hard_usd {
            warn!(
                conversation_id = %conv_id,
                turn_id = %turn_id,
                profile_id = %profile_name,
                spent,
                soft = caps.soft_usd,
                hard = caps.hard_usd,
                "ledger: perfil superó el umbral soft de gasto (solo aviso, no bloquea)"
            );
        }

        if spent < caps.hard_usd {
            return None;
        }

        warn!(
            conversation_id = %conv_id,
            turn_id = %turn_id,
            profile_id = %profile_name,
            spent,
            hard = caps.hard_usd,
            "ledger: turno bloqueado por límite de consumo del PERFIL (hard)"
        );
        let send_error = AgentSendError::new(
            format!(
                "El perfil '{profile_name}' no tiene presupuesto disponible (${spent:.2} de ${:.2}). Pide a un administrador que lo amplíe.",
                caps.hard_usd
            ),
            AgentErrorCode::UserLlmProviderBillingRequired,
            AgentErrorOwnership::Aionui,
            None,
            false,
            false,
            None,
        );
        self.service
            .persist_and_broadcast_send_failure_tip(conv_id, turn_id, &send_error, Some("PROFILE_USAGE_LIMIT_EXCEEDED"))
            .await;
        Some(ConversationTurnResult {
            status: ConversationTurnStatus::Failed,
        })
    }

    pub(crate) async fn run_user_turn(self, input: TurnStartInput) -> ConversationTurnResult {
        let mut turn_claim = input.turn_claim;
        let conv_id = input.conversation.id.clone();
        let turn_id = input.turn_id.clone();
        let build_started_at = now_ms();
        let persistence = self.service.runtime_persistence();
        let runtime_state = self.service.runtime_state();
        let mut turn_failed = false;

        // Enforcement de límite de consumo (Fase 2 #3, pre-flight): si el usuario
        // superó su `hard_usd` del período, NO se construye el agente ni se gasta.
        if let Some(usage_repo) = self.service.usage_repo()
            && let Ok(Some(limit)) = usage_repo.get_limit(&input.user_id).await
            && let Some(hard) = limit.hard_usd
        {
            let since = now_ms() - 30 * 24 * 60 * 60 * 1000;
            let spent = usage_repo
                .summary_for_user(&input.user_id, since)
                .await
                .map(|s| s.cost_usd)
                .unwrap_or(0.0);
            if spent >= hard {
                warn!(
                    conversation_id = %conv_id,
                    turn_id = %turn_id,
                    user_id = %input.user_id,
                    spent,
                    hard,
                    "ledger: turno bloqueado por límite de consumo (hard)"
                );
                let send_error = AgentSendError::new(
                    format!(
                        "Límite de consumo excedido (${spent:.2} de ${hard:.2}). Pide a un administrador que lo amplíe."
                    ),
                    AgentErrorCode::UserLlmProviderBillingRequired,
                    AgentErrorOwnership::Aionui,
                    None,
                    false,
                    false,
                    None,
                );
                self.service
                    .persist_and_broadcast_send_failure_tip(
                        &conv_id,
                        &turn_id,
                        &send_error,
                        Some("USAGE_LIMIT_EXCEEDED"),
                    )
                    .await;
                let was_deleting = turn_claim.release_for_turn(&turn_id);
                self.service
                    .complete_released_turn(&conv_id, &turn_id, was_deleting)
                    .await;
                return ConversationTurnResult {
                    status: ConversationTurnStatus::Failed,
                };
            }
        }

        // Enforcement de techo de gasto POR PERFIL (Fase perfiles — tarea C4,
        // pre-flight): además del límite por usuario (arriba), si la sesión
        // tiene `profile_id` (name del perfil, ver `ConversationContext::profile_id`)
        // y el perfil trae `caps.hard_usd` en su `definition`, se compara el
        // gasto agregado del PERFIL (todos los usuarios, ventana rolling 30
        // días) contra ese techo. `caps.soft_usd` solo se loggea (warning) —
        // no bloquea. Sin `profile_id`, o perfil sin `caps` parseables, el
        // comportamiento es idéntico al de antes (solo límite de usuario).
        if let Some(result) = self.check_profile_cap(&input.build_options, &conv_id, &turn_id).await {
            let was_deleting = turn_claim.release_for_turn(&turn_id);
            self.service
                .complete_released_turn(&conv_id, &turn_id, was_deleting)
                .await;
            return result;
        }

        info!(conversation_id = %conv_id, turn_id = %turn_id, "conversation turn orchestrator started");
        info!(conversation_id = %conv_id, turn_id = %turn_id, "Agent task build started");
        let agent = match self.task_manager.get_or_build_task(&conv_id, input.build_options).await {
            Ok(agent) => agent,
            Err(err) => {
                let top_level_code = agent_error_top_level_code(&err);
                let send_error = AgentSendError::from_agent_error_ref(&err);
                error!(
                    conversation_id = %conv_id,
                    turn_id = %turn_id,
                    error_code = ?send_error.code(),
                    error = %ErrorChain(&err),
                    "Agent task build failed"
                );
                self.service
                    .persist_and_broadcast_send_failure_tip(&conv_id, &turn_id, &send_error, Some(top_level_code))
                    .await;
                let was_deleting = turn_claim.release_for_turn(&turn_id);
                self.service
                    .complete_released_turn(&conv_id, &turn_id, was_deleting)
                    .await;
                return ConversationTurnResult {
                    status: ConversationTurnStatus::Failed,
                };
            }
        };

        if let Err(err) = self
            .service
            .maybe_persist_workspace(&conv_id, &input.stored_workspace, agent.workspace())
            .await
        {
            let top_level_code = err.error_code();
            let send_error = AgentSendError::from_agent_error(err.to_agent_error());
            error!(
                conversation_id = %conv_id,
                turn_id = %turn_id,
                error_code = err.error_code(),
                error = %ErrorChain(&err),
                "Failed to persist resolved workspace"
            );
            self.service
                .persist_and_broadcast_send_failure_tip(&conv_id, &turn_id, &send_error, Some(top_level_code))
                .await;
            let was_deleting = turn_claim.release_for_turn(&turn_id);
            self.service
                .complete_released_turn(&conv_id, &turn_id, was_deleting)
                .await;
            return ConversationTurnResult {
                status: ConversationTurnStatus::Failed,
            };
        }

        info!(
            conversation_id = %conv_id,
            turn_id = %turn_id,
            agent_type = ?agent.agent_type(),
            elapsed_ms = now_ms().saturating_sub(build_started_at),
            "Agent task ready"
        );

        let first_turn_msg_id = ConversationService::mint_msg_id();
        let mut pending_send = Some((
            SendMessageData {
                content: input.request.content,
                msg_id: first_turn_msg_id.clone(),
                turn_id: Some(turn_id.clone()),
                files: input.request.files,
                inject_skills: input.request.inject_skills,
            },
            first_turn_msg_id,
        ));
        let mut continuation_count = 0usize;
        let continuation_policy = TurnContinuationPolicy::new(MAX_CRON_CONTINUATIONS_PER_TURN);

        while let Some((current_send, msg_id)) = pending_send.take() {
            let relay = StreamRelay::new(
                conv_id.clone(),
                msg_id,
                turn_id.clone(),
                input.user_id.clone(),
                self.service.conversation_repo().clone(),
                self.service.broadcaster().clone(),
                self.service.current_cron_service(),
            )
            .with_usage_repo(self.service.usage_repo())
            .with_runtime_state(Arc::clone(&runtime_state))
            .with_persistence(persistence.clone())
            .with_turn_completion(false);

            let rx = agent.subscribe();
            let send_agent = agent.clone();
            let conv_id_send = conv_id.clone();
            let turn_id_for_send = turn_id.clone();
            let (send_error_tx, send_error_rx) = oneshot::channel();

            tokio::spawn(async move {
                if let Err(e) = send_agent.send_message(current_send).await {
                    let task_status = send_agent.status();
                    let agent_type = send_agent.agent_type();
                    error!(
                        conversation_id = %conv_id_send,
                        turn_id = %turn_id_for_send,
                        ?agent_type,
                        ?task_status,
                        error = %ErrorChain(&e),
                        "Agent send_message failed"
                    );
                    if task_status == Some(ConversationStatus::Finished) {
                        debug!(
                            conversation_id = %conv_id_send,
                            turn_id = %turn_id_for_send,
                            ?agent_type,
                            "Agent send_message failure already published runtime terminal; skipping fallback stream error"
                        );
                    } else {
                        warn!(
                            conversation_id = %conv_id_send,
                            turn_id = %turn_id_for_send,
                            ?agent_type,
                            code = ?e.code(),
                            ownership = ?e.ownership(),
                            "Agent send_message returned error without runtime terminal; injecting fallback stream error"
                        );
                        let _ = send_error_tx.send(e);
                    }
                }
            });

            let outcome = relay.consume_with_send_error(rx, send_error_rx).await;
            let lifecycle = runtime_state.lifecycle_for(&conv_id);
            if outcome.terminal.is_error() {
                turn_failed = true;
            }

            if let Some(session_key) = agent.get_session_key() {
                persist_session_key(self.service.conversation_repo(), &persistence, &conv_id, &session_key).await;
            }

            match AgentHealthPolicy::decide(agent.agent_type(), &outcome, lifecycle) {
                AgentHealthAction::Keep => {}
                AgentHealthAction::EvictAcpTask { .. } => {
                    if self
                        .service
                        .evict_acp_task_after_terminal_error(&conv_id, agent.agent_type(), &outcome, &self.task_manager)
                        .await
                    {
                        break;
                    }
                }
            }

            match continuation_policy.decide(&conv_id, continuation_count, &outcome, lifecycle) {
                ContinuationDecision::Continue { content, next_count } => {
                    continuation_count = next_count;
                    let next_turn_msg_id = ConversationService::mint_msg_id();
                    pending_send = Some((
                        SendMessageData {
                            content,
                            msg_id: next_turn_msg_id.clone(),
                            turn_id: Some(turn_id.clone()),
                            files: vec![],
                            inject_skills: vec![],
                        },
                        next_turn_msg_id,
                    ));
                }
                ContinuationDecision::Stop(_) => break,
            }
        }

        let was_deleting = turn_claim.release_for_turn(&turn_id);
        self.service
            .complete_released_turn(&conv_id, &turn_id, was_deleting)
            .await;
        ConversationTurnResult {
            status: if turn_failed {
                ConversationTurnStatus::Failed
            } else {
                ConversationTurnStatus::Completed
            },
        }
    }
}
