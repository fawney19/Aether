use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::actions::{
    RoutingAction, RoutingRulePhase, RoutingSchedulingMode, RoutingSetPriorityMode,
};
use crate::conditions::RoutingCondition;
use crate::RoutingFailoverRules;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingSchedulingPreset {
    pub preset: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RoutingPoolPolicyOverride {
    #[serde(default)]
    pub scheduling_presets: Vec<RoutingSchedulingPreset>,
}

/// Default number of attempts on the first-ranked (sticky) candidate before
/// failing over: one retry on the same key.
pub const DEFAULT_STICKY_KEY_ATTEMPTS: u32 = 2;

/// Request-independent execution behaviours selected by a routing strategy.
///
/// These flags deliberately live beside scheduling rather than in provider
/// transport configuration. A resolved policy is snapshotted for the request
/// and can therefore be consumed by execution without rereading mutable
/// system settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct RoutingExecutionPolicy {
    #[serde(default, skip_serializing_if = "is_false")]
    pub strict_format_conversion: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub enable_cf_heartbeat: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub cyber_continue_failover: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub cancel_on_client_disconnect: bool,
    #[serde(default)]
    pub max_transfer_count: u64,
    #[serde(default)]
    pub max_transfer_timeout_seconds: u64,
    #[serde(default)]
    pub failover_rules: RoutingFailoverRules,
}

