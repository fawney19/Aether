use std::collections::BTreeSet;

use thiserror::Error;

use crate::model::{RoutingGroupConfig, RoutingPoolPolicyOverride};
use crate::mutations::{validate_header_patch, validate_json_patch_operations};
use crate::{RoutingAction, RoutingRulePhase};

pub const MAX_ROUTING_ALLOWED_KEYS: usize = 512;

const ROUTING_POOL_PRESETS: &[&str] = &[
    "lru",
    "cache_affinity",
    "load_balance",
    "single_account",
    "priority_first",
    "free_team_first",
    "free_first",
    "team_first",
    "plus_first",
    "pro_first",
    "health_first",
    "latency_first",
    "cost_first",
    "quota_balanced",
    "recent_refresh",
];

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RoutingValidationError {
    #[error("routing failover rules are invalid: {0}")]
    InvalidFailoverRules(String),
    #[error("routing rule id is empty")]
    EmptyRuleId,
    #[error("duplicate routing rule id: {0}")]
    DuplicateRuleId(String),
    #[error("routing model policy selector is empty")]
    EmptyModelSelector,
    #[error("invalid mutation action: {0}")]
    InvalidMutation(String),
    #[error("routing rule {rule_id} uses unsupported {action} action in provider_request phase")]
    ProviderRequestActionNotAllowed {
        rule_id: String,
        action: &'static str,
    },
    #[error("routing key selector {selector} contains {count} entries; maximum is {max}")]
    TooManyAllowedKeys {
        selector: String,
        count: usize,
        max: usize,
    },
    #[error("routing pool policy {selector} has an empty provider id")]
    EmptyPoolProviderId { selector: String },
    #[error("routing pool policy {selector} uses unsupported preset: {preset}")]
    UnsupportedPoolPreset { selector: String, preset: String },
    #[error("routing pool policy {selector} contains duplicate preset: {preset}")]
    DuplicatePoolPreset { selector: String, preset: String },
    #[error("routing pool policy {selector} preset {preset} has invalid mode: {mode}")]
    InvalidPoolPresetMode {
        selector: String,
        preset: String,
        mode: String,
    },
    #[error("routing pool policy {selector} enables mutually exclusive distribution presets: {presets:?}")]
    ConflictingPoolDistributionPresets {
        selector: String,
        presets: Vec<String>,
    },
}

