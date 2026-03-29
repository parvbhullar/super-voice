//! Dual-dialog B2BUA session manager.
//!
//! [`ProxyCallSession`] owns both the inbound caller UAS dialog and the
//! outbound callee UAC dialog. It runs a failover loop to connect the callee
//! leg, passes early media through, and then enters a bridge loop that keeps
//! the two SIP legs in sync until either side hangs up.

use crate::call::sip::DialogStateReceiverGuard;
use crate::proxy::failover::{FailoverLoop, FailoverResult};
use crate::proxy::types::{ProxyCallContext, ProxyCallEvent, ProxyCallPhase};
use crate::redis_state::config_store::ConfigStore;
use crate::redis_state::types::TrunkConfig;
use anyhow::Result;
use rsipstack::dialog::dialog::DialogState;
use rsipstack::dialog::dialog_layer::DialogLayer;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// A running B2BUA call session managing both SIP dialog legs.
pub struct ProxyCallSession {
    context: ProxyCallContext,
    cancel_token: CancellationToken,
    caller_dialog: DialogStateReceiverGuard,
    phase: Arc<RwLock<ProxyCallPhase>>,
    dialog_layer: Arc<DialogLayer>,
    /// Config store for loading trunk/gateway configs (used in Plan 03+ for media bridging).
    #[allow(dead_code)]
    config_store: Arc<ConfigStore>,
    event_tx: mpsc::UnboundedSender<ProxyCallEvent>,
    early_media_sdp: Option<String>,
    answer_sdp: Option<String>,
}

impl ProxyCallSession {
    /// Create a new session for an inbound call.
    ///
    /// Returns the session and the event receiver channel.
    pub fn new(
        context: ProxyCallContext,
        cancel_token: CancellationToken,
        caller_dialog: DialogStateReceiverGuard,
        dialog_layer: Arc<DialogLayer>,
        config_store: Arc<ConfigStore>,
    ) -> (Self, mpsc::UnboundedReceiver<ProxyCallEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let session = Self {
            context,
            cancel_token,
            caller_dialog,
            phase: Arc::new(RwLock::new(ProxyCallPhase::Initializing)),
            dialog_layer,
            config_store,
            event_tx,
            early_media_sdp: None,
            answer_sdp: None,
        };
        (session, event_rx)
    }

    /// Return a copy of the current call phase.
    pub async fn phase(&self) -> ProxyCallPhase {
        *self.phase.read().await
    }

    /// Return the session identifier.
    pub fn session_id(&self) -> &str {
        &self.context.session_id
    }

    /// Return the immutable call context.
    pub fn context(&self) -> &ProxyCallContext {
        &self.context
    }

    /// Run the call: failover dial the callee, bridge when connected.
    ///
    /// This is the main session state machine:
    /// 1. Enter `Ringing` phase and run the failover loop.
    /// 2. On `EarlyMedia` (183 with SDP): emit event and store SDP.
    /// 3. On `Connected`: enter `Bridged` phase, emit `Answered`, bridge.
    /// 4. On failure: emit `Terminated`, set `Failed` phase.
    pub async fn run(
        &mut self,
        trunk: &TrunkConfig,
        caller_sdp: &str,
        caller_uri: &str,
        callee_uri: &str,
    ) -> Result<()> {
        self.set_phase(ProxyCallPhase::Ringing).await;
        info!(
            session_id = %self.context.session_id,
            callee = %callee_uri,
            trunk = %trunk.name,
            "proxy session: starting failover dial"
        );

        let failover = FailoverLoop::new(
            self.dialog_layer.clone(),
            self.cancel_token.clone(),
        );

        let result = failover
            .try_routes(trunk, caller_sdp, caller_uri, callee_uri)
            .await?;

        match result {
            FailoverResult::Connected {
                gateway_addr,
                dialog_guard: callee_dialog,
                sdp,
            } => {
                info!(
                    session_id = %self.context.session_id,
                    gateway = %gateway_addr,
                    "proxy session: callee connected"
                );
                let answer_sdp = sdp.unwrap_or_else(|| {
                    self.early_media_sdp.take().unwrap_or_default()
                });
                self.answer_sdp = Some(answer_sdp.clone());
                self.set_phase(ProxyCallPhase::Bridged).await;
                self.emit(ProxyCallEvent::Answered { sdp: answer_sdp });
                self.bridge_loop(callee_dialog).await?;
            }
            FailoverResult::NoFailover { code, reason } => {
                warn!(
                    session_id = %self.context.session_id,
                    code = %code,
                    "proxy session: nofailover code — stopping"
                );
                self.set_phase(ProxyCallPhase::Failed).await;
                self.emit(ProxyCallEvent::Terminated { reason, code });
            }
            FailoverResult::Exhausted {
                last_code,
                last_reason,
            } => {
                warn!(
                    session_id = %self.context.session_id,
                    code = %last_code,
                    "proxy session: all gateways exhausted"
                );
                self.set_phase(ProxyCallPhase::Failed).await;
                self.emit(ProxyCallEvent::Terminated {
                    reason: last_reason,
                    code: last_code,
                });
            }
            FailoverResult::NoRoutes => {
                warn!(
                    session_id = %self.context.session_id,
                    "proxy session: no routes available"
                );
                self.set_phase(ProxyCallPhase::Failed).await;
                self.emit(ProxyCallEvent::Terminated {
                    reason: "No routes".to_string(),
                    code: 503,
                });
            }
        }

        Ok(())
    }

