//! SIP endpoint abstractions — `SipEndpoint` trait, `EndpointManager`, and
//! shared digest-auth helper.

use anyhow::Result;
use async_trait::async_trait;
use md5::{Digest, Md5};

pub mod manager;
pub mod rsip_endpoint;

#[cfg(feature = "carrier")]
pub mod pjsip_endpoint;

pub use manager::EndpointManager;
pub use rsip_endpoint::RsipEndpoint;

#[cfg(feature = "carrier")]
pub use pjsip_endpoint::PjsipEndpoint;

/// Common interface implemented by every SIP endpoint.
#[async_trait]
pub trait SipEndpoint: Send + Sync {
    /// Unique name that identifies this endpoint.
    fn name(&self) -> &str;

    /// Underlying SIP stack: `"pjsip"`, `"sofia"`, or `"rsipstack"`.
    fn stack(&self) -> &str;

    /// Local address the endpoint listens on, e.g. `"0.0.0.0:5060"`.
    fn listen_addr(&self) -> String;

    /// Start the endpoint (bind socket, spawn event loop, etc.).
    async fn start(&mut self) -> Result<()>;

    /// Stop the endpoint gracefully.
    async fn stop(&mut self) -> Result<()>;

    /// Returns `true` while the endpoint is actively running.
    fn is_running(&self) -> bool;

    /// Downcast support — returns `self` as `&dyn Any`.
    ///
    /// Used by [`EndpointManager::get_pjsip_bridge`] to extract the
    /// `Arc<PjBridge>` from a `PjsipEndpoint` via type erasure.
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Validate a SIP Digest Authorization header against expected credentials.
///
/// Implements RFC 2617 §3.2.2:
/// - HA1 = MD5(username:realm:password)
/// - HA2 = MD5(method:uri)
/// - response = MD5(HA1:nonce:HA2)
///
/// Returns `true` only when the `response` field in `auth_header` matches the
/// value computed from the supplied credentials.
pub fn validate_digest_auth(
    auth_header: &str,
    expected_username: &str,
    expected_password: &str,
    realm: &str,
    nonce: &str,
) -> bool {
    // Strip optional "Digest " prefix.
    let header = auth_header
        .strip_prefix("Digest ")
        .unwrap_or(auth_header)
        .trim();

    if header.is_empty() {
        return false;
    }

    // Parse key=value pairs, handling quoted and unquoted values.
    let parsed = parse_digest_params(header);

    let username = match parsed.get("username") {
        Some(v) => v.as_str(),
        None => return false,
    };
    let header_realm = match parsed.get("realm") {
        Some(v) => v.as_str(),
        None => return false,
    };
    let header_nonce = match parsed.get("nonce") {
        Some(v) => v.as_str(),
        None => return false,
    };
    let uri = match parsed.get("uri") {
        Some(v) => v.as_str(),
        None => return false,
    };
    let response = match parsed.get("response") {
        Some(v) => v.as_str(),
        None => return false,
    };
    let method = parsed
        .get("method")
        .map(|s| s.as_str())
        .unwrap_or("INVITE");

    // Credential fields must match what we expect.
    if username != expected_username {
        return false;
    }
    if header_realm != realm {
        return false;
    }
    if header_nonce != nonce {
        return false;
    }

    let ha1 = md5_hex(&format!("{}:{}:{}", expected_username, realm, expected_password));
    let ha2 = md5_hex(&format!("{}:{}", method, uri));
    let expected_response = md5_hex(&format!("{}:{}:{}", ha1, nonce, ha2));

    response == expected_response
}

// --- helpers ----------------------------------------------------------------

fn md5_hex(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// Parse `key=value` pairs from a Digest header parameter list.
///
/// Values may be quoted with `"…"` or unquoted.  Returns a [`HashMap`] of
/// lower-cased keys to their stripped (unquoted) string values.
fn parse_digest_params(input: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for part in input.split(',') {
        let part = part.trim();
        if let Some((key, val)) = part.split_once('=') {
            let key = key.trim().to_lowercase();
            let val = val.trim().trim_matches('"').to_string();
            map.insert(key, val);
        }
    }
    map
}
