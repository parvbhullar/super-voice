//! Unit tests for the `validate_digest_auth` function.
//!
//! Known values:
//!   username  = "alice"
//!   password  = "secret"
//!   realm     = "sip.example.com"
//!   nonce     = "abc123"
//!   method    = "INVITE"
//!   uri       = "sip:bob@example.com"
//!
//! HA1      = MD5("alice:sip.example.com:secret") = a0bbf6034b8565747c15ee9850d9215a
//! HA2      = MD5("INVITE:sip:bob@example.com")   = 19c4a1cfd22400d8637f09d6e4337759
//! response = MD5(HA1:abc123:HA2)                 = 7035372a55eb762eafd3f543ef71ab73

use active_call::endpoint::validate_digest_auth;

const USERNAME: &str = "alice";
const PASSWORD: &str = "secret";
const REALM: &str = "sip.example.com";
const NONCE: &str = "abc123";
const URI: &str = "sip:bob@example.com";
const KNOWN_RESPONSE: &str = "7035372a55eb762eafd3f543ef71ab73";

/// Build a well-formed Digest Authorization header from the known values.
fn valid_auth_header() -> String {
    format!(
        r#"Digest username="{USERNAME}", realm="{REALM}", nonce="{NONCE}", uri="{URI}", response="{KNOWN_RESPONSE}", method="INVITE""#,
    )
}

// ---------------------------------------------------------------------------
// Positive case
// ---------------------------------------------------------------------------

#[test]
fn test_valid_digest_returns_true() {
    let header = valid_auth_header();
    assert!(
        validate_digest_auth(&header, USERNAME, PASSWORD, REALM, NONCE),
        "expected validate_digest_auth to return true for a correctly computed digest"
    );
}

#[test]
fn test_valid_digest_with_digest_prefix() {
    // The header may or may not have the literal "Digest " prefix.
    let header = valid_auth_header();
    assert!(
        validate_digest_auth(&header, USERNAME, PASSWORD, REALM, NONCE),
        "should work with Digest prefix"
    );
}

// ---------------------------------------------------------------------------
// Wrong password
// ---------------------------------------------------------------------------

#[test]
fn test_wrong_password_returns_false() {
    let header = valid_auth_header();
    assert!(
        !validate_digest_auth(&header, USERNAME, "wrong_password", REALM, NONCE),
        "expected validate_digest_auth to return false when password differs"
    );
}

// ---------------------------------------------------------------------------
// Malformed header
// ---------------------------------------------------------------------------

#[test]
fn test_malformed_header_missing_response_returns_false() {
    // Missing the `response` field entirely.
    let header = format!(
        r#"Digest username="{USERNAME}", realm="{REALM}", nonce="{NONCE}", uri="{URI}""#,
    );
    assert!(
        !validate_digest_auth(&header, USERNAME, PASSWORD, REALM, NONCE),
        "expected false for header missing the response field"
    );
}

#[test]
fn test_malformed_header_missing_username_returns_false() {
    let header = format!(
        r#"Digest realm="{REALM}", nonce="{NONCE}", uri="{URI}", response="{KNOWN_RESPONSE}""#,
    );
    assert!(
        !validate_digest_auth(&header, USERNAME, PASSWORD, REALM, NONCE),
        "expected false for header missing the username field"
    );
}

// ---------------------------------------------------------------------------
// Empty / blank inputs
// ---------------------------------------------------------------------------

#[test]
fn test_empty_header_returns_false() {
    assert!(
        !validate_digest_auth("", USERNAME, PASSWORD, REALM, NONCE),
        "expected false for empty Authorization header"
    );
}

#[test]
fn test_whitespace_only_header_returns_false() {
    assert!(
        !validate_digest_auth("   ", USERNAME, PASSWORD, REALM, NONCE),
        "expected false for whitespace-only header"
    );
}