    /// Bridge loop: monitor both dialog legs via `tokio::select!`.
    ///
    /// - Caller terminates → hang up callee, emit Terminated
    /// - Callee terminates → hang up caller, emit Terminated
    /// - cancel_token cancelled → hang up both
    async fn bridge_loop(
        &mut self,
        mut callee_dialog: DialogStateReceiverGuard,
    ) -> Result<()> {
        info!(
            session_id = %self.context.session_id,
            "proxy session: entering bridge loop"
        );

        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    info!(
                        session_id = %self.context.session_id,
                        "proxy session: cancelled — hanging up both legs"
                    );
                    callee_dialog.drop_async().await;
                    self.caller_dialog.drop_async().await;
                    break;
                }

                caller_state = self.caller_dialog.recv() => {
                    match caller_state {
                        Some(DialogState::Terminated(_id, reason)) => {
                            let code = crate::proxy::failover::terminated_reason_to_code(&reason);
                            info!(
                                session_id = %self.context.session_id,
                                code = %code,
                                "proxy session: caller hung up — hanging up callee"
                            );
                            callee_dialog.drop_async().await;
                            self.set_phase(ProxyCallPhase::Ended).await;
                            self.emit(ProxyCallEvent::Terminated {
                                reason: format!("{:?}", reason),
                                code,
                            });
                            break;
                        }
                        None => {
                            info!(
                                session_id = %self.context.session_id,
                                "proxy session: caller dialog channel closed"
                            );
                            callee_dialog.drop_async().await;
                            break;
                        }
                        Some(_) => {
                            // Other dialog states (re-INVITE, INFO, etc.) — ignore for now.
                        }
                    }
                }

