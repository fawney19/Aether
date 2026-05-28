use crate::handlers::admin::request::{AdminAppState, AdminRequestContext, AdminRouteRequest};
use crate::handlers::admin::shared::{
    attach_admin_audit_response, query_param_bool, query_param_optional_bool, query_param_value,
    unix_secs_to_rfc3339,
};
use crate::important_notification::{
    important_notification_dispatch_ready_for_item, RISK_CONTROL_AUTO_ACTION_ITEM_KEY,
    RISK_CONTROL_FLAGGED_ITEM_KEY, RISK_CONTROL_USER_ACTION_NOTICE_ITEM_KEY,
};
use crate::maintenance::{read_risk_control_retention_status, run_risk_control_retention_once};
use crate::risk_control::{
    provider_key_statuses, read_risk_control_runtime_config, test_risk_control_text,
    validate_keyword_patterns, RiskControlKeywordMatchMode, RiskControlKeywordMode,
    RiskControlMode, RiskControlModelFilterMode, RiskControlRuntimeConfig, RiskControlScopeMode,
    RISK_CONTROL_CONFIG_KEY, RISK_CONTROL_ENABLED_CONFIG_KEY,
};
use crate::GatewayError;
use aether_data::repository::risk_control::{
    RiskControlHashListQuery, RiskControlLogListQuery, StoredRiskControlFlaggedHash,
    StoredRiskControlLog, StoredRiskControlNotificationOutbox,
};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::collections::BTreeMap;

const DEFAULT_PAGE_SIZE: usize = 20;
const MAX_PAGE_SIZE: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NotificationTarget {
    item_key: &'static str,
    label: &'static str,
}

