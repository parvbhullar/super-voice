use crate::redis_state::types::{TranslationClassConfig, TranslationRule};
use regex::Regex;

/// Input for translation processing.
#[derive(Debug, Clone)]
pub struct TranslationInput {
    pub caller_number: String,
    pub destination_number: String,
    pub caller_name: Option<String>,
    /// "inbound" or "outbound"
    pub direction: String,
}

/// Result of translation processing.
#[derive(Debug, Clone)]
pub struct TranslationResult {
    pub caller_number: String,
    pub destination_number: String,
    pub caller_name: Option<String>,
    /// True if any field was changed.
    pub modified: bool,
}

/// Applies regex-based number/name rewriting using a TranslationClassConfig.
pub struct TranslationEngine;

impl TranslationEngine {
    /// Apply a translation class to the given input.
    ///
    /// Rules are evaluated in order; for each field (caller, destination, name),
    /// the first matching rule's replacement is used.
    pub fn apply(config: &TranslationClassConfig, input: &TranslationInput) -> TranslationResult {
        let mut caller_number = input.caller_number.clone();
        let mut destination_number = input.destination_number.clone();
        let mut caller_name = input.caller_name.clone();
        let mut modified = false;

        let mut caller_matched = false;
        let mut destination_matched = false;
        let mut caller_name_matched = false;

        for rule in &config.rules {
            // Check direction filter
            if !Self::direction_matches(&rule.direction, &input.direction) {
                continue;
            }

            // Handle legacy match_pattern/replace fields
            let (eff_dest_pattern, eff_dest_replace) = Self::effective_dest_pattern(rule);

            // Apply caller_pattern (first match wins)
            if !caller_matched {
                if let Some(pattern) = &rule.caller_pattern {
                    if let Some(replacement) = &rule.caller_replace {
                        if let Ok(re) = Regex::new(pattern) {
                            if re.is_match(&caller_number) {
                                let new_val = re
                                    .replace(&caller_number, replacement.as_str())
                                    .into_owned();
                                if new_val != caller_number {
                                    modified = true;
                                }
                                caller_number = new_val;
                                caller_matched = true;
                            }
                        }
                    }
                }
            }

            // Apply destination_pattern (first match wins)
            if !destination_matched {
                if let Some(pattern) = eff_dest_pattern {
                    if let Some(replacement) = eff_dest_replace {
                        if let Ok(re) = Regex::new(pattern) {
                            if re.is_match(&destination_number) {
                                let new_val = re
                                    .replace(&destination_number, replacement.as_str())
                                    .into_owned();
                                if new_val != destination_number {
                                    modified = true;
                                }
                                destination_number = new_val;
                                destination_matched = true;
                            }
                        }
                    }
                }
            }

            // Apply caller_name_pattern (first match wins)
            if !caller_name_matched {
                if let (Some(pattern), Some(cn)) = (&rule.caller_name_pattern, &caller_name) {
                    if let Some(replacement) = &rule.caller_name_replace {
                        if let Ok(re) = Regex::new(pattern) {
                            if re.is_match(cn) {
                                let new_val =
                                    re.replace(cn, replacement.as_str()).into_owned();
                                if Some(&new_val) != caller_name.as_ref() {
                                    modified = true;
                                }
                                caller_name = Some(new_val);
                                caller_name_matched = true;
                            }
                        }
                    }
                }
            }
        }

        TranslationResult {
            caller_number,
            destination_number,
            caller_name,
            modified,
        }
    }

    fn direction_matches(rule_direction: &str, call_direction: &str) -> bool {
        rule_direction == "both" || rule_direction == call_direction
    }

