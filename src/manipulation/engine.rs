use std::collections::HashMap;

use regex::Regex;

use crate::redis_state::types::{ManipulationClassConfig, ManipulationRule};

/// Call context for manipulation evaluation.
#[derive(Debug, Clone)]
pub struct ManipulationContext {
    /// SIP headers (key -> value).
    pub headers: HashMap<String, String>,
    /// Variables set by previous rules or call setup.
    pub variables: HashMap<String, String>,
}

/// Result of manipulation evaluation.
#[derive(Debug, Clone, Default)]
pub struct ManipulationResult {
    /// Headers to add or set on the SIP message.
    pub set_headers: HashMap<String, String>,
    /// Headers to remove from the SIP message.
    pub remove_headers: Vec<String>,
    /// Variables to set.
    pub set_variables: HashMap<String, String>,
    /// Log messages generated.
    pub log_messages: Vec<String>,
    /// If true, the call should be hung up.
    pub hangup: bool,
    /// Sleep durations in milliseconds.
    pub sleep_ms: Vec<u64>,
}

/// Evaluates ManipulationClassConfig rules against a call context.
pub struct ManipulationEngine;

impl ManipulationEngine {
    /// Evaluate a manipulation class against the given context.
    ///
    /// Each rule's conditions are evaluated; matching rules execute actions,
    /// non-matching rules execute anti-actions.
    pub fn evaluate(
        config: &ManipulationClassConfig,
        context: &ManipulationContext,
    ) -> ManipulationResult {
        let mut result = ManipulationResult::default();

        for rule in &config.rules {
            let conditions_match = Self::evaluate_rule_conditions(rule, context);
            if conditions_match {
                for action in &rule.actions {
                    Self::apply_action(&action.action_type, &action.name, &action.value, &mut result);
                }
                // Handle legacy unconditional action
                if rule.conditions.is_empty() {
                    if let (Some(header), Some(_action_type)) = (&rule.header, &rule.action) {
                        Self::apply_action("set_header", &Some(header.clone()), &rule.value, &mut result);
                    }
                }
            } else {
                for action in &rule.anti_actions {
                    Self::apply_action(&action.action_type, &action.name, &action.value, &mut result);
                }
            }
        }

        result
    }

    /// Returns true if the rule's conditions are satisfied.
    ///
    /// For legacy rules (no conditions), always returns true so the
    /// legacy header/action/value apply unconditionally.
    fn evaluate_rule_conditions(
        rule: &ManipulationRule,
        context: &ManipulationContext,
    ) -> bool {
        if rule.conditions.is_empty() {
            return true;
        }

        let mode = rule.condition_mode.as_str();
        match mode {
            "or" => rule.conditions.iter().any(|c| Self::check_condition(c, context)),
            _ => rule.conditions.iter().all(|c| Self::check_condition(c, context)),
        }
    }

    fn check_condition(
        condition: &crate::redis_state::types::ManipulationCondition,
        context: &ManipulationContext,
    ) -> bool {
        let value = context
            .headers
            .get(&condition.field)
            .or_else(|| context.variables.get(&condition.field));

        let Some(field_value) = value else {
            return false;
        };

        Regex::new(&condition.pattern)
            .map(|re| re.is_match(field_value))
            .unwrap_or(false)
    }

