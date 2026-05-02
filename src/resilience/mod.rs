pub mod circuit_breaker;
pub mod fallback;
pub mod registry;

pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState, Permit};
pub use fallback::{FailureKind, FallbackBudget, classify_error};