pub(crate) async fn maybe_build_local_admin_risk_control_response(
    request: AdminRouteRequest<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let state = request.state();
    let request_context = request.request_context();
    if request_context.route_family() != Some("risk_control_manage") {
        return Ok(None);
    }

    match request_context.route_kind() {
        Some("status") if request_context.method() == http::Method::GET => {
            return Ok(Some(Json(status_payload(&state).await?).into_response()));
        }
        Some("config") if request_context.method() == http::Method::GET => {
            let config = read_risk_control_runtime_config(state.app()).await?;
            let mut config_payload = config.redacted_json();
            config_payload["provider_key_statuses"] =
                json!(provider_key_statuses(&config.provider.api_keys));
            return Ok(Some(
                Json(json!({
                    "enabled": config.enabled,
                    "config": config_payload,
                }))
                .into_response(),
            ));
        }
        Some("config") if request_context.method() == http::Method::PUT => {
            let body = required_body(request.request_body())?;
            return Ok(Some(match update_config_payload(&state, body).await? {
                Ok(payload) => Json(payload).into_response(),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            }));
        }
        Some("logs") if request_context.method() == http::Method::GET => {
            return Ok(Some(
                Json(logs_payload(&state, &request_context).await?).into_response(),
            ));
        }
        Some("retry_notification") if request_context.method() == http::Method::POST => {
            let Some(log_id) = notification_retry_log_id(request_context.path()) else {
                return Ok(Some(
                    (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({"detail": "Risk control log not found"})),
                    )
                        .into_response(),
                ));
            };
            let retried = state
                .app()
                .data
                .reset_risk_control_notification_outbox_for_retry(log_id, current_unix_secs())
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
            let response = if retried.is_empty() {
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({"detail": "没有可重试的通知任务"})),
                )
                    .into_response()
            } else {
                let primary = primary_notification_outbox(&retried);
                Json(json!({
                    "queued": true,
                    "notification": primary.map(notification_outbox_payload),
                    "notifications": retried.iter().map(notification_outbox_payload).collect::<Vec<_>>(),
                }))
                .into_response()
            };
            return Ok(Some(attach_admin_audit_response(
                response,
                "risk_control_notification_retry_queued",
                "retry_notification",
                "risk_control_log",
                log_id,
            )));
        }
        Some("hashes") if request_context.method() == http::Method::GET => {
            return Ok(Some(
                Json(hashes_payload(&state, &request_context).await?).into_response(),
            ));
        }
        Some("delete_hash") if request_context.method() == http::Method::DELETE => {
            let Some(input_hash) =
                path_suffix(request_context.path(), "/api/admin/risk-control/hashes/")
            else {
                return Ok(Some(
                    (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({"detail": "Hash not found"})),
                    )
                        .into_response(),
                ));
            };
            let deleted = state
                .app()
                .data
                .delete_risk_control_flagged_hash(input_hash)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
            return Ok(Some(Json(json!({ "deleted": deleted })).into_response()));
        }
        Some("clear_hashes") if request_context.method() == http::Method::DELETE => {
            let deleted = state
                .app()
                .data
                .clear_risk_control_flagged_hashes()
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
            return Ok(Some(Json(json!({ "deleted": deleted })).into_response()));
        }
        Some("test") if request_context.method() == http::Method::POST => {
            let body = required_body(request.request_body())?;
            return Ok(Some(match test_payload(&state, body).await? {
                Ok(payload) => Json(payload).into_response(),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            }));
        }
        Some("provider_keys_test") if request_context.method() == http::Method::POST => {
            let body = required_body(request.request_body())?;
            return Ok(Some(match test_payload(&state, body).await? {
                Ok(payload) => Json(payload).into_response(),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            }));
        }
        Some("retention_run") if request_context.method() == http::Method::POST => {
            let deleted = run_risk_control_retention_once(state.app()).await?;
            return Ok(Some(
                Json(json!({
                    "hit_deleted": deleted.hit_deleted,
                    "non_hit_deleted": deleted.non_hit_deleted,
                }))
                .into_response(),
            ));
        }
        Some("unlock_user_api_key") if request_context.method() == http::Method::POST => {
            let Some((user_id, api_key_id)) = user_api_key_unlock_parts(request_context.path())
            else {
                return Ok(Some(
                    (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({"detail": "API Key not found"})),
                    )
                        .into_response(),
                ));
            };
            let updated = state
                .app()
                .set_user_api_key_locked(user_id, api_key_id, false)
                .await?;
            return Ok(Some(attach_admin_audit_response(
                match updated {
                    true => Json(json!({
                        "updated": true,
                        "api_key": {
                            "id": api_key_id,
                            "user_id": user_id,
                            "is_locked": false,
                        }
                    }))
                    .into_response(),
                    false => (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({"detail": "API Key not found"})),
                    )
                        .into_response(),
                },
                "risk_control_auto_action_api_key_unlocked",
                "unlock_user_api_key",
                "user_api_key",
                api_key_id,
            )));
        }
        Some("unban_user") if request_context.method() == http::Method::POST => {
            let Some(user_id) = user_id_from_unban_path(request_context.path()) else {
                return Ok(Some(
                    (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({"detail": "User not found"})),
                    )
                        .into_response(),
                ));
            };
            let updated = state
                .app()
                .update_local_auth_user_admin_fields(
                    user_id,
                    None,
                    false,
                    None,
                    false,
                    None,
                    false,
                    None,
                    false,
                    None,
                    Some(true),
                )
                .await?;
            return Ok(Some(attach_admin_audit_response(
                match updated {
                    Some(user) => Json(json!({
                        "updated": true,
                        "user": {
                            "id": user.id,
                            "username": user.username,
                            "email": user.email,
                            "is_active": user.is_active,
                        }
                    }))
                    .into_response(),
                    None => (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({"detail": "User not found"})),
                    )
                        .into_response(),
                },
                "risk_control_auto_action_user_restored",
                "restore_user",
                "user",
                user_id,
            )));
        }
        _ => {}
    }

    Ok(None)
}