    fn apply_action(
        action_type: &str,
        name: &Option<String>,
        value: &Option<String>,
        result: &mut ManipulationResult,
    ) {
        match action_type {
            "set_header" => {
                if let (Some(n), Some(v)) = (name, value) {
                    result.set_headers.insert(n.clone(), v.clone());
                }
            }
            "remove_header" => {
                if let Some(n) = name {
                    result.remove_headers.push(n.clone());
                }
            }
            "set_var" => {
                if let (Some(n), Some(v)) = (name, value) {
                    result.set_variables.insert(n.clone(), v.clone());
                }
            }
            "log" => {
                if let Some(msg) = value {
                    result.log_messages.push(msg.clone());
                }
            }
            "hangup" => {
                result.hangup = true;
            }
            "sleep" => {
                if let Some(v) = value {
                    if let Ok(ms) = v.parse::<u64>() {
                        result.sleep_ms.push(ms);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis_state::types::{
        ManipulationAction, ManipulationClassConfig, ManipulationCondition, ManipulationRule,
    };

    fn make_condition(field: &str, pattern: &str) -> ManipulationCondition {
        ManipulationCondition {
            field: field.to_string(),
            pattern: pattern.to_string(),
        }
    }

    fn make_action(action_type: &str, name: Option<&str>, value: Option<&str>) -> ManipulationAction {
        ManipulationAction {
            action_type: action_type.to_string(),
            name: name.map(|s| s.to_string()),
            value: value.map(|s| s.to_string()),
        }
    }

    fn make_rule(
        condition_mode: &str,
        conditions: Vec<ManipulationCondition>,
        actions: Vec<ManipulationAction>,
        anti_actions: Vec<ManipulationAction>,
    ) -> ManipulationRule {
        ManipulationRule {
            condition_mode: condition_mode.to_string(),
            conditions,
            actions,
            anti_actions,
            header: None,
            action: None,
            value: None,
        }
    }

    fn make_config(rules: Vec<ManipulationRule>) -> ManipulationClassConfig {
        ManipulationClassConfig {
            name: "test".to_string(),
            rules,
        }
    }

    fn empty_context() -> ManipulationContext {
        ManipulationContext {
            headers: HashMap::new(),
            variables: HashMap::new(),
        }
    }

    fn context_with_header(key: &str, val: &str) -> ManipulationContext {
        let mut ctx = empty_context();
        ctx.headers.insert(key.to_string(), val.to_string());
        ctx
    }

    // -- AND conditions --

    #[test]
    fn test_and_both_match_actions_execute() {
        let config = make_config(vec![make_rule(
            "and",
            vec![
                make_condition("From", r"^sip:alice"),
                make_condition("To", r"^sip:bob"),
            ],
            vec![make_action("set_header", Some("X-Test"), Some("matched"))],
            vec![make_action("set_header", Some("X-Test"), Some("not-matched"))],
        )]);
        let mut ctx = empty_context();
        ctx.headers.insert("From".to_string(), "sip:alice@example.com".to_string());
        ctx.headers.insert("To".to_string(), "sip:bob@example.com".to_string());
        let result = ManipulationEngine::evaluate(&config, &ctx);
        assert_eq!(result.set_headers.get("X-Test"), Some(&"matched".to_string()));
    }

    #[test]
    fn test_and_one_fails_anti_actions_execute() {
        let config = make_config(vec![make_rule(
            "and",
            vec![
                make_condition("From", r"^sip:alice"),
                make_condition("To", r"^sip:charlie"),
            ],
            vec![make_action("set_header", Some("X-Test"), Some("matched"))],
            vec![make_action("set_header", Some("X-Test"), Some("not-matched"))],
        )]);
        let mut ctx = empty_context();
        ctx.headers.insert("From".to_string(), "sip:alice@example.com".to_string());
        ctx.headers.insert("To".to_string(), "sip:bob@example.com".to_string());
        let result = ManipulationEngine::evaluate(&config, &ctx);
        assert_eq!(result.set_headers.get("X-Test"), Some(&"not-matched".to_string()));
    }

    // -- OR conditions --

    #[test]
    fn test_or_any_match_actions_execute() {
        let config = make_config(vec![make_rule(
            "or",
            vec![
                make_condition("From", r"^sip:alice"),
                make_condition("From", r"^sip:charlie"),
            ],
            vec![make_action("set_header", Some("X-Or"), Some("matched"))],
            vec![make_action("set_header", Some("X-Or"), Some("not-matched"))],
        )]);
        let ctx = context_with_header("From", "sip:alice@example.com");
        let result = ManipulationEngine::evaluate(&config, &ctx);
        assert_eq!(result.set_headers.get("X-Or"), Some(&"matched".to_string()));
    }

    #[test]
    fn test_or_none_match_anti_actions_execute() {
        let config = make_config(vec![make_rule(
            "or",
            vec![
                make_condition("From", r"^sip:charlie"),
                make_condition("From", r"^sip:dave"),
            ],
            vec![make_action("set_header", Some("X-Or"), Some("matched"))],
            vec![make_action("set_header", Some("X-Or"), Some("not-matched"))],
        )]);
        let ctx = context_with_header("From", "sip:alice@example.com");
        let result = ManipulationEngine::evaluate(&config, &ctx);
        assert_eq!(result.set_headers.get("X-Or"), Some(&"not-matched".to_string()));
    }

    // -- Action types --

    #[test]
    fn test_set_header_action() {
        let config = make_config(vec![make_rule(
            "and",
            vec![],
            vec![make_action("set_header", Some("X-Carrier"), Some("carrier1"))],
            vec![],
        )]);
        let result = ManipulationEngine::evaluate(&config, &empty_context());
        assert_eq!(
            result.set_headers.get("X-Carrier"),
            Some(&"carrier1".to_string())
        );
    }

    #[test]
    fn test_set_var_action() {
        let config = make_config(vec![make_rule(
            "and",
            vec![],
            vec![make_action("set_var", Some("my_var"), Some("my_value"))],
            vec![],
        )]);
        let result = ManipulationEngine::evaluate(&config, &empty_context());
        assert_eq!(
            result.set_variables.get("my_var"),
            Some(&"my_value".to_string())
        );
    }

    #[test]
    fn test_log_action() {
        let config = make_config(vec![make_rule(
            "and",
            vec![],
            vec![make_action("log", None, Some("Call received"))],
            vec![],
        )]);
        let result = ManipulationEngine::evaluate(&config, &empty_context());
        assert_eq!(result.log_messages, vec!["Call received".to_string()]);
    }

    #[test]
    fn test_hangup_action() {
        let config = make_config(vec![make_rule(
            "and",
            vec![],
            vec![make_action("hangup", None, None)],
            vec![],
        )]);
        let result = ManipulationEngine::evaluate(&config, &empty_context());
        assert!(result.hangup);
    }

    #[test]
    fn test_sleep_action() {
        let config = make_config(vec![make_rule(
            "and",
            vec![],
            vec![make_action("sleep", None, Some("500"))],
            vec![],
        )]);
        let result = ManipulationEngine::evaluate(&config, &empty_context());
        assert_eq!(result.sleep_ms, vec![500u64]);
    }

    #[test]
    fn test_anti_action_remove_header() {
        let config = make_config(vec![make_rule(
            "and",
            vec![make_condition("From", r"^sip:nobody")],
            vec![],
            vec![make_action("remove_header", Some("X-Remove-Me"), None)],
        )]);
        let ctx = context_with_header("From", "sip:alice@example.com");
        let result = ManipulationEngine::evaluate(&config, &ctx);
        assert!(result.remove_headers.contains(&"X-Remove-Me".to_string()));
    }

    #[test]
    fn test_legacy_unconditional_set_header() {
        // Old-style rules with no conditions, header/action/value fields
        let rule = ManipulationRule {
            condition_mode: "and".to_string(),
            conditions: vec![],
            actions: vec![],
            anti_actions: vec![],
            header: Some("X-Legacy".to_string()),
            action: Some("set".to_string()),
            value: Some("legacy-val".to_string()),
        };
        let config = make_config(vec![rule]);
        let result = ManipulationEngine::evaluate(&config, &empty_context());
        assert_eq!(
            result.set_headers.get("X-Legacy"),
            Some(&"legacy-val".to_string())
        );
    }
}
