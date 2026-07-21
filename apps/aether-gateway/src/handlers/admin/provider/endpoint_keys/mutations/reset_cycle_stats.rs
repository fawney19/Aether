use crate::handlers::admin::provider::shared::paths::admin_reset_cycle_stats_key_id;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::provider_key_status_snapshot_payload;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn maybe_handle(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    _request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("endpoints_manage")
        || decision.route_kind.as_deref() != Some("reset_cycle_stats")
        || request_context.method() != http::Method::POST
        || !request_context
            .path()
            .starts_with("/api/admin/endpoints/keys/")
        || !request_context.path().ends_with("/reset-cycle-stats")
    {
        return Ok(None);
    }

    let Some(key_id) = admin_reset_cycle_stats_key_id(request_context.path()) else {
        return Ok(Some(not_found_response("Key 不存在")));
    };
    let Some(mut key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(not_found_response(format!("Key {key_id} 不存在"))));
    };
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&key.provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(not_found_response(format!(
            "Provider {} 不存在",
            key.provider_id
        ))));
    };
    if !provider.provider_type.trim().eq_ignore_ascii_case("codex") {
        return Ok(Some(bad_request_response(
            "仅 Codex Provider 支持重置周期统计",
        )));
    }

    let now_unix_secs = current_unix_secs();
    let mut status_snapshot = provider_key_status_snapshot_payload(&key, &provider.provider_type);
    let reset_windows = reset_codex_cycle_usage_windows(&mut status_snapshot, now_unix_secs);
    if reset_windows == 0 {
        return Ok(Some(bad_request_response("当前账号没有可重置的周期窗口")));
    }

    key.status_snapshot = Some(status_snapshot);
    key.updated_at_unix_secs = Some(now_unix_secs);
    let Some(_) = state.update_provider_catalog_key(&key).await? else {
        return Ok(None);
    };

    Ok(Some(
        Json(json!({
            "message": "已重置周期统计",
            "reset_at": now_unix_secs,
            "windows": reset_windows,
        }))
        .into_response(),
    ))
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn reset_codex_cycle_usage_windows(status_snapshot: &mut Value, now_unix_secs: u64) -> usize {
    let Some(windows) = status_snapshot
        .get_mut("quota")
        .and_then(Value::as_object_mut)
        .and_then(|quota| quota.get_mut("windows"))
        .and_then(Value::as_array_mut)
    else {
        return 0;
    };

    let mut reset_count = 0;
    for window in windows.iter_mut().filter_map(Value::as_object_mut) {
        let code = window
            .get("code")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        let scope = window
            .get("scope")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("account");
        let has_zero_window = window.get("window_minutes").and_then(Value::as_u64) == Some(0);
        if code.is_empty()
            || !scope.eq_ignore_ascii_case("account")
            || code.to_ascii_lowercase().starts_with("spark_")
            || has_zero_window
        {
            continue;
        }

        window.insert("usage_reset_at".to_string(), json!(now_unix_secs));
        window.insert(
            "usage".to_string(),
            json!({
                "request_count": 0,
                "total_tokens": 0,
                "total_cost_usd": "0.00000000",
            }),
        );
        reset_count += 1;
    }

    reset_count
}

fn bad_request_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

fn not_found_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::reset_codex_cycle_usage_windows;
    use serde_json::json;

    #[test]
    fn reset_cycle_usage_marks_codex_windows_and_removes_stale_usage() {
        let mut snapshot = json!({
            "quota": {
                "windows": [
                    {
                        "code": "5h",
                        "usage": {
                            "request_count": 9,
                            "total_tokens": 100,
                            "total_cost_usd": "1.00000000"
                        }
                    },
                    {
                        "code": "weekly",
                        "usage": {
                            "request_count": 10,
                            "total_tokens": 200,
                            "total_cost_usd": "2.00000000"
                        }
                    },
                    {
                        "code": "monthly",
                        "usage": {
                            "request_count": 11
                        }
                    }
                ]
            }
        });

        assert_eq!(reset_codex_cycle_usage_windows(&mut snapshot, 1_234), 3);
        let windows = snapshot["quota"]["windows"].as_array().expect("windows");
        assert_eq!(windows[0]["usage_reset_at"], json!(1_234));
        assert_eq!(windows[0]["usage"]["request_count"], json!(0));
        assert_eq!(windows[0]["usage"]["total_tokens"], json!(0));
        assert_eq!(windows[0]["usage"]["total_cost_usd"], json!("0.00000000"));
        assert_eq!(windows[1]["usage_reset_at"], json!(1_234));
        assert_eq!(windows[1]["usage"]["request_count"], json!(0));
        assert_eq!(windows[1]["usage"]["total_tokens"], json!(0));
        assert_eq!(windows[1]["usage"]["total_cost_usd"], json!("0.00000000"));
        assert_eq!(windows[2]["usage_reset_at"], json!(1_234));
        assert_eq!(windows[2]["usage"]["request_count"], json!(0));
    }
}