impl<'de> Deserialize<'de> for RoutingExecutionPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize, Default)]
        struct LegacyCompatibleExecutionPolicy {
            #[serde(default)]
            strict_format_conversion: bool,
            #[serde(default)]
            enable_cf_heartbeat: bool,
            #[serde(default)]
            enable_openai_image_sync_heartbeat: bool,
            #[serde(default)]
            enable_standard_text_sync_heartbeat: bool,
            #[serde(default)]
            cyber_continue_failover: bool,
            #[serde(default)]
            cancel_on_client_disconnect: bool,
            #[serde(default)]
            max_transfer_count: u64,
            #[serde(default)]
            max_transfer_timeout_seconds: u64,
            #[serde(default)]
            failover_rules: RoutingFailoverRules,
        }

        let value = LegacyCompatibleExecutionPolicy::deserialize(deserializer)?;
        Ok(Self {
            strict_format_conversion: value.strict_format_conversion,
            enable_cf_heartbeat: value.enable_cf_heartbeat
                || value.enable_openai_image_sync_heartbeat
                || value.enable_standard_text_sync_heartbeat,
            cyber_continue_failover: value.cyber_continue_failover,
            cancel_on_client_disconnect: value.cancel_on_client_disconnect,
            max_transfer_count: value.max_transfer_count,
            max_transfer_timeout_seconds: value.max_transfer_timeout_seconds,
            failover_rules: value.failover_rules,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingDefaultPolicy {
    #[serde(default)]
    pub priority_mode: RoutingSetPriorityMode,
    #[serde(default)]
    pub scheduling_mode: RoutingSchedulingMode,
    #[serde(default)]
    pub keep_priority_on_conversion: bool,
    /// Total attempts on the first-ranked candidate before moving on. Later
    /// candidates always get a single attempt so failover keeps advancing.
    /// `0` and `1` both mean no same-key retry.
    #[serde(default = "default_sticky_key_attempts")]
    pub sticky_key_attempts: u32,
    /// Strategy-scoped execution behaviour. Flattened for a stable JSON
    /// shape and backwards-compatible migration from system settings.
    #[serde(flatten)]
    pub execution_policy: RoutingExecutionPolicy,
}

impl Default for RoutingDefaultPolicy {
    fn default() -> Self {
        Self {
            priority_mode: RoutingSetPriorityMode::default(),
            scheduling_mode: RoutingSchedulingMode::default(),
            keep_priority_on_conversion: false,
            sticky_key_attempts: DEFAULT_STICKY_KEY_ATTEMPTS,
            execution_policy: RoutingExecutionPolicy::default(),
        }
    }
}

fn default_sticky_key_attempts() -> u32 {
    DEFAULT_STICKY_KEY_ATTEMPTS
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod execution_policy_tests {
    use super::*;

    #[test]
    fn group_visibility_is_opt_in_and_round_trips_without_losing_policy() {
        let legacy: RoutingGroupConfig = serde_json::from_str("{}").unwrap();
        assert!(!legacy.user_visible);
        assert!(!RoutingGroupConfig::default().user_visible);
        for user_visible in [false, true] {
            let config: RoutingGroupConfig = serde_json::from_value(serde_json::json!({
                "user_visible": user_visible,
                "billing_multiplier": 0.5,
                "disabled_providers": ["private-provider"],
                "default_policy": { "scheduling_mode": "fixed_order" }
            }))
            .unwrap();
            let encoded = serde_json::to_value(&config).unwrap();
            assert_eq!(encoded["user_visible"], user_visible);
            assert_eq!(config.billing_multiplier, 0.5);
            assert_eq!(config.disabled_providers, ["private-provider"]);
            assert_eq!(
                serde_json::from_value::<RoutingGroupConfig>(encoded).unwrap(),
                config
            );
        }
        for invalid in [
            serde_json::json!(null),
            serde_json::json!("true"),
            serde_json::json!(1),
        ] {
            assert!(
                serde_json::from_value::<RoutingGroupConfig>(serde_json::json!({
                    "user_visible": invalid
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn group_billing_multiplier_defaults_to_one_and_rejects_invalid_values() {
        let legacy: RoutingGroupConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.billing_multiplier, 1.0);
        assert_eq!(RoutingGroupConfig::default().billing_multiplier, 1.0);
        for multiplier in [0.0, 0.25, 1.0, 2.5] {
            let config: RoutingGroupConfig = serde_json::from_value(serde_json::json!({
                "billing_multiplier": multiplier
            }))
            .unwrap();
            crate::validate_routing_group_config(&config).unwrap();
            assert_eq!(config.billing_multiplier, multiplier);
            assert_eq!(
                serde_json::from_value::<RoutingGroupConfig>(
                    serde_json::to_value(&config).unwrap()
                )
                .unwrap(),
                config
            );
        }
        for multiplier in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let config = RoutingGroupConfig {
                billing_multiplier: multiplier,
                ..RoutingGroupConfig::default()
            };
            assert!(matches!(
                crate::validate_routing_group_config(&config),
                Err(crate::RoutingValidationError::InvalidBillingMultiplier)
            ));
        }
        for value in [
            serde_json::json!(null),
            serde_json::json!("2"),
            serde_json::json!(false),
        ] {
            assert!(
                serde_json::from_value::<RoutingGroupConfig>(serde_json::json!({
                    "billing_multiplier": value
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn routing_failover_configuration_round_trips_and_validates() {
        let config: RoutingGroupConfig = serde_json::from_value(serde_json::json!({
            "default_policy": {
                "max_transfer_count": 3,
                "max_transfer_timeout_seconds": 90,
                "failover_rules": {
                    "success_failover_patterns": [{ "pattern": "(?i)capacity" }],
                    "error_stop_patterns": [{ "status_codes": [400, 413] }]
                }
            }
        }))
        .unwrap();
        crate::validate_routing_group_config(&config).unwrap();
        assert_eq!(config.default_policy.execution_policy.max_transfer_count, 3);
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["default_policy"]["max_transfer_timeout_seconds"], 90);
        assert_eq!(
            serde_json::from_value::<RoutingGroupConfig>(value).unwrap(),
            config
        );
    }

    #[test]
    fn cancellation_defaults_off_and_round_trips_with_legacy_heartbeat() {
        let default: RoutingDefaultPolicy = serde_json::from_str("{}").unwrap();
        assert!(!default.execution_policy.cancel_on_client_disconnect);
        let policy: RoutingDefaultPolicy = serde_json::from_value(serde_json::json!({
            "cancel_on_client_disconnect": true,
            "enable_standard_text_sync_heartbeat": true
        }))
        .unwrap();
        assert!(policy.execution_policy.cancel_on_client_disconnect);
        assert!(policy.execution_policy.enable_cf_heartbeat);
        let encoded = serde_json::to_value(&policy).unwrap();
        assert_eq!(encoded["cancel_on_client_disconnect"], true);
        assert_eq!(
            serde_json::from_value::<RoutingDefaultPolicy>(encoded).unwrap(),
            policy
        );
        assert!(
            serde_json::from_value::<RoutingDefaultPolicy>(serde_json::json!({
                "cancel_on_client_disconnect": "true"
            }))
            .is_err()
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RoutingModelPolicy {
    pub model: String,
    #[serde(default)]
    pub allowed_providers: Vec<String>,
    #[serde(default)]
    pub allowed_keys: Vec<String>,
    /// Per-model provider enablement. A `false` value adds a provider to this
    /// model's exclusions and `true` removes an inherited exclusion, including
    /// one from the legacy group-wide `disabled_providers` baseline.
    #[serde(default)]
    pub provider_enabled_overrides: BTreeMap<String, bool>,
    #[serde(default)]
    pub provider_priority_overrides: BTreeMap<String, i32>,
    #[serde(default)]
    pub key_priority_overrides: BTreeMap<String, i32>,
    /// Key priority overrides scoped to one API format: `api_format -> key_id -> priority`.
    ///
    /// A key can serve several API formats and legacy `global_priority_by_format`
    /// ranks it independently per format. Entries here take precedence over
    /// `key_priority_overrides` when the candidate format matches.
    #[serde(default)]
    pub key_priority_overrides_by_format: BTreeMap<String, BTreeMap<String, i32>>,
    #[serde(default)]
    pub pool_priority_overrides: BTreeMap<String, i32>,
    #[serde(default)]
    pub pool_policy_overrides: BTreeMap<String, RoutingPoolPolicyOverride>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingRule {
    pub id: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub phase: RoutingRulePhase,
    #[serde(default)]
    pub conditions: RoutingCondition,
    #[serde(default)]
    pub actions: Vec<RoutingAction>,
    #[serde(default)]
    pub stop_processing: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingGroupConfig {
    /// Whether authenticated users can discover and explicitly select this
    /// group. Private bindings and automatic defaults remain independent.
    #[serde(default)]
    pub user_visible: bool,
    /// Group-wide billing multiplier, snapshotted when a request is routed.
    #[serde(default = "default_billing_multiplier")]
    pub billing_multiplier: f64,
    /// Legacy provider exclusion baseline for the group. Explicit per-model
    /// enablement overrides may change it; allowlists and rules cannot.
    #[serde(default)]
    pub disabled_providers: Vec<String>,
    /// The default policy is global for the selected strategy group. Model
    /// differences are expressed through `model_policies` and `rules`.
    #[serde(default)]
    pub default_policy: RoutingDefaultPolicy,
    #[serde(default)]
    pub model_policies: Vec<RoutingModelPolicy>,
    #[serde(default)]
    pub rules: Vec<RoutingRule>,
}

pub(crate) fn default_billing_multiplier() -> f64 {
    1.0
}

impl Default for RoutingGroupConfig {
    fn default() -> Self {
        Self {
            user_visible: false,
            billing_multiplier: default_billing_multiplier(),
            disabled_providers: Vec::new(),
            default_policy: RoutingDefaultPolicy::default(),
            model_policies: Vec::new(),
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingGroupRecord {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub enabled: bool,
    pub is_system_default: bool,
    pub config_json: Value,
    pub version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingGroupBindingSubject {
    User,
    ApiKey,
    UserGroup,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingGroupBinding {
    pub id: String,
    pub group_id: String,
    pub subject_type: RoutingGroupBindingSubject,
    pub subject_id: String,
    pub is_default: bool,
    pub allow_explicit_select: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingGroupVersionRecord {
    pub id: String,
    pub group_id: String,
    pub version: i64,
    pub config_json: Value,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
}

fn default_true() -> bool {
    true
}
