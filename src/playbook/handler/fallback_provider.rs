//! Provider-fallback wrapper for `LlmProvider`.
//!
//! Each request is tried against providers in order. On a retryable failure
//! (connection error, HTTP 5xx, timeout), the wrapper rebuilds a per-entry
//! `LlmConfig` and tries the next provider. Terminal errors (4xx) are
//! surfaced immediately.
//!
//! Streaming restart: if streaming fails before the first chunk is yielded,
//! the wrapper transparently retries against the next provider. Once a
//! chunk has been delivered to the caller, mid-stream failover is not
//! attempted (the caller would have to roll back partial output, which is
//! application-level concern).

use super::provider::{LlmProvider, LlmStreamEvent};
use super::super::{ChatMessage, LlmConfig, LlmProviderEntry};
use crate::resilience::{
    CircuitBreaker, FallbackBudget, classify_error, FailureKind,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};

/// One entry in an LLM fallback chain. Holds the provider implementation,
/// its circuit breaker, and the per-entry config (URL, key, model, timeout).
pub struct LlmEntry {
    pub provider_name: String,
    pub provider: Arc<dyn LlmProvider>,
    pub breaker: Arc<CircuitBreaker>,
    /// Per-provider config. The wrapper merges shared fields (history,
    /// prompt, language, etc.) from the call-site `LlmConfig` and overlays
    /// these per-provider fields (`base_url`, `api_key`, `model`, timeout).
    pub config: LlmProviderEntry,
}

pub struct FallbackLlmProvider {
    entries: Vec<LlmEntry>,
    budget: FallbackBudget,
}

impl FallbackLlmProvider {
    pub fn new(entries: Vec<LlmEntry>) -> Result<Self> {
        if entries.is_empty() {
            return Err(anyhow!("FallbackLlmProvider requires at least one entry"));
        }
        Ok(Self {
            entries,
            budget: FallbackBudget::default(),
        })
    }

    pub fn with_budget(mut self, budget: FallbackBudget) -> Self {
        self.budget = budget;
        self
    }

    /// Build a per-provider `LlmConfig` by overlaying entry fields onto the
    /// shared per-call config. Entry fields win when set.
    fn merge_config(base: &LlmConfig, entry: &LlmProviderEntry) -> LlmConfig {
        LlmConfig {
            provider: entry.provider.clone(),
            model: entry.model.clone().or_else(|| base.model.clone()),
            base_url: entry.base_url.clone().or_else(|| base.base_url.clone()),
            api_key: entry.api_key.clone().or_else(|| base.api_key.clone()),
            providers: None,
            prompt: base.prompt.clone(),
            greeting: base.greeting.clone(),
            language: base.language.clone(),
            features: base.features.clone(),
            repair_window_ms: base.repair_window_ms,
            summary_limit: base.summary_limit,
            tool_instructions: base.tool_instructions.clone(),
        }
    }
}

#[async_trait]
impl LlmProvider for FallbackLlmProvider {
    async fn call(&self, config: &LlmConfig, history: &[ChatMessage]) -> Result<String> {
        let start = Instant::now();
        let mut last_err: Option<anyhow::Error> = None;
        let total = self.entries.len();

        for (idx, entry) in self.entries.iter().enumerate() {
            if start.elapsed() >= self.budget.total {
                return Err(anyhow!(
                    "LLM fallback budget of {:?} exhausted",
                    self.budget.total
                ));
            }

            let permit = match entry.breaker.try_acquire() {
                Some(p) => p,
                None => {
                    debug!(provider = %entry.provider_name, "circuit open, skipping");
                    if let Some(next) = self.entries.get(idx + 1) {
                        info!(
                            metric = "provider_fallback_total",
                            tier = "llm",
                            from_provider = %entry.provider_name,
                            to_provider = %next.provider_name,
                            reason = "circuit_open",
                            "provider fallback"
                        );
                    }
                    continue;
                }
            };

            let merged = Self::merge_config(config, &entry.config);

            match entry.provider.call(&merged, history).await {
                Ok(text) => {
                    entry.breaker.record_success(permit);
                    info!(
                        metric = "llm_tier_used",
                        tier = idx,
                        provider = %entry.provider_name,
                        "llm completion served"
                    );
                    return Ok(text);
                }
                Err(e) => match classify_error(&e) {
                    FailureKind::Terminal => {
                        entry.breaker.record_success(permit);
                        return Err(e);
                    }
                    FailureKind::Retryable => {
                        let to = self
                            .entries
                            .get(idx + 1)
                            .map(|n| n.provider_name.as_str())
                            .unwrap_or("(none)");
                        warn!(
                            metric = "provider_fallback_total",
                            tier = "llm",
                            from_provider = %entry.provider_name,
                            to_provider = %to,
                            reason = classify_reason(&e),
                            error = %e,
                            "llm provider failed, trying next"
                        );
                        entry.breaker.record_failure(permit);
                        last_err = Some(e);
                    }
                },
            }
        }

        let _ = total;
        Err(last_err.unwrap_or_else(|| {
            anyhow!("all {} LLM providers failed or are open", self.entries.len())
        }))
    }

