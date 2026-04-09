use super::super::shared::{
    admin_wallet_operator_id, admin_wallet_refund_ids_from_suffix_path,
    build_admin_wallet_not_found_response, build_admin_wallet_refund_not_found_response,
    build_admin_wallet_refund_payload, build_admin_wallet_summary_payload,
    build_admin_wallet_transaction_payload, build_admin_wallets_bad_request_response,
    build_admin_wallets_data_unavailable_response, normalize_admin_wallet_required_text,
    resolve_admin_wallet_owner_summary, AdminWalletRefundFailRequest,
    ADMIN_WALLETS_API_KEY_REFUND_DETAIL,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{attach_admin_audit_response, unix_secs_to_rfc3339};
use crate::GatewayError;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(in super::super) async fn build_admin_wallet_fail_refund_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some((wallet_id, refund_id)) =
        admin_wallet_refund_ids_from_suffix_path(request_context.path(), "/fail")
    else {
        return Ok(build_admin_wallets_bad_request_response(
            "wallet_id 或 refund_id 无效",
        ));
    };
    let Some(request_body) = request_body else {
        return Ok(build_admin_wallets_bad_request_response("请求体不能为空"));
    };
    let payload = match serde_json::from_slice::<AdminWalletRefundFailRequest>(request_body) {
        Ok(value) => value,
        Err(_) => return Ok(build_admin_wallets_bad_request_response("请求体格式无效")),
    };
    let reason = match normalize_admin_wallet_required_text(payload.reason, "reason", 500) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_wallets_bad_request_response(detail)),
    };

    let Some(existing_wallet) = state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::WalletId(
            &wallet_id,
        ))
        .await?
    else {
        return Ok(build_admin_wallet_not_found_response());
    };
    if existing_wallet.api_key_id.is_some() {
        return Ok(build_admin_wallets_bad_request_response(
            ADMIN_WALLETS_API_KEY_REFUND_DETAIL,
        ));
    }

    let operator_id = admin_wallet_operator_id(request_context);
    match state
        .admin_fail_wallet_refund(&wallet_id, &refund_id, &reason, operator_id.as_deref())
        .await?
    {
        crate::AdminWalletMutationOutcome::Applied((wallet, refund, transaction)) => {
            let owner = resolve_admin_wallet_owner_summary(state, &wallet).await?;
            let response = Json(json!({
                "wallet": build_admin_wallet_summary_payload(&wallet, &owner),
                "refund": build_admin_wallet_refund_payload(&wallet, &owner, &refund),
                "transaction": transaction
                    .map(|transaction| {
                        build_admin_wallet_transaction_payload(
                            &wallet,
                            &owner,
                            transaction.id,
                            &transaction.category,
                            &transaction.reason_code,
                            transaction.amount,
                            transaction.balance_before,
                            transaction.balance_after,
                            transaction.recharge_balance_before,
                            transaction.recharge_balance_after,
                            transaction.gift_balance_before,
                            transaction.gift_balance_after,
                            transaction.link_type.as_deref(),
                            transaction.link_id.as_deref(),
                            transaction.operator_id.as_deref(),
                            transaction.description.as_deref(),
                            unix_secs_to_rfc3339(transaction.created_at_unix_secs),
                        )
                    })
                    .unwrap_or(serde_json::Value::Null),
            }))
            .into_response();
            Ok(attach_admin_audit_response(
                response,
                "admin_wallet_refund_failed",
                "fail_wallet_refund",
                "wallet_refund",
                &refund_id,
            ))
        }
        crate::AdminWalletMutationOutcome::NotFound => {
            Ok(build_admin_wallet_refund_not_found_response())
        }
        crate::AdminWalletMutationOutcome::Invalid(detail) => {
            Ok(build_admin_wallets_bad_request_response(detail))
        }
        crate::AdminWalletMutationOutcome::Unavailable => {
            Ok(build_admin_wallets_data_unavailable_response())
        }
    }
}