async fn status_payload(state: &AdminAppState<'_>) -> Result<serde_json::Value, GatewayError> {
    let config = read_risk_control_runtime_config(state.app()).await?;
    let logs = state
        .app()
        .data
        .list_risk_control_logs(&RiskControlLogListQuery {
            offset: 0,
            limit: 1,
            ..RiskControlLogListQuery::default()
        })
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let flagged = state
        .app()
        .data
        .list_risk_control_logs(&RiskControlLogListQuery {
            offset: 0,
            limit: 1,
            flagged: Some(true),
            ..RiskControlLogListQuery::default()
        })
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let hash_count = state
        .app()
        .data
        .count_risk_control_flagged_hashes()
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let (config_validated, config_error) = validate_config(&config);
    let (notification_ready, notification_warning) = notification_status(state, &config).await;
    let notification_outbox = state
        .app()
        .data
        .risk_control_notification_outbox_summary()
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let retention_status = read_risk_control_retention_status(state.app(), &config).await?;
    let observe_queue = state.app().risk_control_observe_queue.snapshot();
    Ok(json!({
        "enabled": config.enabled,
        "mode": config.mode.as_str(),
        "keyword_mode": config.keyword_mode,
        "config_validated": config_validated,
        "config_error": config_error,
        "notification_ready": notification_ready,
        "notification_warning": notification_warning,
        "notification_outbox": notification_outbox,
        "retention_status": retention_status,
        "observe_queue": observe_queue,
        "logs_total": logs.total,
        "flagged_total": flagged.total,
        "flagged_hashes_total": hash_count,
        "provider_key_count": config.provider.api_keys.len(),
        "provider_key_statuses": provider_key_statuses(&config.provider.api_keys),
        "keyword_count": config.keywords.len(),
    }))
}

