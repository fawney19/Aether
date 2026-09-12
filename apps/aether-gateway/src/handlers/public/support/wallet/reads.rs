use super::{
    build_auth_error_response, build_auth_json_response, build_auth_wallet_summary_payload, http,
    query_param_value, resolve_authenticated_local_user, unix_secs_to_rfc3339, AppState, Body,
    GatewayPublicRequestContext, Response, WALLET_LEGACY_TIMEZONE,
};
use crate::handlers::shared::round_to;
use aether_data::repository::wallet::stored_timestamp_unix_secs;
use aether_data_contracts::repository::usage::UsageSettledCostSummaryQuery;
use chrono::{TimeZone, Utc};
use serde_json::json;

const WALLET_TODAY_COST_UNAVAILABLE_DETAIL: &str = "钱包今日费用数据暂不可用";

pub(super) fn build_wallet_payload(
    wallet: Option<&aether_data::repository::wallet::StoredWalletSnapshot>,
) -> serde_json::Value {
    let wallet_payload = build_auth_wallet_summary_payload(wallet);
    json!({
        "wallet": wallet_payload.clone(),
        "unlimited": wallet_payload.get("unlimited").cloned().unwrap_or(json!(false)),
        "limit_mode": wallet_payload
            .get("limit_mode")
            .cloned()
            .unwrap_or_else(|| json!("finite")),
        "balance": wallet_payload.get("balance").cloned().unwrap_or(json!(0.0)),
        "recharge_balance": wallet_payload
            .get("recharge_balance")
            .cloned()
            .unwrap_or(json!(0.0)),
        "gift_balance": wallet_payload
            .get("gift_balance")
            .cloned()
            .unwrap_or(json!(0.0)),
        "refundable_balance": wallet_payload
            .get("refundable_balance")
            .cloned()
            .unwrap_or(json!(0.0)),
        "currency": wallet_payload
            .get("currency")
            .cloned()
            .unwrap_or_else(|| json!("USD")),
    })
}

fn build_wallet_balance_payload(
    wallet: Option<&aether_data::repository::wallet::StoredWalletSnapshot>,
) -> serde_json::Value {
    let mut payload = build_wallet_payload(wallet);
    payload["pending_refund_count"] = json!(0);
    payload
}

pub(in crate::handlers::public::support) async fn build_wallet_balance_payload_for_user(
    state: &AppState,
    user_id: &str,
    wallet: Option<&aether_data::repository::wallet::StoredWalletSnapshot>,
) -> serde_json::Value {
    build_wallet_balance_payload_for_quota_user(state, Some(user_id), wallet).await
}

pub(in crate::handlers::public::support) async fn build_wallet_balance_payload_for_auth_scope(
    state: &AppState,
    user_id: &str,
    api_key_is_standalone: bool,
    wallet: Option<&aether_data::repository::wallet::StoredWalletSnapshot>,
) -> serde_json::Value {
    let quota_user_id = if api_key_is_standalone {
        None
    } else {
        Some(user_id)
    };
    build_wallet_balance_payload_for_quota_user(state, quota_user_id, wallet).await
}

async fn build_wallet_balance_payload_for_quota_user(
    state: &AppState,
    quota_user_id: Option<&str>,
    wallet: Option<&aether_data::repository::wallet::StoredWalletSnapshot>,
) -> serde_json::Value {
    let mut payload = build_wallet_balance_payload(wallet);
    let wallet_balance = wallet
        .map(|value| value.balance + value.gift_balance)
        .unwrap_or(0.0);
    let daily_quota = match quota_user_id {
        Some(user_id) => state
            .find_user_daily_quota_availability(user_id)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let (has_active_daily_quota, total_quota_usd, used_usd, remaining_usd, allow_wallet_overage) =
        daily_quota
            .map(|quota| {
                (
                    quota.has_active_daily_quota,
                    quota.total_quota_usd,
                    quota.used_usd,
                    quota.remaining_usd,
                    quota.allow_wallet_overage,
                )
            })
            .unwrap_or((false, 0.0, 0.0, 0.0, false));
    let package_balance = if has_active_daily_quota {
        remaining_usd.max(0.0)
    } else {
        0.0
    };
    let unlimited = payload
        .get("unlimited")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    payload["daily_quota"] = json!({
        "has_active": has_active_daily_quota,
        "total_usd": round_to(total_quota_usd.max(0.0), 6),
        "used_usd": round_to(used_usd.max(0.0), 6),
        "remaining_usd": round_to(package_balance, 6),
        "allow_wallet_overage": allow_wallet_overage,
    });
    payload["package_balance"] = json!(round_to(package_balance, 6));
    payload["wallet_balance"] = json!(round_to(wallet_balance.max(0.0), 6));
    payload["total_available_balance"] = if unlimited {
        serde_json::Value::Null
    } else {
        json!(round_to((wallet_balance + package_balance).max(0.0), 6))
    };
    payload["deduction_order"] = json!([
        "package_daily_quota",
        "wallet_recharge_balance",
        "wallet_gift_balance"
    ]);
    payload
}

pub(super) fn parse_wallet_limit(query: Option<&str>) -> Result<usize, String> {
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
        None => Ok(50),
    }
}

