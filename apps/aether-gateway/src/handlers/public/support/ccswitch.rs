use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::control::GatewayLocalAuthRejection;
use crate::handlers::shared::round_to;

use super::{
    build_auth_error_response, build_wallet_balance_payload_for_auth_scope,
    build_wallet_live_today_usage_payload_for_api_key,
    build_wallet_live_today_usage_payload_for_user, AppState, GatewayPublicRequestContext,
};

fn ccswitch_usage_auth_error_response(
    rejection: Option<&GatewayLocalAuthRejection>,
) -> Response<Body> {
    match rejection {
        Some(GatewayLocalAuthRejection::InvalidApiKey) | None => {
            build_auth_error_response(http::StatusCode::UNAUTHORIZED, "无效的 API Key", false)
        }
        Some(GatewayLocalAuthRejection::LockedApiKey) => {
            build_auth_error_response(http::StatusCode::FORBIDDEN, "API Key 已被锁定", false)
        }
        Some(GatewayLocalAuthRejection::ProviderNotAllowed { .. })
        | Some(GatewayLocalAuthRejection::ApiFormatNotAllowed { .. })
        | Some(GatewayLocalAuthRejection::ModelNotAllowed { .. })
        | Some(GatewayLocalAuthRejection::IpNotAllowed { .. }) => {
            build_auth_error_response(http::StatusCode::FORBIDDEN, "API Key 无权查询用量", false)
        }
        Some(GatewayLocalAuthRejection::WalletUnavailable) => build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "钱包数据暂不可用",
            false,
        ),
        Some(GatewayLocalAuthRejection::BalanceDenied { .. }) => {
            build_auth_error_response(http::StatusCode::FORBIDDEN, "API Key 余额不足", false)
        }
        Some(GatewayLocalAuthRejection::CodexAdapter { .. }) => {
            build_auth_error_response(http::StatusCode::FORBIDDEN, "Codex 适配器请求被拒绝", false)
        }
    }
}

fn json_f64(value: &serde_json::Value, key: &str) -> Option<f64> {
    value.get(key).and_then(serde_json::Value::as_f64)
}

fn json_bool(value: &serde_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn format_usd(value: f64) -> String {
    format!("${:.4}", round_to(value, 4))
}

fn build_ccswitch_usage_extra(
    wallet_payload: &serde_json::Value,
    today_payload: Option<&serde_json::Value>,
) -> String {
    let mut parts = Vec::new();
    if let Some(today_cost) = today_payload.and_then(|payload| json_f64(payload, "total_cost")) {
        parts.push(format!("今日消耗 {}", format_usd(today_cost)));
    }
    if let Some(wallet_balance) = json_f64(wallet_payload, "wallet_balance") {
        parts.push(format!("钱包 {}", format_usd(wallet_balance)));
    }
    if let Some(package_balance) = json_f64(wallet_payload, "package_balance") {
        if package_balance > 0.0 {
            parts.push(format!("套餐 {}", format_usd(package_balance)));
        }
    }

    parts.join(" · ")
}

pub(super) async fn maybe_build_local_ccswitch_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("ccswitch") {
        return None;
    }
    if decision.route_kind.as_deref() != Some("usage")
        || request_context.request_path.trim_end_matches('/') != "/api/ccswitch/usage"
    {
        return None;
    }

    let auth_context = match decision.auth_context.as_ref() {
        Some(auth_context) if !auth_context.user_id.trim().is_empty() => auth_context,
        _ => {
            return Some(ccswitch_usage_auth_error_response(
                decision.local_auth_rejection.as_ref(),
            ))
        }
    };

    match auth_context.local_rejection.as_ref() {
        None | Some(GatewayLocalAuthRejection::BalanceDenied { .. }) => {}
        rejection => return Some(ccswitch_usage_auth_error_response(rejection)),
    }

    let wallet = match state
        .read_wallet_snapshot_for_auth(
            &auth_context.user_id,
            &auth_context.api_key_id,
            auth_context.api_key_is_standalone,
        )
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return Some(build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("ccswitch usage wallet lookup failed: {err:?}"),
                false,
            ))
        }
    };
    let wallet_payload = build_wallet_balance_payload_for_auth_scope(
        state,
        &auth_context.user_id,
        auth_context.api_key_is_standalone,
        wallet.as_ref(),
    )
    .await;
    let today_payload = match if auth_context.api_key_is_standalone {
        build_wallet_live_today_usage_payload_for_api_key(state, &auth_context.api_key_id).await
    } else {
        build_wallet_live_today_usage_payload_for_user(state, &auth_context.user_id).await
    } {
        Ok(value) => value,
        Err(err) => {
            return Some(build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                err,
                false,
            ))
        }
    };

    let unlimited = json_bool(&wallet_payload, "unlimited");
    let remaining = json_f64(&wallet_payload, "total_available_balance")
        .or_else(|| json_f64(&wallet_payload, "wallet_balance"));
    let used_today = today_payload
        .as_ref()
        .and_then(|payload| json_f64(payload, "total_cost"))
        .unwrap_or(0.0);
    let mut extra = build_ccswitch_usage_extra(&wallet_payload, today_payload.as_ref());
    if extra.is_empty() && unlimited {
        extra = "无限额度".to_string();
    }

    Some(
        Json(json!({
            "is_valid": true,
            "plan_name": if unlimited { "Aether Unlimited" } else { "Aether" },
            "remaining": remaining.map(|value| round_to(value.max(0.0), 6)),
            "used": round_to(used_today.max(0.0), 6),
            "unit": wallet_payload
                .get("currency")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("USD"),
            "extra": extra,
            "unlimited": unlimited,
            "wallet": wallet_payload,
            "today": today_payload,
        }))
        .into_response(),
    )
}
