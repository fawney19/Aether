use aether_data_contracts::repository::{
    candidates::{DecisionTrace, DecisionTraceCandidate},
    provider_catalog::StoredProviderCatalogKey,
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdminMonitoringRoute {
    AuditLogs,
    SystemStatus,
    SuspiciousActivities,
    UserBehavior,
    ResilienceStatus,
    ResilienceErrorStats,
    ResilienceCircuitHistory,
    TraceRequest,
    TraceProviderStats,
    CacheStats,
    CacheAffinity,
    CacheAffinities,
    CacheUsersDelete,
    CacheAffinityDelete,
    CacheFlush,
    CacheProviderDelete,
    CacheConfig,
    CacheMetrics,
    CacheModelMappingStats,
    CacheModelMappingDelete,
    CacheModelMappingDeleteModel,
    CacheModelMappingDeleteProvider,
    CacheRedisKeys,
    CacheRedisKeysDelete,
}

pub fn admin_monitoring_bad_request_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

pub fn admin_monitoring_not_found_response(detail: &'static str) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}

pub fn build_admin_monitoring_audit_logs_payload_response(
    items: Vec<Value>,
    total: usize,
    limit: usize,
    offset: usize,
    username: Option<String>,
    event_type: Option<String>,
    days: i64,
) -> Response<Body> {
    let count = items.len();
    Json(json!({
        "items": items,
        "meta": {
            "total": total,
            "limit": limit,
            "offset": offset,
            "count": count,
        },
        "filters": {
            "username": username,
            "event_type": event_type,
            "days": days,
        },
    }))
    .into_response()
}

pub fn build_admin_monitoring_suspicious_activities_payload_response(
    activities: Vec<Value>,
    hours: i64,
) -> Response<Body> {
    let count = activities.len();
    Json(json!({
        "activities": activities,
        "count": count,
        "time_range_hours": hours,
    }))
    .into_response()
}

pub fn build_admin_monitoring_user_behavior_payload_response(
    user_id: String,
    days: i64,
    event_counts: BTreeMap<String, u64>,
    failed_requests: u64,
    success_requests: u64,
    suspicious_activities: u64,
) -> Response<Body> {
    let total_requests = success_requests.saturating_add(failed_requests);
    let success_rate = if total_requests == 0 {
        0.0
    } else {
        success_requests as f64 / total_requests as f64
    };

    Json(json!({
        "user_id": user_id,
        "period_days": days,
        "event_counts": event_counts,
        "failed_requests": failed_requests,
        "success_requests": success_requests,
        "success_rate": success_rate,
        "suspicious_activities": suspicious_activities,
        "analysis_time": chrono::Utc::now().to_rfc3339(),
    }))
    .into_response()
}

pub fn admin_monitoring_user_behavior_user_id_from_path(request_path: &str) -> Option<String> {
    path_identifier_from_path(request_path, "/api/admin/monitoring/user-behavior/")
}

pub fn admin_monitoring_trace_request_id_from_path(request_path: &str) -> Option<String> {
    path_identifier_from_path(request_path, "/api/admin/monitoring/trace/")
}

pub fn admin_monitoring_trace_provider_id_from_path(request_path: &str) -> Option<String> {
    path_identifier_from_path(request_path, "/api/admin/monitoring/trace/stats/provider/")
}

pub fn parse_admin_monitoring_attempted_only(query: Option<&str>) -> Result<bool, String> {
    match query_param_value(query, "attempted_only") {
        None => Ok(false),
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(true),
            "false" | "0" | "no" => Ok(false),
            _ => Err("attempted_only must be a boolean".to_string()),
        },
    }
}

pub fn admin_monitoring_trace_not_found_response(
    request_id: &str,
    attempted_only: bool,
) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({
            "detail": "Request trace not found",
            "request_id": request_id,
            "attempted_only": attempted_only,
        })),
    )
        .into_response()
}

pub fn build_admin_monitoring_cache_affinity_delete_success_response(
    affinity_key: String,
    endpoint_id: String,
    model_id: String,
    message_name: String,
) -> Response<Body> {
    Json(json!({
        "status": "ok",
        "message": format!("已清除缓存亲和性: {message_name}"),
        "affinity_key": affinity_key,
        "endpoint_id": endpoint_id,
        "model_id": model_id,
    }))
    .into_response()
}

