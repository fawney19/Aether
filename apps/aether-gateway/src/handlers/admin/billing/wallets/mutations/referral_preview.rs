use super::super::shared::{
    admin_wallet_refund_ids_from_suffix_path, build_admin_wallet_not_found_response,
    build_admin_wallet_refund_not_found_response, build_admin_wallets_bad_request_response,
    ADMIN_WALLETS_API_KEY_REFUND_DETAIL,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::shared::{open_runtime_secret_payload, seal_runtime_secret_payload};
use crate::{AdminWalletRefundRecord, GatewayError};
use axum::{
    body::Body,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

const CONFIRMATION_TTL_SECONDS: i64 = 300;

fn confirmation_matches(
    state: &crate::AppState,
    purpose: &str,
    token: Option<&str>,
    risk: &Value,
    now: i64,
) -> bool {
    token
        .and_then(|token| open_runtime_secret_payload(state, purpose, token))
        .and_then(|plain| serde_json::from_str::<Value>(&plain).ok())
        .is_some_and(|claims| {
            claims["expires_at"]
                .as_i64()
                .is_some_and(|expires| expires > now)
                && claims["risk"] == *risk
        })
}

fn confirmation_purpose(wallet_id: &str, refund_id: &str, stage: &str, operator: &str) -> String {
    format!("referral-refund-confirmation-v1:{wallet_id}:{refund_id}:{stage}:{operator}")
}

fn preview_payload(
    refund: &AdminWalletRefundRecord,
    stage: &str,
    mut rewards: Vec<Value>,
) -> Value {
    rewards.sort_by(|a, b| a["reward_id"].as_str().cmp(&b["reward_id"].as_str()));
    let sum = |key: &str| {
        rewards
            .iter()
            .filter_map(|reward| reward[key].as_f64())
            .sum::<f64>()
    };
    json!({
        "refund_id": refund.id,
        "stage": stage,
        "applicable": refund.payment_order_id.is_some(),
        "total_expected_reversal_usd": sum("expected_reversal_usd"),
        "total_deductible_usd": sum("deductible_usd"),
        "total_shortfall_usd": sum("shortfall_usd"),
        "rewards": rewards,
    })
}

pub(super) async fn refund_referral_preview(
    state: &AdminAppState<'_>,
    refund: &AdminWalletRefundRecord,
    stage: &str,
) -> Result<Value, GatewayError> {
    let rewards = if let Some(order_id) = refund.payment_order_id.as_deref() {
        if !state.app().has_referral_data_backend() {
            #[cfg(not(test))]
            return Err(GatewayError::Client {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "邀请返利风险数据暂不可用，请稍后重试".to_string(),
            });
        }
        let amount = if refund.status == "succeeded" {
            0.0
        } else {
            refund.amount_usd
        };
        state
            .app()
            .data
            .referral_refund_preview(order_id, amount)
            .await
            .map_err(|error| GatewayError::Internal(error.to_string()))?
            .into_iter()
            .map(|reward| {
                json!({
                    "reward_id": reward.reward_id,
                    "inviter_user_id": reward.inviter_user_id,
                    "inviter_username": reward.inviter_username,
                    "expected_reversal_usd": reward.amount_to_reverse_usd,
                    "available_gift_usd": reward.available_gift_balance_usd,
                    "deductible_usd": reward.deductible_amount_usd,
                    "shortfall_usd": reward.pending_amount_usd,
                    "already_reversed_amount_usd": reward.already_reversed_amount_usd,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    Ok(preview_payload(refund, stage, rewards))
}

fn operator_id(context: &AdminRequestContext<'_>) -> String {
    context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.as_str())
        .unwrap_or("")
        .to_string()
}

fn add_confirmation_token(
    state: &AdminAppState<'_>,
    context: &AdminRequestContext<'_>,
    refund: &AdminWalletRefundRecord,
    preview: &mut Value,
) -> Result<(), GatewayError> {
    let token = if preview["total_shortfall_usd"].as_f64().unwrap_or(0.0) > 0.00000001 {
        let purpose = confirmation_purpose(
            &refund.wallet_id,
            &refund.id,
            preview["stage"].as_str().unwrap_or(""),
            &operator_id(context),
        );
        let claims = json!({ "expires_at": chrono::Utc::now().timestamp() + CONFIRMATION_TTL_SECONDS, "risk": preview });
        Some(
            seal_runtime_secret_payload(state.app(), &purpose, &claims.to_string()).ok_or_else(
                || GatewayError::Client {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    message: "无法生成退款风险确认凭据，请检查加密配置".to_string(),
                },
            )?,
        )
    } else {
        None
    };
    preview["confirmation_token"] = json!(token);
    Ok(())
}

pub(super) async fn check_referral_confirmation(
    state: &AdminAppState<'_>,
    context: &AdminRequestContext<'_>,
    refund: &AdminWalletRefundRecord,
    stage: &str,
    token: Option<&str>,
) -> Result<Result<Value, Response<Body>>, GatewayError> {
    let mut preview = refund_referral_preview(state, refund, stage).await?;
    if preview["total_shortfall_usd"].as_f64().unwrap_or(0.0) > 0.00000001 {
        let purpose =
            confirmation_purpose(&refund.wallet_id, &refund.id, stage, &operator_id(context));
        let confirmed = confirmation_matches(
            state.app(),
            &purpose,
            token,
            &preview,
            chrono::Utc::now().timestamp(),
        );
        if !confirmed {
            add_confirmation_token(state, context, refund, &mut preview)?;
            return Ok(Err((
                StatusCode::CONFLICT,
                Json(json!({
                    "code": "REFERRAL_SHORTFALL_CONFIRMATION_REQUIRED",
                    "detail": "邀请人赠款余额不足以冲回全部返利，请核对当前缺口后确认继续退款",
                    "referral_preview": preview,
                })),
            )
                .into_response()));
        }
    }
    Ok(Ok(preview))
}

pub(in super::super) async fn build_admin_referral_preview_response(
    state: &AdminAppState<'_>,
    context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some((wallet_id, refund_id)) =
        admin_wallet_refund_ids_from_suffix_path(context.path(), "/referral-preview")
    else {
        return Ok(build_admin_wallets_bad_request_response(
            "wallet_id 或 refund_id 无效",
        ));
    };
    let Some(wallet) = state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::WalletId(
            &wallet_id,
        ))
        .await?
    else {
        return Ok(build_admin_wallet_not_found_response());
    };
    if wallet.api_key_id.is_some() {
        return Ok(build_admin_wallets_bad_request_response(
            ADMIN_WALLETS_API_KEY_REFUND_DETAIL,
        ));
    }
    let Some(refund) = state
        .app()
        .find_wallet_refund(&wallet_id, &refund_id)
        .await?
    else {
        return Ok(build_admin_wallet_refund_not_found_response());
    };
    let refund = super::complete_refund::stored_refund_to_gateway(refund);
    let stage = if matches!(refund.status.as_str(), "pending_approval" | "approved") {
        "process"
    } else {
        "complete"
    };
    let mut preview = refund_referral_preview(state, &refund, stage).await?;
    add_confirmation_token(state, context, &refund, &mut preview)?;
    Ok(Json(preview).into_response())
}

pub(super) async fn reversal_summary(
    state: &AdminAppState<'_>,
    order_id: Option<&str>,
) -> Result<Value, GatewayError> {
    if let Some(order_id) = order_id {
        if !state.app().has_referral_data_backend() {
            #[cfg(not(test))]
            return Err(GatewayError::Client {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "邀请返利风险数据暂不可用，请稍后重试".to_string(),
            });
        }
        let summary = state
            .app()
            .data
            .referral_order_reversal_summary(order_id)
            .await
            .map_err(|error| GatewayError::Internal(error.to_string()))?;
        Ok(
            json!({ "reversed_amount_usd": summary.reversed_amount_usd, "pending_reversal_amount_usd": summary.pending_reversal_amount_usd }),
        )
    } else {
        Ok(json!({ "reversed_amount_usd":0.0, "pending_reversal_amount_usd":0.0 }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn referral_refund_confirmation_rejects_forged_expired_reused_and_changed_risk() {
        let state = crate::AppState::new()
            .expect("state")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::disabled()
                    .with_encryption_key_for_tests(aether_crypto::DEVELOPMENT_ENCRYPTION_KEY),
            );
        let purpose = confirmation_purpose("wallet", "refund", "complete", "admin");
        let risk = json!({"total_shortfall_usd":3.0,"total_deductible_usd":2.0,"rewards":[{"inviter_user_id":"inviter","available_gift_usd":2.0}]});
        let token = seal_runtime_secret_payload(
            &state,
            &purpose,
            &json!({"expires_at":1300,"risk":risk}).to_string(),
        )
        .expect("token");
        assert!(confirmation_matches(
            &state,
            &purpose,
            Some(&token),
            &risk,
            1000
        ));
        assert!(!confirmation_matches(
            &state,
            &purpose,
            Some("forged"),
            &risk,
            1000
        ));
        assert!(!confirmation_matches(
            &state,
            &purpose,
            Some(&token),
            &risk,
            1300
        ));
        assert!(!confirmation_matches(
            &state,
            &confirmation_purpose("wallet", "refund", "process", "admin"),
            Some(&token),
            &risk,
            1000
        ));
        assert!(!confirmation_matches(
            &state,
            &confirmation_purpose("wallet", "refund", "complete", "other-admin"),
            Some(&token),
            &risk,
            1000
        ));
        let changed = json!({"total_shortfall_usd":4.0,"total_deductible_usd":1.0,"rewards":[{"inviter_user_id":"inviter","available_gift_usd":1.0}]});
        assert!(!confirmation_matches(
            &state,
            &purpose,
            Some(&token),
            &changed,
            1000
        ));
    }
}
