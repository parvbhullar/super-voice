//! Provider-fallback wrapper for `SynthesisClient`.
//!
//! TTS clients are stateful: a successful `start()` returns an open stream
//! from one provider, and subsequent `synthesize()` / `stop()` calls go
//! against that same provider. This wrapper performs **start-time fallback**:
//! `start()` walks the chain, returning the first stream that opens
//! successfully. After start, the active provider is fixed for the lifetime
//! of the session — mid-stream failover would require buffering and
//! re-synthesis logic that belongs at the application layer (where it can
//! decide whether the partial audio already played is acceptable).
//!
//! This covers the most common failure mode in production: a cloud TTS
//! provider returning 5xx during session bring-up, or its websocket failing
//! to upgrade. Once audio is actively flowing, the underlying provider
//! tends to be reliable for the duration of a single TTS request.

use super::{SynthesisClient, SynthesisEvent, SynthesisOption, SynthesisType};
use crate::resilience::{CircuitBreaker, classify_error, FailureKind};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::stream::BoxStream;
use std::sync::Arc;
use tracing::{debug, warn};

/// Factory that builds a fresh, unstarted `SynthesisClient` for one
/// provider. The wrapper calls the factory each time it tries a new
/// provider, so a half-failed previous attempt cannot leak state.
pub type TtsClientFactory =
    Arc<dyn Fn() -> Result<Box<dyn SynthesisClient>> + Send + Sync>;

pub struct TtsProviderEntry {
    pub name: String,
    pub provider_type: SynthesisType,
    pub factory: TtsClientFactory,
    pub breaker: Arc<CircuitBreaker>,
}

pub struct FallbackSynthesisClient {
    entries: Vec<TtsProviderEntry>,
    /// Provider chosen at `start()` time. None until then.
    active: Option<Box<dyn SynthesisClient>>,
    active_idx: Option<usize>,
}

impl FallbackSynthesisClient {
    pub fn new(entries: Vec<TtsProviderEntry>) -> Result<Self> {
        if entries.is_empty() {
            return Err(anyhow!(
                "FallbackSynthesisClient requires at least one provider"
            ));
        }
        Ok(Self {
            entries,
            active: None,
            active_idx: None,
        })
    }

    pub fn provider_names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }

    pub fn active_provider_name(&self) -> Option<&str> {
        self.active_idx.map(|i| self.entries[i].name.as_str())
    }
}

#[async_trait]
impl SynthesisClient for FallbackSynthesisClient {
    fn provider(&self) -> SynthesisType {
        // Best-effort: report the active provider if started, else the
        // first entry's type (which is what's about to be tried).
        match self.active_idx {
            Some(idx) => self.entries[idx].provider_type.clone(),
            None => self.entries[0].provider_type.clone(),
        }
    }

    async fn start(
        &mut self,
    ) -> Result<BoxStream<'static, (Option<usize>, Result<SynthesisEvent>)>> {
        let mut last_err: Option<anyhow::Error> = None;

        for (idx, entry) in self.entries.iter().enumerate() {
            let next_name = self
                .entries
                .get(idx + 1)
                .map(|n| n.name.as_str())
                .unwrap_or("(none)");

            let permit = match entry.breaker.try_acquire() {
                Some(p) => p,
                None => {
                    debug!(provider = %entry.name, "tts circuit open, skipping");
                    warn!(
                        metric = "provider_fallback_total",
                        tier = "tts",
                        from_provider = %entry.name,
                        to_provider = %next_name,
                        reason = "circuit_open",
                        "tts provider skipped (circuit open)"
                    );
                    continue;
                }
            };

            let mut client = match (entry.factory)() {
                Ok(c) => c,
                Err(e) => {
                    // Construction itself failed (config / build error).
                    // Treat like a provider failure — this is exactly the
                    // case where fallback is most valuable.
                    match classify_error(&e) {
                        FailureKind::Terminal => {
                            entry.breaker.record_success(permit);
                            return Err(e);
                        }
                        FailureKind::Retryable => {
                            entry.breaker.record_failure(permit);
                            warn!(
                                metric = "provider_fallback_total",
                                tier = "tts",
                                from_provider = %entry.name,
                                to_provider = %next_name,
                                reason = classify_reason(&e),
                                error = %e,
                                "tts client construction failed, trying next"
                            );
                            last_err = Some(e);
                            continue;
                        }
                    }
                }
            };

            match client.start().await {
                Ok(stream) => {
                    entry.breaker.record_success(permit);
                    self.active = Some(client);
                    self.active_idx = Some(idx);
                    debug!(provider = %entry.name, "tts active");
                    return Ok(stream);
                }
                Err(e) => match classify_error(&e) {
                    FailureKind::Terminal => {
                        entry.breaker.record_success(permit);
                        return Err(e);
                    }
                    FailureKind::Retryable => {
                        entry.breaker.record_failure(permit);
                        warn!(
                            metric = "provider_fallback_total",
                            tier = "tts",
                            from_provider = %entry.name,
                            to_provider = %next_name,
                            reason = classify_reason(&e),
                            error = %e,
                            "tts start failed, trying next"
                        );
                        last_err = Some(e);
                    }
                },
            }
        }