pub(super) fn parse_wallet_offset(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "offset") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| "offset must be a non-negative integer".to_string()),
        None => Ok(0),
    }
}

pub(super) fn wallet_fixed_offset() -> chrono::FixedOffset {
    chrono::FixedOffset::east_opt(8 * 3600).expect("Asia/Shanghai offset should be valid")
}

pub(super) fn wallet_today_billing_date_string() -> String {
    Utc::now()
        .with_timezone(&wallet_fixed_offset())
        .date_naive()
        .to_string()
}

fn wallet_today_usage_window() -> Result<(String, String, u64, u64), String> {
    let offset = wallet_fixed_offset();
    let today = Utc::now().with_timezone(&offset).date_naive();
    let Some(local_start_naive) = today.and_hms_opt(0, 0, 0) else {
        return Err("wallet today start is invalid".to_string());
    };
    let Some(local_start) = offset.from_local_datetime(&local_start_naive).single() else {
        return Err("wallet today local start is ambiguous".to_string());
    };
    let local_end = local_start + chrono::Duration::days(1);
    let start_unix_secs = local_start.timestamp().max(0) as u64;
    let end_unix_secs = local_end.timestamp().max(0) as u64;
    Ok((
        today.to_string(),
        WALLET_LEGACY_TIMEZONE.to_string(),
        start_unix_secs,
        end_unix_secs,
    ))
}

pub(super) fn build_wallet_daily_usage_payload(
    id: Option<String>,
    date: String,
    timezone: String,
    total_cost: f64,
    total_requests: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    first_finalized_at: Option<String>,
    last_finalized_at: Option<String>,
    aggregated_at: Option<String>,
    is_today: bool,
) -> serde_json::Value {
    json!({
        "id": id,
        "date": date,
        "timezone": timezone,
        "total_cost": round_to(total_cost, 6),
        "total_requests": total_requests,
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "cache_creation_tokens": cache_creation_tokens,
        "cache_read_tokens": cache_read_tokens,
        "first_finalized_at": first_finalized_at,
        "last_finalized_at": last_finalized_at,
        "aggregated_at": aggregated_at,
        "is_today": is_today,
    })
}

pub(super) fn build_wallet_zero_today_entry() -> serde_json::Value {
    build_wallet_daily_usage_payload(
        None,
        wallet_today_billing_date_string(),
        WALLET_LEGACY_TIMEZONE.to_string(),
        0.0,
        0,
        0,
        0,
        0,
        0,
        None,
        None,
        Some(Utc::now().to_rfc3339()),
        true,
    )
}

async fn build_wallet_live_today_usage_payload_for_auth_scope(
    state: &AppState,
    user_id: Option<&str>,
    api_key_id: Option<&str>,
) -> Result<Option<serde_json::Value>, String> {
    if !state.has_usage_data_reader() {
        return Ok(None);
    }
    let (date, timezone, start_unix_secs, end_unix_secs) = wallet_today_usage_window()?;
    let summary = state
        .summarize_usage_settled_cost(&UsageSettledCostSummaryQuery {
            created_from_unix_secs: start_unix_secs,
            created_until_unix_secs: end_unix_secs,
            user_id: user_id.map(ToOwned::to_owned),
            api_key_id: api_key_id.map(ToOwned::to_owned),
        })
        .await
        .map_err(|err| format!("wallet today cost lookup failed: {err:?}"))?;
    Ok(Some(build_wallet_daily_usage_payload(
        None,
        date,
        timezone,
        summary.total_cost_usd,
        summary.total_requests,
        summary.input_tokens,
        summary.output_tokens,
        summary.cache_creation_tokens,
        summary.cache_read_tokens,
        summary
            .first_finalized_at_unix_secs
            .and_then(unix_secs_to_rfc3339),
        summary
            .last_finalized_at_unix_secs
            .and_then(unix_secs_to_rfc3339),
        Some(Utc::now().to_rfc3339()),
        true,
    )))
}

