//! Parallel dialer for concurrent gateway attempts.
//!
//! Unlike [`super::failover::FailoverLoop`] which tries gateways
//! sequentially, [`ParallelDialer`] sends INVITEs to **all** gateways
//! at once and returns the first one to answer.  Remaining branches
//! are cancelled automatically once a winner is found.

use crate::call::sip::DialogStateReceiverGuard;
use crate::redis_state::config_store::ConfigStore;
use crate::redis_state::types::TrunkConfig;
use anyhow::Result;
use rsipstack::dialog::dialog::DialogState;
use rsipstack::dialog::dialog_layer::DialogLayer;
use rsipstack::dialog::server_dialog::ServerInviteDialog;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::proxy::failover::terminated_reason_to_code;

// ------------------------------------------------------------------ //
// Public result type                                                  //
// ------------------------------------------------------------------ //

/// Outcome of a parallel dial attempt.
pub enum ParallelDialResult {
    /// One gateway answered the call.
    Connected {
        gateway_name: String,
        dialog_guard: DialogStateReceiverGuard,
        /// Answer SDP from the winning gateway.
        sdp: Option<String>,
    },
    /// Every gateway failed or was unreachable.
    AllFailed {
        last_code: u16,
        last_reason: String,
    },
    /// The caller hung up before any gateway answered.
    Cancelled,
}

// ------------------------------------------------------------------ //
// Internal branch event                                               //
// ------------------------------------------------------------------ //

/// Events sent from individual per-gateway tasks back to the collector.
#[allow(dead_code)]
enum BranchEvent {
    /// A gateway returned a 200 OK.
    Answered {
        idx: usize,
        gateway_name: String,
        dialog_guard: DialogStateReceiverGuard,
        sdp: Option<String>,
    },
    /// A gateway returned an error / timed out.
    Failed {
        idx: usize,
        code: u16,
        reason: String,
    },
}

// ------------------------------------------------------------------ //
// ParallelDialer                                                      //
// ------------------------------------------------------------------ //

/// Sends INVITEs to all trunk gateways concurrently; first answer wins.
pub struct ParallelDialer {
    dialog_layer: Arc<DialogLayer>,
    cancel_token: CancellationToken,
    config_store: Arc<ConfigStore>,
}

impl ParallelDialer {
    /// Create a new parallel dialer.
    pub fn new(
        dialog_layer: Arc<DialogLayer>,
        cancel_token: CancellationToken,
        config_store: Arc<ConfigStore>,
    ) -> Self {
        Self {
            dialog_layer,
            cancel_token,
            config_store,
        }
    }

