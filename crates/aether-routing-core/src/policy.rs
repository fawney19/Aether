use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::actions::{
    RoutingAction, RoutingRulePhase, RoutingSchedulingMode, RoutingSetPriorityMode,
};
use crate::conditions::RoutingConditionContext;
use crate::model::{
    RoutingExecutionPolicy, RoutingGroupConfig, RoutingModelPolicy, RoutingPoolPolicyOverride,
};
use crate::mutations::{validate_header_patch, validate_json_patch_operations, MutationPlan};
use crate::ranking::RankingOverlay;
use crate::validation::validate_routing_group_config;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RoutingPolicyError {
    #[error("routing group config is invalid: {0}")]
    InvalidConfig(String),
    #[error("model is not allowed by routing rule: {0}")]
    ModelNotAllowed(String),
    #[error("mutation action is invalid: {0}")]
    InvalidMutation(String),
}

#[derive(Debug, Clone)]
pub struct RoutingPolicyInput<'a> {
    pub group_id: Option<&'a str>,
    pub group_version: Option<i64>,
    pub selection_source: &'a str,
    pub requested_model: &'a str,
    pub resolved_model: &'a str,
    pub api_format: &'a str,
    pub user_id: Option<&'a str>,
    pub api_key_id: Option<&'a str>,
    pub headers: &'a Value,
    pub body: &'a Value,
    pub phase: RoutingRulePhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchedRoutingRule {
    pub id: String,
    pub priority: i32,
    pub phase: RoutingRulePhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedRoutingPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_version: Option<i64>,
    pub selection_source: String,
    pub requested_model: String,
    pub resolved_model: String,
    pub priority_mode: RoutingSetPriorityMode,
    pub scheduling_mode: RoutingSchedulingMode,
    pub keep_priority_on_conversion: bool,
    /// See `RoutingDefaultPolicy::sticky_key_attempts`.
    #[serde(default = "default_sticky_key_attempts")]
    pub sticky_key_attempts: u32,
    #[serde(flatten)]
    pub execution_policy: RoutingExecutionPolicy,
    pub ranking_overlay: RankingOverlay,
    pub mutation_plan: MutationPlan,
    #[serde(default)]
    pub pool_policy_overrides: BTreeMap<String, RoutingPoolPolicyOverride>,
    #[serde(default)]
    pub matched_rules: Vec<MatchedRoutingRule>,
}

pub fn resolve_routing_policy(
    config: &RoutingGroupConfig,
    input: RoutingPolicyInput<'_>,
) -> Result<ResolvedRoutingPolicy, RoutingPolicyError> {
    validate_routing_group_config(config)
        .map_err(|error| RoutingPolicyError::InvalidConfig(error.to_string()))?;

    let mut policy = ResolvedRoutingPolicy {
        group_id: input.group_id.map(str::to_string),
        group_version: input.group_version,
        selection_source: input.selection_source.to_string(),
        requested_model: input.requested_model.to_string(),
        resolved_model: input.resolved_model.to_string(),
        priority_mode: config.default_policy.priority_mode,
        scheduling_mode: config.default_policy.scheduling_mode,
        keep_priority_on_conversion: config.default_policy.keep_priority_on_conversion,
        sticky_key_attempts: config.default_policy.sticky_key_attempts,
        execution_policy: config.default_policy.execution_policy.clone(),
        ranking_overlay: RankingOverlay::default(),
        mutation_plan: MutationPlan::default(),
        pool_policy_overrides: BTreeMap::new(),
        matched_rules: Vec::new(),
    };

    for model_policy in matching_model_policies(config, input.requested_model, input.resolved_model)
    {
        apply_model_policy(&mut policy, model_policy);
    }

    let condition_context = RoutingConditionContext {
        model: input.requested_model,
        api_format: input.api_format,
        user_id: input.user_id,
        api_key_id: input.api_key_id,
        headers: input.headers,
        body: input.body,
    };

    let mut rules = config
        .rules
        .iter()
        .filter(|rule| rule.enabled && rule.phase == input.phase)
        .collect::<Vec<_>>();
    rules.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then(left.id.cmp(&right.id))
    });
    for rule in rules {
        if !rule.conditions.matches(&condition_context) {
            continue;
        }
        for action in &rule.actions {
            apply_action(
                &mut policy,
                action,
                input.requested_model,
                input.resolved_model,
            )?;
        }
        policy.matched_rules.push(MatchedRoutingRule {
            id: rule.id.clone(),
            priority: rule.priority,
            phase: rule.phase,
        });
        if rule.stop_processing {
            break;
        }
    }

    Ok(policy)
}

