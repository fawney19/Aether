use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::unix_secs_to_rfc3339;
use crate::handlers::shared::round_to;
use crate::GatewayError;
use aether_data::repository::wallet::stored_timestamp_unix_secs;
use serde_json::{json, Map, Value};

#[derive(Clone)]
pub(in super::super) struct AdminWalletOwnerSummary {
    pub(in super::super) owner_type: &'static str,
    pub(in super::super) owner_name: Option<String>,
}

fn api_key_display_prefix(api_key_id: &str) -> String {
    api_key_id.chars().take(8).collect()
}

pub(in super::super) fn build_admin_wallet_payment_order_payload(
    order_id: String,
    order_no: String,
    amount_usd: f64,
    payment_method: String,
    status: String,
    created_at: Option<String>,
    credited_at: Option<String>,
) -> serde_json::Value {
    json!({
        "id": order_id,
        "order_no": order_no,
        "amount_usd": amount_usd,
        "payment_method": payment_method,
        "status": status,
        "created_at": created_at,
        "credited_at": credited_at,
    })
}

#[allow(clippy::too_many_arguments)]
pub(in super::super) fn build_admin_wallet_transaction_payload(
    wallet: &aether_data::repository::wallet::StoredWalletSnapshot,
    owner: &AdminWalletOwnerSummary,
    transaction_id: String,
    category: &str,
    reason_code: &str,
    amount: f64,
    balance_before: f64,
    balance_after: f64,
    recharge_balance_before: f64,
    recharge_balance_after: f64,
    gift_balance_before: f64,
    gift_balance_after: f64,
    link_type: Option<&str>,
    link_id: Option<&str>,
    operator_id: Option<&str>,
    description: Option<&str>,
    created_at: Option<String>,
) -> serde_json::Value {
    json!({
        "id": transaction_id,
        "wallet_id": wallet.id,
        "owner_type": owner.owner_type,
        "owner_name": owner.owner_name.clone(),
        "wallet_status": wallet.status,
        "category": category,
        "reason_code": reason_code,
        "amount": amount,
        "balance_before": balance_before,
        "balance_after": balance_after,
        "recharge_balance_before": recharge_balance_before,
        "recharge_balance_after": recharge_balance_after,
        "gift_balance_before": gift_balance_before,
        "gift_balance_after": gift_balance_after,
        "link_type": link_type,
        "link_id": link_id,
        "operator_id": operator_id,
        "operator_name": serde_json::Value::Null,
        "operator_email": serde_json::Value::Null,
        "description": description,
        "created_at": created_at,
    })
}

pub(in super::super) fn wallet_owner_summary_from_fields(
    user_id: Option<&str>,
    user_name: Option<String>,
    api_key_id: Option<&str>,
    api_key_name: Option<String>,
) -> AdminWalletOwnerSummary {
    if user_id.is_some() {
        return AdminWalletOwnerSummary {
            owner_type: "user",
            owner_name: user_name,
        };
    }
    if let Some(api_key_id) = api_key_id {
        return AdminWalletOwnerSummary {
            owner_type: "api_key",
            owner_name: api_key_name
                .filter(|value| !value.trim().is_empty())
                .or_else(|| Some(format!("Key-{}", api_key_display_prefix(api_key_id)))),
        };
    }
    AdminWalletOwnerSummary {
        owner_type: "orphaned",
        owner_name: None,
    }
}

pub(in super::super) async fn resolve_admin_wallet_owner_summary(
    state: &AdminAppState<'_>,
    wallet: &aether_data::repository::wallet::StoredWalletSnapshot,
) -> Result<AdminWalletOwnerSummary, GatewayError> {
    if let Some(user_id) = wallet.user_id.as_deref() {
        let user = state.find_user_auth_by_id(user_id).await?;
        Ok(AdminWalletOwnerSummary {
            owner_type: "user",
            owner_name: user.map(|record| record.username),
        })
    } else if let Some(api_key_id) = wallet.api_key_id.as_deref() {
        let api_key_ids = vec![api_key_id.to_string()];
        let snapshots = state
            .list_auth_api_key_snapshots_by_ids(&api_key_ids)
            .await?;
        let owner_name = snapshots
            .into_iter()
            .find(|snapshot| snapshot.api_key_id == api_key_id)
            .and_then(|snapshot| snapshot.api_key_name)
            .filter(|value| !value.trim().is_empty())
            .or_else(|| Some(format!("Key-{}", api_key_display_prefix(api_key_id))));
        Ok(AdminWalletOwnerSummary {
            owner_type: "api_key",
            owner_name,
        })
    } else {
        Ok(AdminWalletOwnerSummary {
            owner_type: "orphaned",
            owner_name: None,
        })
    }
}