async fn logs_payload(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<serde_json::Value, GatewayError> {
    let query = page_query(request_context.query_string());
    let include_excerpt = include_sensitive_excerpt(request_context);
    let page = state
        .app()
        .data
        .list_risk_control_logs(&RiskControlLogListQuery {
            offset: query.offset,
            limit: query.limit,
            user_id: query_param_value(request_context.query_string(), "user_id"),
            api_key_id: query_param_value(request_context.query_string(), "api_key_id"),
            flagged: query_param_optional_bool(request_context.query_string(), "flagged"),
            action: query_param_value(request_context.query_string(), "action"),
            decision_source: query_param_value(request_context.query_string(), "decision_source"),
            endpoint: query_param_value(request_context.query_string(), "endpoint"),
            model: query_param_value(request_context.query_string(), "model"),
            q: query_param_value(request_context.query_string(), "q"),
            from_unix_secs: query_param_value(request_context.query_string(), "from")
                .and_then(|value| value.parse::<u64>().ok()),
            to_unix_secs: query_param_value(request_context.query_string(), "to")
                .and_then(|value| value.parse::<u64>().ok()),
        })
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let log_ids = page
        .items
        .iter()
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    let outboxes = state
        .app()
        .data
        .list_risk_control_notification_outbox_by_log_ids(&log_ids)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
        .into_iter()
        .fold(
            BTreeMap::<String, Vec<StoredRiskControlNotificationOutbox>>::new(),
            |mut map, item| {
                map.entry(item.log_id.clone()).or_default().push(item);
                map
            },
        );
    Ok(json!({
        "items": page.items.iter().map(|item| {
            log_payload(item, include_excerpt, notification_outboxes_for_log(&outboxes, &item.id))
        }).collect::<Vec<_>>(),
        "total": page.total,
        "page": query.page,
        "page_size": query.limit,
        "pages": pages(page.total, query.limit),
    }))
}

async fn hashes_payload(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<serde_json::Value, GatewayError> {
    let query = page_query(request_context.query_string());
    let include_excerpt = include_sensitive_excerpt(request_context);
    let page = state
        .app()
        .data
        .list_risk_control_flagged_hashes(&RiskControlHashListQuery {
            offset: query.offset,
            limit: query.limit,
        })
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    Ok(json!({
        "items": page.items.iter().map(|item| hash_payload(item, include_excerpt)).collect::<Vec<_>>(),
        "total": page.total,
        "page": query.page,
        "page_size": query.limit,
        "pages": pages(page.total, query.limit),
    }))
}

async fn update_config_payload(
    state: &AdminAppState<'_>,
    body: &Bytes,
) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError> {
    let raw = match parse_json_body(body) {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    let current = read_risk_control_runtime_config(state.app()).await?;
    let enabled = raw
        .get("enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(current.enabled);
    let config_value = raw.get("config").cloned().unwrap_or_else(|| {
        if raw
            .as_object()
            .is_some_and(|map| map.keys().all(|key| key == "enabled"))
        {
            serde_json::to_value(&current).unwrap_or_else(|_| json!({}))
        } else {
            raw.clone()
        }
    });
    let mut next: RiskControlRuntimeConfig = match serde_json::from_value(config_value) {
        Ok(value) => value,
        Err(err) => {
            return Ok(Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": format!("风控配置格式无效: {err}") }),
            )));
        }
    };
    next.provider.api_keys =
        preserve_masked_api_keys(&current.provider.api_keys, &next.provider.api_keys);
    next.enabled = enabled;
    next = next.sanitized();
    let (valid, error) = validate_config(&next);
    if !valid {
        return Ok(Err((
            http::StatusCode::BAD_REQUEST,
            json!({ "detail": error.unwrap_or_else(|| "风控配置无效".to_string()) }),
        )));
    }
    state
        .app()
        .upsert_system_config_json_value(
            RISK_CONTROL_ENABLED_CONFIG_KEY,
            &json!(enabled),
            Some("Risk control center enabled"),
        )
        .await?;
    state
        .app()
        .upsert_system_config_json_value(
            RISK_CONTROL_CONFIG_KEY,
            &serde_json::to_value(&next).map_err(|err| GatewayError::Internal(err.to_string()))?,
            Some("Risk control center config"),
        )
        .await?;
    let mut config_payload = next.redacted_json();
    config_payload["provider_key_statuses"] = json!(provider_key_statuses(&next.provider.api_keys));
    Ok(Ok(json!({
        "enabled": enabled,
        "config": config_payload,
        "config_validated": valid,
        "config_error": error,
    })))
}

async fn test_payload(
    state: &AdminAppState<'_>,
    body: &Bytes,
) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError> {
    let raw = match parse_json_body(body) {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    let text = raw
        .get("text")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("hello");
    let current = read_risk_control_runtime_config(state.app()).await?;
    let mut config = if let Some(value) = raw.get("config") {
        match serde_json::from_value::<RiskControlRuntimeConfig>(value.clone()) {
            Ok(value) => value,
            Err(err) => {
                return Ok(Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": format!("风控配置格式无效: {err}") }),
                )));
            }
        }
    } else {
        current.clone()
    };
    config.provider.api_keys =
        preserve_masked_api_keys(&current.provider.api_keys, &config.provider.api_keys);
    if let Some(keys) = raw.get("api_keys").and_then(serde_json::Value::as_array) {
        config.provider.api_keys = keys
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    config = config.sanitized();
    if config.keyword_match_mode == RiskControlKeywordMatchMode::Regex
        && !matches!(config.keyword_mode, RiskControlKeywordMode::ApiOnly)
    {
        if let Err(err) = validate_keyword_patterns(&config) {
            return Ok(Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": err }),
            )));
        }
    }
    let result = test_risk_control_text(state.app(), &config, text).await?;
    Ok(Ok(json!({
        "input_excerpt": text.chars().take(256).collect::<String>(),
        "result": result,
        "provider_key_statuses": provider_key_statuses(&config.provider.api_keys),
    })))
}

fn validate_config(config: &RiskControlRuntimeConfig) -> (bool, Option<String>) {
    if !matches!(config.model_filter.mode, RiskControlModelFilterMode::All)
        && config.model_filter.models.is_empty()
    {
        return (
            false,
            Some("模型范围选择包含或排除时，需要至少填写一个模型".to_string()),
        );
    }
    if let Some(name) = empty_scope_list_name(config) {
        return (
            false,
            Some(format!(
                "策略粒度「{name}」选择包含或排除时，需要至少填写一个值"
            )),
        );
    }
    if !config.enabled || config.mode == RiskControlMode::Off {
        return (true, None);
    }
    if config.keyword_mode == RiskControlKeywordMode::KeywordOnly && config.keywords.is_empty() {
        return (false, Some("关键词模式需要至少配置一个关键词".to_string()));
    }
    if config.keyword_match_mode == RiskControlKeywordMatchMode::Regex
        && !matches!(config.keyword_mode, RiskControlKeywordMode::ApiOnly)
    {
        if let Err(err) = validate_keyword_patterns(config) {
            return (false, Some(err));
        }
    }
    if !matches!(config.keyword_mode, RiskControlKeywordMode::KeywordOnly)
        && config.provider.api_keys.is_empty()
    {
        return (
            false,
            Some("API 审核模式需要配置 provider.api_keys".to_string()),
        );
    }
    if !matches!(config.keyword_mode, RiskControlKeywordMode::KeywordOnly) {
        if let Err(err) = config.validate_provider_base_url() {
            return (false, Some(err));
        }
    }
    (true, None)
}

