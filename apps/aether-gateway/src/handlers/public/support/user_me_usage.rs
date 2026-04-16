use std::collections::{BTreeMap, BTreeSet};

use aether_billing::{
    normalize_input_tokens_for_billing, normalize_total_input_context_for_cache_hit_rate,
};
use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UsageAuditListQuery};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde_json::json;

use crate::admin_api::AdminAppState;

use super::{
    admin_stats_bad_request_response, build_auth_error_response, build_auth_wallet_summary_payload,
    list_usage_for_optional_range, parse_bounded_u32, query_param_value,
    resolve_authenticated_local_user, round_to, unix_secs_to_rfc3339, AdminStatsTimeRange,
    AdminStatsUsageFilter, AppState, GatewayPublicRequestContext,
};

const USERS_ME_USAGE_DATA_UNAVAILABLE_DETAIL: &str = "用户用量数据暂不可用";

fn build_users_me_usage_reader_unavailable_response() -> Response<Body> {
    build_auth_error_response(
        http::StatusCode::SERVICE_UNAVAILABLE,
        USERS_ME_USAGE_DATA_UNAVAILABLE_DETAIL,
        false,
    )
}

fn parse_users_me_usage_limit(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "limit") {
        Some(value) => {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| "limit must be an integer between 1 and 200".to_string())?;
            if (1..=200).contains(&parsed) {
                Ok(parsed)
            } else {
                Err("limit must be an integer between 1 and 200".to_string())
            }
        }
        None => Ok(100),
    }
}

fn parse_users_me_usage_offset(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "offset") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| "offset must be a non-negative integer".to_string()),
        None => Ok(0),
    }
}

fn parse_users_me_usage_hours(query: Option<&str>) -> Result<u32, String> {
    match query_param_value(query, "hours") {
        Some(value) => parse_bounded_u32("hours", &value, 1, 720),
        None => Ok(24),
    }
}

fn parse_users_me_usage_timeline_limit(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "limit") {
        Some(value) => {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| "limit must be an integer between 100 and 20000".to_string())?;
            if (100..=20_000).contains(&parsed) {
                Ok(parsed)
            } else {
                Err("limit must be an integer between 100 and 20000".to_string())
            }
        }
        None => Ok(2_000),
    }
}

fn parse_users_me_usage_ids(query: Option<&str>) -> Option<BTreeSet<String>> {
    let ids = query_param_value(query, "ids")?;
    let values = ids
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    (!values.is_empty()).then_some(values)
}

fn users_me_usage_cache_creation_tokens(item: &StoredRequestUsageAudit) -> u64 {
    let classified = item
        .cache_creation_ephemeral_5m_input_tokens
        .saturating_add(item.cache_creation_ephemeral_1h_input_tokens);
    if item.cache_creation_input_tokens == 0 && classified > 0 {
        classified
    } else {
        item.cache_creation_input_tokens
    }
}

fn users_me_usage_total_input_context(item: &StoredRequestUsageAudit) -> u64 {
    let api_format = item
        .endpoint_api_format
        .as_deref()
        .or(item.api_format.as_deref());
    let input_tokens = i64::try_from(item.input_tokens).unwrap_or(i64::MAX);
    let cache_creation_tokens =
        i64::try_from(users_me_usage_cache_creation_tokens(item)).unwrap_or(i64::MAX);
    let cache_read_tokens = i64::try_from(item.cache_read_input_tokens).unwrap_or(i64::MAX);
    normalize_total_input_context_for_cache_hit_rate(
        api_format,
        input_tokens,
        cache_creation_tokens,
        cache_read_tokens,
    ) as u64
}

fn users_me_usage_effective_input_tokens(item: &StoredRequestUsageAudit) -> u64 {
    let api_format = item
        .endpoint_api_format
        .as_deref()
        .or(item.api_format.as_deref());
    let input_tokens = i64::try_from(item.input_tokens).unwrap_or(i64::MAX);
    let cache_read_tokens = i64::try_from(item.cache_read_input_tokens).unwrap_or(i64::MAX);
    normalize_input_tokens_for_billing(api_format, input_tokens, cache_read_tokens) as u64
}

fn users_me_usage_effective_unix_secs(item: &StoredRequestUsageAudit) -> u64 {
    item.finalized_at_unix_secs
        .unwrap_or(item.created_at_unix_ms)
}

