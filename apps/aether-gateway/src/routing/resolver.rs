use aether_routing_core::{
    resolve_routing_policy, MutationPlan, RankingOverlay, ResolvedRoutingPolicy,
    RoutingDefaultPolicy, RoutingGroupConfig, RoutingPolicyError, RoutingPolicyInput,
    RoutingRulePhase, RoutingSchedulingMode, RoutingSetPriorityMode, DEFAULT_STICKY_KEY_ATTEMPTS,
};
use http::StatusCode;
use serde_json::Value;
use std::collections::BTreeMap;

use crate::GatewayError;

const INVALID_ROUTING_GROUP_CONFIG_MESSAGE: &str = "invalid routing group config";
const INVALID_ROUTING_MUTATION_MESSAGE: &str = "invalid routing mutation";
const ROUTING_MODEL_NOT_ALLOWED_MESSAGE: &str = "requested model is not allowed by routing policy";

#[derive(Debug, Clone)]
pub(crate) struct GatewayRoutingPolicyInput<'a> {
    pub group_id: Option<&'a str>,
    pub group_version: Option<i64>,
    pub group_config_json: &'a Value,
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

#[derive(Debug, Clone)]
pub(crate) struct GatewayStaticRoutingPolicyInput<'a> {
    pub group_id: Option<&'a str>,
    pub group_version: Option<i64>,
    pub group_config_json: &'a Value,
    pub selection_source: &'a str,
    pub requested_model: &'a str,
    pub resolved_model: &'a str,
}

pub(crate) fn resolve_gateway_routing_policy(
    input: GatewayRoutingPolicyInput<'_>,
) -> Result<ResolvedRoutingPolicy, GatewayError> {
    if let Some(policy) =
        resolve_gateway_static_default_routing_policy(GatewayStaticRoutingPolicyInput {
            group_id: input.group_id,
            group_version: input.group_version,
            group_config_json: input.group_config_json,
            selection_source: input.selection_source,
            requested_model: input.requested_model,
            resolved_model: input.resolved_model,
        })?
    {
        return Ok(policy);
    }

    let config = serde_json::from_value::<RoutingGroupConfig>(input.group_config_json.clone())
        .map_err(|_| invalid_routing_group_config())?;
    let policy = resolve_routing_policy(
        &config,
        RoutingPolicyInput {
            group_id: input.group_id,
            group_version: input.group_version,
            selection_source: input.selection_source,
            requested_model: input.requested_model,
            resolved_model: input.resolved_model,
            api_format: input.api_format,
            user_id: input.user_id,
            api_key_id: input.api_key_id,
            headers: input.headers,
            body: input.body,
            phase: input.phase,
        },
    )
    .map_err(routing_policy_error)?;
    crate::request_lifecycle::configure_client_disconnect(policy.execution_policy.clone());
    Ok(policy)
}

pub(crate) fn resolve_gateway_static_default_routing_policy(
    input: GatewayStaticRoutingPolicyInput<'_>,
) -> Result<Option<ResolvedRoutingPolicy>, GatewayError> {
    let Some(default_policy) = static_default_policy_fields(input.group_config_json)? else {
        return Ok(None);
    };
    crate::request_lifecycle::configure_client_disconnect(default_policy.execution_policy.clone());

    Ok(Some(ResolvedRoutingPolicy {
        group_id: input.group_id.map(str::to_string),
        group_version: input.group_version,
        selection_source: input.selection_source.to_string(),
        requested_model: input.requested_model.to_string(),
        resolved_model: input.resolved_model.to_string(),
        priority_mode: default_policy.priority_mode,
        scheduling_mode: default_policy.scheduling_mode,
        keep_priority_on_conversion: default_policy.keep_priority_on_conversion,
        sticky_key_attempts: default_policy.sticky_key_attempts,
        execution_policy: default_policy.execution_policy,
        ranking_overlay: RankingOverlay::default(),
        mutation_plan: MutationPlan::default(),
        pool_policy_overrides: BTreeMap::new(),
        matched_rules: Vec::new(),
    }))
}

