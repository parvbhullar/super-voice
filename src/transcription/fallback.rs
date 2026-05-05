//! Provider-fallback wrapper for `TranscriptionClient`.
//!
//! Each request to `send_audio` is forwarded to the active provider. On a
//! retryable failure, the wrapper advances to the next provider in the chain.
//! Each provider has an independent `CircuitBreaker`; providers in the
//! `Open` state are skipped without an attempt.
//!
//! Note: ASR is stateful in the underlying client (the providers buffer
//! audio internally and emit transcripts via the event channel). This
//! wrapper performs *send-time* failover, not mid-utterance failover —
//! mid-utterance switching would require reconstructing the audio buffer
//! against a different provider, which is out of scope for this change.

use super::{TranscriptionClient, TranscriptionType};
use crate::media::{Sample, SourcePacket};
use crate::resilience::{CircuitBreaker, FailureKind, classify_error};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tracing::warn;

/// One provider entry in the fallback chain. Holds the underlying client
/// plus its circuit breaker. The `name` field is used for log/metric tags.
pub struct AsrProviderEntry {
    pub name: String,
    pub provider_type: TranscriptionType,
    pub client: Arc<dyn TranscriptionClient>,
    pub breaker: Arc<CircuitBreaker>,
}

/// Wraps an ordered list of `TranscriptionClient` providers and routes
/// `send_audio` to the first healthy one. On retryable failure, advances
/// to the next; terminal failures (4xx) are surfaced immediately.
pub struct FallbackTranscriptionClient {
    providers: Vec<AsrProviderEntry>,
    /// Index of the currently-preferred provider. Walking past this on a
    /// failure advances it sticky-style so we don't re-probe the failed
    /// primary on every audio frame.
    active_idx: AtomicUsize,
}

impl FallbackTranscriptionClient {
    pub fn new(providers: Vec<AsrProviderEntry>) -> Result<Self> {
        if providers.is_empty() {
            return Err(anyhow!(
                "FallbackTranscriptionClient requires at least one provider"
            ));
        }
        Ok(Self {
            providers,
            active_idx: AtomicUsize::new(0),
        })
    }

    pub fn provider_names(&self) -> Vec<&str> {
        self.providers.iter().map(|p| p.name.as_str()).collect()
    }
}

#[async_trait]
impl TranscriptionClient for FallbackTranscriptionClient {
    fn send_audio(&self, samples: &[Sample], src_packet: Option<&SourcePacket>) -> Result<()> {
        let start_idx = self.active_idx.load(Ordering::Relaxed);
        let n = self.providers.len();

        // Walk providers starting from the sticky active index.
        for offset in 0..n {
            let idx = (start_idx + offset) % n;
            let entry = &self.providers[idx];
            let next_name = self
                .providers
                .get((idx + 1) % n)
                .map(|p| p.name.as_str())
                .unwrap_or("(none)");

            let permit = match entry.breaker.try_acquire() {
                Some(p) => p,
                None => {
                    warn!(
                        metric = "provider_fallback_total",
                        tier = "asr",
                        from_provider = %entry.name,
                        to_provider = %next_name,
                        reason = "circuit_open",
                        "asr provider skipped (circuit open)"
                    );
                    continue;
                }
            };

            match entry.client.send_audio(samples, src_packet) {
                Ok(()) => {
                    entry.breaker.record_success(permit);
                    self.active_idx.store(idx, Ordering::Relaxed);
                    return Ok(());
                }
                Err(e) => match classify_error(&e) {
                    FailureKind::Terminal => {
                        // Don't blame the breaker for user errors.
                        entry.breaker.record_success(permit);
                        return Err(e);
                    }
                    FailureKind::Retryable => {
                        warn!(
                            metric = "provider_fallback_total",
                            tier = "asr",
                            from_provider = %entry.name,
                            to_provider = %next_name,
                            reason = classify_reason(&e),
                            error = %e,
                            "asr provider failed, trying next"
                        );
                        entry.breaker.record_failure(permit);
                        // Continue to next provider.
                    }
                },
            }
        }

        Err(anyhow!(
            "all {} ASR providers failed or are open",
            self.providers.len()
        ))
    }
}