fn users_me_usage_cache_hit_rate(total_input_context: u64, cache_read_tokens: u64) -> f64 {
    if total_input_context == 0 {
        0.0
    } else {
        round_to(
            cache_read_tokens as f64 / total_input_context as f64 * 100.0,
            2,
        )
    }
}

fn users_me_usage_api_key_name(
    item: &StoredRequestUsageAudit,
    api_key_names: &BTreeMap<String, String>,
    auth_api_key_reader_available: bool,
) -> Option<String> {
    item.api_key_id
        .as_ref()
        .and_then(|api_key_id| api_key_names.get(api_key_id))
        .cloned()
        .or_else(|| {
            (!auth_api_key_reader_available)
                .then(|| item.api_key_name.clone())
                .flatten()
        })
}

fn users_me_usage_matches_search(
    item: &StoredRequestUsageAudit,
    search: Option<&str>,
    api_key_names: &BTreeMap<String, String>,
    auth_api_key_reader_available: bool,
) -> bool {
    let Some(search) = search.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };

    let model = item.model.to_ascii_lowercase();
    let api_key_name =
        users_me_usage_api_key_name(item, api_key_names, auth_api_key_reader_available)
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
    search.split_whitespace().all(|keyword| {
        let keyword = keyword.to_ascii_lowercase();
        model.contains(&keyword) || api_key_name.contains(&keyword)
    })
}

fn build_users_me_usage_api_key_payload(
    item: &StoredRequestUsageAudit,
    api_key_names: &BTreeMap<String, String>,
    auth_api_key_reader_available: bool,
) -> serde_json::Value {
    let api_key_name =
        users_me_usage_api_key_name(item, api_key_names, auth_api_key_reader_available);
    match item.api_key_id.as_deref() {
        Some(api_key_id) => json!({
            "id": api_key_id,
            "name": api_key_name.clone(),
            "display": api_key_name.unwrap_or_else(|| api_key_id.to_string()),
        }),
        None => serde_json::Value::Null,
    }
}

fn build_users_me_usage_record_payload(
    item: &StoredRequestUsageAudit,
    include_actual_cost: bool,
    api_key_names: &BTreeMap<String, String>,
    auth_api_key_reader_available: bool,
) -> serde_json::Value {
    let input_price_per_1m = item.settlement_input_price_per_1m();
    let output_price_per_1m = item.settlement_output_price_per_1m();
    let cache_creation_price_per_1m = item.settlement_cache_creation_price_per_1m();
    let cache_read_price_per_1m = item.settlement_cache_read_price_per_1m();
    let rate_multiplier = item.settlement_rate_multiplier();
    let mut payload = json!({
        "id": item.id,
        "model": item.model,
        "target_model": serde_json::Value::Null,
        "api_format": item.api_format,
        "endpoint_api_format": item.endpoint_api_format,
        "has_format_conversion": item.has_format_conversion,
        "input_tokens": item.input_tokens,
        "effective_input_tokens": users_me_usage_effective_input_tokens(item),
        "output_tokens": item.output_tokens,
        "total_tokens": item.total_tokens,
        "cost": round_to(item.total_cost_usd, 6),
        "response_time_ms": item.response_time_ms,
        "first_byte_time_ms": item.first_byte_time_ms,
        "is_stream": item.is_stream,
        "status": item.status,
        "created_at": unix_secs_to_rfc3339(item.created_at_unix_ms),
        "cache_creation_input_tokens": item.cache_creation_input_tokens,
        "cache_creation_ephemeral_5m_input_tokens": item.cache_creation_ephemeral_5m_input_tokens,
        "cache_creation_ephemeral_1h_input_tokens": item.cache_creation_ephemeral_1h_input_tokens,
        "cache_read_input_tokens": item.cache_read_input_tokens,
        "status_code": item.status_code,
        "error_message": item.error_message,
        "input_price_per_1m": input_price_per_1m,
        "output_price_per_1m": output_price_per_1m,
        "cache_creation_price_per_1m": cache_creation_price_per_1m,
        "cache_read_price_per_1m": cache_read_price_per_1m,
        "api_key": build_users_me_usage_api_key_payload(
            item,
            api_key_names,
            auth_api_key_reader_available,
        ),
    });

    if item.target_model.is_some() {
        payload["target_model"] = json!(item.target_model.clone());
    }
    if include_actual_cost {
        payload["actual_cost"] = json!(round_to(item.actual_total_cost_usd, 6));
        payload["rate_multiplier"] = json!(rate_multiplier);
    }
    payload
}