    async fn call_stream(
        &self,
        config: &LlmConfig,
        history: &[ChatMessage],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmStreamEvent>> + Send>>> {
        // Stream-pre-first-chunk fallback: each provider's `call_stream` is
        // attempted in order until one returns a stream successfully. Once
        // the stream is returned to the caller, no further failover happens
        // (mid-stream failover would require buffering+rolling-back which
        // is application-layer concern; see design doc decision 4.5).
        let start = Instant::now();
        let mut last_err: Option<anyhow::Error> = None;

        for (idx, entry) in self.entries.iter().enumerate() {
            if start.elapsed() >= self.budget.total {
                return Err(anyhow!(
                    "LLM fallback budget of {:?} exhausted",
                    self.budget.total
                ));
            }

            let permit = match entry.breaker.try_acquire() {
                Some(p) => p,
                None => {
                    if let Some(next) = self.entries.get(idx + 1) {
                        info!(
                            metric = "provider_fallback_total",
                            tier = "llm",
                            from_provider = %entry.provider_name,
                            to_provider = %next.provider_name,
                            reason = "circuit_open",
                            "provider fallback (stream)"
                        );
                    }
                    continue;
                }
            };

            let merged = Self::merge_config(config, &entry.config);

            match entry.provider.call_stream(&merged, history).await {
                Ok(stream) => {
                    entry.breaker.record_success(permit);
                    info!(
                        metric = "llm_tier_used",
                        tier = idx,
                        provider = %entry.provider_name,
                        stream = true,
                        "llm completion served"
                    );
                    return Ok(stream);
                }
                Err(e) => match classify_error(&e) {
                    FailureKind::Terminal => {
                        entry.breaker.record_success(permit);
                        return Err(e);
                    }
                    FailureKind::Retryable => {
                        let to = self
                            .entries
                            .get(idx + 1)
                            .map(|n| n.provider_name.as_str())
                            .unwrap_or("(none)");
                        warn!(
                            metric = "provider_fallback_total",
                            tier = "llm",
                            from_provider = %entry.provider_name,
                            to_provider = %to,
                            reason = classify_reason(&e),
                            error = %e,
                            "llm stream provider failed, trying next"
                        );
                        entry.breaker.record_failure(permit);
                        last_err = Some(e);
                    }
                },
            }
        }

        Err(last_err.unwrap_or_else(|| {
            anyhow!("all {} LLM providers failed or are open", self.entries.len())
        }))
    }
}

/// Coarse reason tag for `provider_fallback_total{reason}`. Buckets
/// the failure into a stable label for Prom — full error text goes
/// alongside in the `error` field for human triage.
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

    struct MockLlm {
        name: String,
        results: Mutex<Vec<Result<String>>>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl MockLlm {
        fn new(name: &str, results: Vec<Result<String>>) -> Arc<Self> {
            Arc::new(Self {
                name: name.to_string(),
                results: Mutex::new(results),
                calls: std::sync::atomic::AtomicUsize::new(0),
            })
        }
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl LlmProvider for MockLlm {
        async fn call(&self, _config: &LlmConfig, _history: &[ChatMessage]) -> Result<String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut r = self.results.lock().unwrap();
            if r.is_empty() {
                return Err(anyhow!("mock {} exhausted", self.name));
            }
            r.remove(0)
        }

        async fn call_stream(
            &self,
            _config: &LlmConfig,
            _history: &[ChatMessage],
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmStreamEvent>> + Send>>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut r = self.results.lock().unwrap();
            if r.is_empty() {
                return Err(anyhow!("mock {} exhausted", self.name));
            }
            match r.remove(0) {
                Ok(text) => Ok(Box::pin(stream::iter(vec![Ok(LlmStreamEvent::Content(
                    text,
                ))]))),
                Err(e) => Err(e),
            }
        }
    }