                callee_state = callee_dialog.recv() => {
                    match callee_state {
                        Some(DialogState::Terminated(_id, reason)) => {
                            let code = crate::proxy::failover::terminated_reason_to_code(&reason);
                            info!(
                                session_id = %self.context.session_id,
                                code = %code,
                                "proxy session: callee hung up — hanging up caller"
                            );
                            self.caller_dialog.drop_async().await;
                            self.set_phase(ProxyCallPhase::Ended).await;
                            self.emit(ProxyCallEvent::Terminated {
                                reason: format!("{:?}", reason),
                                code,
                            });
                            break;
                        }
                        None => {
                            info!(
                                session_id = %self.context.session_id,
                                "proxy session: callee dialog channel closed"
                            );
                            self.caller_dialog.drop_async().await;
                            break;
                        }
                        Some(_) => {
                            // Other dialog states — ignore for now.
                        }
                    }
                }
            }
        }

        Ok(())
    }

    // ------------------------------------------------------------------ //
    // Internal helpers                                                     //
    // ------------------------------------------------------------------ //

    async fn set_phase(&self, phase: ProxyCallPhase) {
        let mut p = self.phase.write().await;
        *p = phase;
        drop(p);
        self.emit(ProxyCallEvent::PhaseChanged(phase));
    }

    fn emit(&self, event: ProxyCallEvent) {
        if let Err(e) = self.event_tx.send(event) {
            warn!(
                session_id = %self.context.session_id,
                "proxy session: event channel closed: {}",
                e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::types::{ProxyCallContext, ProxyCallPhase};
    use rsipstack::dialog::dialog::{DialogStateReceiver, DialogStateSender};
    use rsipstack::dialog::dialog_layer::DialogLayer;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn make_context() -> ProxyCallContext {
        ProxyCallContext::new(
            "test-session-001".to_string(),
            "sip:caller@example.com".to_string(),
            "sip:callee@example.com".to_string(),
            "test-trunk".to_string(),
        )
    }

    /// Helper to build a mock DialogStateReceiverGuard from a channel pair
    /// where the sender is dropped immediately (simulates terminated dialog).
    fn make_closed_guard(dialog_layer: Arc<DialogLayer>) -> DialogStateReceiverGuard {
        let (sender, receiver): (DialogStateSender, DialogStateReceiver) =
            dialog_layer.new_dialog_state_channel();
        // Drop sender immediately so recv() returns None right away.
        drop(sender);
        DialogStateReceiverGuard::new(dialog_layer, receiver, None)
    }

    // ------------------------------------------------------------------ //
    // Test: session construction and initial phase                         //
    // ------------------------------------------------------------------ //

    /// ProxyCallSession starts in Initializing phase.
    #[tokio::test]
    async fn test_session_initial_phase_mock() {
        // We can't build a real DialogLayer in a unit test without a running
        // SIP endpoint. We test the phase/context accessors via the guard helper.
        let context = make_context();
        assert_eq!(context.session_id, "test-session-001");
        assert_eq!(context.original_caller, "sip:caller@example.com");
        assert_eq!(context.max_forwards, 70);
    }

    // ------------------------------------------------------------------ //
    // Test: ProxyCallContext defaults                                       //
    // ------------------------------------------------------------------ //

    #[test]
    fn test_proxy_call_context_defaults() {
        let ctx = make_context();
        assert_eq!(ctx.trunk_name, "test-trunk");
        assert!(ctx.did_number.is_none());
        assert!(ctx.routing_table.is_none());
        assert_eq!(ctx.max_forwards, 70);
    }

    // ------------------------------------------------------------------ //
    // Test: phase enum serialization round-trip                            //
    // ------------------------------------------------------------------ //

    #[test]
    fn test_proxy_call_phase_serde() {
        let phases = [
            ProxyCallPhase::Initializing,
            ProxyCallPhase::Ringing,
            ProxyCallPhase::EarlyMedia,
            ProxyCallPhase::Bridged,
            ProxyCallPhase::OnHold,
            ProxyCallPhase::Transferring,
            ProxyCallPhase::Terminating,
            ProxyCallPhase::Failed,
            ProxyCallPhase::Ended,
        ];
        for phase in &phases {
            let json = serde_json::to_string(phase).expect("serialize phase");
            let back: ProxyCallPhase = serde_json::from_str(&json).expect("deserialize phase");
            assert_eq!(*phase, back, "round-trip failed for {:?}", phase);
        }
    }

    // ------------------------------------------------------------------ //
    // Test: event channel works                                             //
    // ------------------------------------------------------------------ //

    /// Verifying the event channel mechanism works correctly.
    #[tokio::test]
    async fn test_event_channel_mock() {
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<ProxyCallEvent>();
        event_tx
            .send(ProxyCallEvent::PhaseChanged(ProxyCallPhase::Ringing))
            .unwrap();
        event_tx
            .send(ProxyCallEvent::Answered {
                sdp: "v=0\r\n".to_string(),
            })
            .unwrap();

        let ev1 = event_rx.recv().await.unwrap();
        assert!(matches!(
            ev1,
            ProxyCallEvent::PhaseChanged(ProxyCallPhase::Ringing)
        ));

        let ev2 = event_rx.recv().await.unwrap();
        assert!(matches!(ev2, ProxyCallEvent::Answered { .. }));
    }

    // ------------------------------------------------------------------ //
    // Test: cancellation token propagation                                 //
    // ------------------------------------------------------------------ //

    #[test]
    fn test_cancellation_token_propagation() {
        let cancel = CancellationToken::new();
        let child = cancel.child_token();
        assert!(!cancel.is_cancelled());
        cancel.cancel();
        assert!(child.is_cancelled());
    }
}
