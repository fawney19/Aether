use super::super::shared::{
    admin_wallet_refund_ids_from_suffix_path, build_admin_wallet_not_found_response,
    build_admin_wallet_refund_not_found_response, build_admin_wallet_refund_payload,
    build_admin_wallets_bad_request_response, build_admin_wallets_data_unavailable_response,
    normalize_admin_wallet_optional_text, resolve_admin_wallet_owner_summary,
    AdminWalletRefundCompleteRequest, ADMIN_WALLETS_API_KEY_REFUND_DETAIL,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::handlers::shared::{
    payment_gateway_provider_for_payment_method, payment_gateway_refund_enabled,
};
use crate::GatewayError;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use tracing::warn;

fn merge_gateway_refund_proof(
    proof: Option<Value>,
    gateway_refund: Option<&crate::handlers::shared::DirectGatewayRefundResult>,
) -> Option<Value> {
    let Some(gateway_refund) = gateway_refund else {
        return proof;
    };
    let mut object = proof
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    object.insert(
        "gateway_refund".to_string(),
        json!({
            "id": gateway_refund.gateway_refund_id,
            "status": gateway_refund.status,
            "payload": gateway_refund.payload,
        }),
    );
    Some(Value::Object(object))
}

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
    let Some(refund_before_complete) = state
        .app()
        .find_wallet_refund(&wallet_id, &refund_id)
        .await?
    else {
        return Ok(build_admin_wallet_refund_not_found_response());
    };
    let mut gateway_refund_id = gateway_refund_id;
    let mut payout_proof = payload.payout_proof;
    if payload.gateway_refund {
        let Some(payment_order_id) = refund_before_complete.payment_order_id.as_deref() else {
            return Ok(build_admin_wallets_bad_request_response(
                "网关原路退款需要退款申请关联支付订单",
            ));
        };
        let order = match state.read_admin_payment_order(payment_order_id).await? {
            crate::AdminWalletMutationOutcome::Applied(order) => order,
            crate::AdminWalletMutationOutcome::NotFound => {
                return Ok(build_admin_wallets_bad_request_response("支付订单不存在"))
            }
            crate::AdminWalletMutationOutcome::Invalid(detail) => {
                return Ok(build_admin_wallets_bad_request_response(detail))
            }
            crate::AdminWalletMutationOutcome::Unavailable => {
                return Ok(build_admin_wallets_data_unavailable_response())
            }
        };
        if let Some(provider) = payment_gateway_provider_for_payment_method(&order.payment_method) {
            let refund_enabled = state
                .app()
                .find_payment_gateway_config(provider)
                .await?
                .is_some_and(|record| payment_gateway_refund_enabled(&record.channels_json));
            if !refund_enabled {
                return Ok(build_admin_wallets_bad_request_response(
                    "该支付方式未启用退款",
                ));
            }
        }
        match crate::handlers::shared::refund_direct_gateway_order(
            state.app(),
            &order,
            &refund_before_complete.refund_no,
            refund_before_complete.amount_usd,
            refund_before_complete.reason.as_deref(),
        )
        .await
        {
            Ok(Some(result)) => {
                gateway_refund_id = Some(result.gateway_refund_id.clone());
                payout_proof = merge_gateway_refund_proof(payout_proof, Some(&result));
            }
            Ok(None) => {
                return Ok(build_admin_wallets_bad_request_response(
                    "该支付方式不支持官方直连退款，请使用线下完成",
                ))
            }
            Err(detail) => return Ok(build_admin_wallets_bad_request_response(detail)),
        }
    }
    match state
        .admin_complete_wallet_refund(
            &wallet_id,
            &refund_id,
            gateway_refund_id.as_deref(),
            payout_reference.as_deref(),
            payout_proof,
        )
        .await?
    {
        crate::AdminWalletMutationOutcome::Applied(refund) => {
            if let Some(order_id) = refund.payment_order_id.as_deref() {
                if let Err(err) = state
                    .app()
                    .reverse_referral_rewards_for_order(order_id, refund.amount_usd)
                    .await
                {
                    warn!(
                        error = ?err,
                        order_id = %order_id,
                        refund_id = %refund.id,
                        "failed to reverse referral rewards for completed refund"
                    );
                }
            }
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