fn apply_model_policy(policy: &mut ResolvedRoutingPolicy, model_policy: &RoutingModelPolicy) {
    if !model_policy.allowed_providers.is_empty() {
        policy.ranking_overlay.allowed_providers = model_policy.allowed_providers.clone();
    }
    if !model_policy.allowed_keys.is_empty() {
        policy.ranking_overlay.allowed_keys = model_policy.allowed_keys.clone();
    }
    policy.ranking_overlay.provider_priority_overrides.extend(
        model_policy
            .provider_priority_overrides
            .iter()
            .map(|(key, value)| (key.clone(), *value)),
    );
    policy.ranking_overlay.key_priority_overrides.extend(
        model_policy
            .key_priority_overrides
            .iter()
            .map(|(key, value)| (key.clone(), *value)),
    );
    for (api_format, overrides) in &model_policy.key_priority_overrides_by_format {
        for (key_id, priority) in overrides {
            policy
                .ranking_overlay
                .insert_key_priority_override_for_format(api_format, key_id.clone(), *priority);
        }
    }
    policy.ranking_overlay.pool_priority_overrides.extend(
        model_policy
            .pool_priority_overrides
            .iter()
            .map(|(key, value)| (key.clone(), *value)),
    );
    policy
        .pool_policy_overrides
        .extend(model_policy.pool_policy_overrides.clone());
}

fn apply_action(
    policy: &mut ResolvedRoutingPolicy,
    action: &RoutingAction,
    requested_model: &str,
    resolved_model: &str,
) -> Result<(), RoutingPolicyError> {
    match action {
        RoutingAction::RestrictModels { models } => {
            if !model_allowed(models, requested_model) && !model_allowed(models, resolved_model) {
                return Err(RoutingPolicyError::ModelNotAllowed(
                    requested_model.to_string(),
                ));
            }
        }
        RoutingAction::RestrictProviders { provider_ids } => {
            policy.ranking_overlay.allowed_providers = provider_ids.clone();
        }
        RoutingAction::RestrictKeys { key_ids } => {
            policy.ranking_overlay.allowed_keys = key_ids.clone();
        }
        RoutingAction::SetScheduling {
            priority_mode,
            scheduling_mode,
            keep_priority_on_conversion,
            sticky_key_attempts,
        } => {
            if let Some(priority_mode) = priority_mode {
                policy.priority_mode = *priority_mode;
            }
            if let Some(scheduling_mode) = scheduling_mode {
                policy.scheduling_mode = *scheduling_mode;
            }
            if let Some(keep_priority_on_conversion) = keep_priority_on_conversion {
                policy.keep_priority_on_conversion = *keep_priority_on_conversion;
            }
            if let Some(sticky_key_attempts) = sticky_key_attempts {
                policy.sticky_key_attempts = *sticky_key_attempts;
            }
        }
        RoutingAction::SetProviderPriority {
            provider_id,
            priority,
        } => {
            policy
                .ranking_overlay
                .provider_priority_overrides
                .insert(provider_id.clone(), *priority);
        }
        RoutingAction::SetKeyPriority {
            key_id,
            priority,
            api_format,
        } => match api_format
            .as_deref()
            .map(str::trim)
            .filter(|f| !f.is_empty())
        {
            Some(api_format) => {
                policy
                    .ranking_overlay
                    .insert_key_priority_override_for_format(api_format, key_id.clone(), *priority);
            }
            None => {
                policy
                    .ranking_overlay
                    .key_priority_overrides
                    .insert(key_id.clone(), *priority);
            }
        },
        RoutingAction::JsonPatchBody { patch } => {
            validate_json_patch_operations(patch)
                .map_err(|error| RoutingPolicyError::InvalidMutation(error.to_string()))?;
            policy.mutation_plan.body_patch.extend(patch.clone());
        }
        RoutingAction::PatchHeaders { patch } => {
            validate_header_patch(patch)
                .map_err(|error| RoutingPolicyError::InvalidMutation(error.to_string()))?;
            policy.mutation_plan.header_patch.extend(patch.clone());
        }
    }
    Ok(())
}

fn matching_model_policies<'a>(
    config: &'a RoutingGroupConfig,
    requested_model: &str,
    resolved_model: &str,
) -> Vec<&'a RoutingModelPolicy> {
    config
        .model_policies
        .iter()
        .filter(|policy| {
            model_pattern_matches(&policy.model, requested_model)
                || model_pattern_matches(&policy.model, resolved_model)
        })
        .collect()
}

fn model_allowed(patterns: &[String], requested_model: &str) -> bool {
    patterns.is_empty()
        || patterns
            .iter()
            .any(|pattern| model_pattern_matches(pattern, requested_model))
}