fn static_default_policy_fields(
    config_json: &Value,
) -> Result<Option<RoutingDefaultPolicy>, GatewayError> {
    let Some(object) = config_json.as_object() else {
        return Ok(None);
    };
    // A strategy's default policy applies to every model. Only model policies
    // and rules require the request-context-aware resolver; unknown legacy
    // fields (including the removed group allowlist) are intentionally ignored.
    if !routing_array_field_is_missing_or_empty(object, "model_policies")
        || !routing_array_field_is_missing_or_empty(object, "rules")
    {
        return Ok(None);
    }

    let Some(default_policy) = object.get("default_policy") else {
        return Ok(Some(RoutingDefaultPolicy::default()));
    };
    let Some(default_policy) = default_policy.as_object() else {
        return Ok(None);
    };

    let priority_mode = routing_enum_field(
        default_policy.get("priority_mode"),
        RoutingSetPriorityMode::default,
    )?;
    let scheduling_mode = routing_enum_field(
        default_policy.get("scheduling_mode"),
        RoutingSchedulingMode::default,
    )?;
    let keep_priority_on_conversion = match default_policy.get("keep_priority_on_conversion") {
        Some(value) => value.as_bool().ok_or_else(invalid_routing_group_config)?,
        None => false,
    };
    let sticky_key_attempts = match default_policy.get("sticky_key_attempts") {
        Some(value) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(invalid_routing_group_config)?,
        None => DEFAULT_STICKY_KEY_ATTEMPTS,
    };
    let execution_policy: aether_routing_core::RoutingExecutionPolicy =
        serde_json::from_value(Value::Object(default_policy.clone()))
            .map_err(|_| invalid_routing_group_config())?;
    aether_routing_core::validate_routing_failover_rules(&execution_policy.failover_rules)
        .map_err(|_| invalid_routing_group_config())?;

    Ok(Some(RoutingDefaultPolicy {
        priority_mode,
        scheduling_mode,
        keep_priority_on_conversion,
        sticky_key_attempts,
        execution_policy,
    }))
}