    /// Dial all gateways in `trunk` concurrently.
    ///
    /// The first gateway to answer wins; all other branches are
    /// cancelled.  If every gateway fails the last error is returned.
    pub async fn try_parallel(
        &self,
        trunk: &TrunkConfig,
        caller_sdp: &str,
        caller_uri: &str,
        callee_uri: &str,
        _server_dialog: &ServerInviteDialog,
    ) -> Result<ParallelDialResult> {
        let gateways = &trunk.gateways;

        if gateways.is_empty() {
            return Ok(ParallelDialResult::AllFailed {
                last_code: 503,
                last_reason: "No gateways configured".to_string(),
            });
        }

        let total = gateways.len();

        // Shared channel for branch results.
        let (tx, mut rx) = mpsc::unbounded_channel::<BranchEvent>();

        // Child token so we can cancel all branches independently.
        let branch_token = self.cancel_token.child_token();

        // Spawn one task per gateway.
        for (idx, gateway_ref) in gateways.iter().enumerate() {
            let tx = tx.clone();
            let dialog_layer = self.dialog_layer.clone();
            let config_store = self.config_store.clone();
            let gateway_name = gateway_ref.name.clone();
            let caller_sdp = caller_sdp.to_string();
            let caller_uri = caller_uri.to_string();
            let callee_uri = callee_uri.to_string();
            let child_cancel = branch_token.clone();

            tokio::spawn(async move {
                let event = Self::run_branch(
                    idx,
                    &gateway_name,
                    &dialog_layer,
                    &config_store,
                    &caller_sdp,
                    &caller_uri,
                    &callee_uri,
                    child_cancel,
                )
                .await;
                // Ignore send error — collector may have already exited.
                let _ = tx.send(event);
            });
        }

        // Drop our copy so the channel closes once all tasks finish.
        drop(tx);

        // Collect results.
        let mut failures: usize = 0;
        let mut last_code: u16 = 503;
        let mut last_reason = "Service Unavailable".to_string();

        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    branch_token.cancel();
                    return Ok(ParallelDialResult::Cancelled);
                }
                event = rx.recv() => {
                    match event {
                        Some(BranchEvent::Answered {
                            idx: _,
                            gateway_name,
                            dialog_guard,
                            sdp,
                        }) => {
                            info!(
                                gateway = %gateway_name,
                                "parallel_dial: gateway answered first"
                            );
                            // Cancel remaining branches.
                            branch_token.cancel();
                            return Ok(ParallelDialResult::Connected {
                                gateway_name,
                                dialog_guard,
                                sdp,
                            });
                        }
                        Some(BranchEvent::Failed { idx: _, code, reason }) => {
                            failures += 1;
                            last_code = code;
                            last_reason = reason;
                            if failures >= total {
                                return Ok(ParallelDialResult::AllFailed {
                                    last_code,
                                    last_reason,
                                });
                            }
                        }
                        None => {
                            // All senders dropped — every branch finished.
                            return Ok(ParallelDialResult::AllFailed {
                                last_code,
                                last_reason,
                            });
                        }
                    }
                }
            }
        }
    }

    /// Drive a single gateway branch to completion.
    ///
    /// This mirrors the per-gateway logic in
    /// [`FailoverLoop::try_routes`](super::failover::FailoverLoop::try_routes)
    /// but is self-contained for spawning into a task.
    async fn run_branch(
        idx: usize,
        gateway_name: &str,
        dialog_layer: &Arc<DialogLayer>,
        config_store: &Arc<ConfigStore>,
        caller_sdp: &str,
        caller_uri: &str,
        callee_uri: &str,
        cancel: CancellationToken,
    ) -> BranchEvent {
        // Resolve gateway config.
        let gw_cfg = match config_store.get_gateway(gateway_name).await {
            Ok(Some(cfg)) => cfg,
            Ok(None) => {
                warn!(
                    gateway = %gateway_name,
                    "parallel_dial: gateway config not found"
                );
                return BranchEvent::Failed {
                    idx,
                    code: 503,
                    reason: format!(
                        "gateway '{}' not configured",
                        gateway_name
                    ),
                };
            }
            Err(e) => {
                warn!(
                    gateway = %gateway_name,
                    "parallel_dial: error loading gateway config: {e}"
                );
                return BranchEvent::Failed {
                    idx,
                    code: 503,
                    reason: e.to_string(),
                };
            }
        };

        let gateway_proxy_addr = gw_cfg.proxy_addr.clone();
        info!(
            gateway = %gateway_name,
            proxy_addr = %gateway_proxy_addr,
            "parallel_dial: resolved gateway address"
        );

        // Build SIP URIs.
        let callee_number = {
            let stripped = callee_uri
                .strip_prefix("sips:")
                .or_else(|| callee_uri.strip_prefix("sip:"))
                .unwrap_or(callee_uri);
            if let Some(at) = stripped.find('@') {
                stripped[..at].to_string()
            } else {
                stripped.to_string()
            }
        };
        let callee_uri_with_host =
            format!("sip:{}@{}", callee_number, gateway_proxy_addr);

        let callee_sip_uri: rsip::Uri =
            match callee_uri_with_host.as_str().try_into() {
                Ok(u) => u,
                Err(e) => {
                    warn!(
                        "parallel_dial: invalid callee URI {}: {}",
                        callee_uri_with_host, e
                    );
                    return BranchEvent::Failed {
                        idx,
                        code: 503,
                        reason: format!("bad callee URI: {e}"),
                    };
                }
            };

        let caller_sip_uri: rsip::Uri = match caller_uri.try_into() {
            Ok(u) => u,
            Err(e) => {
                warn!(
                    "parallel_dial: invalid caller URI {}: {}",
                    caller_uri, e
                );
                return BranchEvent::Failed {
                    idx,
                    code: 503,
                    reason: format!("bad caller URI: {e}"),
                };
            }
        };

        let proxy_addr_str = format!("sip:{}", gateway_proxy_addr);
        let destination_uri: rsip::Uri =
            match proxy_addr_str.as_str().try_into() {
                Ok(u) => u,
                Err(e) => {
                    warn!(
                        "parallel_dial: invalid gateway URI {}: {}",
                        proxy_addr_str, e
                    );
                    return BranchEvent::Failed {
                        idx,
                        code: 503,
                        reason: format!("bad gateway URI: {e}"),
                    };
                }
            };

        // Credentials.
        let credential = gw_cfg.auth.map(
            |auth| rsipstack::dialog::authenticate::Credential {
                username: auth.username,
                password: auth.password,
                realm: auth.realm,
            },
        );

        let (state_sender, state_receiver) =
            dialog_layer.new_dialog_state_channel();

        let invite_option = rsipstack::dialog::invitation::InviteOption {
            caller: caller_sip_uri.clone(),
            callee: callee_sip_uri,
            contact: caller_sip_uri,
            destination: Some(rsipstack::transport::SipAddr {
                r#type: None,
                addr: destination_uri.host_with_port.clone(),
            }),
            offer: Some(caller_sdp.as_bytes().to_vec()),
            content_type: Some("application/sdp".to_string()),
            credential,
            headers: None,
            caller_display_name: None,
            caller_params: vec![],
            support_prack: false,
            call_id: None,
        };

        let dialog_guard = DialogStateReceiverGuard::new(
            dialog_layer.clone(),
            state_receiver,
            None,
        );

        match dialog_layer.do_invite_async(invite_option, state_sender) {
            Ok((_client_dialog, _join_handle)) => {
                Self::wait_branch_outcome(
                    idx,
                    gateway_name,
                    dialog_guard,
                    cancel,
                )
                .await
            }
            Err(e) => {
                warn!(
                    gateway = %gateway_name,
                    "parallel_dial: do_invite_async failed: {e}"
                );
                BranchEvent::Failed {
                    idx,
                    code: 503,
                    reason: e.to_string(),
                }
            }
        }
    }

    /// Wait for a single branch dialog to reach a terminal state.
    async fn wait_branch_outcome(
        idx: usize,
        gateway_name: &str,
        mut guard: DialogStateReceiverGuard,
        cancel: CancellationToken,
    ) -> BranchEvent {
        let gateway_name_owned = gateway_name.to_string();
        let timeout_duration = tokio::time::Duration::from_secs(30);
        let mut early_media_sdp: Option<String> = None;

        loop {
            let state = tokio::select! {
                _ = cancel.cancelled() => {
                    return BranchEvent::Failed {
                        idx,
                        code: 487,
                        reason: "Cancelled".to_string(),
                    };
                }
                s = tokio::time::timeout(timeout_duration, guard.recv()) => {
                    match s {
                        Ok(Some(state)) => state,
                        Ok(None) => {
                            return BranchEvent::Failed {
                                idx,
                                code: 503,
                                reason: "Dialog channel closed".to_string(),
                            };
                        }
                        Err(_) => {
                            warn!(
                                gateway = %gateway_name_owned,
                                "parallel_dial: gateway timed out"
                            );
                            return BranchEvent::Failed {
                                idx,
                                code: 408,
                                reason: "Request Timeout".to_string(),
                            };
                        }
                    }
                }
            };

            match state {
                DialogState::Calling(_) | DialogState::Trying(_) => {
                    // Still in progress.
                }
                DialogState::Early(_dialog_id, ref resp) => {
                    let body = resp.body();
                    if !body.is_empty() {
                        let sdp =
                            String::from_utf8_lossy(body).to_string();
                        info!(
                            gateway = %gateway_name_owned,
                            "parallel_dial: early media SDP received"
                        );
                        early_media_sdp = Some(sdp);
                    }
                }
                DialogState::WaitAck(_, _) => {
                    // ACK pending — treat as about to confirm.
                }
                DialogState::Confirmed(_dialog_id, ref resp) => {
                    let body = resp.body();
                    let sdp = if body.is_empty() {
                        early_media_sdp.take()
                    } else {
                        Some(
                            String::from_utf8_lossy(body).to_string(),
                        )
                    };
                    info!(
                        gateway = %gateway_name_owned,
                        has_sdp = %sdp.is_some(),
                        "parallel_dial: call connected"
                    );
                    return BranchEvent::Answered {
                        idx,
                        gateway_name: gateway_name_owned,
                        dialog_guard: guard,
                        sdp,
                    };
                }
                DialogState::Terminated(_dialog_id, ref reason) => {
                    let code = terminated_reason_to_code(reason);
                    let reason_str = format!("{:?}", reason);
                    info!(
                        gateway = %gateway_name_owned,
                        code = %code,
                        "parallel_dial: gateway rejected call"
                    );
                    return BranchEvent::Failed {
                        idx,
                        code,
                        reason: reason_str,
                    };
                }
                _ => {
                    // Ignore mid-dialog events during dialing.
                }
            }
        }
    }
}