fn default_sticky_key_attempts() -> u32 {
    crate::model::DEFAULT_STICKY_KEY_ATTEMPTS
}

fn model_pattern_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.trim();
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return value.starts_with(prefix);
    }
    pattern == value
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use crate::actions::{
        RoutingJsonPatchOperation, RoutingRulePhase, RoutingSchedulingMode, RoutingSetPriorityMode,
    };
    use crate::conditions::{RoutingCondition, RoutingConditionOp};
    use crate::model::{RoutingDefaultPolicy, RoutingRule};

    use super::*;

    #[test]
    fn all_model_scheduling_and_rankings_apply_to_future_models() {
        let config: RoutingGroupConfig = serde_json::from_value(json!({
            "default_policy": {
                "priority_mode": "global_key",
                "scheduling_mode": "load_balance"
            },
            "model_policies": [{
                "model": "*",
                "provider_priority_overrides": { "provider-a": 7 }
            }],
            "rules": []
        }))
        .expect("all-model scheduling config should deserialize");

        for model in ["existing-model", "future-model"] {
            let policy = resolve_routing_policy(
                &config,
                RoutingPolicyInput {
                    group_id: Some("group-1"),
                    group_version: Some(1),
                    selection_source: "explicit",
                    requested_model: model,
                    resolved_model: model,
                    api_format: "openai:chat",
                    user_id: None,
                    api_key_id: None,
                    headers: &json!({}),
                    body: &json!({}),
                    phase: RoutingRulePhase::ClientRequest,
                },
            )
            .expect("all-model scheduling policy should resolve");
            assert_eq!(policy.priority_mode, RoutingSetPriorityMode::GlobalKey);
            assert_eq!(policy.scheduling_mode, RoutingSchedulingMode::LoadBalance);
            assert_eq!(
                policy
                    .ranking_overlay
                    .provider_priority_overrides
                    .get("provider-a"),
                Some(&7)
            );
            assert!(policy.matched_rules.is_empty());
        }
    }

    #[test]
    fn shared_scheduling_rule_applies_only_to_selected_models() {
        let config: RoutingGroupConfig = serde_json::from_value(json!({
            "default_policy": {
                "priority_mode": "provider",
                "scheduling_mode": "cache_affinity"
            },
            "model_policies": [
                { "model": "model-a", "provider_priority_overrides": { "provider-a": 7 } },
                { "model": "model-b", "provider_priority_overrides": { "provider-a": 7 } }
            ],
            "rules": [{
                "id": "ui_scheduling_policy:shared",
                "priority": 10000,
                "enabled": true,
                "phase": "client_request",
                "conditions": { "any": [
                    { "field": "model", "op": "eq", "value": "model-a" },
                    { "field": "model", "op": "eq", "value": "model-b" }
                ] },
                "actions": [{
                    "type": "set_scheduling",
                    "priority_mode": "global_key",
                    "scheduling_mode": "fixed_order"
                }],
                "stop_processing": false
            }]
        }))
        .expect("shared scheduling config should deserialize");

        for model in ["model-a", "model-b", "other-model"] {
            let policy = resolve_routing_policy(
                &config,
                RoutingPolicyInput {
                    group_id: Some("group-1"),
                    group_version: Some(1),
                    selection_source: "explicit",
                    requested_model: model,
                    resolved_model: model,
                    api_format: "openai:chat",
                    user_id: None,
                    api_key_id: None,
                    headers: &json!({}),
                    body: &json!({}),
                    phase: RoutingRulePhase::ClientRequest,
                },
            )
            .expect("shared scheduling policy should resolve");
            if model == "other-model" {
                assert_eq!(policy.priority_mode, RoutingSetPriorityMode::Provider);
                assert_eq!(policy.scheduling_mode, RoutingSchedulingMode::CacheAffinity);
                assert!(policy
                    .ranking_overlay
                    .provider_priority_overrides
                    .is_empty());
                assert!(policy.matched_rules.is_empty());
            } else {
                assert_eq!(policy.priority_mode, RoutingSetPriorityMode::GlobalKey);
                assert_eq!(policy.scheduling_mode, RoutingSchedulingMode::FixedOrder);
                assert_eq!(
                    policy
                        .ranking_overlay
                        .provider_priority_overrides
                        .get("provider-a"),
                    Some(&7)
                );
                assert_eq!(policy.matched_rules.len(), 1);
            }
        }
    }

    #[test]
    fn resolves_model_policy_and_matching_rule() {
        let config = RoutingGroupConfig {
            default_policy: RoutingDefaultPolicy::default(),
            model_policies: vec![RoutingModelPolicy {
                model: "gpt-5".to_string(),
                allowed_providers: vec!["provider-a".to_string()],
                provider_priority_overrides: BTreeMap::from([("provider-a".to_string(), 0)]),
                pool_priority_overrides: BTreeMap::from([("provider-a".to_string(), 3)]),
                ..RoutingModelPolicy::default()
            }],
            rules: vec![RoutingRule {
                id: "high".to_string(),
                priority: 10,
                enabled: true,
                phase: RoutingRulePhase::ClientRequest,
                conditions: RoutingCondition::Predicate {
                    field: "body.reasoning_effort".to_string(),
                    op: RoutingConditionOp::Eq,
                    value: Some(json!("high")),
                },
                actions: vec![RoutingAction::JsonPatchBody {
                    patch: vec![RoutingJsonPatchOperation::Add {
                        path: "/metadata/routing".to_string(),
                        value: json!("high"),
                    }],
                }],
                stop_processing: false,
            }],
        };

        let policy = resolve_routing_policy(
            &config,
            RoutingPolicyInput {
                group_id: Some("group-1"),
                group_version: Some(1),
                selection_source: "explicit",
                requested_model: "gpt-5",
                resolved_model: "gpt-5",
                api_format: "openai:chat",
                user_id: Some("user-1"),
                api_key_id: Some("api-key-1"),
                headers: &json!({}),
                body: &json!({"reasoning_effort":"high"}),
                phase: RoutingRulePhase::ClientRequest,
            },
        )
        .expect("policy should resolve");

        assert_eq!(policy.ranking_overlay.allowed_providers, vec!["provider-a"]);
        assert_eq!(
            policy
                .ranking_overlay
                .provider_priority_overrides
                .get("provider-a"),
            Some(&0)
        );
        assert_eq!(
            policy
                .ranking_overlay
                .pool_priority_overrides
                .get("provider-a"),
            Some(&3)
        );
        assert_eq!(policy.matched_rules.len(), 1);
        assert_eq!(policy.mutation_plan.body_patch.len(), 1);
    }

    #[test]
    fn default_policy_applies_to_models_without_an_override() {
        let config = RoutingGroupConfig {
            default_policy: RoutingDefaultPolicy {
                priority_mode: RoutingSetPriorityMode::GlobalKey,
                scheduling_mode: RoutingSchedulingMode::LoadBalance,
                keep_priority_on_conversion: true,
                sticky_key_attempts: 3,
                execution_policy: Default::default(),
            },
            model_policies: vec![RoutingModelPolicy {
                model: "special-model".to_string(),
                allowed_providers: vec!["provider-special".to_string()],
                provider_priority_overrides: BTreeMap::from([("provider-special".to_string(), 0)]),
                ..RoutingModelPolicy::default()
            }],
            rules: vec![],
        };

        let special = resolve_routing_policy(
            &config,
            RoutingPolicyInput {
                group_id: Some("group-1"),
                group_version: Some(1),
                selection_source: "test",
                requested_model: "special-model",
                resolved_model: "special-model",
                api_format: "openai:chat",
                user_id: None,
                api_key_id: None,
                headers: &json!({}),
                body: &json!({}),
                phase: RoutingRulePhase::ClientRequest,
            },
        )
        .expect("the specially configured model should resolve");

        assert_eq!(special.priority_mode, RoutingSetPriorityMode::GlobalKey);
        assert_eq!(special.scheduling_mode, RoutingSchedulingMode::LoadBalance);
        assert!(special.keep_priority_on_conversion);
        assert_eq!(special.sticky_key_attempts, 3);
        assert_eq!(
            special.ranking_overlay.allowed_providers,
            vec!["provider-special"]
        );
        assert_eq!(
            special
                .ranking_overlay
                .provider_priority_overrides
                .get("provider-special"),
            Some(&0)
        );

        let ordinary = resolve_routing_policy(
            &config,
            RoutingPolicyInput {
                group_id: Some("group-1"),
                group_version: Some(1),
                selection_source: "test",
                requested_model: "ordinary-model",
                resolved_model: "ordinary-model",
                api_format: "openai:chat",
                user_id: None,
                api_key_id: None,
                headers: &json!({}),
                body: &json!({}),
                phase: RoutingRulePhase::ClientRequest,
            },
        )
        .expect("an unconfigured model should keep using the default policy");

        assert_eq!(ordinary.priority_mode, RoutingSetPriorityMode::GlobalKey);
        assert_eq!(ordinary.scheduling_mode, RoutingSchedulingMode::LoadBalance);
        assert!(ordinary.keep_priority_on_conversion);
        assert_eq!(ordinary.sticky_key_attempts, 3);
        assert!(ordinary.ranking_overlay.allowed_providers.is_empty());
        assert!(ordinary.ranking_overlay.allowed_keys.is_empty());
        assert!(ordinary
            .ranking_overlay
            .provider_priority_overrides
            .is_empty());
    }

    #[test]
    fn legacy_group_model_allowlist_is_ignored() {
        let config: RoutingGroupConfig = serde_json::from_value(json!({
            "allowed_models": ["gpt-5"],
            "default_policy": {
                "priority_mode": "provider",
                "scheduling_mode": "cache_affinity"
            },
            "model_policies": [],
            "rules": []
        }))
        .expect("legacy routing config should remain readable");

        resolve_routing_policy(
            &config,
            RoutingPolicyInput {
                group_id: Some("group-1"),
                group_version: Some(1),
                selection_source: "system_default",
                requested_model: "claude-sonnet",
                resolved_model: "claude-sonnet",
                api_format: "openai:chat",
                user_id: None,
                api_key_id: None,
                headers: &json!({}),
                body: &json!({}),
                phase: RoutingRulePhase::ClientRequest,
            },
        )
        .expect("the legacy allowlist must not reject another model");
    }

    #[test]
    fn sticky_key_attempts_defaults_to_two_and_can_be_overridden_by_rule() {
        let default_config = RoutingGroupConfig::default();
        let default_policy = resolve_routing_policy(
            &default_config,
            RoutingPolicyInput {
                group_id: None,
                group_version: None,
                selection_source: "test",
                requested_model: "gpt-5",
                resolved_model: "gpt-5",
                api_format: "openai:chat",
                user_id: None,
                api_key_id: None,
                headers: &json!({}),
                body: &json!({}),
                phase: RoutingRulePhase::ClientRequest,
            },
        )
        .expect("default config should resolve");
        assert_eq!(
            default_policy.sticky_key_attempts,
            crate::DEFAULT_STICKY_KEY_ATTEMPTS
        );

        let parsed: RoutingGroupConfig =
            serde_json::from_value(json!({ "default_policy": { "priority_mode": "provider" } }))
                .expect("legacy config without sticky_key_attempts should deserialize");
        assert_eq!(
            parsed.default_policy.sticky_key_attempts,
            crate::DEFAULT_STICKY_KEY_ATTEMPTS
        );

        let config = RoutingGroupConfig {
            rules: vec![RoutingRule {
                id: "no-sticky-retry".to_string(),
                priority: 1,
                enabled: true,
                phase: RoutingRulePhase::ClientRequest,
                conditions: RoutingCondition::default(),
                actions: vec![RoutingAction::SetScheduling {
                    priority_mode: None,
                    scheduling_mode: None,
                    keep_priority_on_conversion: None,
                    sticky_key_attempts: Some(1),
                }],
                stop_processing: false,
            }],
            ..RoutingGroupConfig::default()
        };
        let policy = resolve_routing_policy(
            &config,
            RoutingPolicyInput {
                group_id: None,
                group_version: None,
                selection_source: "test",
                requested_model: "gpt-5",
                resolved_model: "gpt-5",
                api_format: "openai:chat",
                user_id: None,
                api_key_id: None,
                headers: &json!({}),
                body: &json!({}),
                phase: RoutingRulePhase::ClientRequest,
            },
        )
        .expect("rule config should resolve");
        assert_eq!(policy.sticky_key_attempts, 1);
    }

    #[test]
    fn restrict_model_action_rejects_matching_request() {
        let config = RoutingGroupConfig {
            rules: vec![RoutingRule {
                id: "restrict".to_string(),
                priority: 1,
                enabled: true,
                phase: RoutingRulePhase::ClientRequest,
                conditions: RoutingCondition::default(),
                actions: vec![RoutingAction::RestrictModels {
                    models: vec!["gpt-5".to_string()],
                }],
                stop_processing: false,
            }],
            ..RoutingGroupConfig::default()
        };

        let err = resolve_routing_policy(
            &config,
            RoutingPolicyInput {
                group_id: None,
                group_version: None,
                selection_source: "test",
                requested_model: "claude",
                resolved_model: "claude",
                api_format: "openai:chat",
                user_id: None,
                api_key_id: None,
                headers: &json!({}),
                body: &json!({}),
                phase: RoutingRulePhase::ClientRequest,
            },
        )
        .unwrap_err();

        assert_eq!(
            err,
            RoutingPolicyError::ModelNotAllowed("claude".to_string())
        );
    }
}
