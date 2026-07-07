use aether_routing_core::{
    resolve_routing_policy, MutationPlan, RankingOverlay, ResolvedRoutingPolicy,
    RoutingGroupConfig, RoutingPolicyInput, RoutingRulePhase, RoutingSchedulingMode,
    RoutingSetPriorityMode,
};
use http::StatusCode;
use serde_json::Value;
use std::collections::BTreeMap;

use crate::GatewayError;

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
        .map_err(|err| GatewayError::Client {
            status: StatusCode::BAD_REQUEST,
            message: format!("invalid routing group config: {err}"),
        })?;
    resolve_routing_policy(
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
    .map_err(|err| GatewayError::Client {
        status: StatusCode::BAD_REQUEST,
        message: err.to_string(),
    })
}

pub(crate) fn resolve_gateway_static_default_routing_policy(
    input: GatewayStaticRoutingPolicyInput<'_>,
) -> Result<Option<ResolvedRoutingPolicy>, GatewayError> {
    let Some((priority_mode, scheduling_mode, keep_priority_on_conversion)) =
        static_default_policy_fields(input.group_config_json)?
    else {
        return Ok(None);
    };

    Ok(Some(ResolvedRoutingPolicy {
        group_id: input.group_id.map(str::to_string),
        group_version: input.group_version,
        selection_source: input.selection_source.to_string(),
        requested_model: input.requested_model.to_string(),
        resolved_model: input.resolved_model.to_string(),
        priority_mode,
        scheduling_mode,
        keep_priority_on_conversion,
        ranking_overlay: RankingOverlay::default(),
        mutation_plan: MutationPlan::default(),
        pool_policy_overrides: BTreeMap::new(),
        matched_rules: Vec::new(),
    }))
}

fn static_default_policy_fields(
    config_json: &Value,
) -> Result<Option<(RoutingSetPriorityMode, RoutingSchedulingMode, bool)>, GatewayError> {
    let Some(object) = config_json.as_object() else {
        return Ok(None);
    };
    if !routing_array_field_is_missing_or_empty(object, "allowed_models")
        || !routing_array_field_is_missing_or_empty(object, "model_policies")
        || !routing_array_field_is_missing_or_empty(object, "rules")
    {
        return Ok(None);
    }

    let Some(default_policy) = object.get("default_policy") else {
        return Ok(Some((
            RoutingSetPriorityMode::default(),
            RoutingSchedulingMode::default(),
            false,
        )));
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
        Some(value) => value.as_bool().ok_or_else(|| {
            invalid_routing_group_config("keep_priority_on_conversion must be a boolean")
        })?,
        None => false,
    };

    Ok(Some((
        priority_mode,
        scheduling_mode,
        keep_priority_on_conversion,
    )))
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
        Some(value) => serde_json::from_value(value.clone()).map_err(|err| {
            invalid_routing_group_config(format!("invalid default routing policy: {err}"))
        }),
        None => Ok(default()),
    }
}

fn invalid_routing_group_config(message: impl Into<String>) -> GatewayError {
    GatewayError::Client {
        status: StatusCode::BAD_REQUEST,
        message: format!("invalid routing group config: {}", message.into()),
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
                "keep_priority_on_conversion": true
            },
            "allowed_models": [],
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
}
