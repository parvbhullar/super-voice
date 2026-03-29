use crate::redis_state::types::{MatchType, RoutingRecord};

/// Find the routing record with the longest prefix that matches the given
/// destination string.
///
/// Only records with `match_type == MatchType::Lpm` are considered. The record
/// whose `value` field is the longest prefix of `destination` wins. Returns
/// `None` when no LPM record matches.
pub fn lpm_lookup<'a>(records: &'a [RoutingRecord], destination: &str) -> Option<&'a RoutingRecord> {
    let mut best: Option<&'a RoutingRecord> = None;
    let mut best_len = 0usize;

    for record in records {
        if record.match_type != MatchType::Lpm {
            continue;
        }
        if destination.starts_with(record.value.as_str()) {
            let prefix_len = record.value.len();
            if prefix_len > best_len {
                best_len = prefix_len;
                best = Some(record);
            }
        }
    }

    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis_state::types::{MatchType, RoutingRecord, RoutingTarget};

    fn lpm_record(prefix: &str, trunk: &str) -> RoutingRecord {
        RoutingRecord {
            match_type: MatchType::Lpm,
            value: prefix.to_string(),
            compare_op: None,
            match_field: "destination_number".to_string(),
            targets: vec![RoutingTarget {
                trunk: trunk.to_string(),
                load_percent: None,
            }],
            jump_to: None,
            priority: 100,
            is_default: false,
        }
    }

    #[test]
    fn test_lpm_longest_prefix_wins() {
        let records = vec![
            lpm_record("+1", "trunk-us"),
            lpm_record("+1415", "trunk-sf"),
            lpm_record("+14155", "trunk-sf5"),
        ];
        let result = lpm_lookup(&records, "+14155551234");
        assert_eq!(result.map(|r| r.targets[0].trunk.as_str()), Some("trunk-sf5"));
    }

    #[test]
    fn test_lpm_medium_prefix_wins_when_no_longer_match() {
        let records = vec![
            lpm_record("+1", "trunk-us"),
            lpm_record("+1415", "trunk-sf"),
            lpm_record("+14155", "trunk-sf5"),
        ];
        // +14161234567 matches +1 but not +1415 or +14155
        let result = lpm_lookup(&records, "+14161234567");
        assert_eq!(result.map(|r| r.targets[0].trunk.as_str()), Some("trunk-us"));
    }

    #[test]
    fn test_lpm_uk_prefix_match() {
        let records = vec![lpm_record("+4420", "trunk-uk")];
        let result = lpm_lookup(&records, "+442071234567");
        assert_eq!(result.map(|r| r.targets[0].trunk.as_str()), Some("trunk-uk"));
    }

    #[test]
    fn test_lpm_no_match_returns_none() {
        let records = vec![lpm_record("+1415", "trunk-sf")];
        let result = lpm_lookup(&records, "+33123456789");
        assert!(result.is_none());
    }

    #[test]
    fn test_lpm_empty_records_returns_none() {
        let result = lpm_lookup(&[], "+14155551234");
        assert!(result.is_none());
    }

    #[test]
    fn test_lpm_skips_non_lpm_records() {
        let mut exact_record = lpm_record("+1415", "trunk-exact");
        exact_record.match_type = MatchType::ExactMatch;

        let records = vec![exact_record, lpm_record("+1", "trunk-us")];
        // Should match +1 (Lpm), not +1415 (ExactMatch)
        let result = lpm_lookup(&records, "+14155551234");
        assert_eq!(result.map(|r| r.targets[0].trunk.as_str()), Some("trunk-us"));
    }

    #[test]
    fn test_lpm_exact_prefix_match() {
        // destination exactly equals the prefix
        let records = vec![lpm_record("+14155551234", "trunk-exact")];
        let result = lpm_lookup(&records, "+14155551234");
        assert_eq!(result.map(|r| r.targets[0].trunk.as_str()), Some("trunk-exact"));
    }
}