/// Bucket the failure into a stable label for `provider_fallback_total{reason}`.
fn classify_reason(err: &anyhow::Error) -> &'static str {
    let s = err.to_string().to_ascii_lowercase();
    if s.contains("timeout") || s.contains("timed out") {
        "timeout"
    } else if s.contains("connect") || s.contains("connection") || s.contains("refused") {
        "connection"
    } else if s.contains("5") && s.contains("status") {
        "5xx"
    } else if s.contains("4") && s.contains("status") {
        "4xx"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resilience::CircuitBreakerConfig;
    use std::sync::Mutex;

    /// Test fixture: a controllable ASR provider that records calls and
    /// returns a configured sequence of results.
    struct MockAsr {
        name: String,
        results: Mutex<Vec<Result<()>>>,
        calls: AtomicUsize,
    }

    impl MockAsr {
        fn new(name: &str, results: Vec<Result<()>>) -> Arc<Self> {
            Arc::new(Self {
                name: name.to_string(),
                results: Mutex::new(results),
                calls: AtomicUsize::new(0),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl TranscriptionClient for MockAsr {
        fn send_audio(&self, _samples: &[Sample], _src_packet: Option<&SourcePacket>) -> Result<()> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let mut results = self.results.lock().unwrap();
            if results.is_empty() {
                return Err(anyhow!("mock {} exhausted", self.name));
            }
            results.remove(0)
        }
    }

    fn entry(name: &str, client: Arc<dyn TranscriptionClient>) -> AsrProviderEntry {
        AsrProviderEntry {
            name: name.to_string(),
            provider_type: TranscriptionType::Other(name.to_string()),
            client,
            breaker: Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
                failure_threshold: 2,
                ..CircuitBreakerConfig::default()
            })),
        }
    }

    #[test]
    fn primary_success_does_not_touch_secondary() {
        let p = MockAsr::new("primary", vec![Ok(())]);
        let s = MockAsr::new("secondary", vec![]);
        let client = FallbackTranscriptionClient::new(vec![
            entry("primary", p.clone()),
            entry("secondary", s.clone()),
        ])
        .unwrap();
        client.send_audio(&[0i16; 16], None).unwrap();
        assert_eq!(p.calls(), 1);
        assert_eq!(s.calls(), 0);
    }

    #[test]
    fn fails_over_on_retryable_error() {
        let p = MockAsr::new("primary", vec![Err(anyhow!("connection reset"))]);
        let s = MockAsr::new("secondary", vec![Ok(())]);
        let client = FallbackTranscriptionClient::new(vec![
            entry("primary", p.clone()),
            entry("secondary", s.clone()),
        ])
        .unwrap();
        client.send_audio(&[0i16; 16], None).unwrap();
        assert_eq!(p.calls(), 1);
        assert_eq!(s.calls(), 1);
    }

    #[test]
    fn does_not_fail_over_on_terminal_error() {
        let p = MockAsr::new("primary", vec![Err(anyhow!("HTTP 401 Unauthorized"))]);
        let s = MockAsr::new("secondary", vec![Ok(())]);
        let client = FallbackTranscriptionClient::new(vec![
            entry("primary", p.clone()),
            entry("secondary", s.clone()),
        ])
        .unwrap();
        let res = client.send_audio(&[0i16; 16], None);
        assert!(res.is_err());
        assert_eq!(p.calls(), 1);
        assert_eq!(s.calls(), 0);
    }

    #[test]
    fn sticky_active_index_avoids_reprobing_failed_primary() {
        let p = MockAsr::new(
            "primary",
            vec![Err(anyhow!("connection refused")), Ok(())],
        );
        let s = MockAsr::new("secondary", vec![Ok(()), Ok(())]);
        let client = FallbackTranscriptionClient::new(vec![
            entry("primary", p.clone()),
            entry("secondary", s.clone()),
        ])
        .unwrap();

        // First call fails over from p → s.
        client.send_audio(&[0i16; 16], None).unwrap();
        assert_eq!(p.calls(), 1);
        assert_eq!(s.calls(), 1);

        // Second call should go straight to s (sticky), not re-probe p.
        client.send_audio(&[0i16; 16], None).unwrap();
        assert_eq!(p.calls(), 1, "primary should not be re-probed");
        assert_eq!(s.calls(), 2);
    }

    #[test]
    fn errors_when_all_providers_exhausted() {
        let p = MockAsr::new("primary", vec![Err(anyhow!("connection error"))]);
        let s = MockAsr::new("secondary", vec![Err(anyhow!("connection error"))]);
        let client = FallbackTranscriptionClient::new(vec![
            entry("primary", p.clone()),
            entry("secondary", s.clone()),
        ])
        .unwrap();
        let res = client.send_audio(&[0i16; 16], None);
        assert!(res.is_err());
        assert!(format!("{}", res.unwrap_err()).contains("all"));
    }

    #[test]
    fn empty_provider_list_rejected() {
        let res = FallbackTranscriptionClient::new(vec![]);
        assert!(res.is_err());
    }

    #[test]
    fn open_circuit_skipped_without_attempt() {
        // Use a never-ready primary (would fail if called), and Open its
        // breaker directly so we can assert the wrapper doesn't probe it.
        let p = MockAsr::new("primary", vec![]);
        let s = MockAsr::new("secondary", vec![Ok(())]);
        let primary_entry = entry("primary", p.clone());
        // Force primary's circuit Open by failing it past the threshold.
        for _ in 0..2 {
            let permit = primary_entry.breaker.try_acquire().unwrap();
            primary_entry.breaker.record_failure(permit);
        }

        let client = FallbackTranscriptionClient::new(vec![
            primary_entry,
            entry("secondary", s.clone()),
        ])
        .unwrap();

        // start_idx = 0 → primary should be skipped (Open) without attempt,
        // wrapper proceeds to secondary.
        client.send_audio(&[0i16; 16], None).unwrap();
        assert_eq!(p.calls(), 0, "primary not called when its breaker is open");
        assert_eq!(s.calls(), 1);
    }
}