fn empty_scope_list_name(config: &RiskControlRuntimeConfig) -> Option<&'static str> {
    [
        (
            "用户 ID",
            config.scope.users.mode,
            config.scope.users.values.len(),
        ),
        (
            "用户组",
            config.scope.user_groups.mode,
            config.scope.user_groups.values.len(),
        ),
        (
            "API Key ID",
            config.scope.api_keys.mode,
            config.scope.api_keys.values.len(),
        ),
        (
            "Route Family",
            config.scope.route_families.mode,
            config.scope.route_families.values.len(),
        ),
        (
            "Route Kind",
            config.scope.route_kinds.mode,
            config.scope.route_kinds.values.len(),
        ),
        (
            "端点签名",
            config.scope.endpoints.mode,
            config.scope.endpoints.values.len(),
        ),
    ]
    .into_iter()
    .find_map(|(name, mode, len)| {
        (!matches!(mode, RiskControlScopeMode::All) && len == 0).then_some(name)
    })
}

async fn notification_status(
    state: &AdminAppState<'_>,
    config: &RiskControlRuntimeConfig,
) -> (bool, Option<String>) {
    if !config.notification.enabled {
        return (false, None);
    }
    let targets = notification_targets(config);
    if targets.is_empty() {
        return (
            false,
            Some("通知已启用，但未选择命中事件、自动处置或用户处置通知".to_string()),
        );
    }

    let mut unavailable = Vec::new();
    for target in targets {
        match important_notification_dispatch_ready_for_item(state.app(), target.item_key).await {
            Ok(true) => {}
            Ok(false) => unavailable.push(target.label),
            Err(err) => {
                return (false, Some(format!("通知中心状态读取失败：{err:?}")));
            }
        }
    }

    if unavailable.is_empty() {
        (true, None)
    } else {
        (
            false,
            Some(format!(
                "通知已启用，但通知中心事件或渠道未就绪：{}",
                unavailable.join("、")
            )),
        )
    }
}

fn notification_targets(config: &RiskControlRuntimeConfig) -> Vec<NotificationTarget> {
    if !config.notification.enabled {
        return Vec::new();
    }
    let mut targets = Vec::new();
    if config.notification.notify_on_flagged {
        targets.push(NotificationTarget {
            item_key: RISK_CONTROL_FLAGGED_ITEM_KEY,
            label: "风控命中告警",
        });
    }
    if config.notification.notify_on_auto_action {
        targets.push(NotificationTarget {
            item_key: RISK_CONTROL_AUTO_ACTION_ITEM_KEY,
            label: "风控自动处置",
        });
    }
    if config.notification.notify_on_user_action_notice {
        targets.push(NotificationTarget {
            item_key: RISK_CONTROL_USER_ACTION_NOTICE_ITEM_KEY,
            label: "风控用户处置通知",
        });
    }
    targets
}

fn include_sensitive_excerpt(request_context: &AdminRequestContext<'_>) -> bool {
    if !query_param_bool(request_context.query_string(), "include_excerpt", false) {
        return false;
    }
    let Some(principal) = request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
    else {
        return false;
    };
    if !crate::roles::is_full_admin_role(&principal.user_role) {
        return false;
    }
    let Some(permissions) = principal.management_token_permissions.as_deref() else {
        return true;
    };
    permissions.iter().any(|permission| {
        matches!(
            permission.as_str(),
            "admin:risk_control:write" | "admin:risk_control:admin"
        )
    }) || crate::control::management_token_permissions_cover_all_assignable_permissions(permissions)
}