fn build_users_me_usage_active_payload(item: &StoredRequestUsageAudit) -> serde_json::Value {
    let mut payload = json!({
        "id": item.id,
        "status": item.status,
        "input_tokens": item.input_tokens,
        "effective_input_tokens": users_me_usage_effective_input_tokens(item),
        "output_tokens": item.output_tokens,
        "cache_creation_input_tokens": item.cache_creation_input_tokens,
        "cache_creation_ephemeral_5m_input_tokens": item.cache_creation_ephemeral_5m_input_tokens,
        "cache_creation_ephemeral_1h_input_tokens": item.cache_creation_ephemeral_1h_input_tokens,
        "cache_read_input_tokens": item.cache_read_input_tokens,
        "cost": round_to(item.total_cost_usd, 6),
        "actual_cost": round_to(item.actual_total_cost_usd, 6),
        "rate_multiplier": item.settlement_rate_multiplier(),
        "response_time_ms": item.response_time_ms,
        "first_byte_time_ms": item.first_byte_time_ms,
        "api_format": item.api_format,
        "endpoint_api_format": item.endpoint_api_format,
        "has_format_conversion": item.has_format_conversion,
        "target_model": item.target_model,
    });
    if item.api_format.is_none() {
        payload
            .as_object_mut()
            .expect("object")
            .remove("api_format");
    }
    if item.endpoint_api_format.is_none() {
        payload
            .as_object_mut()
            .expect("object")
            .remove("endpoint_api_format");
    }
    if item.target_model.is_none() {
        payload
            .as_object_mut()
            .expect("object")
            .remove("target_model");
    }
    payload
}

