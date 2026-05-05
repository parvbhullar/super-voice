//! Process-wide circuit breaker registry.
//!
//! Circuit breaker state must be shared across calls so a provider that
//! has opened its breaker stays open until cooldown elapses, regardless of
//! which call observed the failures. Both `StreamEngine` (for ASR/TTS) and
//! the playbook LLM handler look up breakers by `(tier, provider)` here.
//!
//! Tiers used: `"asr"`, `"tts"`, `"llm"`.

use super::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

static REGISTRY: OnceLock<Mutex<HashMap<(String, String), Arc<CircuitBreaker>>>> = OnceLock::new();

fn map() -> &'static Mutex<HashMap<(String, String), Arc<CircuitBreaker>>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Get-or-create the circuit breaker for `(tier, provider)`. The breaker
/// is shared across all callers in the process; closing/opening state
/// affects every call afterwards.
pub fn get_or_create(tier: &str, provider: &str) -> Arc<CircuitBreaker> {
    let mut m = map().lock().unwrap();
    let name = format!("{tier}:{provider}");
    m.entry((tier.to_string(), provider.to_string()))
        .or_insert_with(|| {
            Arc::new(CircuitBreaker::with_name(
                CircuitBreakerConfig::default(),
                name,
            ))
        })
        .clone()
}

/// Get-or-create with explicit config. The config only applies on first
/// creation; subsequent calls return the existing breaker unchanged.
pub fn get_or_create_with(
    tier: &str,
    provider: &str,
    config: CircuitBreakerConfig,
) -> Arc<CircuitBreaker> {
    let mut m = map().lock().unwrap();
    let name = format!("{tier}:{provider}");
    m.entry((tier.to_string(), provider.to_string()))
        .or_insert_with(|| Arc::new(CircuitBreaker::with_name(config, name)))
        .clone()
}

/// Clear all registered breakers. Test-only escape hatch — production code
/// should never reset breakers (failures observed in earlier traffic are
/// load-bearing for routing decisions).
#[cfg(test)]
pub fn reset_for_tests() {
    if let Some(m) = REGISTRY.get() {
        m.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_key_returns_same_breaker() {
        reset_for_tests();
        let a = get_or_create("tts", "test-aliyun");
        let b = get_or_create("tts", "test-aliyun");
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn different_keys_are_independent() {
        reset_for_tests();
        let a = get_or_create("tts", "test-prov-x");
        let b = get_or_create("tts", "test-prov-y");
        assert!(!Arc::ptr_eq(&a, &b));

        let c = get_or_create("asr", "test-prov-x");
        assert!(!Arc::ptr_eq(&a, &c));
    }
}