pub(in super::super) fn build_admin_wallet_summary_payload(
    wallet: &aether_data::repository::wallet::StoredWalletSnapshot,
    owner: &AdminWalletOwnerSummary,
) -> serde_json::Value {
    json!({
        "id": wallet.id.clone(),
        "user_id": wallet.user_id.clone(),
        "api_key_id": wallet.api_key_id.clone(),
        "owner_type": owner.owner_type,
        "owner_name": owner.owner_name.clone(),
        "balance": wallet.balance + wallet.gift_balance,
        "recharge_balance": wallet.balance,
        "gift_balance": wallet.gift_balance,
        "refundable_balance": wallet.balance,
        "currency": wallet.currency.clone(),
        "status": wallet.status.clone(),
        "limit_mode": wallet.limit_mode.clone(),
        "unlimited": wallet.limit_mode.eq_ignore_ascii_case("unlimited"),
        "total_recharged": wallet.total_recharged,
        "total_consumed": wallet.total_consumed,
        "total_refunded": wallet.total_refunded,
        "total_adjusted": wallet.total_adjusted,
        "created_at": serde_json::Value::Null,
        "updated_at": unix_secs_to_rfc3339(wallet.updated_at_unix_secs),
    })
}

pub(in super::super) async fn build_admin_wallet_summary_payload_with_package(
    state: &AdminAppState<'_>,
    wallet: &aether_data::repository::wallet::StoredWalletSnapshot,
    owner: &AdminWalletOwnerSummary,
) -> Result<serde_json::Value, GatewayError> {
    let mut payload = build_admin_wallet_summary_payload(wallet, owner);
    enrich_admin_wallet_package_summary(
        state,
        &mut payload,
        wallet.user_id.as_deref(),
        wallet.balance + wallet.gift_balance,
        wallet.limit_mode.eq_ignore_ascii_case("unlimited"),
    )
    .await?;
    Ok(payload)
}

pub(in super::super) async fn enrich_admin_wallet_package_summary(
    state: &AdminAppState<'_>,
    payload: &mut serde_json::Value,
    user_id: Option<&str>,
    wallet_balance: f64,
    unlimited: bool,
) -> Result<(), GatewayError> {
    let daily_quota = match user_id {
        Some(user_id) if !user_id.trim().is_empty() => {
            state
                .app()
                .find_user_daily_quota_availability(user_id)
                .await?
        }
        _ => None,
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
    Ok(())
}

fn admin_refund_proof_identifier(value: Option<&Value>, max_bytes: usize) -> Option<String> {
    let value = value?.as_str()?.trim();
    if value.is_empty()
        || value.len() > max_bytes
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return None;
    }
    Some(value.to_string())
}

fn admin_gateway_refund_proof_projection(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut projected = Map::new();

    if let Some(gateway) = source
        .get("gateway")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .filter(|value| matches!(value.as_str(), "alipay" | "wxpay"))
    {
        projected.insert("gateway".to_string(), json!(gateway));
    }
    for (key, max_bytes) in [
        ("id", 128usize),
        ("order_no", 64usize),
        ("refund_no", 64usize),
    ] {
        if let Some(value) = admin_refund_proof_identifier(source.get(key), max_bytes) {
            projected.insert(key.to_string(), json!(value));
        }
    }
    if let Some(status) = source
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .and_then(|value| match value.as_str() {
            "success" | "succeeded" => Some("success"),
            "pending" | "processing" => Some("processing"),
            "failed" | "closed" | "abnormal" => Some("failed"),
            _ => None,
        })
    {
        projected.insert("status".to_string(), json!(status));
    }
    if let Some(amount) = source
        .get("amount")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
    {
        projected.insert("amount".to_string(), json!(amount));
    }
    if let Some(currency) = source
        .get("currency")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_alphabetic()))
        .map(str::to_ascii_uppercase)
    {
        projected.insert("currency".to_string(), json!(currency));
    }
    if let Some(processed_at) = source
        .get("processed_at")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| chrono::DateTime::parse_from_rfc3339(value).is_ok())
    {
        projected.insert("processed_at".to_string(), json!(processed_at));
    }

    (!projected.is_empty()).then_some(Value::Object(projected))
}