    fn entry(name: &str, provider: Arc<dyn LlmProvider>) -> LlmEntry {
        LlmEntry {
            provider_name: name.to_string(),
            provider,
            breaker: Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
                failure_threshold: 2,
                ..CircuitBreakerConfig::default()
            })),
            config: LlmProviderEntry {
                provider: name.to_string(),
                ..LlmProviderEntry::default()
            },
        }
    }

    #[tokio::test]
    async fn primary_success_short_circuits_chain() {
        let p = MockLlm::new("p", vec![Ok("hello".into())]);
        let s = MockLlm::new("s", vec![]);
        let f = FallbackLlmProvider::new(vec![
            entry("p", p.clone()),
            entry("s", s.clone()),
        ])
        .unwrap();
        let out = f.call(&LlmConfig::default(), &[]).await.unwrap();
        assert_eq!(out, "hello");
        assert_eq!(p.calls(), 1);
        assert_eq!(s.calls(), 0);
    }

    #[tokio::test]
    async fn fails_over_on_retryable() {
        let p = MockLlm::new("p", vec![Err(anyhow!("HTTP 503 unavailable"))]);
        let s = MockLlm::new("s", vec![Ok("ok".into())]);
        let f = FallbackLlmProvider::new(vec![
            entry("p", p.clone()),
            entry("s", s.clone()),
        ])
        .unwrap();
        let out = f.call(&LlmConfig::default(), &[]).await.unwrap();
        assert_eq!(out, "ok");
        assert_eq!(p.calls(), 1);
        assert_eq!(s.calls(), 1);
    }

    #[tokio::test]
    async fn terminal_error_stops_chain() {
        let p = MockLlm::new("p", vec![Err(anyhow!("HTTP 401 unauthorized"))]);
        let s = MockLlm::new("s", vec![Ok("ok".into())]);
        let f = FallbackLlmProvider::new(vec![
            entry("p", p.clone()),
            entry("s", s.clone()),
        ])
        .unwrap();
        let res = f.call(&LlmConfig::default(), &[]).await;
        assert!(res.is_err());
        assert_eq!(p.calls(), 1);
        assert_eq!(s.calls(), 0);
    }

    #[tokio::test]
    async fn streaming_fails_over_pre_first_chunk() {
        let p = MockLlm::new("p", vec![Err(anyhow!("connection reset"))]);
        let s = MockLlm::new("s", vec![Ok("hi".into())]);
        let f = FallbackLlmProvider::new(vec![
            entry("p", p.clone()),
            entry("s", s.clone()),
        ])
        .unwrap();
        let _stream = f
            .call_stream(&LlmConfig::default(), &[])
            .await
            .expect("should yield stream");
        assert_eq!(p.calls(), 1);
        assert_eq!(s.calls(), 1);
    }

    #[tokio::test]
    async fn empty_provider_list_rejected() {
        let res = FallbackLlmProvider::new(vec![]);
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn merge_config_overlays_entry_fields() {
        let base = LlmConfig {
            base_url: Some("https://shared.example/v1".into()),
            model: Some("shared-model".into()),
            api_key: Some("base-key".into()),
            language: Some("en".into()),
            ..LlmConfig::default()
        };
        let entry = LlmProviderEntry {
            provider: "openai".into(),
            base_url: Some("https://override.example/v1".into()),
            api_key: Some("override-key".into()),
            model: None, // base wins
            ..LlmProviderEntry::default()
        };
        let merged = FallbackLlmProvider::merge_config(&base, &entry);
        assert_eq!(merged.base_url.as_deref(), Some("https://override.example/v1"));
        assert_eq!(merged.api_key.as_deref(), Some("override-key"));
        assert_eq!(merged.model.as_deref(), Some("shared-model"));
        assert_eq!(merged.language.as_deref(), Some("en"));
        // providers is intentionally None on the merged config to prevent
        // recursive fallback if the underlying provider re-checks it.
        assert!(merged.providers.is_none());
    }
}
