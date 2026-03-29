/// A collection of SIP headers as name-value pairs.
#[derive(Debug, Clone, Default)]
pub struct SipHeaders {
    pub headers: Vec<(String, String)>,
}

impl SipHeaders {
    /// Create a new SipHeaders from a list of (name, value) pairs.
    pub fn new(headers: Vec<(String, String)>) -> Self {
        Self { headers }
    }
}

/// Strip internal topology information from SIP headers.
///
/// Rules:
/// 1. Via: keep only the first (outermost) Via; remove any Via whose host
///    matches an internal domain pattern.
/// 2. Record-Route: remove entries whose URI contains an internal domain.
/// 3. All other headers are left unchanged.
///
/// `internal_domains` is a list of substrings considered internal
/// (e.g. "10.0.0.", "192.168.", "internal.example.com").
pub fn hide_topology(headers: &mut SipHeaders, internal_domains: &[&str]) {
    let mut new_headers: Vec<(String, String)> = Vec::with_capacity(headers.headers.len());
    let mut first_via_kept = false;

    for (name, value) in &headers.headers {
        let lower_name = name.to_lowercase();

        if lower_name == "via" {
            if !first_via_kept {
                // Keep only the first Via header
                first_via_kept = true;
                new_headers.push((name.clone(), value.clone()));
            }
            // Skip all subsequent Via headers (internal hops)
        } else if lower_name == "record-route" {
            // Remove Record-Route entries that reference internal domains
            if !is_internal_value(value, internal_domains) {
                new_headers.push((name.clone(), value.clone()));
            }
        } else {
            new_headers.push((name.clone(), value.clone()));
        }
    }

    headers.headers = new_headers;
}

/// Check whether a header value contains any of the internal domain substrings.
fn is_internal_value(value: &str, internal_domains: &[&str]) -> bool {
    internal_domains.iter().any(|domain| value.contains(domain))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_headers(pairs: &[(&str, &str)]) -> SipHeaders {
        SipHeaders::new(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    // Test 5: hide_topology strips internal Via headers (keeps only outermost)
    #[test]
    fn test_strips_internal_via_keeps_outermost() {
        let mut headers = make_headers(&[
            ("Via", "SIP/2.0/UDP external.example.com;branch=z9hG4bK1"),
            ("Via", "SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK2"),
            ("Via", "SIP/2.0/UDP 192.168.1.5;branch=z9hG4bK3"),
            ("From", "<sip:alice@example.com>"),
        ]);

        hide_topology(&mut headers, &["10.0.0.", "192.168."]);

        let vias: Vec<_> = headers
            .headers
            .iter()
            .filter(|(k, _)| k.to_lowercase() == "via")
            .collect();

        assert_eq!(vias.len(), 1, "only outermost Via should remain");
        assert!(
            vias[0].1.contains("external.example.com"),
            "outermost Via should be preserved"
        );
    }

    // Test 6: hide_topology strips internal Record-Route headers
    #[test]
    fn test_strips_internal_record_route() {
        let mut headers = make_headers(&[
            ("Record-Route", "<sip:external.example.com;lr>"),
            ("Record-Route", "<sip:10.0.0.1;lr>"),
            ("Record-Route", "<sip:192.168.1.5;lr>"),
            ("Via", "SIP/2.0/UDP external.example.com;branch=z9hG4bK1"),
        ]);

        hide_topology(&mut headers, &["10.0.0.", "192.168."]);

        let rr: Vec<_> = headers
            .headers
            .iter()
            .filter(|(k, _)| k.to_lowercase() == "record-route")
            .collect();

        assert_eq!(rr.len(), 1, "internal Record-Route entries should be removed");
        assert!(
            rr[0].1.contains("external.example.com"),
            "external Record-Route should be preserved"
        );
    }

    // Test 7: hide_topology preserves non-topology headers unchanged
    #[test]
    fn test_non_topology_headers_preserved() {
        let mut headers = make_headers(&[
            ("Via", "SIP/2.0/UDP external.example.com;branch=z9hG4bK1"),
            ("From", "<sip:alice@example.com>"),
            ("To", "<sip:bob@example.com>"),
            ("Call-ID", "abc123@example.com"),
            ("CSeq", "1 INVITE"),
        ]);

        hide_topology(&mut headers, &["10.0.0.", "192.168."]);

        let from: Vec<_> = headers
            .headers
            .iter()
            .filter(|(k, _)| k.to_lowercase() == "from")
            .collect();
        assert_eq!(from.len(), 1);
        assert!(from[0].1.contains("alice@example.com"));

        let to: Vec<_> = headers
            .headers
            .iter()
            .filter(|(k, _)| k.to_lowercase() == "to")
            .collect();
        assert_eq!(to.len(), 1);

        let call_id: Vec<_> = headers
            .headers
            .iter()
            .filter(|(k, _)| k.to_lowercase() == "call-id")
            .collect();
        assert_eq!(call_id.len(), 1);
    }
}
