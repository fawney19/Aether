use super::{
    build_auth_error_response, build_auth_json_response, build_auth_wallet_summary_payload, http,
    resolve_authenticated_local_user, sanitize_wallet_gateway_response, unix_secs_to_rfc3339,
    wallet_normalize_optional_string_field, AppState, Body, GatewayPublicRequestContext, Response,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct WalletRedeemRequest {
    code: String,
}

fn wallet_build_redeem_order_no(now: chrono::DateTime<chrono::Utc>) -> String {
    format!(
        "po_{}_{}",
        now.format("%Y%m%d%H%M%S%6f"),
        &Uuid::new_v4().simple().to_string()[..12]
    )
}

fn build_wallet_payment_order_payload(
    record: &aether_data::repository::wallet::StoredAdminPaymentOrder,
) -> serde_json::Value {
    json!({
        "id": record.id,
        "order_no": record.order_no,
        "wallet_id": record.wallet_id,
        "user_id": record.user_id,
        "amount_usd": record.amount_usd,
        "pay_amount": record.pay_amount,
        "pay_currency": record.pay_currency,
        "exchange_rate": record.exchange_rate,
        "refunded_amount_usd": record.refunded_amount_usd,
        "refundable_amount_usd": record.refundable_amount_usd,
        "payment_method": record.payment_method,
        "gateway_order_id": record.gateway_order_id,
        "gateway_response": sanitize_wallet_gateway_response(record.gateway_response.clone()),
        "status": record.status,
        "created_at": unix_secs_to_rfc3339(record.created_at_unix_ms),
        "paid_at": record.paid_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "credited_at": record.credited_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "expires_at": record.expires_at_unix_secs.and_then(unix_secs_to_rfc3339),
    })
}

pub(super) async fn handle_wallet_redeem(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Response<Body> {
    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(request_body) = request_body else {
        return build_auth_error_response(http::StatusCode::BAD_REQUEST, "缺少请求体", false);
    };
    let payload = match serde_json::from_slice::<WalletRedeemRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, "输入验证失败", false)
        }
    };
    let code = match wallet_normalize_optional_string_field(Some(payload.code), 128) {
        Ok(Some(value)) => value,
        _ => {
            return build_auth_error_response(http::StatusCode::BAD_REQUEST, "输入验证失败", false)
        }
    };

    let outcome = match state
        .redeem_wallet_code(aether_data::repository::wallet::RedeemWalletCodeInput {
            code,
            user_id: auth.user.id.clone(),
            order_no: wallet_build_redeem_order_no(Utc::now()),
        })
        .await
    {
        Ok(Some(value)) => value,
        Ok(None) => {
            return build_auth_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "钱包兑换后端暂不可用",
                false,
            );
        }
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("wallet redeem failed: {err:?}"),
                false,
            );
        }
    };

    match outcome {
        aether_data::repository::wallet::RedeemWalletCodeOutcome::Redeemed {
            wallet,
            order,
            amount_usd,
            batch_name,
        } => build_auth_json_response(
            http::StatusCode::OK,
            json!({
                "order": build_wallet_payment_order_payload(&order),
                "wallet": build_auth_wallet_summary_payload(Some(&wallet)),
                "amount_usd": amount_usd,
                "batch_name": batch_name,
            }),
            None,
        ),
        aether_data::repository::wallet::RedeemWalletCodeOutcome::InvalidCode => {
            build_auth_error_response(http::StatusCode::BAD_REQUEST, "兑换码格式无效", false)
        }
        aether_data::repository::wallet::RedeemWalletCodeOutcome::CodeNotFound => {
            build_auth_error_response(http::StatusCode::NOT_FOUND, "兑换码不存在", false)
        }
        aether_data::repository::wallet::RedeemWalletCodeOutcome::CodeDisabled
        | aether_data::repository::wallet::RedeemWalletCodeOutcome::BatchDisabled => {
            build_auth_error_response(http::StatusCode::BAD_REQUEST, "兑换码已停用", false)
        }
        aether_data::repository::wallet::RedeemWalletCodeOutcome::CodeExpired => {
            build_auth_error_response(http::StatusCode::BAD_REQUEST, "兑换码已过期", false)
        }
        aether_data::repository::wallet::RedeemWalletCodeOutcome::CodeRedeemed => {
            build_auth_error_response(http::StatusCode::BAD_REQUEST, "兑换码已被使用", false)
        }
        aether_data::repository::wallet::RedeemWalletCodeOutcome::WalletInactive => {
            build_auth_error_response(http::StatusCode::BAD_REQUEST, "wallet is not active", false)
        }
    }
}