pub fn build_admin_monitoring_cache_users_delete_api_key_success_response(
    user_id: String,
    username: Option<String>,
    email: Option<String>,
    api_key_id: String,
    api_key_name: Option<String>,
) -> Response<Body> {
    let display_name = api_key_name.clone().unwrap_or_else(|| api_key_id.clone());
    Json(json!({
        "status": "ok",
        "message": format!("已清除 API Key {display_name} 的缓存亲和性"),
        "user_info": {
            "user_id": Some(user_id),
            "username": username,
            "email": email,
            "api_key_id": api_key_id,
            "api_key_name": api_key_name,
        },
    }))
    .into_response()
}

pub fn build_admin_monitoring_cache_users_delete_user_success_response(
    user_id: String,
    username: String,
    email: Option<String>,
) -> Response<Body> {
    Json(json!({
        "status": "ok",
        "message": format!("已清除用户 {username} 的所有缓存亲和性"),
        "user_info": {
            "user_id": user_id,
            "username": username,
            "email": email,
        },
    }))
    .into_response()
}

pub fn build_admin_monitoring_cache_provider_delete_success_response(
    provider_id: String,
    deleted_affinities: usize,
) -> Response<Body> {
    Json(json!({
        "status": "ok",
        "message": format!("已清除 provider {provider_id} 的缓存亲和性"),
        "provider_id": provider_id,
        "deleted_affinities": deleted_affinities,
    }))
    .into_response()
}

pub fn build_admin_monitoring_trace_provider_stats_payload_response(
    provider_id: String,
    total_attempts: usize,
    success_count: usize,
    failed_count: usize,
    cancelled_count: usize,
    skipped_count: usize,
    pending_count: usize,
    available_count: usize,
    unused_count: usize,
    failure_rate: f64,
    avg_latency_ms: f64,
) -> Response<Body> {
    Json(json!({
        "provider_id": provider_id,
        "total_attempts": total_attempts,
        "success_count": success_count,
        "failed_count": failed_count,
        "cancelled_count": cancelled_count,
        "skipped_count": skipped_count,
        "pending_count": pending_count,
        "available_count": available_count,
        "unused_count": unused_count,
        "failure_rate": failure_rate,
        "avg_latency_ms": avg_latency_ms,
    }))
    .into_response()
}

pub fn build_admin_monitoring_trace_request_payload_response(
    trace: &DecisionTrace,
) -> Response<Body> {
    let candidates = trace
        .candidates
        .iter()
        .map(build_admin_monitoring_trace_request_candidate_payload)
        .collect::<Vec<_>>();
    Json(json!({
        "request_id": trace.request_id,
        "total_candidates": trace.total_candidates,
        "final_status": trace.final_status,
        "total_latency_ms": trace.total_latency_ms,
        "candidates": candidates,
    }))
    .into_response()
}

pub fn build_admin_monitoring_trace_request_candidate_payload(
    item: &DecisionTraceCandidate,
) -> Value {
    let candidate = &item.candidate;
    json!({
        "id": candidate.id,
        "request_id": candidate.request_id,
        "candidate_index": candidate.candidate_index,
        "retry_index": candidate.retry_index,
        "provider_id": candidate.provider_id,
        "provider_name": item.provider_name,
        "provider_website": item.provider_website,
        "endpoint_id": candidate.endpoint_id,
        "endpoint_name": item.endpoint_api_format,
        "key_id": candidate.key_id,
        "key_name": item.provider_key_name,
        "key_account_label": serde_json::Value::Null,
        "key_preview": serde_json::Value::Null,
        "key_auth_type": item.provider_key_auth_type,
        "key_oauth_plan_type": serde_json::Value::Null,
        "key_capabilities": item.provider_key_capabilities,
        "required_capabilities": candidate.required_capabilities,
        "status": candidate.status,
        "skip_reason": candidate.skip_reason,
        "is_cached": candidate.is_cached,
        "status_code": candidate.status_code,
        "error_type": candidate.error_type,
        "error_message": candidate.error_message,
        "latency_ms": candidate.latency_ms,
        "concurrent_requests": candidate.concurrent_requests,
        "extra_data": candidate.extra_data,
        "created_at": unix_secs_to_rfc3339(candidate.created_at_unix_secs),
        "started_at": candidate.started_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "finished_at": candidate.finished_at_unix_secs.and_then(unix_secs_to_rfc3339),
    })
}

