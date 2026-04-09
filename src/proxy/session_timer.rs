//! Session Timer implementation (RFC 4028)
//!
//! SIP Session Timers detect and recover from hung sessions by requiring
//! periodic refresh requests. This module provides the state machine and
//! header parsing utilities needed to track session timer negotiation and
//! refresh cycles.

use std::str::FromStr;
use std::time::{Duration, Instant};

// ------------------------------------------------------------------ //
// Constants                                                           //
// ------------------------------------------------------------------ //

/// Header name for Session-Expires (RFC 4028).
pub const HEADER_SESSION_EXPIRES: &str = "Session-Expires";

/// Header name for Min-SE (RFC 4028).
pub const HEADER_MIN_SE: &str = "Min-SE";

/// Header name for Supported.
pub const HEADER_SUPPORTED: &str = "Supported";

/// Header name for Require.
pub const HEADER_REQUIRE: &str = "Require";

/// Feature tag indicating timer support.
pub const TIMER_TAG: &str = "timer";

/// Default session expiration interval in seconds (30 min per RFC 4028).
pub const DEFAULT_SESSION_EXPIRES: u64 = 1800;

/// Minimum acceptable session expiration interval in seconds (RFC 4028).
pub const MIN_MIN_SE: u64 = 90;

// ------------------------------------------------------------------ //
// SessionRefresher                                                    //
// ------------------------------------------------------------------ //

/// Which party is responsible for refreshing the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRefresher {
    /// User Agent Client (caller) refreshes.
    Uac,
    /// User Agent Server (callee) refreshes.
    Uas,
}

impl std::fmt::Display for SessionRefresher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionRefresher::Uac => write!(f, "uac"),
            SessionRefresher::Uas => write!(f, "uas"),
        }
    }
}

impl FromStr for SessionRefresher {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "uac" => Ok(SessionRefresher::Uac),
            "uas" => Ok(SessionRefresher::Uas),
            _ => Err(()),
        }
    }
}

// ------------------------------------------------------------------ //
// SessionTimerState                                                   //
// ------------------------------------------------------------------ //

/// State machine for RFC 4028 session timers.
#[derive(Debug, Clone)]
pub struct SessionTimerState {
    /// Timer has been negotiated via Session-Expires header.
    pub enabled: bool,
    /// Negotiated session expiration interval.
    pub session_interval: Duration,
    /// Minimum session expiration from Min-SE header.
    pub min_se: Duration,
    /// Which party is responsible for sending refreshes.
    pub refresher: SessionRefresher,
    /// Timer is actively running (session established).
    pub active: bool,
    /// A refresh is currently in flight.
    pub refreshing: bool,
    /// Timestamp of the last successful refresh (or session start).
    pub last_refresh: Instant,
    /// Total number of successful refreshes.
    pub refresh_count: u32,
    /// Total number of failed refresh attempts.
    pub failed_refreshes: u32,
}

impl Default for SessionTimerState {
    fn default() -> Self {
        Self {
            enabled: false,
            session_interval: Duration::from_secs(DEFAULT_SESSION_EXPIRES),
            min_se: Duration::from_secs(MIN_MIN_SE),
            refresher: SessionRefresher::Uac,
            active: false,
            refreshing: false,
            last_refresh: Instant::now(),
            refresh_count: 0,
            failed_refreshes: 0,
        }
    }
}

impl SessionTimerState {
    /// Returns `true` when a refresh should be sent.
    ///
    /// Per RFC 4028 the refresher should send a re-INVITE at half the
    /// negotiated session interval.
    pub fn should_refresh(&self) -> bool {
        if !self.active || !self.enabled || self.refreshing {
            return false;
        }
        self.last_refresh.elapsed() >= self.session_interval / 2
    }

    /// Returns `true` when the session has expired (no refresh received
    /// within the full interval).
    pub fn is_expired(&self) -> bool {
        if !self.active || !self.enabled {
            return false;
        }
        self.last_refresh.elapsed() >= self.session_interval
    }

    /// Instant at which the next refresh should be sent, if active.
    pub fn next_refresh_time(&self) -> Option<Instant> {
        if !self.active || !self.enabled {
            return None;
        }
        Some(self.last_refresh + self.session_interval / 2)
    }

    /// Instant at which the session will expire, if active.
    pub fn expiration_time(&self) -> Option<Instant> {
        if !self.active || !self.enabled {
            return None;
        }
        Some(self.last_refresh + self.session_interval)
    }

    /// Remaining duration until the session expires, or `None` if inactive.
    pub fn time_until_expiration(&self) -> Option<Duration> {
        self.expiration_time().map(|exp| {
            let now = Instant::now();
            if exp > now {
                exp - now
            } else {
                Duration::ZERO
            }
        })
    }

    /// Remaining duration until a refresh is needed, or `None` if inactive.
    pub fn time_until_refresh(&self) -> Option<Duration> {
        self.next_refresh_time().map(|next| {
            let now = Instant::now();
            if next > now {
                next - now
            } else {
                Duration::ZERO
            }
        })
    }

    /// Begin a refresh cycle. Returns `false` if already refreshing.
    pub fn start_refresh(&mut self) -> bool {
        if self.refreshing {
            return false;
        }
        self.refreshing = true;
        true
    }

    /// Mark the current refresh as successfully completed.
    pub fn complete_refresh(&mut self) {
        self.last_refresh = Instant::now();
        self.refreshing = false;
        self.refresh_count += 1;
    }

    /// Mark the current refresh as failed.
    pub fn fail_refresh(&mut self) {
        self.refreshing = false;
        self.failed_refreshes += 1;
    }

    /// Update the last-refresh timestamp when a remote refresh arrives.
    pub fn update_refresh(&mut self) {
        self.last_refresh = Instant::now();
        self.refresh_count += 1;
    }