fn redacted_excerpt_payload(
    excerpt: Option<&str>,
    include_sensitive: bool,
) -> (Option<String>, bool) {
    match (excerpt, include_sensitive) {
        (Some(value), true) => (Some(value.to_string()), false),
        (Some(_), false) => (None, true),
        (None, _) => (None, false),
    }
}

fn log_payload(
    item: &StoredRiskControlLog,
    include_sensitive_excerpt: bool,
    notification_outboxes: &[StoredRiskControlNotificationOutbox],
) -> serde_json::Value {
    let (excerpt, excerpt_redacted) =
        redacted_excerpt_payload(item.excerpt.as_deref(), include_sensitive_excerpt);
    let primary_notification_outbox = primary_notification_outbox(notification_outboxes);
    json!({
        "id": item.id,
        "trace_id": item.trace_id,
        "request_id": item.request_id,
        "user_id": item.user_id,
        "username": item.username,
        "user_email": item.user_email,
        "api_key_id": item.api_key_id,
        "api_key_name": item.api_key_name,
        "route_family": item.route_family,
        "route_kind": item.route_kind,
        "api_format": item.api_format,
        "endpoint": item.endpoint,
        "model": item.model,
        "mode": item.mode,
        "action": item.action,
        "decision_source": item.decision_source,
        "flagged": item.flagged,
        "highest_category": item.highest_category,
        "highest_score": item.highest_score,
        "category_scores": item.category_scores,
        "thresholds": item.thresholds,
        "matched_keywords": item.matched_keywords,
        "input_hash": item.input_hash,
        "excerpt": excerpt,
        "excerpt_redacted": excerpt_redacted,
        "excerpt_redaction_reason": excerpt_redacted.then_some("sensitive_excerpt_hidden"),
        "latency_ms": item.latency_ms,
        "queue_delay_ms": item.queue_delay_ms,
        "violation_count": item.violation_count,
        "auto_action": item.auto_action,
        "auto_action_enforced": item.auto_action.is_some() && item.mode == "pre_block",
        "notification_sent": item.notification_sent,
        "notification_attempts": item.notification_attempts,
        "notification_last_error": item.notification_last_error,
        "notification_last_attempt_at": item.notification_last_attempt_at_unix_secs.map(unix_secs_to_rfc3339),
        "notification_last_attempt_at_unix_secs": item.notification_last_attempt_at_unix_secs,
        "notification_outbox": primary_notification_outbox.map(notification_outbox_payload),
        "notification_outboxes": notification_outboxes.iter().map(notification_outbox_payload).collect::<Vec<_>>(),
        "error_message": item.error_message,
        "created_at": unix_secs_to_rfc3339(item.created_at_unix_secs),
        "created_at_unix_secs": item.created_at_unix_secs,
    })
}

fn notification_outboxes_for_log<'a>(
    outboxes: &'a BTreeMap<String, Vec<StoredRiskControlNotificationOutbox>>,
    log_id: &str,
) -> &'a [StoredRiskControlNotificationOutbox] {
    outboxes.get(log_id).map(Vec::as_slice).unwrap_or(&[])
}

fn primary_notification_outbox(
    outboxes: &[StoredRiskControlNotificationOutbox],
) -> Option<&StoredRiskControlNotificationOutbox> {
    outboxes.iter().min_by(|left, right| {
        notification_outbox_rank(&left.status)
            .cmp(&notification_outbox_rank(&right.status))
            .then_with(|| right.updated_at_unix_secs.cmp(&left.updated_at_unix_secs))
    })
}

fn notification_outbox_rank(status: &str) -> u8 {
    match status {
        "dead" => 0,
        "processing" => 1,
        "pending" => 2,
        "sent" => 3,
        _ => 4,
    }
}

