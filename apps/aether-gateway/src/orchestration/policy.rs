use std::collections::BTreeSet;

use aether_contracts::ExecutionPlan;
use serde_json::{json, Value};
use tracing::debug;

use crate::provider_transport::GatewayProviderTransportSnapshot;
use crate::AppState;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LocalFailoverPolicy {
    pub(crate) max_retries: Option<u64>,
    pub(crate) stop_status_codes: BTreeSet<u16>,
    pub(crate) continue_status_codes: BTreeSet<u16>,
    pub(crate) success_failover_patterns: Vec<LocalFailoverRegexRule>,
    pub(crate) error_stop_patterns: Vec<LocalFailoverRegexRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LocalFailoverRegexRule {
    pub(crate) pattern: String,
    pub(crate) status_codes: BTreeSet<u16>,
}

pub(crate) async fn resolve_local_failover_policy(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
) -> LocalFailoverPolicy {
    if let Some(policy) = local_failover_policy_from_report_context(report_context) {
        debug!(
            event_name = "local_failover_policy_loaded",
            log_type = "debug",
            request_id = %plan.request_id,
            provider_id = %plan.provider_id,
            endpoint_id = %plan.endpoint_id,
            key_id = %plan.key_id,
            source = "report_context",
            max_retries = ?policy.max_retries,
            stop_status_code_count = policy.stop_status_codes.len(),
            continue_status_code_count = policy.continue_status_codes.len(),
            success_failover_pattern_count = policy.success_failover_patterns.len(),
            error_stop_pattern_count = policy.error_stop_patterns.len(),
            "gateway loaded local failover policy from report context"
        );
        return policy;
    }

    let transport = match state
        .read_provider_transport_snapshot(&plan.provider_id, &plan.endpoint_id, &plan.key_id)
        .await
    {
        Ok(Some(transport)) => transport,
        Ok(None) | Err(_) => return LocalFailoverPolicy::default(),
    };
    let policy = local_failover_policy_from_transport(&transport);
    debug!(
        event_name = "local_failover_policy_loaded",
        log_type = "debug",
        request_id = %plan.request_id,
        provider_id = %plan.provider_id,
        endpoint_id = %plan.endpoint_id,
        key_id = %plan.key_id,
        source = "transport_snapshot",
        max_retries = ?policy.max_retries,
        stop_status_code_count = policy.stop_status_codes.len(),
        continue_status_code_count = policy.continue_status_codes.len(),
        success_failover_pattern_count = policy.success_failover_patterns.len(),
        error_stop_pattern_count = policy.error_stop_patterns.len(),
        "gateway loaded local failover policy from transport snapshot"
    );
    policy
}

pub(crate) fn local_failover_policy_from_transport(
    transport: &GatewayProviderTransportSnapshot,
) -> LocalFailoverPolicy {
    let rules = transport
        .provider
        .config
        .as_ref()
        .and_then(|config| config.get("failover_rules"))
        .and_then(Value::as_object);
    let max_retries = rules
        .and_then(|value| value.get("max_retries"))
        .and_then(parse_u64_value)
        .or_else(|| {
            transport
                .endpoint
                .max_retries
                .and_then(|value| u64::try_from(value).ok())
        })
        .or_else(|| {
            transport
                .provider
                .max_retries
                .and_then(|value| u64::try_from(value).ok())
        });

    LocalFailoverPolicy {
        max_retries,
        stop_status_codes: rules
            .map(|value| {
                parse_status_code_set(
                    value,
                    &[
                        "stop_on_status_codes",
                        "early_stop_status_codes",
                        "non_retryable_status_codes",
                        "stop_status_codes",
                    ],
                )
            })
            .unwrap_or_default(),
        continue_status_codes: rules
            .map(|value| {
                parse_status_code_set(
                    value,
                    &[
                        "continue_on_status_codes",
                        "retryable_status_codes",
                        "retry_on_status_codes",
                        "continue_status_codes",
                    ],
                )
            })
            .unwrap_or_default(),
        success_failover_patterns: rules
            .map(|value| parse_regex_rules(value, "success_failover_patterns"))
            .unwrap_or_default(),
        error_stop_patterns: rules
            .map(|value| parse_regex_rules(value, "error_stop_patterns"))
            .unwrap_or_default(),
    }
}

pub(crate) fn local_failover_policy_from_report_context(
    report_context: Option<&Value>,
) -> Option<LocalFailoverPolicy> {
    let object = report_context
        .and_then(Value::as_object)?
        .get("local_failover_policy")?
        .as_object()?;

    Some(LocalFailoverPolicy {
        max_retries: object.get("max_retries").and_then(parse_u64_value),
        stop_status_codes: object
            .get("stop_status_codes")
            .map(parse_status_code_list)
            .unwrap_or_default(),
        continue_status_codes: object
            .get("continue_status_codes")
            .map(parse_status_code_list)
            .unwrap_or_default(),
        success_failover_patterns: parse_regex_rules(object, "success_failover_patterns"),
        error_stop_patterns: parse_regex_rules(object, "error_stop_patterns"),
    })
}

pub(crate) fn append_local_failover_policy_to_value(
    value: Value,
    transport: &GatewayProviderTransportSnapshot,
) -> Value {
    let Value::Object(mut object) = value else {
        return value;
    };
    object.insert(
        "local_failover_policy".to_string(),
        local_failover_policy_to_value(&local_failover_policy_from_transport(transport)),
    );
    Value::Object(object)
}

fn parse_status_code_list(value: &Value) -> BTreeSet<u16> {
    value
        .as_array()
        .into_iter()
        .flat_map(|values| values.iter())
        .filter_map(|value| parse_u64_value(value).and_then(|value| u16::try_from(value).ok()))
        .collect()
}

fn local_failover_policy_to_value(policy: &LocalFailoverPolicy) -> Value {
    json!({
        "max_retries": policy.max_retries,
        "stop_status_codes": policy.stop_status_codes.iter().copied().collect::<Vec<_>>(),
        "continue_status_codes": policy.continue_status_codes.iter().copied().collect::<Vec<_>>(),
        "success_failover_patterns": policy.success_failover_patterns.iter().map(local_failover_regex_rule_to_value).collect::<Vec<_>>(),
        "error_stop_patterns": policy.error_stop_patterns.iter().map(local_failover_regex_rule_to_value).collect::<Vec<_>>(),
    })
}

fn local_failover_regex_rule_to_value(rule: &LocalFailoverRegexRule) -> Value {
    json!({
        "pattern": rule.pattern,
        "status_codes": rule.status_codes.iter().copied().collect::<Vec<_>>(),
    })
}

fn parse_regex_rules(
    rules: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Vec<LocalFailoverRegexRule> {
    rules
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flat_map(|items| items.iter())
        .filter_map(parse_regex_rule)
        .collect()
}

fn parse_regex_rule(value: &serde_json::Value) -> Option<LocalFailoverRegexRule> {
    let object = value.as_object()?;
    let pattern = object
        .get("pattern")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(LocalFailoverRegexRule {
        pattern: pattern.to_string(),
        status_codes: object
            .get("status_codes")
            .and_then(Value::as_array)
            .into_iter()
            .flat_map(|values| values.iter())
            .filter_map(|value| parse_u64_value(value).and_then(|value| u16::try_from(value).ok()))
            .collect(),
    })
}

fn parse_status_code_set(
    rules: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> BTreeSet<u16> {
    keys.iter()
        .filter_map(|key| rules.get(*key))
        .filter_map(Value::as_array)
        .flat_map(|values| values.iter())
        .filter_map(|value| parse_u64_value(value).and_then(|value| u16::try_from(value).ok()))
        .collect()
}

fn parse_u64_value(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        append_local_failover_policy_to_value, local_failover_policy_from_report_context,
        LocalFailoverPolicy, LocalFailoverRegexRule,
    };
    use crate::provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_transport(
        provider_max_retries: Option<i32>,
        endpoint_max_retries: Option<i32>,
        provider_config: Option<serde_json::Value>,
    ) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "OpenAI".to_string(),
                provider_type: "llm".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: true,
                concurrent_limit: None,
                max_retries: provider_max_retries,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: provider_config,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "openai:chat".to_string(),
                api_family: Some("openai".to_string()),
                endpoint_kind: Some("chat".to_string()),
                is_active: true,
                base_url: "https://example.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: endpoint_max_retries,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "primary".to_string(),
                auth_type: "bearer".to_string(),
                is_active: true,
                api_formats: None,
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,

                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    #[test]
    fn append_local_failover_policy_to_value_round_trips_policy_shape() {
        let report_context = append_local_failover_policy_to_value(
            json!({
                "request_id": "req-1",
            }),
            &sample_transport(
                Some(5),
                Some(4),
                Some(json!({
                    "failover_rules": {
                        "max_retries": 2,
                        "continue_status_codes": [429],
                        "stop_status_codes": [400],
                        "success_failover_patterns": [{"pattern": "quota", "status_codes": [200]}],
                        "error_stop_patterns": [{"pattern": "validation", "status_codes": [422]}]
                    }
                })),
            ),
        );

        assert_eq!(
            local_failover_policy_from_report_context(Some(&report_context)),
            Some(LocalFailoverPolicy {
                max_retries: Some(2),
                stop_status_codes: [400].into_iter().collect(),
                continue_status_codes: [429].into_iter().collect(),
                success_failover_patterns: vec![LocalFailoverRegexRule {
                    pattern: "quota".to_string(),
                    status_codes: [200].into_iter().collect(),
                }],
                error_stop_patterns: vec![LocalFailoverRegexRule {
                    pattern: "validation".to_string(),
                    status_codes: [422].into_iter().collect(),
                }],
            })
        );
    }
}