        Err(last_err.unwrap_or_else(|| {
            anyhow!("all {} TTS providers failed or are open", self.entries.len())
        }))
    }

    async fn synthesize(
        &mut self,
        text: &str,
        cmd_seq: Option<usize>,
        option: Option<SynthesisOption>,
    ) -> Result<()> {
        match &mut self.active {
            Some(client) => client.synthesize(text, cmd_seq, option).await,
            None => Err(anyhow!("FallbackSynthesisClient not started")),
        }
    }

    async fn stop(&mut self) -> Result<()> {
        match &mut self.active {
            Some(client) => client.stop().await,
            None => Ok(()),
        }
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
    use futures::stream;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockTts {
        name: String,
        start_results: Mutex<Vec<Result<()>>>,
        synth_results: Mutex<Vec<Result<()>>>,
        starts: AtomicUsize,
        synths: AtomicUsize,
        stops: AtomicUsize,
    }

    impl MockTts {
        fn new(name: &str, start: Vec<Result<()>>, synth: Vec<Result<()>>) -> Self {
            Self {
                name: name.to_string(),
                start_results: Mutex::new(start),
                synth_results: Mutex::new(synth),
                starts: AtomicUsize::new(0),
                synths: AtomicUsize::new(0),
                stops: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl SynthesisClient for MockTts {
        fn provider(&self) -> SynthesisType {
            SynthesisType::Other(self.name.clone())
        }

        async fn start(
            &mut self,
        ) -> Result<BoxStream<'static, (Option<usize>, Result<SynthesisEvent>)>> {
            self.starts.fetch_add(1, Ordering::Relaxed);
            let mut r = self.start_results.lock().unwrap();
            if r.is_empty() {
                return Err(anyhow!("mock {} start exhausted", self.name));
            }
            match r.remove(0) {
                Ok(()) => {
                    let s = stream::iter(vec![]);
                    Ok(Box::pin(s))
                }
                Err(e) => Err(e),
            }
        }

        async fn synthesize(
            &mut self,
            _text: &str,
            _cmd_seq: Option<usize>,
            _option: Option<SynthesisOption>,
        ) -> Result<()> {
            self.synths.fetch_add(1, Ordering::Relaxed);
            let mut r = self.synth_results.lock().unwrap();
            if r.is_empty() {
                return Ok(());
            }
            r.remove(0)
        }

        async fn stop(&mut self) -> Result<()> {
            self.stops.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    /// Test scenario shared with the factory closure: each call to the
    /// factory pops one queued (start_result, synth_results) tuple. Lets
    /// us pre-script multiple constructions deterministically without
    /// requiring `Clone` on `anyhow::Error`.
    #[derive(Default)]
    struct Scenario {
        constructs: AtomicUsize,
        // Each entry: a Result for start() and a vec of Results for synth().
        queue: Mutex<Vec<(Result<()>, Vec<Result<()>>)>>,
    }

    fn entry_with_scenario(
        name: &str,
        scenario: Arc<Scenario>,
    ) -> TtsProviderEntry {
        let name_owned = name.to_string();
        let factory: TtsClientFactory = Arc::new(move || {
            scenario.constructs.fetch_add(1, Ordering::Relaxed);
            let mut q = scenario.queue.lock().unwrap();
            if q.is_empty() {
                return Err(anyhow!("scenario {} exhausted", name_owned));
            }
            let (start, synth) = q.remove(0);
            Ok(Box::new(MockTts::new(&name_owned, vec![start], synth))
                as Box<dyn SynthesisClient>)
        });
        TtsProviderEntry {
            name: name.to_string(),
            provider_type: SynthesisType::Other(name.to_string()),
            factory,
            breaker: Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
                failure_threshold: 2,
                ..CircuitBreakerConfig::default()
            })),
        }
    }

    fn scenario(starts: Vec<Result<()>>) -> Arc<Scenario> {
        let queue: Vec<(Result<()>, Vec<Result<()>>)> =
            starts.into_iter().map(|r| (r, vec![])).collect();
        Arc::new(Scenario {
            constructs: AtomicUsize::new(0),
            queue: Mutex::new(queue),
        })
    }

    #[tokio::test]
    async fn primary_start_succeeds_short_circuits_chain() {
        let p = scenario(vec![Ok(())]);
        let s = scenario(vec![Ok(())]);
        let mut client = FallbackSynthesisClient::new(vec![
            entry_with_scenario("primary", p.clone()),
            entry_with_scenario("secondary", s.clone()),
        ])
        .unwrap();
        client.start().await.expect("primary should succeed");
        assert_eq!(client.active_provider_name(), Some("primary"));
        assert_eq!(p.constructs.load(Ordering::Relaxed), 1);
        assert_eq!(s.constructs.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn fails_over_when_primary_start_returns_5xx() {
        let p = scenario(vec![Err(anyhow!("HTTP 503 Service Unavailable"))]);
        let s = scenario(vec![Ok(())]);
        let mut client = FallbackSynthesisClient::new(vec![
            entry_with_scenario("primary", p.clone()),
            entry_with_scenario("secondary", s.clone()),
        ])
        .unwrap();
        client.start().await.expect("secondary should succeed");
        assert_eq!(client.active_provider_name(), Some("secondary"));
        assert_eq!(p.constructs.load(Ordering::Relaxed), 1);
        assert_eq!(s.constructs.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn terminal_error_stops_chain_at_start() {
        let p = scenario(vec![Err(anyhow!("HTTP 401 unauthorized"))]);
        let s = scenario(vec![Ok(())]);
        let mut client = FallbackSynthesisClient::new(vec![
            entry_with_scenario("primary", p.clone()),
            entry_with_scenario("secondary", s.clone()),
        ])
        .unwrap();
        let res = client.start().await;
        assert!(res.is_err());
        assert_eq!(p.constructs.load(Ordering::Relaxed), 1);
        assert_eq!(s.constructs.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn synthesize_before_start_errors() {
        let p = scenario(vec![Ok(())]);
        let mut client = FallbackSynthesisClient::new(vec![entry_with_scenario(
            "primary",
            p,
        )])
        .unwrap();
        let res = client.synthesize("hi", None, None).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn empty_chain_rejected() {
        assert!(FallbackSynthesisClient::new(vec![]).is_err());
    }

    #[tokio::test]
    async fn all_providers_open_returns_error() {
        let p = scenario(vec![]);
        let s = scenario(vec![]);
        let primary = entry_with_scenario("primary", p.clone());
        let secondary = entry_with_scenario("secondary", s.clone());

        // Pre-open both circuits without consuming the scenario queue.
        for _ in 0..2 {
            primary
                .breaker
                .record_failure(primary.breaker.try_acquire().unwrap());
            secondary
                .breaker
                .record_failure(secondary.breaker.try_acquire().unwrap());
        }

        let mut client = FallbackSynthesisClient::new(vec![primary, secondary]).unwrap();
        let res = client.start().await;
        assert!(res.is_err());
        // Neither factory was invoked because both circuits were open.
        assert_eq!(p.constructs.load(Ordering::Relaxed), 0);
        assert_eq!(s.constructs.load(Ordering::Relaxed), 0);
    }
}