fn notification_outbox_payload(item: &StoredRiskControlNotificationOutbox) -> serde_json::Value {
    json!({
        "id": item.id,
        "log_id": item.log_id,
        "item_key": item.item_key,
        "status": item.status,
        "attempt_count": item.attempt_count,
        "max_attempts": item.max_attempts,
        "next_attempt_at": item.next_attempt_at_unix_secs.map(unix_secs_to_rfc3339),
        "next_attempt_at_unix_secs": item.next_attempt_at_unix_secs,
        "lease_until": item.lease_until_unix_secs.map(unix_secs_to_rfc3339),
        "lease_until_unix_secs": item.lease_until_unix_secs,
        "last_error": item.last_error,
        "created_at": unix_secs_to_rfc3339(item.created_at_unix_secs),
        "created_at_unix_secs": item.created_at_unix_secs,
        "updated_at": unix_secs_to_rfc3339(item.updated_at_unix_secs),
        "updated_at_unix_secs": item.updated_at_unix_secs,
        "sent_at": item.sent_at_unix_secs.map(unix_secs_to_rfc3339),
        "sent_at_unix_secs": item.sent_at_unix_secs,
    })
}

fn hash_payload(
    item: &StoredRiskControlFlaggedHash,
    include_sensitive_excerpt: bool,
) -> serde_json::Value {
    let (excerpt, excerpt_redacted) =
        redacted_excerpt_payload(item.excerpt.as_deref(), include_sensitive_excerpt);
    json!({
        "input_hash": item.input_hash,
        "source_log_id": item.source_log_id,
        "reason": item.reason,
        "highest_category": item.highest_category,
        "highest_score": item.highest_score,
        "excerpt": excerpt,
        "excerpt_redacted": excerpt_redacted,
        "excerpt_redaction_reason": excerpt_redacted.then_some("sensitive_excerpt_hidden"),
        "first_seen_at": unix_secs_to_rfc3339(item.first_seen_at_unix_secs),
        "first_seen_at_unix_secs": item.first_seen_at_unix_secs,
        "last_seen_at": unix_secs_to_rfc3339(item.last_seen_at_unix_secs),
        "last_seen_at_unix_secs": item.last_seen_at_unix_secs,
        "hit_count": item.hit_count,
    })
}

#[derive(Debug, Clone, Copy)]
struct PageQuery {
    page: usize,
    offset: usize,
    limit: usize,
}

fn page_query(query: Option<&str>) -> PageQuery {
    let page = query_param_value(query, "page")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let limit = query_param_value(query, "page_size")
        .or_else(|| query_param_value(query, "limit"))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    PageQuery {
        page,
        offset: page.saturating_sub(1).saturating_mul(limit),
        limit,
    }
}

fn pages(total: u64, limit: usize) -> u64 {
    if total == 0 {
        0
    } else {
        (total + limit as u64 - 1) / limit as u64
    }
}

fn required_body(body: Option<&Bytes>) -> Result<&Bytes, GatewayError> {
    body.ok_or_else(|| {
        GatewayError::Internal("admin risk control request body missing".to_string())
    })
}

fn parse_json_body(
    body: &Bytes,
) -> Result<serde_json::Value, (http::StatusCode, serde_json::Value)> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(body).map_err(|err| {
        (
            http::StatusCode::BAD_REQUEST,
            json!({ "detail": format!("请求体必须是合法 JSON: {err}") }),
        )
    })
}

fn preserve_masked_api_keys(existing: &[String], incoming: &[String]) -> Vec<String> {
    incoming
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            let value = value.trim();
            if value.is_empty() {
                None
            } else if value.contains("****") {
                existing
                    .get(index)
                    .filter(|existing| mask_provider_key(existing) == value)
                    .cloned()
                    .or_else(|| {
                        existing
                            .iter()
                            .find(|candidate| mask_provider_key(candidate) == value)
                            .cloned()
                    })
            } else {
                Some(value.to_string())
            }
        })
        .collect()
}

fn mask_provider_key(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return String::new();
    }
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= 8 {
        return "****".to_string();
    }
    let prefix = chars.iter().take(4).collect::<String>();
    let suffix = chars
        .iter()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{prefix}****{suffix}")
}

fn path_suffix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let value = path.strip_prefix(prefix)?.trim_matches('/');
    (!value.is_empty() && !value.contains('/')).then_some(value)
}