// ------------------------------------------------------------------ //
// Tests                                                               //
// ------------------------------------------------------------------ //

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parallel_dial_result_variants() {
        // Verify the enum can be constructed in each variant.
        let _connected = ParallelDialResult::AllFailed {
            last_code: 503,
            last_reason: "Service Unavailable".to_string(),
        };
        let _cancelled = ParallelDialResult::Cancelled;

        // AllFailed with different codes.
        let result = ParallelDialResult::AllFailed {
            last_code: 486,
            last_reason: "Busy Here".to_string(),
        };
        match result {
            ParallelDialResult::AllFailed {
                last_code,
                last_reason,
            } => {
                assert_eq!(last_code, 486);
                assert_eq!(last_reason, "Busy Here");
            }
            _ => panic!("expected AllFailed"),
        }
    }

    #[test]
    fn test_branch_event_variants() {
        // Verify the internal enum can be constructed.
        let failed = BranchEvent::Failed {
            idx: 0,
            code: 408,
            reason: "Request Timeout".to_string(),
        };
        match failed {
            BranchEvent::Failed { idx, code, reason } => {
                assert_eq!(idx, 0);
                assert_eq!(code, 408);
                assert_eq!(reason, "Request Timeout");
            }
            _ => panic!("expected Failed"),
        }

        let failed_503 = BranchEvent::Failed {
            idx: 2,
            code: 503,
            reason: "Service Unavailable".to_string(),
        };
        match failed_503 {
            BranchEvent::Failed { idx, code, .. } => {
                assert_eq!(idx, 2);
                assert_eq!(code, 503);
            }
            _ => panic!("expected Failed"),
        }
    }
}