pub fn build_admin_monitoring_system_status_payload_response(
    timestamp: chrono::DateTime<chrono::Utc>,
    total_users: u64,
    active_users: u64,
    total_providers: usize,
    active_providers: usize,
    total_api_keys: u64,
    active_api_keys: u64,
    today_requests: usize,
    today_tokens: u64,
    today_cost: f64,
    proxy_connections: usize,
    nodes: usize,
    active_streams: usize,
    path_prefixes: &[&str],
    recent_errors: usize,
) -> Response<Body> {
    Json(json!({
        "timestamp": timestamp.to_rfc3339(),
        "users": {
            "total": total_users,
            "active": active_users,
        },
        "providers": {
            "total": total_providers,
            "active": active_providers,
        },
        "api_keys": {
            "total": total_api_keys,
            "active": active_api_keys,
        },
        "today_stats": {
            "requests": today_requests,
            "tokens": today_tokens,
            "cost_usd": format!("${today_cost:.4}"),
        },
        "tunnel": {
            "proxy_connections": proxy_connections,
            "nodes": nodes,
            "active_streams": active_streams,
        },
        "internal_gateway": {
            "status": "rust_native_control_plane",
            "path_prefixes": path_prefixes,
        },
        "recent_errors": recent_errors,
    }))
    .into_response()
}

pub fn build_admin_monitoring_resilience_status_payload_response(
    timestamp: chrono::DateTime<chrono::Utc>,
    health_score: i64,
    status: &'static str,
    error_statistics: Value,
    recent_errors: Vec<Value>,
    recommendations: Vec<String>,
) -> Response<Body> {
    Json(json!({
        "timestamp": timestamp.to_rfc3339(),
        "health_score": health_score,
        "status": status,
        "error_statistics": error_statistics,
        "recent_errors": recent_errors,
        "recommendations": recommendations,
    }))
    .into_response()
}

pub fn build_admin_monitoring_reset_error_stats_payload_response(
    previous_stats: Value,
    reset_by: Option<String>,
    reset_at: chrono::DateTime<chrono::Utc>,
) -> Response<Body> {
    Json(json!({
        "message": "错误统计已重置",
        "previous_stats": previous_stats,
        "reset_by": reset_by,
        "reset_at": reset_at.to_rfc3339(),
    }))
    .into_response()
}

pub fn parse_admin_monitoring_circuit_history_limit(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "limit") {
        None => Ok(50),
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
    }
}

pub fn build_admin_monitoring_circuit_history_items(
    keys: &[StoredProviderCatalogKey],
    provider_name_by_id: &BTreeMap<String, String>,
    limit: usize,
) -> Vec<Value> {
    let mut items = Vec::new();

    for key in keys {
        let health_by_format = key
            .health_by_format
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        let circuit_by_format = key
            .circuit_breaker_by_format
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();

        for (api_format, circuit_value) in circuit_by_format {
            let Some(circuit) = circuit_value.as_object() else {
                continue;
            };
            let is_open = circuit
                .get("open")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let is_half_open = circuit
                .get("half_open_until")
                .and_then(serde_json::Value::as_str)
                .is_some();

            if !is_open && !is_half_open {
                continue;
            }

            let health = health_by_format
                .get(&api_format)
                .and_then(serde_json::Value::as_object);
            let timestamp = circuit
                .get("open_at")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    circuit
                        .get("half_open_until")
                        .and_then(serde_json::Value::as_str)
                })
                .or_else(|| {
                    health.and_then(|value| {
                        value
                            .get("last_failure_at")
                            .and_then(serde_json::Value::as_str)
                    })
                })
                .map(ToOwned::to_owned);
            let event = if is_half_open { "half_open" } else { "opened" };
            let reason = circuit
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .or_else(|| {
                    health
                        .and_then(|value| {
                            value
                                .get("consecutive_failures")
                                .and_then(serde_json::Value::as_i64)
                        })
                        .filter(|value| *value > 0)
                        .map(|value| format!("连续失败 {value} 次"))
                })
                .or_else(|| {
                    Some(if is_half_open {
                        "熔断器处于半开状态".to_string()
                    } else {
                        "熔断器处于打开状态".to_string()
                    })
                });
            let recovery_seconds = circuit
                .get("recovery_seconds")
                .and_then(serde_json::Value::as_i64)
                .or_else(|| {
                    let open_at = circuit
                        .get("open_at")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok());
                    let next_probe_at = circuit
                        .get("next_probe_at")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok());
                    match (open_at, next_probe_at) {
                        (Some(open_at), Some(next_probe_at)) => {
                            Some((next_probe_at - open_at).num_seconds().max(0))
                        }
                        _ => None,
                    }
                });

            items.push(json!({
                "event": event,
                "key_id": key.id,
                "provider_id": key.provider_id,
                "provider_name": provider_name_by_id.get(&key.provider_id).cloned(),
                "key_name": key.name,
                "api_format": api_format,
                "reason": reason,
                "recovery_seconds": recovery_seconds,
                "timestamp": timestamp,
            }));
        }
    }

    items.sort_by(|left, right| {
        let left_ts = left
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let right_ts = right
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        right_ts.cmp(left_ts)
    });
    items.truncate(limit);
    items
}

