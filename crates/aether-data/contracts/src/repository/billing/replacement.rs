use std::collections::HashSet;

use serde_json::Value;

pub const ENTITLEMENT_REPLACEMENT_GROUP_FIELD: &str = "replacement_group";
pub const MAX_ENTITLEMENT_REPLACEMENT_GROUP_LENGTH: usize = 128;

const LEGACY_REPLACEMENT_ENTITLEMENT_TYPES: [&str; 2] = ["daily_quota", "membership_group"];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EntitlementReplacementGroupValidationError {
    #[error("entitlements must be an array")]
    EntitlementsMustBeArray,
    #[error("entitlements[{index}].replacement_group must be a string")]
    InvalidType { index: usize },
    #[error("entitlements[{index}].replacement_group must not be empty")]
    Empty { index: usize },
    #[error("entitlements[{index}].replacement_group exceeds maximum length {max_len}")]
    TooLong { index: usize, max_len: usize },
}

pub fn validate_entitlement_replacement_groups(
    entitlements: &Value,
) -> Result<(), EntitlementReplacementGroupValidationError> {
    let items = entitlements
        .as_array()
        .ok_or(EntitlementReplacementGroupValidationError::EntitlementsMustBeArray)?;

    for (index, item) in items.iter().enumerate() {
        let Some(group) = item.get(ENTITLEMENT_REPLACEMENT_GROUP_FIELD) else {
            continue;
        };
        let group = group
            .as_str()
            .ok_or(EntitlementReplacementGroupValidationError::InvalidType { index })?
            .trim();
        if group.is_empty() {
            return Err(EntitlementReplacementGroupValidationError::Empty { index });
        }
        if group.chars().count() > MAX_ENTITLEMENT_REPLACEMENT_GROUP_LENGTH {
            return Err(EntitlementReplacementGroupValidationError::TooLong {
                index,
                max_len: MAX_ENTITLEMENT_REPLACEMENT_GROUP_LENGTH,
            });
        }
    }

    Ok(())
}

pub fn entitlements_have_replacement_selector(entitlements: &Value) -> bool {
    let Some(items) = entitlements.as_array() else {
        return false;
    };

    items.iter().any(|item| {
        let entitlement_type = item.get("type").and_then(Value::as_str);
        LEGACY_REPLACEMENT_ENTITLEMENT_TYPES.contains(&entitlement_type.unwrap_or_default())
            || replacement_group(item).is_some()
    })
}

pub fn entitlements_should_replace_existing(incoming: &Value, existing: &Value) -> bool {
    let (Some(incoming), Some(existing)) = (incoming.as_array(), existing.as_array()) else {
        return false;
    };

    if LEGACY_REPLACEMENT_ENTITLEMENT_TYPES.iter().any(|kind| {
        entitlement_items_have_type(incoming, kind) && entitlement_items_have_type(existing, kind)
    }) {
        return true;
    }

    let incoming_groups = incoming
        .iter()
        .filter_map(replacement_group)
        .collect::<HashSet<_>>();
    !incoming_groups.is_empty()
        && existing
            .iter()
            .filter_map(replacement_group)
            .any(|group| incoming_groups.contains(group))
}

fn entitlement_items_have_type(items: &[Value], entitlement_type: &str) -> bool {
    items
        .iter()
        .any(|item| item.get("type").and_then(Value::as_str) == Some(entitlement_type))
}

fn replacement_group(item: &Value) -> Option<&str> {
    item.get(ENTITLEMENT_REPLACEMENT_GROUP_FIELD)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|group| !group.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_daily_quota_and_membership_groups_remain_mutually_exclusive() {
        assert!(entitlements_should_replace_existing(
            &json!([{"type": "daily_quota", "daily_quota_usd": 20}]),
            &json!([
                {"type": "daily_quota", "daily_quota_usd": 10},
                {"type": "usage_policy", "rules": []}
            ]),
        ));
        assert!(entitlements_should_replace_existing(
            &json!([{"type": "membership_group", "grant_user_groups": ["pro"]}]),
            &json!([{"type": "membership_group", "grant_user_groups": ["basic"]}]),
        ));
    }

    #[test]
    fn usage_policies_stack_by_default() {
        let incoming = json!([{"type": "usage_policy", "policy_id": "weekly", "rules": []}]);
        let existing = json!([{"type": "usage_policy", "policy_id": "five-hour", "rules": []}]);

        assert!(!entitlements_have_replacement_selector(&incoming));
        assert!(!entitlements_should_replace_existing(&incoming, &existing));
    }

    #[test]
    fn matching_explicit_groups_replace_the_whole_package() {
        let incoming = json!([{
            "type": "usage_policy",
            "replacement_group": "pro-tier",
            "rules": []
        }]);
        let existing = json!([
            {"type": "wallet_credit", "amount_usd": 10},
            {
                "type": "usage_policy",
                "replacement_group": "pro-tier",
                "rules": []
            }
        ]);

        assert!(entitlements_have_replacement_selector(&incoming));
        assert!(entitlements_should_replace_existing(&incoming, &existing));
        assert!(!entitlements_should_replace_existing(
            &incoming,
            &json!([{
                "type": "usage_policy",
                "replacement_group": "team-tier",
                "rules": []
            }]),
        ));
    }

    #[test]
    fn explicit_groups_can_span_entitlement_types_and_ignore_outer_whitespace() {
        assert!(entitlements_should_replace_existing(
            &json!([{
                "type": "usage_policy",
                "replacement_group": " traffic-tier ",
                "rules": []
            }]),
            &json!([{
                "type": "wallet_credit",
                "replacement_group": "traffic-tier",
                "amount_usd": 10
            }]),
        ));
    }

    #[test]
    fn validates_explicit_group_shape_and_bounds() {
        assert!(validate_entitlement_replacement_groups(&json!([{
            "type": "usage_policy",
            "replacement_group": "pro-tier"
        }]))
        .is_ok());
        assert_eq!(
            validate_entitlement_replacement_groups(&json!([{
                "type": "usage_policy",
                "replacement_group": "   "
            }])),
            Err(EntitlementReplacementGroupValidationError::Empty { index: 0 })
        );
        assert_eq!(
            validate_entitlement_replacement_groups(&json!([{
                "type": "usage_policy",
                "replacement_group": 42
            }])),
            Err(EntitlementReplacementGroupValidationError::InvalidType { index: 0 })
        );
        assert!(matches!(
            validate_entitlement_replacement_groups(&json!([{
                "type": "usage_policy",
                "replacement_group": "x".repeat(MAX_ENTITLEMENT_REPLACEMENT_GROUP_LENGTH + 1)
            }])),
            Err(EntitlementReplacementGroupValidationError::TooLong { index: 0, .. })
        ));
    }
}