pub fn validate_routing_group_config(
    config: &RoutingGroupConfig,
) -> Result<(), RoutingValidationError> {
    crate::validate_routing_failover_rules(&config.default_policy.execution_policy.failover_rules)
        .map_err(RoutingValidationError::InvalidFailoverRules)?;
    let mut rule_ids = BTreeSet::new();
    for model_policy in &config.model_policies {
        if model_policy.model.trim().is_empty() {
            return Err(RoutingValidationError::EmptyModelSelector);
        }
        validate_allowed_key_count(
            format!("model:{}", model_policy.model.trim()),
            model_policy.allowed_keys.len(),
        )?;
        for (provider_id, override_policy) in &model_policy.pool_policy_overrides {
            let model_selector = format!("model:{}", model_policy.model.trim());
            if provider_id.trim().is_empty() {
                return Err(RoutingValidationError::EmptyPoolProviderId {
                    selector: model_selector,
                });
            }
            validate_pool_policy_override(
                format!("{model_selector}:provider:{}", provider_id.trim()),
                override_policy,
            )?;
        }
    }
    for rule in &config.rules {
        if rule.id.trim().is_empty() {
            return Err(RoutingValidationError::EmptyRuleId);
        }
        if !rule_ids.insert(rule.id.clone()) {
            return Err(RoutingValidationError::DuplicateRuleId(rule.id.clone()));
        }
        for action in &rule.actions {
            if rule.phase == RoutingRulePhase::ProviderRequest
                && !matches!(
                    action,
                    RoutingAction::JsonPatchBody { .. } | RoutingAction::PatchHeaders { .. }
                )
            {
                return Err(RoutingValidationError::ProviderRequestActionNotAllowed {
                    rule_id: rule.id.clone(),
                    action: routing_action_name(action),
                });
            }
            match action {
                RoutingAction::JsonPatchBody { patch } => {
                    validate_json_patch_operations(patch).map_err(|error| {
                        RoutingValidationError::InvalidMutation(error.to_string())
                    })?;
                }
                RoutingAction::PatchHeaders { patch } => {
                    validate_header_patch(patch).map_err(|error| {
                        RoutingValidationError::InvalidMutation(error.to_string())
                    })?;
                }
                RoutingAction::RestrictKeys { key_ids } => {
                    validate_allowed_key_count(format!("rule:{}", rule.id), key_ids.len())?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn validate_pool_policy_override(
    selector: String,
    override_policy: &RoutingPoolPolicyOverride,
) -> Result<(), RoutingValidationError> {
    let mut seen = BTreeSet::new();
    let mut enabled_distribution_presets = Vec::new();
    for preset in &override_policy.scheduling_presets {
        let normalized = preset.preset.trim().to_ascii_lowercase();
        if !ROUTING_POOL_PRESETS.contains(&normalized.as_str()) {
            return Err(RoutingValidationError::UnsupportedPoolPreset {
                selector,
                preset: normalized,
            });
        }
        if !seen.insert(normalized.clone()) {
            return Err(RoutingValidationError::DuplicatePoolPreset {
                selector,
                preset: normalized,
            });
        }
        if let Some(mode) = preset.mode.as_deref() {
            let mode = mode.trim().to_ascii_lowercase();
            if !routing_pool_preset_mode_valid(&normalized, &mode) {
                return Err(RoutingValidationError::InvalidPoolPresetMode {
                    selector,
                    preset: normalized,
                    mode,
                });
            }
        }
        if preset.enabled && routing_pool_distribution_preset(&normalized) {
            enabled_distribution_presets.push(normalized);
        }
    }
    if enabled_distribution_presets.len() > 1 {
        return Err(RoutingValidationError::ConflictingPoolDistributionPresets {
            selector,
            presets: enabled_distribution_presets,
        });
    }
    Ok(())
}

fn routing_pool_preset_mode_valid(preset: &str, mode: &str) -> bool {
    match preset {
        "free_team_first" => matches!(mode, "free_only" | "team_only" | "both"),
        "free_first" => mode == "free_only",
        "team_first" => mode == "team_only",
        "plus_first" => mode == "plus_only",
        "pro_first" => mode == "pro_only",
        _ => false,
    }
}

fn routing_pool_distribution_preset(preset: &str) -> bool {
    matches!(
        preset,
        "lru" | "cache_affinity" | "load_balance" | "single_account"
    )
}

fn validate_allowed_key_count(
    selector: String,
    count: usize,
) -> Result<(), RoutingValidationError> {
    if count > MAX_ROUTING_ALLOWED_KEYS {
        return Err(RoutingValidationError::TooManyAllowedKeys {
            selector,
            count,
            max: MAX_ROUTING_ALLOWED_KEYS,
        });
    }
    Ok(())
}

fn routing_action_name(action: &RoutingAction) -> &'static str {
    match action {
        RoutingAction::RestrictModels { .. } => "restrict_models",
        RoutingAction::RestrictProviders { .. } => "restrict_providers",
        RoutingAction::RestrictKeys { .. } => "restrict_keys",
        RoutingAction::SetScheduling { .. } => "set_scheduling",
        RoutingAction::SetProviderPriority { .. } => "set_provider_priority",
        RoutingAction::SetKeyPriority { .. } => "set_key_priority",
        RoutingAction::JsonPatchBody { .. } => "json_patch_body",
        RoutingAction::PatchHeaders { .. } => "patch_headers",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{
        RoutingCondition, RoutingGroupConfig, RoutingHeaderPatch, RoutingJsonPatchOperation,
        RoutingModelPolicy, RoutingRule,
    };

    use super::*;

    fn provider_request_config(actions: Vec<RoutingAction>) -> RoutingGroupConfig {
        RoutingGroupConfig {
            rules: vec![RoutingRule {
                id: "provider-rule".to_string(),
                priority: 0,
                enabled: true,
                phase: RoutingRulePhase::ProviderRequest,
                conditions: RoutingCondition::default(),
                actions,
                stop_processing: false,
            }],
            ..RoutingGroupConfig::default()
        }
    }

    #[test]
    fn rejects_non_mutation_actions_in_provider_request_phase() {
        let actions = [
            (
                RoutingAction::RestrictModels {
                    models: vec!["gpt-5".to_string()],
                },
                "restrict_models",
            ),
            (
                RoutingAction::RestrictProviders {
                    provider_ids: vec!["provider-1".to_string()],
                },
                "restrict_providers",
            ),
            (
                RoutingAction::RestrictKeys {
                    key_ids: vec!["key-1".to_string()],
                },
                "restrict_keys",
            ),
            (
                RoutingAction::SetScheduling {
                    priority_mode: None,
                    scheduling_mode: None,
                    keep_priority_on_conversion: Some(true),
                    sticky_key_attempts: None,
                },
                "set_scheduling",
            ),
            (
                RoutingAction::SetProviderPriority {
                    provider_id: "provider-1".to_string(),
                    priority: 1,
                },
                "set_provider_priority",
            ),
            (
                RoutingAction::SetKeyPriority {
                    key_id: "key-1".to_string(),
                    priority: 1,
                    api_format: None,
                },
                "set_key_priority",
            ),
        ];

        for (action, action_name) in actions {
            let error = validate_routing_group_config(&provider_request_config(vec![action]))
                .expect_err("provider_request must only accept mutations");
            assert_eq!(
                error,
                RoutingValidationError::ProviderRequestActionNotAllowed {
                    rule_id: "provider-rule".to_string(),
                    action: action_name,
                }
            );
        }
    }

    #[test]
    fn accepts_mutation_actions_in_provider_request_phase() {
        let config = provider_request_config(vec![
            RoutingAction::JsonPatchBody {
                patch: vec![RoutingJsonPatchOperation::Add {
                    path: "/metadata/routed".to_string(),
                    value: json!(true),
                }],
            },
            RoutingAction::PatchHeaders {
                patch: vec![RoutingHeaderPatch::Set {
                    name: "x-routing-profile".to_string(),
                    value: "provider-rule".to_string(),
                }],
            },
        ]);

        validate_routing_group_config(&config)
            .expect("provider_request mutation actions should remain valid");
    }

    #[test]
    fn rejects_oversized_model_allowed_key_selector() {
        let config = RoutingGroupConfig {
            model_policies: vec![RoutingModelPolicy {
                model: "gpt-5".to_string(),
                allowed_keys: key_ids(MAX_ROUTING_ALLOWED_KEYS + 1),
                ..RoutingModelPolicy::default()
            }],
            ..RoutingGroupConfig::default()
        };

        assert_eq!(
            validate_routing_group_config(&config),
            Err(RoutingValidationError::TooManyAllowedKeys {
                selector: "model:gpt-5".to_string(),
                count: MAX_ROUTING_ALLOWED_KEYS + 1,
                max: MAX_ROUTING_ALLOWED_KEYS,
            })
        );
    }

    #[test]
    fn rejects_oversized_rule_allowed_key_selector() {
        let config = RoutingGroupConfig {
            rules: vec![RoutingRule {
                id: "restrict-keys".to_string(),
                priority: 0,
                enabled: true,
                phase: RoutingRulePhase::ClientRequest,
                conditions: RoutingCondition::default(),
                actions: vec![RoutingAction::RestrictKeys {
                    key_ids: key_ids(MAX_ROUTING_ALLOWED_KEYS + 1),
                }],
                stop_processing: false,
            }],
            ..RoutingGroupConfig::default()
        };

        assert_eq!(
            validate_routing_group_config(&config),
            Err(RoutingValidationError::TooManyAllowedKeys {
                selector: "rule:restrict-keys".to_string(),
                count: MAX_ROUTING_ALLOWED_KEYS + 1,
                max: MAX_ROUTING_ALLOWED_KEYS,
            })
        );
    }

    #[test]
    fn accepts_allowed_key_selectors_at_the_limit() {
        let config = RoutingGroupConfig {
            model_policies: vec![RoutingModelPolicy {
                model: "gpt-5".to_string(),
                allowed_keys: key_ids(MAX_ROUTING_ALLOWED_KEYS),
                ..RoutingModelPolicy::default()
            }],
            ..RoutingGroupConfig::default()
        };

        validate_routing_group_config(&config)
            .expect("allowed key selectors at the scan limit should remain valid");
    }

    #[test]
    fn rejects_unknown_pool_override_preset() {
        let config = pool_override_config(vec![pool_preset("typo_priority", true, None)]);

        assert_eq!(
            validate_routing_group_config(&config),
            Err(RoutingValidationError::UnsupportedPoolPreset {
                selector: "model:gpt-5:provider:provider-1".to_string(),
                preset: "typo_priority".to_string(),
            })
        );
    }

    #[test]
    fn rejects_duplicate_pool_override_presets() {
        let config = pool_override_config(vec![
            pool_preset("health_first", true, None),
            pool_preset(" HEALTH_FIRST ", false, None),
        ]);

        assert_eq!(
            validate_routing_group_config(&config),
            Err(RoutingValidationError::DuplicatePoolPreset {
                selector: "model:gpt-5:provider:provider-1".to_string(),
                preset: "health_first".to_string(),
            })
        );
    }

    #[test]
    fn rejects_invalid_pool_override_mode() {
        let config =
            pool_override_config(vec![pool_preset("free_team_first", true, Some("pro_only"))]);

        assert_eq!(
            validate_routing_group_config(&config),
            Err(RoutingValidationError::InvalidPoolPresetMode {
                selector: "model:gpt-5:provider:provider-1".to_string(),
                preset: "free_team_first".to_string(),
                mode: "pro_only".to_string(),
            })
        );
    }

    #[test]
    fn rejects_conflicting_pool_override_distribution_modes() {
        let config = pool_override_config(vec![
            pool_preset("lru", true, None),
            pool_preset("cache_affinity", true, None),
        ]);

        assert_eq!(
            validate_routing_group_config(&config),
            Err(RoutingValidationError::ConflictingPoolDistributionPresets {
                selector: "model:gpt-5:provider:provider-1".to_string(),
                presets: vec!["lru".to_string(), "cache_affinity".to_string()],
            })
        );
    }

    #[test]
    fn accepts_valid_pool_override_modes_and_disabled_alternatives() {
        let config = pool_override_config(vec![
            pool_preset("cache_affinity", true, None),
            pool_preset("lru", false, None),
            pool_preset("free_team_first", true, Some("team_only")),
        ]);

        validate_routing_group_config(&config).expect("valid pool override should pass");
    }

    fn pool_override_config(
        scheduling_presets: Vec<crate::RoutingSchedulingPreset>,
    ) -> RoutingGroupConfig {
        RoutingGroupConfig {
            model_policies: vec![RoutingModelPolicy {
                model: "gpt-5".to_string(),
                pool_policy_overrides: std::collections::BTreeMap::from([(
                    "provider-1".to_string(),
                    RoutingPoolPolicyOverride { scheduling_presets },
                )]),
                ..RoutingModelPolicy::default()
            }],
            ..RoutingGroupConfig::default()
        }
    }

    fn pool_preset(
        preset: &str,
        enabled: bool,
        mode: Option<&str>,
    ) -> crate::RoutingSchedulingPreset {
        crate::RoutingSchedulingPreset {
            preset: preset.to_string(),
            enabled,
            mode: mode.map(str::to_string),
        }
    }

    fn key_ids(count: usize) -> Vec<String> {
        (0..count).map(|index| format!("key-{index}")).collect()
    }
}
