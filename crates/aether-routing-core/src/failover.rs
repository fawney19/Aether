use std::collections::BTreeSet;

use regex::Regex;
use serde::{Deserialize, Serialize};

pub const MAX_ROUTING_FAILOVER_RULES: usize = 64;
pub const MAX_ROUTING_FAILOVER_PATTERN_BYTES: usize = 4096;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoutingFailoverRule {
    pub pattern: String,
    pub status_codes: BTreeSet<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoutingFailoverRules {
    pub success_failover_patterns: Vec<RoutingFailoverRule>,
    pub error_stop_patterns: Vec<RoutingFailoverRule>,
}

pub fn validate_routing_failover_rules(rules: &RoutingFailoverRules) -> Result<(), String> {
    for (name, entries, success) in [
        (
            "success_failover_patterns",
            &rules.success_failover_patterns,
            true,
        ),
        ("error_stop_patterns", &rules.error_stop_patterns, false),
    ] {
        if entries.len() > MAX_ROUTING_FAILOVER_RULES {
            return Err(format!("{name} exceeds {MAX_ROUTING_FAILOVER_RULES} rules"));
        }
        for (index, rule) in entries.iter().enumerate() {
            let pattern = rule.pattern.trim();
            if pattern.is_empty() && (success || rule.status_codes.is_empty()) {
                return Err(format!(
                    "{name}[{index}] requires a pattern or error status codes"
                ));
            }
            if pattern.len() > MAX_ROUTING_FAILOVER_PATTERN_BYTES {
                return Err(format!(
                    "{name}[{index}] pattern exceeds {MAX_ROUTING_FAILOVER_PATTERN_BYTES} bytes"
                ));
            }
            if !pattern.is_empty() {
                Regex::new(pattern)
                    .map_err(|error| format!("{name}[{index}] invalid regex: {error}"))?;
            }
            if rule.status_codes.iter().any(|status| {
                if success {
                    *status != 200
                } else {
                    !(400..=599).contains(status)
                }
            }) {
                return Err(format!("{name}[{index}] contains invalid status codes"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_regex_and_status_only_stop_rules() {
        let rules = RoutingFailoverRules {
            success_failover_patterns: vec![RoutingFailoverRule {
                pattern: "(?i)capacity.*exhausted".to_string(),
                ..Default::default()
            }],
            error_stop_patterns: vec![RoutingFailoverRule {
                status_codes: [400, 413].into_iter().collect(),
                ..Default::default()
            }],
        };
        assert!(validate_routing_failover_rules(&rules).is_ok());
    }

    #[test]
    fn rejects_invalid_or_unbounded_rule_configuration() {
        for rule in [
            RoutingFailoverRule::default(),
            RoutingFailoverRule {
                pattern: "[".to_string(),
                ..Default::default()
            },
            RoutingFailoverRule {
                pattern: "error".to_string(),
                status_codes: [429].into_iter().collect(),
            },
            RoutingFailoverRule {
                pattern: "a".repeat(MAX_ROUTING_FAILOVER_PATTERN_BYTES + 1),
                ..Default::default()
            },
        ] {
            let rules = RoutingFailoverRules {
                success_failover_patterns: vec![rule],
                ..Default::default()
            };
            assert!(validate_routing_failover_rules(&rules).is_err());
        }
        let rules = RoutingFailoverRules {
            error_stop_patterns: vec![
                RoutingFailoverRule {
                    status_codes: [400].into_iter().collect(),
                    ..Default::default()
                };
                MAX_ROUTING_FAILOVER_RULES + 1
            ],
            ..Default::default()
        };
        assert!(validate_routing_failover_rules(&rules).is_err());
    }
}
