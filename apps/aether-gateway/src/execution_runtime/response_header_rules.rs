use std::collections::BTreeMap;
use std::time::Duration;

use aether_contracts::ExecutionPlan;
use serde_json::{Map, Value};
use tracing::warn;

use crate::{AppState, GatewayError};

const RESPONSE_HEADER_RULES_KEY: &str = "response_header_rules";
const RESPONSE_HEADER_RULES_CAMEL_KEY: &str = "responseHeaderRules";
const PROVIDER_RESPONSE_HEADERS_CONTEXT_KEY: &str = "provider_response_headers";
const RESPONSE_HEADER_RULE_PROTECTED_KEYS: &[&str] = &["content-length"];
const RESPONSE_HEADER_RULES_CACHE_TTL: Duration = Duration::from_secs(5);

fn endpoint_response_header_rules_from_config(config: Option<&Value>) -> Option<&Value> {
    let config = config?.as_object()?;
    config
        .get(RESPONSE_HEADER_RULES_KEY)
        .or_else(|| config.get(RESPONSE_HEADER_RULES_CAMEL_KEY))
        .filter(|value| !value.is_null())
}

async fn read_endpoint_response_header_rules(state: &AppState, endpoint_id: &str) -> Option<Value> {
    let endpoint_id = endpoint_id.trim();
    if endpoint_id.is_empty() {
        return None;
    }
    let endpoint_id = endpoint_id.to_string();

    let endpoint_id_for_load = endpoint_id.clone();
    match state
        .endpoint_response_header_rules_cache
        .get_or_load(endpoint_id, RESPONSE_HEADER_RULES_CACHE_TTL, || async {
            state
                .read_provider_catalog_endpoints_by_ids(std::slice::from_ref(&endpoint_id_for_load))
                .await
                .map(|endpoints| {
                    endpoints.into_iter().next().and_then(|endpoint| {
                        endpoint_response_header_rules_from_config(endpoint.config.as_ref())
                            .cloned()
                    })
                })
        })
        .await
    {
        Ok(rules) => rules,
        Err(err) => {
            warn!(
                event_name = "response_header_rules_endpoint_read_failed",
                log_type = "ops",
                endpoint_id = %endpoint_id_for_load,
                error = ?err,
                "gateway failed to read endpoint response header rules; skipping response header edits"
            );
            None
        }
    }
}

pub(crate) async fn apply_endpoint_response_header_rules(
    state: &AppState,
    plan: &ExecutionPlan,
    headers: &mut BTreeMap<String, String>,
    response_body: Option<&Value>,
) -> Result<(), GatewayError> {
    let Some(rules) = read_endpoint_response_header_rules(state, plan.endpoint_id.as_str()).await
    else {
        return Ok(());
    };

    if !rules.is_array() {
        warn!(
            event_name = "response_header_rules_invalid_shape",
            log_type = "ops",
            endpoint_id = %plan.endpoint_id,
            "gateway skipped endpoint response header rules because response_header_rules is not an array"
        );
        return Ok(());
    }

    let empty_body = Value::Null;
    let body = response_body.unwrap_or(&empty_body);
    if !crate::provider_transport::apply_local_header_rules(
        headers,
        Some(&rules),
        RESPONSE_HEADER_RULE_PROTECTED_KEYS,
        body,
        response_body,
    ) {
        return Err(GatewayError::Internal(
            "response_header_rules 应用失败".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn attach_provider_response_headers_to_report_context(
    report_context: Option<Value>,
    provider_headers: &BTreeMap<String, String>,
) -> Option<Value> {
    let provider_headers = serde_json::to_value(provider_headers).ok()?;
    let mut object = match report_context {
        Some(Value::Object(object)) => object,
        Some(other) => Map::from_iter([("seed".to_string(), other)]),
        None => Map::new(),
    };
    object.insert(
        PROVIDER_RESPONSE_HEADERS_CONTEXT_KEY.to_string(),
        provider_headers,
    );
    Some(Value::Object(object))
}
