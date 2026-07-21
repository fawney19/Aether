use super::super::shared::{
    build_provider_quota_execution_plan, execute_provider_quota_plan,
    resolve_provider_quota_execution_timeouts, ProviderQuotaExecutionOutcome,
};
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::GatewayError;
use aether_admin::provider::quota::{
    glm_coding_plan_business_error, parse_glm_coding_plan_quota_limit_response,
    parse_glm_coding_plan_usage_response, GlmCodingPlanBusinessErrorClass,
};
use aether_contracts::{ExecutionResult, ProxySnapshot};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint;
use aether_provider_pool::build_glm_coding_plan_pool_quota_request_with_base_url;
use chrono::{FixedOffset, TimeZone};
use serde_json::json;

use super::metadata::{merge_metadata_patch, push_probe_warning};
use super::RefreshOutcome;

const GLM_MONITOR_TIMEZONE_OFFSET_SECS: i32 = 8 * 60 * 60;

#[derive(Debug, Clone, Copy)]
pub(super) enum GlmCodingPlanProbeParser {
    QuotaLimit,
    Usage {
        kind: &'static str,
        window: Option<&'static str>,
    },
}

pub(super) fn usage_query_for_cycle_window(
    reset_at_unix_secs: Option<u64>,
    window_seconds: u64,
    now_unix_secs: u64,
    allow_rolling_fallback: bool,
) -> Option<String> {
    let start_unix_secs = reset_at_unix_secs
        .map(|reset_at| reset_at.saturating_sub(window_seconds))
        .or_else(|| allow_rolling_fallback.then(|| now_unix_secs.saturating_sub(window_seconds)))?;
    usage_query_for_unix_window(start_unix_secs, now_unix_secs)
}

fn usage_query_for_unix_window(start_unix_secs: u64, end_unix_secs: u64) -> Option<String> {
    let start = format_glm_monitor_time(start_unix_secs)?;
    let end = format_glm_monitor_time(end_unix_secs)?;
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("startTime", &start);
    serializer.append_pair("endTime", &end);
    Some(serializer.finish())
}

fn format_glm_monitor_time(unix_secs: u64) -> Option<String> {
    let timestamp = i64::try_from(unix_secs).ok()?;
    let timezone = FixedOffset::east_opt(GLM_MONITOR_TIMEZONE_OFFSET_SECS)?;
    let datetime = timezone.timestamp_opt(timestamp, 0).single()?;
    Some(datetime.format("%Y-%m-%d %H:%M:%S").to_string())
}

pub(super) async fn execute_kind(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    endpoint: &StoredProviderCatalogEndpoint,
    api_key: &str,
    kind: &str,
    query: Option<&str>,
    proxy_override: Option<&ProxySnapshot>,
) -> Result<ProviderQuotaExecutionOutcome, GatewayError> {
    let spec = build_glm_coding_plan_pool_quota_request_with_base_url(
        &transport.key.id,
        &endpoint.base_url,
        api_key,
        kind,
        query,
    );
    let proxy = match proxy_override {
        Some(proxy) => Some(proxy.clone()),
        None => {
            state
                .resolve_transport_proxy_snapshot_with_tunnel_affinity(transport)
                .await
        }
    };
    let timeouts = Some(resolve_provider_quota_execution_timeouts(
        state.resolve_transport_execution_timeouts(transport),
        proxy.as_ref(),
    ));
    let plan = build_provider_quota_execution_plan(
        transport,
        spec,
        proxy,
        state.resolve_transport_profile(transport),
        timeouts,
    );
    execute_provider_quota_plan(state, transport, plan, &format!("glm_coding_plan:{kind}")).await
}

pub(super) fn handle_probe_result(
    kind: &str,
    parser: GlmCodingPlanProbeParser,
    result: ProviderQuotaExecutionOutcome,
    now_unix_secs: u64,
    outcome: &mut RefreshOutcome,
) {
    let result = match result {
        ProviderQuotaExecutionOutcome::Response(result) => result,
        ProviderQuotaExecutionOutcome::Failure(detail) => {
            push_probe_warning(
                &mut outcome.metadata,
                kind,
                format!("请求执行失败: {detail}"),
            );
            return;
        }
    };
    let body_json = result
        .body
        .as_ref()
        .and_then(|body| body.json_body.as_ref());
    if let Some(error) = body_json.and_then(glm_coding_plan_business_error) {
        let message = error.message;
        match error.class {
            GlmCodingPlanBusinessErrorClass::CredentialInvalid => {
                outcome.oauth_invalid_at_unix_secs = Some(now_unix_secs);
                outcome.oauth_invalid_reason = Some(message.clone());
            }
            GlmCodingPlanBusinessErrorClass::QuotaBlocked => {
                outcome
                    .metadata
                    .insert("quota_exhausted".to_string(), json!(true));
                if let Some(code) = error.code {
                    outcome
                        .metadata
                        .insert("quota_error_code".to_string(), json!(code));
                }
                outcome
                    .metadata
                    .insert("quota_error_reason".to_string(), json!(message.clone()));
                outcome.parsed_probe_count += 1;
            }
            GlmCodingPlanBusinessErrorClass::Other if result.status_code != 200 => {
                handle_http_error(kind, &result, now_unix_secs, outcome);
                return;
            }
            GlmCodingPlanBusinessErrorClass::Other => {}
        }
        push_probe_warning(&mut outcome.metadata, kind, message);
        return;
    }
    if result.status_code != 200 {
        handle_http_error(kind, &result, now_unix_secs, outcome);
        return;
    }
    let Some(body_json) = body_json else {
        push_probe_warning(&mut outcome.metadata, kind, "响应中未包含 JSON".to_string());
        return;
    };
    let parsed = match parser {
        GlmCodingPlanProbeParser::Usage { kind, window } => {
            parse_glm_coding_plan_usage_response(body_json, kind, window, now_unix_secs)
        }
        GlmCodingPlanProbeParser::QuotaLimit => {
            parse_glm_coding_plan_quota_limit_response(body_json, now_unix_secs)
        }
    };
    if parsed.is_some_and(|parsed| merge_metadata_patch(&mut outcome.metadata, parsed)) {
        outcome.parsed_probe_count += 1;
    } else {
        push_probe_warning(
            &mut outcome.metadata,
            kind,
            "响应中未包含用量信息".to_string(),
        );
    }
}

fn handle_http_error(
    kind: &str,
    result: &ExecutionResult,
    now_unix_secs: u64,
    outcome: &mut RefreshOutcome,
) {
    let err_msg = aether_admin::provider::quota::extract_execution_error_message(result);
    if result.status_code == 401 {
        outcome.oauth_invalid_at_unix_secs = Some(now_unix_secs);
        outcome.oauth_invalid_reason = Some(
            err_msg
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| format!("GLM Coding Plan Token 无效 ({})", result.status_code)),
        );
    }
    let message = match err_msg.as_deref() {
        Some(detail) if !detail.is_empty() => {
            format!("返回状态码 {}: {}", result.status_code, detail)
        }
        _ => format!("返回状态码 {}", result.status_code),
    };
    push_probe_warning(&mut outcome.metadata, kind, message);
}

#[cfg(test)]
mod tests;