pub fn build_admin_monitoring_circuit_history_payload_response(
    items: Vec<Value>,
) -> Response<Body> {
    let count = items.len();
    Json(json!({
        "items": items,
        "count": count,
    }))
    .into_response()
}

pub fn admin_monitoring_unknown_cache_category_response(category: &str) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": format!("未知的缓存分类: {category}") })),
    )
        .into_response()
}

pub fn build_admin_monitoring_redis_keys_delete_success_response(
    category: &str,
    name: &str,
    deleted_count: usize,
) -> Response<Body> {
    Json(json!({
        "status": "ok",
        "message": format!("已清除 {name} 缓存"),
        "category": category,
        "deleted_count": deleted_count,
    }))
    .into_response()
}

pub fn build_admin_monitoring_cache_flush_success_response(
    deleted_affinities: usize,
) -> Response<Body> {
    Json(json!({
        "status": "ok",
        "message": "已清除全部缓存亲和性",
        "deleted_affinities": deleted_affinities,
    }))
    .into_response()
}

pub fn admin_monitoring_cache_provider_not_found_response(provider_id: &str) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({
            "detail": format!("未找到 provider {provider_id} 的缓存亲和性记录")
        })),
    )
        .into_response()
}

pub fn build_admin_monitoring_model_mapping_delete_success_response(
    deleted_count: usize,
) -> Response<Body> {
    Json(json!({
        "status": "ok",
        "message": "已清除所有模型映射缓存",
        "deleted_count": deleted_count,
    }))
    .into_response()
}

pub fn build_admin_monitoring_model_mapping_delete_model_success_response(
    model_name: String,
    deleted_keys: Vec<String>,
) -> Response<Body> {
    Json(json!({
        "status": "ok",
        "message": format!("已清除模型 {model_name} 的映射缓存"),
        "model_name": model_name,
        "deleted_keys": deleted_keys,
    }))
    .into_response()
}

pub fn build_admin_monitoring_model_mapping_delete_provider_success_response(
    provider_id: String,
    global_model_id: String,
    deleted_keys: Vec<String>,
) -> Response<Body> {
    Json(json!({
        "status": "ok",
        "message": "已清除 Provider 模型映射缓存",
        "provider_id": provider_id,
        "global_model_id": global_model_id,
        "deleted_keys": deleted_keys,
    }))
    .into_response()
}