pub(in super::super) fn admin_wallet_payout_proof_projection(
    payout_proof: Option<&Value>,
) -> Option<Value> {
    let source = payout_proof?.as_object()?;
    let mut projected = source.clone();
    if source.contains_key("gateway_refund") {
        match source
            .get("gateway_refund")
            .and_then(admin_gateway_refund_proof_projection)
        {
            Some(gateway_refund) => {
                projected.insert("gateway_refund".to_string(), gateway_refund);
            }
            None => {
                projected.remove("gateway_refund");
            }
        }
    }
    Some(Value::Object(projected))
}

pub(in super::super) fn build_admin_wallet_refund_payload(
    wallet: &aether_data::repository::wallet::StoredWalletSnapshot,
    owner: &AdminWalletOwnerSummary,
    refund: &crate::AdminWalletRefundRecord,
) -> serde_json::Value {
    json!({
        "id": refund.id.clone(),
        "refund_no": refund.refund_no.clone(),
        "wallet_id": refund.wallet_id.clone(),
        "owner_type": owner.owner_type,
        "owner_name": owner.owner_name.clone(),
        "wallet_status": wallet.status.clone(),
        "user_id": refund.user_id.clone(),
        "payment_order_id": refund.payment_order_id.clone(),
        "source_type": refund.source_type.clone(),
        "source_id": refund.source_id.clone(),
        "refund_mode": refund.refund_mode.clone(),
        "amount_usd": refund.amount_usd,
        "status": refund.status.clone(),
        "reason": refund.reason.clone(),
        "failure_reason": refund.failure_reason.clone(),
        "gateway_refund_id": refund.gateway_refund_id.clone(),
        "payout_method": refund.payout_method.clone(),
        "payout_reference": refund.payout_reference.clone(),
        "payout_proof": admin_wallet_payout_proof_projection(refund.payout_proof.as_ref()),
        "requested_by": refund.requested_by.clone(),
        "approved_by": refund.approved_by.clone(),
        "processed_by": refund.processed_by.clone(),
        "created_at": unix_secs_to_rfc3339(stored_timestamp_unix_secs(refund.created_at_unix_ms)),
        "updated_at": unix_secs_to_rfc3339(refund.updated_at_unix_secs),
        "processed_at": refund.processed_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "completed_at": refund.completed_at_unix_secs.and_then(unix_secs_to_rfc3339),
    })
}

#[cfg(test)]
mod tests {
    use super::admin_wallet_payout_proof_projection;
    use serde_json::json;

    #[test]
    fn admin_refund_proof_projection_removes_historical_gateway_payloads() {
        let proof = json!({
            "channel": "manual",
            "gateway_refund": {
                "gateway": "WXPAY",
                "id": "refund-1",
                "status": "SUCCESS",
                "order_no": "order-1",
                "refund_no": "request-1",
                "amount": 8.5,
                "currency": "cny",
                "processed_at": "2026-08-27T12:00:00Z",
                "payload": {
                    "authorization": "Bearer payment-secret",
                    "url": "https://internal.example/refund?token=secret",
                    "payer": {"openid": "openid-secret"},
                    "credential": "gateway-credential"
                },
                "message": "upstream secret message"
            }
        });

        let projection = admin_wallet_payout_proof_projection(Some(&proof))
            .expect("object payout proof should be projected");
        assert_eq!(projection["channel"], "manual");
        assert_eq!(projection["gateway_refund"]["gateway"], "wxpay");
        assert_eq!(projection["gateway_refund"]["status"], "success");
        assert_eq!(
            projection["gateway_refund"]
                .as_object()
                .expect("gateway proof should be an object")
                .len(),
            8
        );
        let encoded = projection.to_string();
        for sensitive in [
            "payment-secret",
            "?token=secret",
            "openid-secret",
            "gateway-credential",
            "upstream secret message",
            "payload",
            "authorization",
            "payer",
            "openid",
            "credential",
            "message",
        ] {
            assert!(!encoded.contains(sensitive));
        }
    }

    #[test]
    fn admin_refund_proof_projection_drops_mistyped_gateway_fields() {
        let proof = json!({
            "operator": "finance",
            "gateway_refund": {
                "gateway": {"credential": "secret"},
                "id": ["refund-1"],
                "status": "unknown-secret-status",
                "order_no": "https://example.test/?token=secret",
                "refund_no": 123,
                "amount": "8.5",
                "currency": "CNY?token=secret",
                "processed_at": "Bearer secret"
            }
        });

        let projection = admin_wallet_payout_proof_projection(Some(&proof))
            .expect("manual payout proof should remain available");
        assert_eq!(projection, json!({"operator": "finance"}));
        assert!(!projection.to_string().contains("secret"));
    }
}