fn user_id_from_unban_path(path: &str) -> Option<&str> {
    path.strip_prefix("/api/admin/risk-control/users/")
        .and_then(|value| value.strip_suffix("/unban"))
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.contains('/'))
}

fn user_api_key_unlock_parts(path: &str) -> Option<(&str, &str)> {
    let value = path
        .strip_prefix("/api/admin/risk-control/users/")?
        .strip_suffix("/unlock")?;
    let mut parts = value.split('/');
    let user_id = parts.next()?.trim();
    let api_keys = parts.next()?.trim();
    let api_key_id = parts.next()?.trim();
    if parts.next().is_some()
        || user_id.is_empty()
        || api_key_id.is_empty()
        || api_keys != "api-keys"
    {
        return None;
    }
    Some((user_id, api_key_id))
}

fn notification_retry_log_id(path: &str) -> Option<&str> {
    path.strip_prefix("/api/admin/risk-control/logs/")
        .and_then(|value| value.strip_suffix("/notification/retry"))
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.contains('/'))
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserve_masked_api_keys_matches_by_mask_when_reordered() {
        let existing = vec!["key-first-1111".to_string(), "key-second-2222".to_string()];
        let incoming = vec![
            mask_provider_key(&existing[1]),
            mask_provider_key(&existing[0]),
            "key-new-3333".to_string(),
        ];

        assert_eq!(
            preserve_masked_api_keys(&existing, &incoming),
            vec![
                "key-second-2222".to_string(),
                "key-first-1111".to_string(),
                "key-new-3333".to_string(),
            ]
        );
    }

    #[test]
    fn notification_targets_follow_enabled_events() {
        let mut config = RiskControlRuntimeConfig::default();
        assert!(notification_targets(&config).is_empty());

        config.notification.enabled = true;
        assert_eq!(
            notification_targets(&config),
            vec![
                NotificationTarget {
                    item_key: RISK_CONTROL_FLAGGED_ITEM_KEY,
                    label: "风控命中告警",
                },
                NotificationTarget {
                    item_key: RISK_CONTROL_AUTO_ACTION_ITEM_KEY,
                    label: "风控自动处置",
                },
            ]
        );

        config.notification.notify_on_flagged = false;
        assert_eq!(
            notification_targets(&config),
            vec![NotificationTarget {
                item_key: RISK_CONTROL_AUTO_ACTION_ITEM_KEY,
                label: "风控自动处置",
            }]
        );

        config.notification.notify_on_auto_action = false;
        assert!(notification_targets(&config).is_empty());

        config.notification.notify_on_user_action_notice = true;
        assert_eq!(
            notification_targets(&config),
            vec![NotificationTarget {
                item_key: RISK_CONTROL_USER_ACTION_NOTICE_ITEM_KEY,
                label: "风控用户处置通知",
            }]
        );
    }

    #[test]
    fn risk_control_excerpt_payload_is_redacted_by_default() {
        assert_eq!(
            redacted_excerpt_payload(Some("sensitive prompt"), false),
            (None, true)
        );
        assert_eq!(
            redacted_excerpt_payload(Some("sensitive prompt"), true),
            (Some("sensitive prompt".to_string()), false)
        );
        assert_eq!(redacted_excerpt_payload(None, false), (None, false));
    }

    #[test]
    fn risk_control_api_key_unlock_path_extracts_user_and_key() {
        assert_eq!(
            user_api_key_unlock_parts("/api/admin/risk-control/users/user-1/api-keys/key-1/unlock"),
            Some(("user-1", "key-1"))
        );
        assert!(user_api_key_unlock_parts(
            "/api/admin/risk-control/users/user-1/api-keys/key-1/other"
        )
        .is_none());
    }

    #[test]
    fn risk_control_notification_retry_path_extracts_log_id() {
        assert_eq!(
            notification_retry_log_id("/api/admin/risk-control/logs/log-1/notification/retry"),
            Some("log-1")
        );
        assert!(notification_retry_log_id("/api/admin/risk-control/logs/log-1/other").is_none());
    }
}
