//! Current provider finance snapshots. This endpoint never calls upstream services.
use super::build_admin_billing_data_unavailable_response;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

fn finite(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_str().and_then(|v| v.parse::<f64>().ok()))
        })
        .filter(|v| v.is_finite())
}
fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.len() <= 256 && !v.chars().any(char::is_control))
}
fn timestamp(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(raw) = value.as_str() {
        if let Ok(date) = chrono::DateTime::parse_from_rfc3339(raw) {
            return Some(date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        }
    }
    let secs = finite(Some(value))?;
    if !(0.0..=253_402_300_799.0).contains(&secs) {
        return None;
    }
    chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|v| v.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}
fn subscription(value: &Value) -> Value {
    json!({
        "group_name": text(value.get("group_name")),
        "status": text(value.get("status")),
        "daily_used_usd": finite(value.get("daily_used_usd")),
        "daily_limit_usd": finite(value.get("daily_limit_usd")),
        "weekly_used_usd": finite(value.get("weekly_used_usd")),
        "weekly_limit_usd": finite(value.get("weekly_limit_usd")),
        "monthly_used_usd": finite(value.get("monthly_used_usd")),
        "monthly_limit_usd": finite(value.get("monthly_limit_usd")),
        "expires_at": timestamp(value.get("expires_at")),
    })
}
fn balance(value: &Value) -> Option<Value> {
    if value.get("action_type").and_then(Value::as_str) != Some("query_balance") {
        return None;
    }
    let status = text(value.get("status"))?;
    if !matches!(status, "success" | "auth_expired" | "auth_failed") {
        return None;
    }
    let data = value
        .get("data")
        .filter(|_| matches!(status, "success" | "auth_expired"));
    let extra = data.and_then(|d| d.get("extra"));
    let subscriptions = extra
        .and_then(|e| e.get("subscriptions"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|v| v.is_object())
                .take(128)
                .map(subscription)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(json!({
        "status": status,
        "observed_at": timestamp(value.get("executed_at")),
        "currency": data.and_then(|d| text(d.get("currency"))),
        "available": data.and_then(|d| finite(d.get("total_available"))),
        "used": data.and_then(|d| finite(d.get("total_used"))),
        "granted": data.and_then(|d| finite(d.get("total_granted"))),
        "plan_name": extra.and_then(|e| text(e.get("plan_name"))),
        "subscriptions": subscriptions,
    }))
}
pub(crate) fn custom_quota(value: &Value) -> Option<Value> {
    if value.get("action_type").and_then(Value::as_str) != Some("query_quota") {
        return None;
    }
    if value.get("status").and_then(Value::as_str) != Some("success") {
        return None;
    }
    let data = value.get("data")?;
    let periods = data
        .get("periods")
        .and_then(Value::as_array)
        .and_then(|items| {
            let projected = items
                .iter()
                .take(8)
                .filter_map(|period| {
                    let key = text(period.get("key"))?;
                    Some(json!({
                        "key": key,
                        "used": finite(period.get("used")),
                        "limit": finite(period.get("limit")),
                        "reset_at": period.get("reset_at").and_then(|value| {
                            timestamp(Some(value))
                                .or_else(|| value.as_str().map(|raw| raw.trim().to_owned()))
                        }),
                    }))
                })
                .collect::<Vec<_>>();
            (!projected.is_empty()).then_some(projected)
        });
    let updated_at = value
        .get("executed_at")
        .and_then(Value::as_str)
        .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
        .map(|date| date.timestamp());
    let projected = json!({
        "updated_at": updated_at,
        "is_valid": data.get("is_valid").and_then(Value::as_bool),
        "invalid_message": text(data.get("invalid_message")),
        "remaining": finite(data.get("remaining")),
        "unit": text(data.get("unit")),
        "plan_name": text(data.get("plan_name")),
        "total": finite(data.get("total")),
        "used": finite(data.get("used")),
        "extra": text(data.get("extra")),
        "periods": periods,
    });
    let has_any = projected
        .as_object()
        .map(|fields| fields.values().any(|value| !value.is_null()))
        .unwrap_or(false);
    has_any.then_some(projected)
}
pub(super) async fn response(
    state: &AdminAppState<'_>,
    context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if context.method() != http::Method::GET
        || context.path().trim_end_matches('/') != "/api/admin/billing/provider-accounts"
        || context.route_family() != Some("billing_manage")
    {
        return Ok(None);
    }
    if !state.has_provider_catalog_data_reader() {
        return Ok(Some(build_admin_billing_data_unavailable_response()));
    }
    let mut providers = state.list_provider_catalog_providers(false).await?;
    providers.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    let mut keys = Vec::with_capacity(providers.len() * 2);
    for p in &providers {
        keys.push(format!("provider_ops:balance:{}", p.id));
    }
    for p in &providers {
        keys.push(format!("provider_ops:quota:{}", p.id));
    }
    let (cached, unavailable) = if keys.is_empty() {
        (Vec::new(), false)
    } else {
        match state.runtime_state().kv_get_many(&keys).await {
            Ok(v) => (v, false),
            Err(_) => (vec![None; keys.len()], true),
        }
    };
    let (balance_cached, quota_cached) = cached.split_at(providers.len());
    let items = providers.iter().enumerate().map(|(index, p)| {
        let limit = p.monthly_quota_usd.filter(|v| v.is_finite() && *v >= 0.0);
        let used = p.monthly_used_usd.filter(|v| v.is_finite() && *v >= 0.0);
        let quota = if p.billing_type.as_deref() == Some("monthly_quota") || limit.is_some() {
            json!({
                "limit": limit, "used": used,
                "remaining": limit.zip(used).map(|(l,u)| (l-u).max(0.0)),
                "currency": "USD",
                "period_start": p.quota_last_reset_at_unix_secs.and_then(|v| timestamp(Some(&json!(v)))),
                "expires_at": p.quota_expires_at_unix_secs.and_then(|v| timestamp(Some(&json!(v)))),
            })
        } else { Value::Null };
        let balance = balance_cached.get(index).and_then(|v| v.as_deref())
            .and_then(|v| serde_json::from_str::<Value>(v).ok()).and_then(|v| balance(&v));
        let custom_quota = quota_cached.get(index).and_then(|v| v.as_deref())
            .and_then(|v| serde_json::from_str::<Value>(v).ok()).and_then(|v| custom_quota(&v));
        json!({
            "provider_id": p.id, "provider_name": p.name, "is_active": p.is_active,
            "billing_type": p.billing_type, "quota": quota, "balance": balance,
            "custom_quota": custom_quota,
        })
    }).collect::<Vec<_>>();
    Ok(Some((
        [(http::header::CACHE_CONTROL, "private, no-store")],
        Json(json!({
            "observed_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis,true),
            "items": items, "balance_snapshot_unavailable": unavailable,
        })),
    ).into_response()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_accounts_only_expose_finance_allowlist_and_preserve_unknown() {
        let snapshot=balance(&json!({"status":"success","action_type":"query_balance","executed_at":"2026-09-20T00:00:00Z","data":{"currency":"USD","total_available":null,"extra":{"access_token":"secret","plan_name":"Pro","subscriptions":[{"group_name":"Team","monthly_used_usd":"12.25","expires_at":1800000000,"private_token":"secret"}]}}})).unwrap();
        assert!(snapshot["available"].is_null());
        assert_eq!(
            snapshot["subscriptions"][0]["monthly_used_usd"],
            json!(12.25)
        );
        assert!(!snapshot.to_string().contains("secret"));
        assert!(!snapshot.to_string().contains("access_token"));
        let failed=balance(&json!({"status":"auth_failed","action_type":"query_balance","data":{"total_available":999}})).unwrap();
        assert!(failed["available"].is_null());
    }
}