pub fn match_admin_monitoring_route(
    method: &http::Method,
    path: &str,
) -> Option<AdminMonitoringRoute> {
    let path = normalize_admin_monitoring_path(path);

    match *method {
        http::Method::GET => match path {
            "/api/admin/monitoring/audit-logs" => Some(AdminMonitoringRoute::AuditLogs),
            "/api/admin/monitoring/system-status" => Some(AdminMonitoringRoute::SystemStatus),
            "/api/admin/monitoring/suspicious-activities" => {
                Some(AdminMonitoringRoute::SuspiciousActivities)
            }
            "/api/admin/monitoring/resilience-status" => {
                Some(AdminMonitoringRoute::ResilienceStatus)
            }
            "/api/admin/monitoring/resilience/circuit-history" => {
                Some(AdminMonitoringRoute::ResilienceCircuitHistory)
            }
            "/api/admin/monitoring/cache/stats" => Some(AdminMonitoringRoute::CacheStats),
            "/api/admin/monitoring/cache/affinities" => Some(AdminMonitoringRoute::CacheAffinities),
            "/api/admin/monitoring/cache/config" => Some(AdminMonitoringRoute::CacheConfig),
            "/api/admin/monitoring/cache/metrics" => Some(AdminMonitoringRoute::CacheMetrics),
            "/api/admin/monitoring/cache/model-mapping/stats" => {
                Some(AdminMonitoringRoute::CacheModelMappingStats)
            }
            "/api/admin/monitoring/cache/redis-keys" => Some(AdminMonitoringRoute::CacheRedisKeys),
            _ if matches_dynamic_segments(path, "/api/admin/monitoring/user-behavior/", 1) => {
                Some(AdminMonitoringRoute::UserBehavior)
            }
            _ if matches_dynamic_segments(
                path,
                "/api/admin/monitoring/trace/stats/provider/",
                1,
            ) =>
            {
                Some(AdminMonitoringRoute::TraceProviderStats)
            }
            _ if matches_dynamic_segments(path, "/api/admin/monitoring/trace/", 1) => {
                Some(AdminMonitoringRoute::TraceRequest)
            }
            _ if matches_dynamic_segments(path, "/api/admin/monitoring/cache/affinity/", 1) => {
                Some(AdminMonitoringRoute::CacheAffinity)
            }
            _ => None,
        },
        http::Method::DELETE => match path {
            "/api/admin/monitoring/resilience/error-stats" => {
                Some(AdminMonitoringRoute::ResilienceErrorStats)
            }
            "/api/admin/monitoring/cache" => Some(AdminMonitoringRoute::CacheFlush),
            "/api/admin/monitoring/cache/model-mapping" => {
                Some(AdminMonitoringRoute::CacheModelMappingDelete)
            }
            _ if matches_dynamic_segments(path, "/api/admin/monitoring/cache/users/", 1) => {
                Some(AdminMonitoringRoute::CacheUsersDelete)
            }
            _ if matches_dynamic_segments(path, "/api/admin/monitoring/cache/providers/", 1) => {
                Some(AdminMonitoringRoute::CacheProviderDelete)
            }
            _ if matches_dynamic_segments(path, "/api/admin/monitoring/cache/redis-keys/", 1) => {
                Some(AdminMonitoringRoute::CacheRedisKeysDelete)
            }
            _ if matches_dynamic_segments(
                path,
                "/api/admin/monitoring/cache/model-mapping/provider/",
                2,
            ) =>
            {
                Some(AdminMonitoringRoute::CacheModelMappingDeleteProvider)
            }
            _ if matches_dynamic_segments(
                path,
                "/api/admin/monitoring/cache/model-mapping/",
                1,
            ) =>
            {
                Some(AdminMonitoringRoute::CacheModelMappingDeleteModel)
            }
            _ if matches_dynamic_segments(path, "/api/admin/monitoring/cache/affinity/", 4) => {
                Some(AdminMonitoringRoute::CacheAffinityDelete)
            }
            _ => None,
        },
        _ => None,
    }
}

fn normalize_admin_monitoring_path(path: &str) -> &str {
    let normalized = path.trim_end_matches('/');
    if normalized.is_empty() {
        "/"
    } else {
        normalized
    }
}

fn path_identifier_from_path(request_path: &str, prefix: &str) -> Option<String> {
    let value = request_path
        .strip_prefix(prefix)?
        .trim()
        .trim_matches('/')
        .to_string();
    if value.is_empty() || value.contains('/') {
        None
    } else {
        Some(value)
    }
}

fn unix_secs_to_rfc3339(unix_secs: u64) -> Option<String> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix_secs as i64, 0)
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn query_param_value(query: Option<&str>, key: &str) -> Option<String> {
    let query = query?;
    for (entry_key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if entry_key == key {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn matches_dynamic_segments(path: &str, prefix: &str, dynamic_segments: usize) -> bool {
    let Some(suffix) = path.strip_prefix(prefix) else {
        return false;
    };

    let segments = suffix
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    segments.len() == dynamic_segments
}