fn build_users_me_usage_summary_by_model(
    items: &[StoredRequestUsageAudit],
    include_actual_cost: bool,
) -> Vec<serde_json::Value> {
    let mut grouped: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for item in items {
        let entry = grouped.entry(item.model.clone()).or_insert_with(|| {
            json!({
                "model": item.model,
                "requests": 0_u64,
                "input_tokens": 0_u64,
                "effective_input_tokens": 0_u64,
                "output_tokens": 0_u64,
                "total_tokens": 0_u64,
                "cache_read_tokens": 0_u64,
                "cache_creation_tokens": 0_u64,
                "cache_creation_ephemeral_5m_tokens": 0_u64,
                "cache_creation_ephemeral_1h_tokens": 0_u64,
                "total_input_context": 0_u64,
                "cache_hit_rate": 0.0,
                "total_cost_usd": 0.0,
            })
        });
        entry["requests"] = json!(entry["requests"].as_u64().unwrap_or(0).saturating_add(1));
        entry["input_tokens"] = json!(entry["input_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.input_tokens));
        entry["effective_input_tokens"] = json!(entry["effective_input_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(users_me_usage_effective_input_tokens(item)));
        entry["output_tokens"] = json!(entry["output_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.output_tokens));
        entry["total_tokens"] = json!(entry["total_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.total_tokens));
        entry["cache_read_tokens"] = json!(entry["cache_read_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.cache_read_input_tokens));
        entry["cache_creation_tokens"] = json!(entry["cache_creation_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(users_me_usage_cache_creation_tokens(item)));
        entry["cache_creation_ephemeral_5m_tokens"] = json!(entry
            ["cache_creation_ephemeral_5m_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.cache_creation_ephemeral_5m_input_tokens));
        entry["cache_creation_ephemeral_1h_tokens"] = json!(entry
            ["cache_creation_ephemeral_1h_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.cache_creation_ephemeral_1h_input_tokens));
        entry["total_input_context"] = json!(entry["total_input_context"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(users_me_usage_total_input_context(item)));
        entry["total_cost_usd"] =
            json!(entry["total_cost_usd"].as_f64().unwrap_or(0.0) + item.total_cost_usd);
        if include_actual_cost {
            if entry.get("actual_total_cost_usd").is_none() {
                entry["actual_total_cost_usd"] = json!(0.0);
            }
            entry["actual_total_cost_usd"] = json!(
                entry["actual_total_cost_usd"].as_f64().unwrap_or(0.0) + item.actual_total_cost_usd
            );
        }
    }

    let mut values = grouped.into_values().collect::<Vec<_>>();
    for value in &mut values {
        let total_input_context = value["total_input_context"].as_u64().unwrap_or(0);
        let cache_read_tokens = value["cache_read_tokens"].as_u64().unwrap_or(0);
        value["cache_hit_rate"] = json!(users_me_usage_cache_hit_rate(
            total_input_context,
            cache_read_tokens
        ));
        value["total_cost_usd"] =
            json!(round_to(value["total_cost_usd"].as_f64().unwrap_or(0.0), 6));
        if include_actual_cost && value.get("actual_total_cost_usd").is_some() {
            value["actual_total_cost_usd"] = json!(round_to(
                value["actual_total_cost_usd"].as_f64().unwrap_or(0.0),
                6,
            ));
        }
    }
    values.sort_by(|left, right| {
        right["requests"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&left["requests"].as_u64().unwrap_or(0))
            .then_with(|| {
                left["model"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["model"].as_str().unwrap_or_default())
            })
    });
    values
}

fn build_users_me_usage_summary_by_provider(
    items: &[StoredRequestUsageAudit],
) -> Vec<serde_json::Value> {
    let mut grouped: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for item in items {
        let entry = grouped
            .entry(item.provider_name.clone())
            .or_insert_with(|| {
                json!({
                    "provider": item.provider_name,
                    "requests": 0_u64,
                    "effective_input_tokens": 0_u64,
                    "total_tokens": 0_u64,
                    "total_input_context": 0_u64,
                    "output_tokens": 0_u64,
                    "cache_read_tokens": 0_u64,
                    "cache_creation_tokens": 0_u64,
                    "cache_creation_ephemeral_5m_tokens": 0_u64,
                    "cache_creation_ephemeral_1h_tokens": 0_u64,
                    "cache_hit_rate": 0.0,
                    "total_cost_usd": 0.0,
                    "success_rate": 0.0,
                    "avg_response_time_ms": 0.0,
                    "_success_count": 0_u64,
                    "_response_time_sum_ms": 0.0,
                    "_response_time_count": 0_u64,
                })
            });
        entry["requests"] = json!(entry["requests"].as_u64().unwrap_or(0).saturating_add(1));
        entry["total_tokens"] = json!(entry["total_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.total_tokens));
        entry["total_input_context"] = json!(entry["total_input_context"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(users_me_usage_total_input_context(item)));
        entry["effective_input_tokens"] = json!(entry["effective_input_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(users_me_usage_effective_input_tokens(item)));
        entry["output_tokens"] = json!(entry["output_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.output_tokens));
        entry["cache_read_tokens"] = json!(entry["cache_read_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.cache_read_input_tokens));
        entry["cache_creation_tokens"] = json!(entry["cache_creation_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(users_me_usage_cache_creation_tokens(item)));
        entry["cache_creation_ephemeral_5m_tokens"] = json!(entry
            ["cache_creation_ephemeral_5m_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.cache_creation_ephemeral_5m_input_tokens));
        entry["cache_creation_ephemeral_1h_tokens"] = json!(entry
            ["cache_creation_ephemeral_1h_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.cache_creation_ephemeral_1h_input_tokens));
        entry["total_cost_usd"] =
            json!(entry["total_cost_usd"].as_f64().unwrap_or(0.0) + item.total_cost_usd);

        let is_success = item.status != "failed"
            && item.status_code.is_none_or(|status| status < 400)
            && item.error_message.is_none();
        if is_success {
            entry["_success_count"] = json!(entry["_success_count"]
                .as_u64()
                .unwrap_or(0)
                .saturating_add(1));
            if let Some(response_time_ms) = item.response_time_ms {
                entry["_response_time_sum_ms"] = json!(
                    entry["_response_time_sum_ms"].as_f64().unwrap_or(0.0)
                        + response_time_ms as f64
                );
                entry["_response_time_count"] = json!(entry["_response_time_count"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_add(1));
            }
        }
    }

    let mut values = grouped.into_values().collect::<Vec<_>>();
    for value in &mut values {
        let total_input_context = value["total_input_context"].as_u64().unwrap_or(0);
        let cache_read_tokens = value["cache_read_tokens"].as_u64().unwrap_or(0);
        let success_count = value["_success_count"].as_u64().unwrap_or(0);
        let requests = value["requests"].as_u64().unwrap_or(0);
        let response_time_count = value["_response_time_count"].as_u64().unwrap_or(0);
        let response_time_sum_ms = value["_response_time_sum_ms"].as_f64().unwrap_or(0.0);
        value["cache_hit_rate"] = json!(users_me_usage_cache_hit_rate(
            total_input_context,
            cache_read_tokens
        ));
        value["total_cost_usd"] =
            json!(round_to(value["total_cost_usd"].as_f64().unwrap_or(0.0), 6));
        value["success_rate"] = json!(if requests == 0 {
            100.0
        } else {
            round_to(success_count as f64 / requests as f64 * 100.0, 2)
        });
        value["avg_response_time_ms"] = json!(if response_time_count == 0 {
            0.0
        } else {
            round_to(response_time_sum_ms / response_time_count as f64, 2)
        });
        value
            .as_object_mut()
            .expect("object")
            .remove("_success_count");
        value
            .as_object_mut()
            .expect("object")
            .remove("_response_time_sum_ms");
        value
            .as_object_mut()
            .expect("object")
            .remove("_response_time_count");
    }
    values.sort_by(|left, right| {
        right["requests"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&left["requests"].as_u64().unwrap_or(0))
            .then_with(|| {
                left["provider"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["provider"].as_str().unwrap_or_default())
            })
    });
    values
}

fn build_users_me_usage_summary_by_api_format(
    items: &[StoredRequestUsageAudit],
) -> Vec<serde_json::Value> {
    let mut grouped: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for item in items.iter().filter(|item| item.api_format.is_some()) {
        let api_format = item
            .api_format
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let entry = grouped.entry(api_format.clone()).or_insert_with(|| {
            json!({
                "api_format": api_format,
                "request_count": 0_u64,
                "total_tokens": 0_u64,
                "effective_input_tokens": 0_u64,
                "total_input_context": 0_u64,
                "output_tokens": 0_u64,
                "cache_read_tokens": 0_u64,
                "cache_creation_tokens": 0_u64,
                "cache_creation_ephemeral_5m_tokens": 0_u64,
                "cache_creation_ephemeral_1h_tokens": 0_u64,
                "cache_hit_rate": 0.0,
                "total_cost_usd": 0.0,
                "avg_response_time_ms": 0.0,
                "_response_time_sum_ms": 0.0,
                "_response_time_count": 0_u64,
            })
        });
        entry["request_count"] = json!(entry["request_count"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(1));
        entry["total_tokens"] = json!(entry["total_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.total_tokens));
        entry["total_input_context"] = json!(entry["total_input_context"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(users_me_usage_total_input_context(item)));
        entry["effective_input_tokens"] = json!(entry["effective_input_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(users_me_usage_effective_input_tokens(item)));
        entry["output_tokens"] = json!(entry["output_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.output_tokens));
        entry["cache_read_tokens"] = json!(entry["cache_read_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.cache_read_input_tokens));
        entry["cache_creation_tokens"] = json!(entry["cache_creation_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(users_me_usage_cache_creation_tokens(item)));
        entry["cache_creation_ephemeral_5m_tokens"] = json!(entry
            ["cache_creation_ephemeral_5m_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.cache_creation_ephemeral_5m_input_tokens));
        entry["cache_creation_ephemeral_1h_tokens"] = json!(entry
            ["cache_creation_ephemeral_1h_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(item.cache_creation_ephemeral_1h_input_tokens));
        entry["total_cost_usd"] =
            json!(entry["total_cost_usd"].as_f64().unwrap_or(0.0) + item.total_cost_usd);
        if let Some(response_time_ms) = item.response_time_ms {
            entry["_response_time_sum_ms"] = json!(
                entry["_response_time_sum_ms"].as_f64().unwrap_or(0.0) + response_time_ms as f64
            );
            entry["_response_time_count"] = json!(entry["_response_time_count"]
                .as_u64()
                .unwrap_or(0)
                .saturating_add(1));
        }
    }

    let mut values = grouped.into_values().collect::<Vec<_>>();
    for value in &mut values {
        let total_input_context = value["total_input_context"].as_u64().unwrap_or(0);
        let cache_read_tokens = value["cache_read_tokens"].as_u64().unwrap_or(0);
        let response_time_count = value["_response_time_count"].as_u64().unwrap_or(0);
        let response_time_sum_ms = value["_response_time_sum_ms"].as_f64().unwrap_or(0.0);
        value["cache_hit_rate"] = json!(users_me_usage_cache_hit_rate(
            total_input_context,
            cache_read_tokens
        ));
        value["total_cost_usd"] =
            json!(round_to(value["total_cost_usd"].as_f64().unwrap_or(0.0), 6));
        value["avg_response_time_ms"] = json!(if response_time_count == 0 {
            0.0
        } else {
            round_to(response_time_sum_ms / response_time_count as f64, 2)
        });
        value
            .as_object_mut()
            .expect("object")
            .remove("_response_time_sum_ms");
        value
            .as_object_mut()
            .expect("object")
            .remove("_response_time_count");
    }
    values.sort_by(|left, right| {
        right["request_count"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&left["request_count"].as_u64().unwrap_or(0))
            .then_with(|| {
                left["api_format"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["api_format"].as_str().unwrap_or_default())
            })
    });
    values
}

pub(super) async fn handle_users_me_usage_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_usage_data_reader() {
        return build_users_me_usage_reader_unavailable_response();
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = request_context.request_query_string.as_deref();
    let time_range = match AdminStatsTimeRange::resolve_optional(query) {
        Ok(value) => value,
        Err(detail) => return admin_stats_bad_request_response(detail),
    };
    let search = query_param_value(query, "search");
    let limit = match parse_users_me_usage_limit(query) {
        Ok(value) => value,
        Err(detail) => return admin_stats_bad_request_response(detail),
    };
    let offset = match parse_users_me_usage_offset(query) {
        Ok(value) => value,
        Err(detail) => return admin_stats_bad_request_response(detail),
    };

    // When no time range is specified, default to 7 days to avoid full-table scans.
    let effective_time_range = time_range.or_else(|| {
        let today = Utc::now().date_naive();
        let start_date = today
            .checked_sub_signed(chrono::Duration::days(6))
            .unwrap_or(today);
        Some(AdminStatsTimeRange {
            start_date,
            end_date: today,
            tz_offset_minutes: 0,
        })
    });

    let usage = match list_usage_for_optional_range(
        &AdminAppState::new(state),
        effective_time_range.as_ref(),
        &AdminStatsUsageFilter {
            user_id: Some(auth.user.id.clone()),
            provider_name: None,
            model: None,
        },
    )
    .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user usage lookup failed: {err:?}"),
                false,
            )
        }
    };
    let api_key_names = if state.has_auth_api_key_data_reader() {
        let api_key_ids = usage
            .iter()
            .filter_map(|item| item.api_key_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if api_key_ids.is_empty() {
            BTreeMap::new()
        } else {
            match state.resolve_auth_api_key_names_by_ids(&api_key_ids).await {
                Ok(value) => value,
                Err(err) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("user api key name lookup failed: {err:?}"),
                        false,
                    )
                }
            }
        }
    } else {
        BTreeMap::new()
    };

    let summary_items = usage
        .iter()
        .filter(|item| {
            !matches!(item.status.as_str(), "pending" | "streaming")
                && !matches!(item.provider_name.as_str(), "unknown" | "pending")
        })
        .cloned()
        .collect::<Vec<_>>();
    let include_actual_cost = auth.user.role.eq_ignore_ascii_case("admin");
    let total_requests = summary_items.len() as u64;
    let total_input_tokens = summary_items
        .iter()
        .map(|item| item.input_tokens)
        .sum::<u64>();
    let total_output_tokens = summary_items
        .iter()
        .map(|item| item.output_tokens)
        .sum::<u64>();
    let total_tokens = summary_items
        .iter()
        .map(|item| item.total_tokens)
        .sum::<u64>();
    let total_cost = round_to(
        summary_items
            .iter()
            .map(|item| item.total_cost_usd)
            .sum::<f64>(),
        6,
    );
    let total_actual_cost = round_to(
        summary_items
            .iter()
            .map(|item| item.actual_total_cost_usd)
            .sum::<f64>(),
        6,
    );

    let successful_response_times = summary_items
        .iter()
        .filter(|item| {
            item.status != "failed"
                && item.status_code.is_none_or(|status| status < 400)
                && item.error_message.is_none()
        })
        .filter_map(|item| item.response_time_ms)
        .collect::<Vec<_>>();
    let avg_response_time = if successful_response_times.is_empty() {
        0.0
    } else {
        round_to(
            successful_response_times
                .iter()
                .map(|value| *value as f64)
                .sum::<f64>()
                / successful_response_times.len() as f64
                / 1000.0,
            2,
        )
    };

    let mut records = usage
        .into_iter()
        .filter(|item| {
            users_me_usage_matches_search(
                item,
                search.as_deref(),
                &api_key_names,
                state.has_auth_api_key_data_reader(),
            )
        })
        .collect::<Vec<_>>();
    records.sort_by(|left, right| {
        right
            .created_at_unix_ms
            .cmp(&left.created_at_unix_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    let total_record_count = records.len();
    let records = records
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|item| {
            build_users_me_usage_record_payload(
                &item,
                include_actual_cost,
                &api_key_names,
                state.has_auth_api_key_data_reader(),
            )
        })
        .collect::<Vec<_>>();

    let wallet = state
        .read_wallet_snapshot_for_auth(&auth.user.id, "", false)
        .await
        .ok()
        .flatten();

    let mut payload = json!({
        "total_requests": total_requests,
        "total_input_tokens": total_input_tokens,
        "total_output_tokens": total_output_tokens,
        "total_tokens": total_tokens,
        "total_cost": total_cost,
        "avg_response_time": avg_response_time,
        "billing": build_auth_wallet_summary_payload(wallet.as_ref()),
        "summary_by_model": build_users_me_usage_summary_by_model(&summary_items, include_actual_cost),
        "summary_by_provider": build_users_me_usage_summary_by_provider(&summary_items),
        "summary_by_api_format": build_users_me_usage_summary_by_api_format(&summary_items),
        "pagination": {
            "total": total_record_count,
            "limit": limit,
            "offset": offset,
            "has_more": offset.saturating_add(limit) < total_record_count,
        },
        "records": records,
    });
    if include_actual_cost {
        payload["total_actual_cost"] = json!(total_actual_cost);
    }
    Json(payload).into_response()
}

pub(super) async fn handle_users_me_usage_active_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_usage_data_reader() {
        return build_users_me_usage_reader_unavailable_response();
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let ids = parse_users_me_usage_ids(request_context.request_query_string.as_deref());
    // When polling for active (pending/streaming) requests without specific ids,
    // limit to the last 1 hour to avoid scanning all historical records.
    let created_from = if ids.is_none() {
        Some(Utc::now().timestamp().saturating_sub(3600) as u64)
    } else {
        None
    };
    let items = match state
        .list_usage_audits(&UsageAuditListQuery {
            created_from_unix_secs: created_from,
            created_until_unix_secs: None,
            user_id: Some(auth.user.id.clone()),
            provider_name: None,
            model: None,
            api_format: None,
            statuses: None,
            is_stream: None,
            error_only: false,
            limit: None,
            offset: None,
            newest_first: false,
        })
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user active usage lookup failed: {err:?}"),
                false,
            )
        }
    };

    let mut items = items
        .into_iter()
        .filter(|item| match ids.as_ref() {
            Some(ids) => ids.contains(&item.id),
            None => matches!(item.status.as_str(), "pending" | "streaming"),
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        right
            .created_at_unix_ms
            .cmp(&left.created_at_unix_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    if ids.is_none() && items.len() > 50 {
        items.truncate(50);
    }

    Json(json!({
        "requests": items
            .iter()
            .map(build_users_me_usage_active_payload)
            .collect::<Vec<_>>(),
    }))
    .into_response()
}

pub(super) async fn handle_users_me_usage_interval_timeline_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_usage_data_reader() {
        return build_users_me_usage_reader_unavailable_response();
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = request_context.request_query_string.as_deref();
    let hours = match parse_users_me_usage_hours(query) {
        Ok(value) => value,
        Err(detail) => return admin_stats_bad_request_response(detail),
    };
    let limit = match parse_users_me_usage_timeline_limit(query) {
        Ok(value) => value,
        Err(detail) => return admin_stats_bad_request_response(detail),
    };
    let now_unix_secs = u64::try_from(Utc::now().timestamp()).unwrap_or_default();
    let created_from_unix_secs = now_unix_secs.saturating_sub(u64::from(hours) * 3600);

    let mut items = match state
        .list_usage_audits(&UsageAuditListQuery {
            created_from_unix_secs: Some(created_from_unix_secs),
            created_until_unix_secs: None,
            user_id: Some(auth.user.id.clone()),
            provider_name: None,
            model: None,
            api_format: None,
            statuses: None,
            is_stream: None,
            error_only: false,
            limit: None,
            offset: None,
            newest_first: false,
        })
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user interval timeline lookup failed: {err:?}"),
                false,
            )
        }
    };
    items.retain(|item| item.status == "completed");
    items.sort_by(|left, right| {
        left.created_at_unix_ms
            .cmp(&right.created_at_unix_ms)
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut points = Vec::new();
    let mut previous_created_at_unix_ms = None;
    for item in items {
        if let Some(previous) = previous_created_at_unix_ms {
            let interval_minutes = (item.created_at_unix_ms.saturating_sub(previous) as f64) / 60.0;
            if interval_minutes <= 120.0 {
                points.push(json!({
                    "x": unix_secs_to_rfc3339(item.created_at_unix_ms),
                    "y": round_to(interval_minutes, 2),
                    "model": item.model,
                }));
                if points.len() >= limit {
                    break;
                }
            }
        }
        previous_created_at_unix_ms = Some(item.created_at_unix_ms);
    }

    Json(json!({
        "analysis_period_hours": hours,
        "total_points": points.len(),
        "points": points,
    }))
    .into_response()
}

pub(super) async fn handle_users_me_usage_heatmap_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_usage_data_reader() {
        return build_users_me_usage_reader_unavailable_response();
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let today = Utc::now().date_naive();
    let start_date = today
        .checked_sub_signed(chrono::Duration::days(364))
        .unwrap_or(today);
    let Some(start_of_day) = start_date.and_hms_opt(0, 0, 0) else {
        return build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            "heatmap start date is invalid",
            false,
        );
    };
    let created_from_unix_secs = u64::try_from(
        chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(start_of_day, chrono::Utc)
            .timestamp(),
    )
    .unwrap_or_default();

    let summaries = match state
        .summarize_usage_daily_heatmap(
            &aether_data_contracts::repository::usage::UsageDailyHeatmapQuery {
                created_from_unix_secs,
                user_id: Some(auth.user.id.clone()),
                admin_mode: false,
            },
        )
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user heatmap lookup failed: {err:?}"),
                false,
            )
        }
    };

    let include_actual_cost = auth.user.role.eq_ignore_ascii_case("admin");
    let grouped: std::collections::HashMap<String, _> =
        summaries.into_iter().map(|s| (s.date.clone(), s)).collect();

    let mut max_requests = 0_u64;
    let mut cursor = start_date;
    let mut days = Vec::new();
    while cursor <= today {
        let date_str = cursor.to_string();
        let (requests, total_tokens, total_cost, actual_total_cost) =
            if let Some(s) = grouped.get(&date_str) {
                (
                    s.requests,
                    s.total_tokens,
                    s.total_cost_usd,
                    s.actual_total_cost_usd,
                )
            } else {
                (0, 0, 0.0, 0.0)
            };
        max_requests = max_requests.max(requests);
        let mut day = json!({
            "date": date_str,
            "requests": requests,
            "total_tokens": total_tokens,
            "total_cost": round_to(total_cost, 6),
        });
        if include_actual_cost {
            day["actual_total_cost"] = json!(round_to(actual_total_cost, 6));
        }
        days.push(day);
        cursor = cursor
            .checked_add_signed(chrono::Duration::days(1))
            .unwrap_or(today + chrono::Duration::days(1));
    }

    Json(json!({
        "start_date": start_date.to_string(),
        "end_date": today.to_string(),
        "total_days": days.len(),
        "max_requests": max_requests,
        "days": days,
    }))
    .into_response()
}
