use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const USAGE_POLICY_ENTITLEMENT_TYPE: &str = "usage_policy";
pub const MAX_USAGE_POLICY_RULES: usize = 32;
pub const MAX_USAGE_POLICY_ENTITLEMENTS: usize = 16;
pub const MAX_USAGE_POLICY_TOTAL_RULES: usize = 64;
pub const MAX_USAGE_POLICY_ROLLING_WINDOW_SECONDS: u64 = 30 * 24 * 60 * 60;
pub const MAX_USAGE_POLICY_TEXT_LENGTH: usize = 128;
pub const MAX_USAGE_POLICY_EXACT_INTEGER: u64 = (1_u64 << 53) - 1;
pub const USAGE_POLICY_COST_UNITS_PER_USD: u64 = 100_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsagePolicyEntitlementType {
    UsagePolicy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsagePolicyEntitlement {
    #[serde(rename = "type")]
    pub entitlement_type: UsagePolicyEntitlementType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_group: Option<String>,
    pub rules: Vec<UsagePolicyRule>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsagePolicyRule {
    pub metric: UsagePolicyMetric,
    pub window: UsagePolicyWindow,
    pub limit: f64,
    #[serde(default)]
    pub enforcement: UsagePolicyEnforcement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsagePolicyMetric {
    RequestCount,
    Concurrency,
    ActualCostUsd,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsagePolicyEnforcement {
    #[default]
    HardCap,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UsagePolicyWindow {
    Rolling {
        seconds: u64,
    },
    CalendarDay {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
    },
    CalendarWeek {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
        #[serde(default = "default_week_start")]
        week_start: u8,
    },
    CalendarMonth {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
    },
    SubscriptionPeriod,
    Concurrent,
}

const fn default_week_start() -> u8 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UsagePolicyValidationError {
    #[error("{field} must not be empty")]
    EmptyText { field: String },
    #[error("{field} exceeds maximum length {max_len}")]
    TextTooLong { field: String, max_len: usize },
    #[error("usage_policy.rules must not be empty")]
    EmptyRules,
    #[error("usage_policy.rules must contain at most {max_rules} rules")]
    TooManyRules { max_rules: usize },
    #[error("entitlements must contain at most {max_policies} usage_policy entries")]
    TooManyPolicies { max_policies: usize },
    #[error("usage_policy entries must contain at most {max_rules} rules in total")]
    TooManyTotalRules { max_rules: usize },
    #[error("usage_policy.rules[{index}].limit must be finite and positive")]
    InvalidLimit { index: usize },
    #[error("usage_policy.rules[{index}].limit must be a positive exact integer")]
    InvalidIntegerLimit { index: usize },
    #[error("usage_policy.rules[{index}].limit is outside the supported cost range")]
    InvalidCostLimit { index: usize },
    #[error("usage_policy.rules[{index}].window.seconds must be positive")]
    ZeroRollingWindow { index: usize },
    #[error("usage_policy.rules[{index}].window.seconds must not exceed {max_seconds} seconds")]
    RollingWindowTooLong { index: usize, max_seconds: u64 },
    #[error("usage_policy.rules[{index}].window.timezone is not a valid IANA timezone")]
    InvalidTimezone { index: usize },
    #[error("usage_policy.rules[{index}].window.week_start must be between 1 and 7")]
    InvalidWeekStart { index: usize },
    #[error("usage_policy.rules[{index}] request_count requires a time-based window")]
    RequestCountRequiresTimeWindow { index: usize },
    #[error("usage_policy.rules[{index}] concurrency requires a concurrent window")]
    ConcurrencyRequiresConcurrentWindow { index: usize },
    #[error("usage_policy.rules[{index}] actual_cost_usd requires a time-based window")]
    ActualCostRequiresTimeWindow { index: usize },
}

#[derive(Debug, thiserror::Error)]
pub enum UsagePolicyParseError {
    #[error("entitlements must be an array")]
    EntitlementsMustBeArray,
    #[error("usage_policy entitlement at index {index} has invalid JSON shape: {source}")]
    InvalidShape {
        index: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("usage_policy entitlement at index {index} is invalid: {source}")]
    InvalidPolicy {
        index: usize,
        #[source]
        source: UsagePolicyValidationError,
    },
}

impl UsagePolicyEntitlement {
    pub fn validate(&self) -> Result<(), UsagePolicyValidationError> {
        validate_optional_text(
            self.policy_id.as_deref(),
            "usage_policy.policy_id",
            MAX_USAGE_POLICY_TEXT_LENGTH,
        )?;
        validate_optional_text(
            self.name.as_deref(),
            "usage_policy.name",
            MAX_USAGE_POLICY_TEXT_LENGTH,
        )?;
        validate_optional_text(
            self.replacement_group.as_deref(),
            "usage_policy.replacement_group",
            MAX_USAGE_POLICY_TEXT_LENGTH,
        )?;

        if self.rules.is_empty() {
            return Err(UsagePolicyValidationError::EmptyRules);
        }
        if self.rules.len() > MAX_USAGE_POLICY_RULES {
            return Err(UsagePolicyValidationError::TooManyRules {
                max_rules: MAX_USAGE_POLICY_RULES,
            });
        }

        for (index, rule) in self.rules.iter().enumerate() {
            rule.validate(index)?;
        }
        Ok(())
    }
}

impl UsagePolicyRule {
    fn validate(&self, index: usize) -> Result<(), UsagePolicyValidationError> {
        if !self.limit.is_finite() || self.limit <= 0.0 {
            return Err(UsagePolicyValidationError::InvalidLimit { index });
        }
        match self.metric {
            UsagePolicyMetric::RequestCount | UsagePolicyMetric::Concurrency => {
                if self.request_limit().is_none() {
                    return Err(UsagePolicyValidationError::InvalidIntegerLimit { index });
                }
            }
            UsagePolicyMetric::ActualCostUsd => {
                if self.cost_limit_units().is_none() {
                    return Err(UsagePolicyValidationError::InvalidCostLimit { index });
                }
            }
        }

        match &self.window {
            UsagePolicyWindow::Rolling { seconds: 0 } => {
                return Err(UsagePolicyValidationError::ZeroRollingWindow { index });
            }
            UsagePolicyWindow::Rolling { seconds }
                if *seconds > MAX_USAGE_POLICY_ROLLING_WINDOW_SECONDS =>
            {
                return Err(UsagePolicyValidationError::RollingWindowTooLong {
                    index,
                    max_seconds: MAX_USAGE_POLICY_ROLLING_WINDOW_SECONDS,
                });
            }
            UsagePolicyWindow::CalendarDay { timezone }
            | UsagePolicyWindow::CalendarMonth { timezone }
            | UsagePolicyWindow::CalendarWeek { timezone, .. } => {
                validate_timezone(timezone.as_deref(), index)?;
            }
            UsagePolicyWindow::Rolling { .. }
            | UsagePolicyWindow::SubscriptionPeriod
            | UsagePolicyWindow::Concurrent => {}
        }

        if let UsagePolicyWindow::CalendarWeek { week_start, .. } = &self.window {
            if !(1..=7).contains(week_start) {
                return Err(UsagePolicyValidationError::InvalidWeekStart { index });
            }
        }

        match (&self.metric, &self.window) {
            (UsagePolicyMetric::RequestCount, UsagePolicyWindow::Concurrent) => {
                Err(UsagePolicyValidationError::RequestCountRequiresTimeWindow { index })
            }
            (UsagePolicyMetric::Concurrency, UsagePolicyWindow::Concurrent) => Ok(()),
            (UsagePolicyMetric::Concurrency, _) => {
                Err(UsagePolicyValidationError::ConcurrencyRequiresConcurrentWindow { index })
            }
            (UsagePolicyMetric::RequestCount, _) => Ok(()),
            (UsagePolicyMetric::ActualCostUsd, UsagePolicyWindow::Concurrent) => {
                Err(UsagePolicyValidationError::ActualCostRequiresTimeWindow { index })
            }
            (UsagePolicyMetric::ActualCostUsd, _) => Ok(()),
        }
    }

    pub fn request_limit(&self) -> Option<u64> {
        if !matches!(
            self.metric,
            UsagePolicyMetric::RequestCount | UsagePolicyMetric::Concurrency
        ) || !self.limit.is_finite()
            || self.limit <= 0.0
            || self.limit.fract() != 0.0
            || self.limit > MAX_USAGE_POLICY_EXACT_INTEGER as f64
        {
            return None;
        }
        Some(self.limit as u64)
    }

    pub fn cost_limit_units(&self) -> Option<u64> {
        if self.metric != UsagePolicyMetric::ActualCostUsd {
            return None;
        }
        usd_to_usage_policy_cost_units(self.limit)
    }
}

pub fn usd_to_usage_policy_cost_units(value: f64) -> Option<u64> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let scaled = value * USAGE_POLICY_COST_UNITS_PER_USD as f64;
    if !scaled.is_finite() || scaled < 0.5 || scaled > i64::MAX as f64 {
        return None;
    }
    let rounded = scaled.round();
    let units = rounded as u64;
    (units <= i64::MAX as u64).then_some(units)
}

pub fn nonnegative_usd_to_usage_policy_cost_units(value: f64) -> Option<u64> {
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let scaled = value * USAGE_POLICY_COST_UNITS_PER_USD as f64;
    if !scaled.is_finite() || scaled > i64::MAX as f64 {
        return None;
    }
    let units = scaled.round() as u64;
    (units <= i64::MAX as u64).then_some(units)
}

pub fn parse_usage_policy_entitlements(
    entitlements: &Value,
) -> Result<Vec<UsagePolicyEntitlement>, UsagePolicyParseError> {
    let items = entitlements
        .as_array()
        .ok_or(UsagePolicyParseError::EntitlementsMustBeArray)?;
    let mut policies = Vec::new();
    let mut total_rules = 0_usize;

    for (index, item) in items.iter().enumerate() {
        if item.get("type").and_then(Value::as_str) != Some(USAGE_POLICY_ENTITLEMENT_TYPE) {
            continue;
        }

        let policy = serde_json::from_value::<UsagePolicyEntitlement>(item.clone())
            .map_err(|source| UsagePolicyParseError::InvalidShape { index, source })?;
        policy
            .validate()
            .map_err(|source| UsagePolicyParseError::InvalidPolicy { index, source })?;
        if policies.len() >= MAX_USAGE_POLICY_ENTITLEMENTS {
            return Err(UsagePolicyParseError::InvalidPolicy {
                index,
                source: UsagePolicyValidationError::TooManyPolicies {
                    max_policies: MAX_USAGE_POLICY_ENTITLEMENTS,
                },
            });
        }
        total_rules = total_rules.saturating_add(policy.rules.len());
        if total_rules > MAX_USAGE_POLICY_TOTAL_RULES {
            return Err(UsagePolicyParseError::InvalidPolicy {
                index,
                source: UsagePolicyValidationError::TooManyTotalRules {
                    max_rules: MAX_USAGE_POLICY_TOTAL_RULES,
                },
            });
        }
        policies.push(policy);
    }

    Ok(policies)
}

fn validate_optional_text(
    value: Option<&str>,
    field: &str,
    max_len: usize,
) -> Result<(), UsagePolicyValidationError> {
    let Some(value) = value else {
        return Ok(());
    };
    let value = value.trim();
    if value.is_empty() {
        return Err(UsagePolicyValidationError::EmptyText {
            field: field.to_string(),
        });
    }
    if value.chars().count() > max_len {
        return Err(UsagePolicyValidationError::TextTooLong {
            field: field.to_string(),
            max_len,
        });
    }
    Ok(())
}

fn validate_timezone(
    timezone: Option<&str>,
    index: usize,
) -> Result<(), UsagePolicyValidationError> {
    let Some(timezone) = timezone else {
        return Ok(());
    };
    let timezone = timezone.trim();
    if timezone.is_empty() || timezone.parse::<chrono_tz::Tz>().is_err() {
        return Err(UsagePolicyValidationError::InvalidTimezone { index });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_flexible_combinations_and_ignores_legacy_entitlements() {
        let entitlements = json!([
            {
                "type": "daily_quota",
                "daily_quota_usd": 25.0
            },
            {
                "type": "usage_policy",
                "policy_id": "standard-traffic",
                "name": "Standard traffic limits",
                "replacement_group": "pro-tier",
                "rules": [
                    {
                        "metric": "request_count",
                        "window": { "kind": "rolling", "seconds": 18000 },
                        "limit": 500
                    },
                    {
                        "metric": "request_count",
                        "window": {
                            "kind": "calendar_week",
                            "timezone": "Asia/Shanghai",
                            "week_start": 1
                        },
                        "limit": 10000,
                        "enforcement": "hard_cap"
                    },
                    {
                        "metric": "concurrency",
                        "window": { "kind": "concurrent" },
                        "limit": 4
                    }
                ]
            }
        ]);

        let policies = parse_usage_policy_entitlements(&entitlements).unwrap();

        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].policy_id.as_deref(), Some("standard-traffic"));
        assert_eq!(policies[0].replacement_group.as_deref(), Some("pro-tier"));
        assert_eq!(policies[0].rules.len(), 3);
        assert_eq!(
            policies[0].rules[0].enforcement,
            UsagePolicyEnforcement::HardCap
        );
        assert_eq!(policies[0].rules[2].window, UsagePolicyWindow::Concurrent);
    }

    #[test]
    fn supports_single_weekly_rule_and_subscription_period() {
        let entitlements = json!([
            {
                "type": "usage_policy",
                "rules": [{
                    "metric": "request_count",
                    "window": { "kind": "calendar_week" },
                    "limit": 1000
                }]
            },
            {
                "type": "usage_policy",
                "rules": [{
                    "metric": "request_count",
                    "window": { "kind": "subscription_period" },
                    "limit": 10000
                }]
            }
        ]);

        let policies = parse_usage_policy_entitlements(&entitlements).unwrap();

        assert_eq!(policies.len(), 2);
        assert_eq!(
            policies[0].rules[0].window,
            UsagePolicyWindow::CalendarWeek {
                timezone: None,
                week_start: 1,
            }
        );
        assert_eq!(
            policies[1].rules[0].window,
            UsagePolicyWindow::SubscriptionPeriod
        );
    }

    #[test]
    fn serializes_the_public_json_contract() {
        let policy = UsagePolicyEntitlement {
            entitlement_type: UsagePolicyEntitlementType::UsagePolicy,
            policy_id: None,
            name: None,
            replacement_group: None,
            rules: vec![UsagePolicyRule {
                metric: UsagePolicyMetric::RequestCount,
                window: UsagePolicyWindow::Rolling { seconds: 60 },
                limit: 120.0,
                enforcement: UsagePolicyEnforcement::HardCap,
            }],
        };

        assert_eq!(
            serde_json::to_value(policy).unwrap(),
            json!({
                "type": "usage_policy",
                "rules": [{
                    "metric": "request_count",
                    "window": { "kind": "rolling", "seconds": 60 },
                    "limit": 120.0,
                    "enforcement": "hard_cap"
                }]
            })
        );
    }

    #[test]
    fn rejects_invalid_limits_and_zero_rolling_windows() {
        let zero_limit = json!([{
            "type": "usage_policy",
            "rules": [{
                "metric": "request_count",
                "window": { "kind": "rolling", "seconds": 60 },
                "limit": 0
            }]
        }]);
        assert!(matches!(
            parse_usage_policy_entitlements(&zero_limit),
            Err(UsagePolicyParseError::InvalidPolicy {
                source: UsagePolicyValidationError::InvalidLimit { index: 0 },
                ..
            })
        ));

        let zero_window = json!([{
            "type": "usage_policy",
            "rules": [{
                "metric": "request_count",
                "window": { "kind": "rolling", "seconds": 0 },
                "limit": 1
            }]
        }]);
        assert!(matches!(
            parse_usage_policy_entitlements(&zero_window),
            Err(UsagePolicyParseError::InvalidPolicy {
                source: UsagePolicyValidationError::ZeroRollingWindow { index: 0 },
                ..
            })
        ));

        let excessive_window = json!([{
            "type": "usage_policy",
            "rules": [{
                "metric": "request_count",
                "window": { "kind": "rolling", "seconds": 2_592_001 },
                "limit": 1
            }]
        }]);
        assert!(matches!(
            parse_usage_policy_entitlements(&excessive_window),
            Err(UsagePolicyParseError::InvalidPolicy {
                source: UsagePolicyValidationError::RollingWindowTooLong {
                    index: 0,
                    max_seconds: 2_592_000
                },
                ..
            })
        ));
    }

    #[test]
    fn validates_metric_specific_limits_and_cost_conversion() {
        let cost_policy = json!([{
            "type": "usage_policy",
            "rules": [{
                "metric": "actual_cost_usd",
                "window": { "kind": "rolling", "seconds": 18000 },
                "limit": 12.34567891
            }]
        }]);
        let policies = parse_usage_policy_entitlements(&cost_policy).unwrap();
        assert_eq!(policies[0].rules[0].cost_limit_units(), Some(1_234_567_891));
        assert_eq!(policies[0].rules[0].request_limit(), None);

        let fractional_requests = json!([{
            "type": "usage_policy",
            "rules": [{
                "metric": "request_count",
                "window": { "kind": "rolling", "seconds": 60 },
                "limit": 1.5
            }]
        }]);
        assert!(matches!(
            parse_usage_policy_entitlements(&fractional_requests),
            Err(UsagePolicyParseError::InvalidPolicy {
                source: UsagePolicyValidationError::InvalidIntegerLimit { index: 0 },
                ..
            })
        ));

        let sub_unit_cost = json!([{
            "type": "usage_policy",
            "rules": [{
                "metric": "actual_cost_usd",
                "window": { "kind": "calendar_day" },
                "limit": 0.000000001
            }]
        }]);
        assert!(matches!(
            parse_usage_policy_entitlements(&sub_unit_cost),
            Err(UsagePolicyParseError::InvalidPolicy {
                source: UsagePolicyValidationError::InvalidCostLimit { index: 0 },
                ..
            })
        ));
    }

    #[test]
    fn rejects_metric_and_window_mismatches() {
        let concurrency_with_rolling_window = json!([{
            "type": "usage_policy",
            "rules": [{
                "metric": "concurrency",
                "window": { "kind": "rolling", "seconds": 60 },
                "limit": 3
            }]
        }]);
        assert!(matches!(
            parse_usage_policy_entitlements(&concurrency_with_rolling_window),
            Err(UsagePolicyParseError::InvalidPolicy {
                source: UsagePolicyValidationError::ConcurrencyRequiresConcurrentWindow {
                    index: 0
                },
                ..
            })
        ));

        let cost_with_concurrent_window = json!([{
            "type": "usage_policy",
            "rules": [{
                "metric": "actual_cost_usd",
                "window": { "kind": "concurrent" },
                "limit": 1
            }]
        }]);
        assert!(matches!(
            parse_usage_policy_entitlements(&cost_with_concurrent_window),
            Err(UsagePolicyParseError::InvalidPolicy {
                source: UsagePolicyValidationError::ActualCostRequiresTimeWindow { index: 0 },
                ..
            })
        ));

        let request_count_with_concurrent_window = json!([{
            "type": "usage_policy",
            "rules": [{
                "metric": "request_count",
                "window": { "kind": "concurrent" },
                "limit": 3
            }]
        }]);
        assert!(matches!(
            parse_usage_policy_entitlements(&request_count_with_concurrent_window),
            Err(UsagePolicyParseError::InvalidPolicy {
                source: UsagePolicyValidationError::RequestCountRequiresTimeWindow { index: 0 },
                ..
            })
        ));
    }

    #[test]
    fn rejects_invalid_calendar_configuration() {
        let invalid_timezone = json!([{
            "type": "usage_policy",
            "rules": [{
                "metric": "request_count",
                "window": { "kind": "calendar_day", "timezone": "Mars/Olympus" },
                "limit": 100
            }]
        }]);
        assert!(matches!(
            parse_usage_policy_entitlements(&invalid_timezone),
            Err(UsagePolicyParseError::InvalidPolicy {
                source: UsagePolicyValidationError::InvalidTimezone { index: 0 },
                ..
            })
        ));

        let invalid_week_start = json!([{
            "type": "usage_policy",
            "rules": [{
                "metric": "request_count",
                "window": { "kind": "calendar_week", "week_start": 0 },
                "limit": 100
            }]
        }]);
        assert!(matches!(
            parse_usage_policy_entitlements(&invalid_week_start),
            Err(UsagePolicyParseError::InvalidPolicy {
                source: UsagePolicyValidationError::InvalidWeekStart { index: 0 },
                ..
            })
        ));
    }

    #[test]
    fn rejects_empty_rules_unknown_fields_and_non_array_roots() {
        let empty_rules = json!([{"type": "usage_policy", "rules": []}]);
        assert!(matches!(
            parse_usage_policy_entitlements(&empty_rules),
            Err(UsagePolicyParseError::InvalidPolicy {
                source: UsagePolicyValidationError::EmptyRules,
                ..
            })
        ));

        let typo = json!([{
            "type": "usage_policy",
            "rules": [{
                "metric": "request_count",
                "window": { "kind": "rolling", "seconds": 60 },
                "limit": 100,
                "enforcment": "hard_cap"
            }]
        }]);
        assert!(matches!(
            parse_usage_policy_entitlements(&typo),
            Err(UsagePolicyParseError::InvalidShape { .. })
        ));

        assert!(matches!(
            parse_usage_policy_entitlements(&json!({})),
            Err(UsagePolicyParseError::EntitlementsMustBeArray)
        ));
    }

    #[test]
    fn window_discriminator_requires_kind_instead_of_type() {
        for window in [
            json!({"type": "rolling", "seconds": 60}),
            json!({"seconds": 60}),
        ] {
            let entitlements = json!([{
                "type": "usage_policy",
                "rules": [{
                    "metric": "request_count",
                    "window": window,
                    "limit": 100
                }]
            }]);

            assert!(matches!(
                parse_usage_policy_entitlements(&entitlements),
                Err(UsagePolicyParseError::InvalidShape { .. })
            ));
        }
    }

    #[test]
    fn enforces_collection_and_metadata_bounds() {
        let rule = json!({
            "metric": "request_count",
            "window": { "kind": "rolling", "seconds": 1 },
            "limit": 1
        });
        let maximum_rules = json!([{
            "type": "usage_policy",
            "rules": vec![rule.clone(); MAX_USAGE_POLICY_RULES]
        }]);
        assert_eq!(
            parse_usage_policy_entitlements(&maximum_rules)
                .unwrap()
                .first()
                .unwrap()
                .rules
                .len(),
            MAX_USAGE_POLICY_RULES
        );

        let too_many_rules = json!([{
            "type": "usage_policy",
            "rules": vec![rule; MAX_USAGE_POLICY_RULES + 1]
        }]);
        assert!(matches!(
            parse_usage_policy_entitlements(&too_many_rules),
            Err(UsagePolicyParseError::InvalidPolicy {
                source: UsagePolicyValidationError::TooManyRules {
                    max_rules: MAX_USAGE_POLICY_RULES
                },
                ..
            })
        ));

        let empty_name = json!([{
            "type": "usage_policy",
            "name": "   ",
            "rules": [{
                "metric": "concurrency",
                "window": { "kind": "concurrent" },
                "limit": 1
            }]
        }]);
        assert!(matches!(
            parse_usage_policy_entitlements(&empty_name),
            Err(UsagePolicyParseError::InvalidPolicy {
                source: UsagePolicyValidationError::EmptyText { .. },
                ..
            })
        ));
    }

    #[test]
    fn bounds_policy_count_and_total_rule_count() {
        let one_rule_policy = json!({
            "type": "usage_policy",
            "rules": [{
                "metric": "request_count",
                "window": { "kind": "rolling", "seconds": 60 },
                "limit": 1
            }]
        });
        let too_many_policies = Value::Array(
            (0..=MAX_USAGE_POLICY_ENTITLEMENTS)
                .map(|_| one_rule_policy.clone())
                .collect(),
        );
        assert!(matches!(
            parse_usage_policy_entitlements(&too_many_policies),
            Err(UsagePolicyParseError::InvalidPolicy {
                source: UsagePolicyValidationError::TooManyPolicies { .. },
                ..
            })
        ));

        let max_rules_policy = json!({
            "type": "usage_policy",
            "rules": vec![one_rule_policy["rules"][0].clone(); MAX_USAGE_POLICY_RULES]
        });
        let too_many_total_rules =
            json!([max_rules_policy.clone(), max_rules_policy, one_rule_policy]);
        assert!(matches!(
            parse_usage_policy_entitlements(&too_many_total_rules),
            Err(UsagePolicyParseError::InvalidPolicy {
                source: UsagePolicyValidationError::TooManyTotalRules { .. },
                ..
            })
        ));
    }

    #[test]
    fn supports_week_only_and_five_hour_plus_week_combinations() {
        let entitlements = json!([
            {
                "type": "usage_policy",
                "policy_id": "weekly-only",
                "rules": [{
                    "metric": "request_count",
                    "window": { "kind": "calendar_week", "timezone": "Asia/Shanghai" },
                    "limit": 10_000
                }]
            },
            {
                "type": "usage_policy",
                "policy_id": "burst-and-weekly",
                "rules": [
                    {
                        "metric": "request_count",
                        "window": { "kind": "rolling", "seconds": 18_000 },
                        "limit": 500
                    },
                    {
                        "metric": "request_count",
                        "window": { "kind": "calendar_week", "timezone": "Asia/Shanghai" },
                        "limit": 20_000
                    }
                ]
            }
        ]);

        let policies = parse_usage_policy_entitlements(&entitlements).expect("policy combinations");

        assert_eq!(policies.len(), 2);
        assert_eq!(policies[0].rules.len(), 1);
        assert_eq!(policies[1].rules.len(), 2);
        assert!(matches!(
            policies[1].rules[0].window,
            UsagePolicyWindow::Rolling { seconds: 18_000 }
        ));
        assert!(matches!(
            policies[1].rules[1].window,
            UsagePolicyWindow::CalendarWeek { .. }
        ));
    }

    #[test]
    fn supports_qps_rpm_concurrency_and_cost_in_one_policy() {
        let entitlements = json!([{
            "type": "usage_policy",
            "policy_id": "full-traffic-policy",
            "rules": [
                {
                    "metric": "request_count",
                    "window": { "kind": "rolling", "seconds": 1 },
                    "limit": 2
                },
                {
                    "metric": "request_count",
                    "window": { "kind": "rolling", "seconds": 60 },
                    "limit": 60
                },
                {
                    "metric": "concurrency",
                    "window": { "kind": "concurrent" },
                    "limit": 8
                },
                {
                    "metric": "actual_cost_usd",
                    "window": { "kind": "calendar_month", "timezone": "UTC" },
                    "limit": 125.50
                }
            ]
        }]);

        let policy = &parse_usage_policy_entitlements(&entitlements).expect("combined policy")[0];

        assert_eq!(policy.rules.len(), 4);
        assert_eq!(policy.rules[0].request_limit(), Some(2));
        assert_eq!(policy.rules[1].request_limit(), Some(60));
        assert_eq!(policy.rules[2].request_limit(), Some(8));
        assert_eq!(policy.rules[3].cost_limit_units(), Some(12_550_000_000));
        assert!(matches!(
            policy.rules[2].window,
            UsagePolicyWindow::Concurrent
        ));
    }

    #[test]
    fn keeps_multiple_policy_entitlements_independent() {
        let entitlements = json!([
            {
                "type": "usage_policy",
                "policy_id": "api-traffic",
                "rules": [{
                    "metric": "request_count",
                    "window": { "kind": "rolling", "seconds": 60 },
                    "limit": 100
                }]
            },
            {
                "type": "usage_policy",
                "policy_id": "model-cost",
                "rules": [{
                    "metric": "actual_cost_usd",
                    "window": { "kind": "rolling", "seconds": 18_000 },
                    "limit": 5
                }]
            },
            {
                "type": "usage_policy",
                "policy_id": "subscription-lifetime",
                "rules": [{
                    "metric": "request_count",
                    "window": { "kind": "subscription_period" },
                    "limit": 10_000
                }]
            }
        ]);

        let policies =
            parse_usage_policy_entitlements(&entitlements).expect("independent policies");

        assert_eq!(
            policies
                .iter()
                .map(|policy| policy.rules.len())
                .sum::<usize>(),
            3
        );
        assert_eq!(
            policies
                .iter()
                .map(|policy| policy.policy_id.as_deref())
                .collect::<Vec<_>>(),
            vec![
                Some("api-traffic"),
                Some("model-cost"),
                Some("subscription-lifetime")
            ]
        );
    }

    #[test]
    fn validates_dst_timezones_and_week_start_boundaries() {
        let dst_policies = json!([
            {
                "type": "usage_policy",
                "rules": [{
                    "metric": "request_count",
                    "window": { "kind": "calendar_day", "timezone": "America/New_York" },
                    "limit": 100
                }]
            },
            {
                "type": "usage_policy",
                "rules": [{
                    "metric": "request_count",
                    "window": {
                        "kind": "calendar_week",
                        "timezone": "Europe/Berlin",
                        "week_start": 7
                    },
                    "limit": 100
                }]
            },
            {
                "type": "usage_policy",
                "rules": [{
                    "metric": "actual_cost_usd",
                    "window": { "kind": "calendar_month", "timezone": "Pacific/Apia" },
                    "limit": 1
                }]
            }
        ]);

        let policies = parse_usage_policy_entitlements(&dst_policies).expect("DST timezones");
        assert_eq!(policies.len(), 3);
        assert!(matches!(
            policies[1].rules[0].window,
            UsagePolicyWindow::CalendarWeek { week_start: 7, .. }
        ));

        for (timezone, expected) in [
            ("America/New_York", true),
            ("Europe/Berlin", true),
            ("Pacific/Apia", true),
            ("UTC", true),
            // Text fields are normalized by trimming surrounding whitespace before parsing.
            ("America/New_York ", true),
            // chrono-tz intentionally accepts the POSIX-compatible EST alias.
            ("EST", true),
        ] {
            let value = json!([{
                "type": "usage_policy",
                "rules": [{
                    "metric": "request_count",
                    "window": { "kind": "calendar_day", "timezone": timezone },
                    "limit": 1
                }]
            }]);
            assert_eq!(
                parse_usage_policy_entitlements(&value).is_ok(),
                expected,
                "{timezone}"
            );
        }
    }

    #[test]
    fn accepts_week_start_one_and_rejects_values_outside_one_through_seven() {
        for week_start in 1..=7 {
            let value = json!([{
                "type": "usage_policy",
                "rules": [{
                    "metric": "request_count",
                    "window": { "kind": "calendar_week", "week_start": week_start },
                    "limit": 1
                }]
            }]);
            assert!(
                parse_usage_policy_entitlements(&value).is_ok(),
                "week_start={week_start}"
            );
        }

        for week_start in [0, 8, u8::MAX] {
            let value = json!([{
                "type": "usage_policy",
                "rules": [{
                    "metric": "request_count",
                    "window": { "kind": "calendar_week", "week_start": week_start },
                    "limit": 1
                }]
            }]);
            assert!(matches!(
                parse_usage_policy_entitlements(&value),
                Err(UsagePolicyParseError::InvalidPolicy {
                    source: UsagePolicyValidationError::InvalidWeekStart { index: 0 },
                    ..
                })
            ));
        }
    }
}