    /// Build the value for a `Session-Expires` header.
    pub fn get_session_expires_value(&self) -> String {
        format!(
            "{};refresher={}",
            self.session_interval.as_secs(),
            self.refresher
        )
    }

    /// Build the value for a `Min-SE` header.
    pub fn get_min_se_value(&self) -> String {
        self.min_se.as_secs().to_string()
    }
}

// ------------------------------------------------------------------ //
// Header helpers (rsip)                                               //
// ------------------------------------------------------------------ //

/// Extract the raw `Session-Expires` header value from an `rsip` header
/// slice.
///
/// This searches for a custom header whose name matches
/// `Session-Expires` (case-insensitive) and returns its value.
pub fn extract_session_expires(headers: &[rsip::Header]) -> Option<String> {
    headers.iter().find_map(|h| {
        if let rsip::Header::Other(name, value) = h {
            if name.eq_ignore_ascii_case(HEADER_SESSION_EXPIRES) {
                return Some(value.to_string());
            }
        }
        None
    })
}

/// Parse a `Session-Expires` header value string.
///
/// The value has the form `<seconds>[;refresher=uac|uas]`.
/// Returns `(interval, optional_refresher)` on success.
pub fn parse_session_expires(
    value: &str,
) -> Option<(Duration, Option<SessionRefresher>)> {
    let parts: Vec<&str> = value.split(';').collect();
    if parts.is_empty() {
        return None;
    }

    let seconds = parts[0].trim().parse::<u64>().ok()?;
    let mut refresher = None;

    for part in parts.iter().skip(1) {
        let part = part.trim();
        if part.starts_with("refresher=") {
            let val = part.trim_start_matches("refresher=");
            refresher = SessionRefresher::from_str(val).ok();
        }
    }

    Some((Duration::from_secs(seconds), refresher))
}

// ------------------------------------------------------------------ //
// Tests                                                               //
// ------------------------------------------------------------------ //

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_should_refresh_at_half_interval() {
        let mut state = SessionTimerState {
            enabled: true,
            active: true,
            session_interval: Duration::from_millis(200),
            last_refresh: Instant::now(),
            ..Default::default()
        };

        // Immediately after creation — not yet time.
        assert!(!state.should_refresh());

        // Advance past half-interval (100 ms).
        state.last_refresh = Instant::now() - Duration::from_millis(110);
        assert!(state.should_refresh());
    }

    #[test]
    fn test_is_expired_at_full_interval() {
        let mut state = SessionTimerState {
            enabled: true,
            active: true,
            session_interval: Duration::from_millis(200),
            last_refresh: Instant::now(),
            ..Default::default()
        };

        assert!(!state.is_expired());

        // Advance past the full interval.
        state.last_refresh = Instant::now() - Duration::from_millis(210);
        assert!(state.is_expired());
    }

    #[test]
    fn test_not_expired_when_disabled() {
        let mut state = SessionTimerState::default();
        // Disabled + inactive — should never report expired.
        state.last_refresh = Instant::now() - Duration::from_secs(3600);
        assert!(!state.is_expired());

        // Enabled but not active — still not expired.
        state.enabled = true;
        assert!(!state.is_expired());
    }

    #[test]
    fn test_parse_session_expires_with_refresher() {
        let (dur, refresher) =
            parse_session_expires("1800;refresher=uac").unwrap();
        assert_eq!(dur, Duration::from_secs(1800));
        assert_eq!(refresher, Some(SessionRefresher::Uac));

        let (dur, refresher) =
            parse_session_expires("900 ; refresher=uas").unwrap();
        assert_eq!(dur, Duration::from_secs(900));
        assert_eq!(refresher, Some(SessionRefresher::Uas));
    }

    #[test]
    fn test_parse_session_expires_without_refresher() {
        let (dur, refresher) = parse_session_expires("1800").unwrap();
        assert_eq!(dur, Duration::from_secs(1800));
        assert_eq!(refresher, None);
    }

    #[test]
    fn test_start_complete_refresh_cycle() {
        let mut state = SessionTimerState {
            enabled: true,
            active: true,
            ..Default::default()
        };

        // First start succeeds.
        assert!(state.start_refresh());
        assert!(state.refreshing);

        // Second start fails (already refreshing).
        assert!(!state.start_refresh());

        // Complete resets refreshing and bumps counter.
        state.complete_refresh();
        assert!(!state.refreshing);
        assert_eq!(state.refresh_count, 1);

        // Can start again after completion.
        assert!(state.start_refresh());
    }

    #[test]
    fn test_fail_refresh_increments_counter() {
        let mut state = SessionTimerState {
            enabled: true,
            active: true,
            ..Default::default()
        };

        assert!(state.start_refresh());
        state.fail_refresh();
        assert!(!state.refreshing);
        assert_eq!(state.failed_refreshes, 1);

        assert!(state.start_refresh());
        state.fail_refresh();
        assert_eq!(state.failed_refreshes, 2);
    }

    #[test]
    fn test_default_values() {
        let state = SessionTimerState::default();
        assert!(!state.enabled);
        assert!(!state.active);
        assert!(!state.refreshing);
        assert_eq!(
            state.session_interval,
            Duration::from_secs(DEFAULT_SESSION_EXPIRES)
        );
        assert_eq!(state.min_se, Duration::from_secs(MIN_MIN_SE));
        assert_eq!(state.refresher, SessionRefresher::Uac);
        assert_eq!(state.refresh_count, 0);
        assert_eq!(state.failed_refreshes, 0);

        // Header generation with defaults.
        assert_eq!(
            state.get_session_expires_value(),
            "1800;refresher=uac"
        );
        assert_eq!(state.get_min_se_value(), "90");
    }
}