fn routing_array_field_is_missing_or_empty(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> bool {
    match object.get(key) {
        None => true,
        Some(Value::Array(values)) => values.is_empty(),
        Some(_) => false,
    }
}

fn routing_enum_field<T>(
    value: Option<&Value>,
    default: impl FnOnce() -> T,
) -> Result<T, GatewayError>
where
    T: serde::de::DeserializeOwned,
{
    match value {
        Some(value) => {
            serde_json::from_value(value.clone()).map_err(|_| invalid_routing_group_config())
        }
        None => Ok(default()),
    }
}

fn routing_policy_error(error: RoutingPolicyError) -> GatewayError {
    let message = match error {
        RoutingPolicyError::InvalidConfig(_) => INVALID_ROUTING_GROUP_CONFIG_MESSAGE,
        RoutingPolicyError::InvalidMutation(_) => INVALID_ROUTING_MUTATION_MESSAGE,
        RoutingPolicyError::ModelNotAllowed(_) => ROUTING_MODEL_NOT_ALLOWED_MESSAGE,
    };
    GatewayError::Client {
        status: StatusCode::BAD_REQUEST,
        message: message.to_string(),
    }
}

fn invalid_routing_group_config() -> GatewayError {
    GatewayError::Client {
        status: StatusCode::BAD_REQUEST,
        message: INVALID_ROUTING_GROUP_CONFIG_MESSAGE.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn static_default_policy_matches_full_resolver_without_body_context() {
        let config = json!({
            "default_policy": {
                "priority_mode": "global_key",
                "scheduling_mode": "load_balance",
                "keep_priority_on_conversion": true,
                "cancel_on_client_disconnect": true,
                "max_transfer_count": 3,
                "max_transfer_timeout_seconds": 90,
                "failover_rules": {
                    "success_failover_patterns": [{"pattern": "(?i)capacity.*exhausted"}],
                    "error_stop_patterns": [{"status_codes": [400]}]
                }
            },
            "allowed_models": ["legacy-model"],
            "model_policies": [],
            "rules": []
        });

        let static_policy =
            resolve_gateway_static_default_routing_policy(GatewayStaticRoutingPolicyInput {
                group_id: Some("group-1"),
                group_version: Some(7),
                group_config_json: &config,
                selection_source: "system_default",
                requested_model: "mock-model",
                resolved_model: "mock-model",
            })
            .expect("static default policy should resolve")
            .expect("static default policy should be detected");

        let full_policy = resolve_gateway_routing_policy(GatewayRoutingPolicyInput {
            group_id: Some("group-1"),
            group_version: Some(7),
            group_config_json: &config,
            selection_source: "system_default",
            requested_model: "mock-model",
            resolved_model: "mock-model",
            api_format: "openai:chat",
            user_id: Some("user-1"),
            api_key_id: Some("key-1"),
            headers: &json!({"x-test": "value"}),
            body: &json!({"model": "mock-model"}),
            phase: RoutingRulePhase::ClientRequest,
        })
        .expect("full policy should resolve");

        assert_eq!(static_policy, full_policy);
        assert_eq!(static_policy.execution_policy.max_transfer_count, 3);
        assert_eq!(
            static_policy.execution_policy.max_transfer_timeout_seconds,
            90
        );
        assert_eq!(
            static_policy
                .execution_policy
                .failover_rules
                .error_stop_patterns
                .len(),
            1
        );
        assert_eq!(
            static_policy.priority_mode,
            RoutingSetPriorityMode::GlobalKey
        );
        assert_eq!(
            static_policy.scheduling_mode,
            RoutingSchedulingMode::LoadBalance
        );
        assert!(static_policy.keep_priority_on_conversion);
        assert!(static_policy.mutation_plan.is_empty());
        assert!(static_policy.matched_rules.is_empty());
    }

    #[test]
    fn dynamic_routing_config_is_not_static_default() {
        let config = json!({
            "rules": [{
                "id": "rule-1",
                "conditions": {},
                "actions": [{
                    "type": "restrict_providers",
                    "provider_ids": ["provider-1"]
                }]
            }]
        });

        let policy =
            resolve_gateway_static_default_routing_policy(GatewayStaticRoutingPolicyInput {
                group_id: Some("group-1"),
                group_version: Some(1),
                group_config_json: &config,
                selection_source: "system_default",
                requested_model: "mock-model",
                resolved_model: "mock-model",
            })
            .expect("dynamic config should not fail static detection");

        assert!(policy.is_none());
    }

    #[test]
    fn routing_config_errors_do_not_echo_config_values() {
        let secret = "https://internal.example/?token=Bearer-secret";
        let config = json!({
            "default_policy": {
                "priority_mode": secret
            }
        });

        let error =
            resolve_gateway_static_default_routing_policy(GatewayStaticRoutingPolicyInput {
                group_id: Some("group-1"),
                group_version: Some(1),
                group_config_json: &config,
                selection_source: "system_default",
                requested_model: "mock-model",
                resolved_model: "mock-model",
            })
            .expect_err("invalid config should fail");

        assert!(matches!(
            error,
            GatewayError::Client {
                status: StatusCode::BAD_REQUEST,
                ref message,
            } if message == INVALID_ROUTING_GROUP_CONFIG_MESSAGE && !message.contains(secret)
        ));
    }

    #[test]
    fn routing_policy_errors_do_not_echo_requested_model() {
        let secret = "model?token=Bearer-secret";
        let config = json!({
            "rules": [{
                "id": "restrict-model",
                "conditions": {},
                "actions": [{
                    "type": "restrict_models",
                    "models": ["allowed-model"]
                }]
            }]
        });

        let error = resolve_gateway_routing_policy(GatewayRoutingPolicyInput {
            group_id: Some("group-1"),
            group_version: Some(1),
            group_config_json: &config,
            selection_source: "system_default",
            requested_model: secret,
            resolved_model: secret,
            api_format: "openai:chat",
            user_id: Some("user-1"),
            api_key_id: Some("key-1"),
            headers: &json!({}),
            body: &json!({"model": secret}),
            phase: RoutingRulePhase::ClientRequest,
        })
        .expect_err("disallowed model should fail");

        assert!(matches!(
            error,
            GatewayError::Client {
                status: StatusCode::BAD_REQUEST,
                ref message,
            } if message == ROUTING_MODEL_NOT_ALLOWED_MESSAGE && !message.contains(secret)
        ));
    }
}
