/// Information extracted from a SIP message for validation.
#[derive(Debug, Clone)]
pub struct SipMessageInfo {
    pub max_forwards: Option<u32>,
    pub content_length: Option<usize>,
    pub body_length: usize,
    pub method: String,
}

/// Result of SIP message validation.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationResult {
    Valid,
    InvalidMaxForwards { value: Option<u32> },
    ContentLengthMismatch { header: usize, actual: usize },
    MissingMaxForwards,
}

/// Validate a SIP message for correctness.
///
/// Checks:
/// 1. Max-Forwards header present
/// 2. Max-Forwards != 0
/// 3. Content-Length matches actual body length (if header present)
pub fn validate_sip_message(msg: &SipMessageInfo) -> ValidationResult {
    match msg.max_forwards {
        None => return ValidationResult::MissingMaxForwards,
        Some(0) => {
            return ValidationResult::InvalidMaxForwards { value: Some(0) }
        }
        Some(_) => {}
    }

    if let Some(declared_len) = msg.content_length {
        if declared_len != msg.body_length {
            return ValidationResult::ContentLengthMismatch {
                header: declared_len,
                actual: msg.body_length,
            };
        }
    }

    ValidationResult::Valid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(max_fwd: Option<u32>, content_len: Option<usize>, body_len: usize) -> SipMessageInfo {
        SipMessageInfo {
            max_forwards: max_fwd,
            content_length: content_len,
            body_length: body_len,
            method: "REGISTER".to_string(),
        }
    }

    // Test 1: reject Max-Forwards: 0
    #[test]
    fn test_max_forwards_zero_rejected() {
        let result = validate_sip_message(&msg(Some(0), None, 0));
        assert_eq!(
            result,
            ValidationResult::InvalidMaxForwards { value: Some(0) }
        );
    }

    // Test 2: reject Content-Length mismatch (header 100, body 50)
    #[test]
    fn test_content_length_mismatch_rejected() {
        let result = validate_sip_message(&msg(Some(70), Some(100), 50));
        assert_eq!(
            result,
            ValidationResult::ContentLengthMismatch {
                header: 100,
                actual: 50
            }
        );
    }

    // Test 3: valid message with correct Max-Forwards and Content-Length
    #[test]
    fn test_valid_message_accepted() {
        let result = validate_sip_message(&msg(Some(70), Some(50), 50));
        assert_eq!(result, ValidationResult::Valid);
    }

    // Test 4: message with no Max-Forwards header rejected
    #[test]
    fn test_missing_max_forwards_rejected() {
        let result = validate_sip_message(&msg(None, None, 0));
        assert_eq!(result, ValidationResult::MissingMaxForwards);
    }

    #[test]
    fn test_valid_message_without_content_length() {
        // No Content-Length header — skip that check
        let result = validate_sip_message(&msg(Some(70), None, 0));
        assert_eq!(result, ValidationResult::Valid);
    }
}
