use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde_json::json;

use crate::control::{GatewayControlAuthContext, GatewayControlDecision};

use super::{
    build_auth_error_response, resolve_authenticated_local_user, AppState,
    GatewayPublicRequestContext,
};

fn normalize_rate_limit_value(value: Option<i32>) -> u32 {
    value
        .map(|raw| raw.max(0))
        .and_then(|raw| u32::try_from(raw).ok())
        .unwrap_or(0)
}

fn daily_usage_available_payload(
    status: crate::daily_usage_limit::FrontdoorDailyUsageStatus,
) -> serde_json::Value {
    if !status.available {
        return daily_usage_empty_payload(
            false,
            "unavailable",
            &status.timezone,
            &status.window_start,
            &status.window_end,
            &status.window_end,
        );
    }
    let primary = match (status.user.as_ref(), status.key.as_ref()) {
        (Some(user), Some(key)) => {
            if user.remaining_usd <= key.remaining_usd {
                Some(user)
            } else {
                Some(key)
            }
        }
        (Some(user), None) => Some(user),
        (None, Some(key)) => Some(key),
        (None, None) => None,
    };
    json!({
        "available": true,
        "status": "available",
        "limit_usd": primary.map(|scope| scope.limit_usd),
        "used_usd": primary.map(|scope| scope.used_usd),
        "remaining_usd": primary.map(|scope| scope.remaining_usd),
        "scope": primary.map(|scope| scope.scope),
        "user": status.user.map(|scope| json!({
            "limit_usd": scope.limit_usd,
            "used_usd": scope.used_usd,
            "remaining_usd": scope.remaining_usd,
        })),
        "key": status.key.map(|scope| json!({
            "limit_usd": scope.limit_usd,
            "used_usd": scope.used_usd,
            "remaining_usd": scope.remaining_usd,
        })),
        "timezone": status.timezone,
        "window": "1d",
        "window_start": status.window_start,
        "window_end": status.window_end,
        "reset_time": chrono::DateTime::<Utc>::from_timestamp(
            status.reset_at_unix_secs as i64,
            0,
        ).map(|value| value.to_rfc3339()),
    })
}

fn daily_usage_empty_payload(
    available: bool,
    status: &'static str,
    timezone: &str,
    window_start: &str,
    window_end: &str,
    reset_time: &str,
) -> serde_json::Value {
    json!({
        "available": available,
        "status": status,
        "limit_usd": null,
        "used_usd": null,
        "remaining_usd": null,
        "scope": null,
        "user": null,
        "key": null,
        "timezone": timezone,
        "window": "1d",
        "window_start": window_start,
        "window_end": window_end,
        "reset_time": reset_time,
    })
}

