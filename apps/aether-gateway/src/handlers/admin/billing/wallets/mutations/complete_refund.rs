use super::super::shared::{
    admin_wallet_refund_ids_from_suffix_path, build_admin_wallet_not_found_response,
    build_admin_wallet_refund_not_found_response, build_admin_wallet_refund_payload,
    build_admin_wallets_bad_request_response, build_admin_wallets_data_unavailable_response,
    normalize_admin_wallet_optional_text, resolve_admin_wallet_owner_summary,
    AdminWalletRefundCompleteRequest, ADMIN_WALLETS_API_KEY_REFUND_DETAIL,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::GatewayError;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(in super::super) async fn build_admin_wallet_complete_refund_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some((wallet_id, refund_id)) =
        admin_wallet_refund_ids_from_suffix_path(request_context.path(), "/complete")
    else {
        return Ok(build_admin_wallets_bad_request_response(
            "wallet_id 或 refund_id 无效",
        ));
    };
    let Some(request_body) = request_body else {
        return Ok(build_admin_wallets_bad_request_response("请求体不能为空"));
    };
    let payload = match serde_json::from_slice::<AdminWalletRefundCompleteRequest>(request_body) {
        Ok(value) => value,
        Err(_) => return Ok(build_admin_wallets_bad_request_response("请求体格式无效")),
    };
    let gateway_refund_id = match normalize_admin_wallet_optional_text(
        payload.gateway_refund_id,
        "gateway_refund_id",
        128,
    ) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_wallets_bad_request_response(detail)),
    };
    let payout_reference = match normalize_admin_wallet_optional_text(
        payload.payout_reference,
        "payout_reference",
        255,
    ) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_wallets_bad_request_response(detail)),
    };
    if payload
        .payout_proof
        .as_ref()
        .is_some_and(|value| !value.is_object())
    {
        return Ok(build_admin_wallets_bad_request_response(
            "payout_proof 必须为对象",
        ));
    }

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

    let owner = resolve_admin_wallet_owner_summary(state, &wallet).await?;
    match state
        .admin_complete_wallet_refund(
            &wallet_id,
            &refund_id,
            gateway_refund_id.as_deref(),
            payout_reference.as_deref(),
            payload.payout_proof,
        )
        .await?
    {
        crate::AdminWalletMutationOutcome::Applied(refund) => {
            let response = Json(json!({
                "refund": build_admin_wallet_refund_payload(&wallet, &owner, &refund),
            }))
            .into_response();
            Ok(attach_admin_audit_response(
                response,
                "admin_wallet_refund_completed",
                "complete_wallet_refund",
                "wallet_refund",
                &refund_id,
            ))
        }
        crate::AdminWalletMutationOutcome::NotFound => {
            Ok(build_admin_wallet_refund_not_found_response())
        }
        crate::AdminWalletMutationOutcome::Invalid(detail) => {
            let detail = if detail == "refund status must be processing before completion" {
                "只有 processing 状态的退款可以标记完成".to_string()
            } else {
                detail
            };
            Ok(build_admin_wallets_bad_request_response(detail))
        }
        crate::AdminWalletMutationOutcome::Unavailable => {
            Ok(build_admin_wallets_data_unavailable_response())
        }
    }
}