    /// Returns effective (dest_pattern, dest_replace) considering legacy fields.
    fn effective_dest_pattern<'a>(
        rule: &'a TranslationRule,
    ) -> (Option<&'a String>, Option<&'a String>) {
        if rule.destination_pattern.is_some() {
            (
                rule.destination_pattern.as_ref(),
                rule.destination_replace.as_ref(),
            )
        } else if rule.legacy_match.is_some() {
            (rule.legacy_match.as_ref(), rule.legacy_replace.as_ref())
        } else {
            (None, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis_state::types::TranslationRule;

    fn make_rule_dest(pattern: &str, replace: &str) -> TranslationRule {
        TranslationRule {
            caller_pattern: None,
            caller_replace: None,
            destination_pattern: Some(pattern.to_string()),
            destination_replace: Some(replace.to_string()),
            caller_name_pattern: None,
            caller_name_replace: None,
            direction: "both".to_string(),
            legacy_match: None,
            legacy_replace: None,
        }
    }

    fn make_rule_caller(pattern: &str, replace: &str) -> TranslationRule {
        TranslationRule {
            caller_pattern: Some(pattern.to_string()),
            caller_replace: Some(replace.to_string()),
            destination_pattern: None,
            destination_replace: None,
            caller_name_pattern: None,
            caller_name_replace: None,
            direction: "both".to_string(),
            legacy_match: None,
            legacy_replace: None,
        }
    }

    fn make_rule_caller_name(pattern: &str, replace: &str) -> TranslationRule {
        TranslationRule {
            caller_pattern: None,
            caller_replace: None,
            destination_pattern: None,
            destination_replace: None,
            caller_name_pattern: Some(pattern.to_string()),
            caller_name_replace: Some(replace.to_string()),
            direction: "both".to_string(),
            legacy_match: None,
            legacy_replace: None,
        }
    }

    fn make_rule_dest_directional(
        pattern: &str,
        replace: &str,
        direction: &str,
    ) -> TranslationRule {
        TranslationRule {
            caller_pattern: None,
            caller_replace: None,
            destination_pattern: Some(pattern.to_string()),
            destination_replace: Some(replace.to_string()),
            caller_name_pattern: None,
            caller_name_replace: None,
            direction: direction.to_string(),
            legacy_match: None,
            legacy_replace: None,
        }
    }

    fn make_config(rules: Vec<TranslationRule>) -> TranslationClassConfig {
        TranslationClassConfig {
            name: "test".to_string(),
            rules,
        }
    }

    fn inbound_input(dest: &str) -> TranslationInput {
        TranslationInput {
            caller_number: "01234567890".to_string(),
            destination_number: dest.to_string(),
            caller_name: None,
            direction: "inbound".to_string(),
        }
    }

    #[test]
    fn test_destination_rewrite_local_to_e164() {
        let config = make_config(vec![make_rule_dest(
            r"^0(\d{10})$",
            "+44$1",
        )]);
        let input = TranslationInput {
            caller_number: "01234567890".to_string(),
            destination_number: "02071234567".to_string(),
            caller_name: None,
            direction: "inbound".to_string(),
        };
        let result = TranslationEngine::apply(&config, &input);
        assert_eq!(result.destination_number, "+442071234567");
        assert!(result.modified);
    }

    #[test]
    fn test_no_match_leaves_number_unchanged() {
        let config = make_config(vec![make_rule_dest(r"^0(\d{10})$", "+44$1")]);
        let input = inbound_input("+442071234567");
        let result = TranslationEngine::apply(&config, &input);
        assert_eq!(result.destination_number, "+442071234567");
        assert!(!result.modified);
    }

    #[test]
    fn test_multiple_rules_first_match_wins() {
        let config = make_config(vec![
            make_rule_dest(r"^0(\d{10})$", "+44$1"),
            make_rule_dest(r"^0(\d{10})$", "+1$1"),
        ]);
        let input = inbound_input("02071234567");
        let result = TranslationEngine::apply(&config, &input);
        assert_eq!(result.destination_number, "+442071234567");
    }

    #[test]
    fn test_caller_pattern_rewrites_caller_number() {
        let config = make_config(vec![make_rule_caller(r"^0(\d{10})$", "+44$1")]);
        let input = TranslationInput {
            caller_number: "07911123456".to_string(),
            destination_number: "+15551234567".to_string(),
            caller_name: None,
            direction: "inbound".to_string(),
        };
        let result = TranslationEngine::apply(&config, &input);
        assert_eq!(result.caller_number, "+447911123456");
        assert!(result.modified);
    }

    #[test]
    fn test_caller_name_pattern_rewrites_caller_name() {
        let config = make_config(vec![make_rule_caller_name(
            r"^Unknown$",
            "Anonymous",
        )]);
        let input = TranslationInput {
            caller_number: "+15551234567".to_string(),
            destination_number: "+15559876543".to_string(),
            caller_name: Some("Unknown".to_string()),
            direction: "inbound".to_string(),
        };
        let result = TranslationEngine::apply(&config, &input);
        assert_eq!(result.caller_name, Some("Anonymous".to_string()));
        assert!(result.modified);
    }

    #[test]
    fn test_direction_inbound_rule_applies_inbound() {
        let config = make_config(vec![make_rule_dest_directional(
            r"^0(\d{10})$",
            "+44$1",
            "inbound",
        )]);
        let input = inbound_input("02071234567");
        let result = TranslationEngine::apply(&config, &input);
        assert_eq!(result.destination_number, "+442071234567");
    }

    #[test]
    fn test_direction_inbound_rule_does_not_apply_outbound() {
        let config = make_config(vec![make_rule_dest_directional(
            r"^0(\d{10})$",
            "+44$1",
            "inbound",
        )]);
        let input = TranslationInput {
            caller_number: "0".to_string(),
            destination_number: "02071234567".to_string(),
            caller_name: None,
            direction: "outbound".to_string(),
        };
        let result = TranslationEngine::apply(&config, &input);
        assert_eq!(result.destination_number, "02071234567");
        assert!(!result.modified);
    }

    #[test]
    fn test_direction_outbound_rule_applies_outbound() {
        let config = make_config(vec![make_rule_dest_directional(
            r"^0(\d{10})$",
            "+44$1",
            "outbound",
        )]);
        let input = TranslationInput {
            caller_number: "0".to_string(),
            destination_number: "02071234567".to_string(),
            caller_name: None,
            direction: "outbound".to_string(),
        };
        let result = TranslationEngine::apply(&config, &input);
        assert_eq!(result.destination_number, "+442071234567");
    }

    #[test]
    fn test_direction_both_applies_either() {
        let config = make_config(vec![make_rule_dest_directional(
            r"^0(\d{10})$",
            "+44$1",
            "both",
        )]);
        let input_in = inbound_input("02071234567");
        let result_in = TranslationEngine::apply(&config, &input_in);
        assert_eq!(result_in.destination_number, "+442071234567");

        let input_out = TranslationInput {
            caller_number: "0".to_string(),
            destination_number: "02071234567".to_string(),
            caller_name: None,
            direction: "outbound".to_string(),
        };
        let result_out = TranslationEngine::apply(&config, &input_out);
        assert_eq!(result_out.destination_number, "+442071234567");
    }

    #[test]
    fn test_legacy_match_pattern_backward_compat() {
        // Legacy rules use match_pattern/replace which map to destination
        let config = TranslationClassConfig {
            name: "legacy".to_string(),
            rules: vec![TranslationRule {
                caller_pattern: None,
                caller_replace: None,
                destination_pattern: None,
                destination_replace: None,
                caller_name_pattern: None,
                caller_name_replace: None,
                direction: "both".to_string(),
                legacy_match: Some(r"^0(\d{10})$".to_string()),
                legacy_replace: Some("+44$1".to_string()),
            }],
        };
        let input = inbound_input("02071234567");
        let result = TranslationEngine::apply(&config, &input);
        assert_eq!(result.destination_number, "+442071234567");
    }
}