pub(super) async fn handle_user_rate_limit_status(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };

    let limiter = state.frontdoor_user_rpm();
    let now = Utc::now();
    let now_unix_secs = u64::try_from(now.timestamp()).unwrap_or(0);
    let system_default_limit = match limiter.current_system_default_limit(state).await {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(error = ?err, "user rate limit status system default read failed");
            0
        }
    };
    let bucket = limiter.current_bucket(now_unix_secs);
    let reset_time = (now
        + chrono::Duration::seconds(
            i64::try_from(limiter.retry_after(now_unix_secs)).unwrap_or(0),
        ))
    .to_rfc3339();
    let window = format!("{}s", limiter.config().bucket_seconds());
    let daily_limiter = state.frontdoor_daily_usage();
    let daily_timezone = crate::app_timezone::app_timezone();
    let (_, daily_window_start, daily_window_end) =
        crate::app_timezone::local_day_window(now, daily_timezone);
    let daily_window_start = daily_window_start.to_rfc3339();
    let daily_window_end = daily_window_end.to_rfc3339();
    let daily_reset_at = daily_window_end.clone();

    let export_records = match state
        .list_auth_api_key_export_records_by_user_ids(std::slice::from_ref(&auth.user.id))
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user rate limit status read failed: {err:?}"),
                false,
            )
        }
    };

    let mut api_keys = Vec::new();
    for record in export_records {
        if !record.is_active {
            continue;
        }

        let snapshot = match state
            .data
            .read_auth_api_key_snapshot(&auth.user.id, &record.api_key_id, now_unix_secs)
            .await
        {
            Ok(value) => value,
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("user api key snapshot read failed: {err:?}"),
                    false,
                )
            }
        };
        let is_standalone = snapshot
            .as_ref()
            .map(|value| value.api_key_is_standalone)
            .unwrap_or(record.is_standalone);
        let user_limit = if is_standalone {
            match record.rate_limit {
                Some(value) => normalize_rate_limit_value(Some(value)),
                None => system_default_limit,
            }
        } else {
            normalize_rate_limit_value(
                snapshot
                    .as_ref()
                    .and_then(|value| value.user_rate_limit)
                    .or(Some(
                        i32::try_from(system_default_limit).unwrap_or(i32::MAX),
                    )),
            )
        };
        let key_limit = if is_standalone {
            0
        } else {
            normalize_rate_limit_value(
                snapshot
                    .as_ref()
                    .and_then(|value| value.api_key_rate_limit)
                    .or(record.rate_limit),
            )
        };

        let user_scope_key = if is_standalone {
            limiter.standalone_scope_key(&record.api_key_id, bucket)
        } else {
            limiter.user_scope_key(&auth.user.id, bucket)
        };
        let key_scope_key = limiter.key_scope_key(&record.api_key_id, bucket);

        let user_count = if user_limit > 0 {
            match limiter
                .get_scope_count(state, &user_scope_key, bucket)
                .await
            {
                Ok(value) => value,
                Err(err) => {
                    tracing::warn!(error = ?err, scope_key = %user_scope_key, "user rpm scope read failed");
                    0
                }
            }
        } else {
            0
        };
        let key_count = if key_limit > 0 {
            match limiter.get_scope_count(state, &key_scope_key, bucket).await {
                Ok(value) => value,
                Err(err) => {
                    tracing::warn!(error = ?err, scope_key = %key_scope_key, "api key rpm scope read failed");
                    0
                }
            }
        } else {
            0
        };

        let user_remaining = if user_limit > 0 {
            Some(user_limit.saturating_sub(user_count))
        } else {
            None
        };
        let key_remaining = if key_limit > 0 {
            Some(key_limit.saturating_sub(key_count))
        } else {
            None
        };

        let primary_scope = match (user_remaining, key_remaining) {
            (Some(user_remaining), Some(key_remaining)) => {
                if user_remaining <= key_remaining {
                    Some(("user", user_limit, user_remaining))
                } else {
                    Some(("key", key_limit, key_remaining))
                }
            }
            (Some(user_remaining), None) => Some(("user", user_limit, user_remaining)),
            (None, Some(key_remaining)) => Some(("key", key_limit, key_remaining)),
            (None, None) => None,
        };

        let mut daily_decision = GatewayControlDecision::synthetic(
            "/v1/monitoring/rate-limit-status",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
        );
        daily_decision.auth_context = Some(GatewayControlAuthContext {
            user_id: auth.user.id.clone(),
            api_key_id: record.api_key_id.clone(),
            username: Some(auth.user.username.clone()),
            api_key_name: record.name.clone(),
            balance_remaining: None,
            access_allowed: true,
            user_rate_limit: snapshot.as_ref().and_then(|value| value.user_rate_limit),
            api_key_rate_limit: snapshot.as_ref().and_then(|value| value.api_key_rate_limit),
            user_daily_usage_limit_usd: snapshot
                .as_ref()
                .and_then(|value| value.user_daily_usage_limit_usd),
            api_key_daily_usage_limit_usd: snapshot
                .as_ref()
                .and_then(|value| value.api_key_daily_usage_limit_usd)
                .or(record.daily_usage_limit_usd),
            api_key_is_standalone: is_standalone,
            admin_bypass_limits: snapshot.as_ref().is_some_and(|value| {
                value.user_role.eq_ignore_ascii_case("admin") && !value.api_key_is_standalone
            }),
            ip_bypass_limits: false,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
        });
        let daily_usage = match daily_limiter.current_status(state, &daily_decision).await {
            Ok(Some(status)) => daily_usage_available_payload(status),
            Ok(None) => daily_usage_empty_payload(
                true,
                "unlimited",
                daily_timezone.name(),
                &daily_window_start,
                &daily_window_end,
                &daily_reset_at,
            ),
            Err(err) => {
                tracing::warn!(
                    error = ?err,
                    api_key_id = %record.api_key_id,
                    "daily usage runtime status unavailable"
                );
                daily_usage_empty_payload(
                    false,
                    "unavailable",
                    daily_timezone.name(),
                    &daily_window_start,
                    &daily_window_end,
                    &daily_reset_at,
                )
            }
        };

        api_keys.push(json!({
            "api_key_name": record
                .name
                .clone()
                .unwrap_or_else(|| format!("Key-{}", record.api_key_id)),
            "limit": primary_scope.map(|(_, limit, _)| limit),
            "remaining": primary_scope.map(|(_, _, remaining)| remaining),
            "scope": primary_scope.map(|(scope, _, _)| scope),
            "reset_time": primary_scope.map(|_| reset_time.clone()),
            "window": primary_scope.map(|_| window.clone()),
            "user_limit": if user_limit > 0 { Some(user_limit) } else { None::<u32> },
            "user_remaining": user_remaining,
            "key_limit": if key_limit > 0 { Some(key_limit) } else { None::<u32> },
            "key_remaining": key_remaining,
            "daily_usage": daily_usage,
        }));
    }

    Json(json!({
        "user_id": auth.user.id,
        "api_keys": api_keys,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::daily_usage_available_payload;
    use crate::daily_usage_limit::{DailyUsageScopeStatus, FrontdoorDailyUsageStatus};

    #[test]
    fn daily_usage_monitoring_payload_exposes_primary_and_nested_scopes() {
        let payload = daily_usage_available_payload(FrontdoorDailyUsageStatus {
            available: true,
            timezone: "Asia/Shanghai".to_string(),
            window_start: "2026-08-02T16:00:00Z".to_string(),
            window_end: "2026-08-03T16:00:00Z".to_string(),
            reset_at_unix_secs: 1_775_232_000,
            user: Some(DailyUsageScopeStatus {
                scope: "user",
                limit_usd: 10.0,
                used_usd: 4.0,
                remaining_usd: 6.0,
            }),
            key: Some(DailyUsageScopeStatus {
                scope: "key",
                limit_usd: 5.0,
                used_usd: 3.0,
                remaining_usd: 2.0,
            }),
        });

        assert_eq!(payload["status"], "available");
        assert_eq!(payload["scope"], "key");
        assert_eq!(payload["limit_usd"], 5.0);
        assert_eq!(payload["user"]["used_usd"], 4.0);
        assert_eq!(payload["key"]["remaining_usd"], 2.0);
        assert_eq!(payload["timezone"], "Asia/Shanghai");
    }

    #[test]
    fn daily_usage_monitoring_payload_marks_runtime_failures_unavailable() {
        let payload = daily_usage_available_payload(FrontdoorDailyUsageStatus {
            available: false,
            timezone: "Asia/Shanghai".to_string(),
            window_start: "start".to_string(),
            window_end: "end".to_string(),
            reset_at_unix_secs: 0,
            user: None,
            key: None,
        });

        assert_eq!(payload["available"], false);
        assert_eq!(payload["status"], "unavailable");
        assert!(payload["used_usd"].is_null());
        assert!(payload["remaining_usd"].is_null());
    }
}