pub(in crate::handlers::public::support) async fn build_wallet_live_today_usage_payload_for_user(
    state: &AppState,
    user_id: &str,
) -> Result<Option<serde_json::Value>, String> {
    build_wallet_live_today_usage_payload_for_auth_scope(state, Some(user_id), None).await
}

pub(in crate::handlers::public::support) async fn build_wallet_live_today_usage_payload_for_api_key(
    state: &AppState,
    api_key_id: &str,
) -> Result<Option<serde_json::Value>, String> {
    build_wallet_live_today_usage_payload_for_auth_scope(state, None, Some(api_key_id)).await
}

pub(super) fn wallet_transaction_payload_from_record(
    record: &aether_data::repository::wallet::StoredAdminWalletTransaction,
) -> serde_json::Value {
    json!({
        "id": record.id.clone(),
        "category": record.category.clone(),
        "reason_code": record.reason_code.clone(),
        "amount": record.amount,
        "balance_before": record.balance_before,
        "balance_after": record.balance_after,
        "recharge_balance_before": record.recharge_balance_before,
        "recharge_balance_after": record.recharge_balance_after,
        "gift_balance_before": record.gift_balance_before,
        "gift_balance_after": record.gift_balance_after,
        "link_type": record.link_type.clone(),
        "link_id": record.link_id.clone(),
        "operator_id": record.operator_id.clone(),
        "description": record.description.clone(),
        "created_at": record
            .created_at_unix_ms
            .map(stored_timestamp_unix_secs)
            .and_then(unix_secs_to_rfc3339),
    })
}

pub(super) async fn handle_wallet_balance(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let wallet = state
        .read_wallet_snapshot_for_auth(&auth.user.id, "", false)
        .await
        .ok()
        .flatten();
    build_auth_json_response(
        http::StatusCode::OK,
        build_wallet_balance_payload_for_user(state, &auth.user.id, wallet.as_ref()).await,
        None,
    )
}

pub(super) async fn handle_wallet_today_cost(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_usage_data_reader() {
        return build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            WALLET_TODAY_COST_UNAVAILABLE_DETAIL,
            false,
        );
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match build_wallet_live_today_usage_payload_for_user(state, &auth.user.id).await {
        Ok(Some(payload)) => build_auth_json_response(http::StatusCode::OK, payload, None),
        Ok(None) => build_auth_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            WALLET_TODAY_COST_UNAVAILABLE_DETAIL,
            false,
        ),
        Err(detail) => {
            build_auth_error_response(http::StatusCode::INTERNAL_SERVER_ERROR, detail, false)
        }
    }
}

pub(super) async fn handle_wallet_transactions(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = request_context.request_query_string.as_deref();
    let limit = match parse_wallet_limit(query) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    let offset = match parse_wallet_offset(query) {
        Ok(value) => value,
        Err(detail) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
        }
    };
    let wallet = match state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::UserId(
            &auth.user.id,
        ))
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("wallet lookup failed: {err:?}"),
                false,
            )
        }
    };
    let Some(wallet) = wallet else {
        return build_auth_json_response(
            http::StatusCode::OK,
            json!({
                "items": [],
                "total": 0,
                "limit": limit,
                "offset": offset,
            })
            .as_object()
            .cloned()
            .map(|mut value| {
                if let Some(wallet_payload) = build_wallet_payload(None).as_object() {
                    value.extend(wallet_payload.clone());
                }
                serde_json::Value::Object(value)
            })
            .unwrap_or_else(|| json!({})),
            None,
        );
    };

    let (transactions, total) = match state
        .list_admin_wallet_transactions(&wallet.id, limit, offset)
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("wallet transaction lookup failed: {err:?}"),
                false,
            )
        }
    };
    let items = transactions
        .iter()
        .map(wallet_transaction_payload_from_record)
        .collect::<Vec<_>>();
    let mut payload = json!({
        "items": items,
        "total": total,
        "limit": limit,
        "offset": offset,
    });
    if let Some(object) = payload.as_object_mut() {
        if let Some(wallet_payload) = build_wallet_payload(Some(&wallet)).as_object() {
            object.extend(wallet_payload.clone());
        }
    }
    build_auth_json_response(http::StatusCode::OK, payload, None)
}
