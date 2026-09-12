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
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use tracing::warn;

fn is_safe_gateway_refund_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn gateway_refund_mode_allowed(refund_mode: &str) -> bool {
    refund_mode.trim().eq_ignore_ascii_case("original_channel")
}

fn stored_refund_to_gateway(
    refund: aether_data::repository::wallet::StoredAdminWalletRefund,
) -> crate::AdminWalletRefundRecord {
    crate::AdminWalletRefundRecord {
        id: refund.id,
        refund_no: refund.refund_no,
        wallet_id: refund.wallet_id,
        user_id: refund.user_id,
        payment_order_id: refund.payment_order_id,
        source_type: refund.source_type,
        source_id: refund.source_id,
        refund_mode: refund.refund_mode,
        amount_usd: refund.amount_usd,
        status: refund.status,
        reason: refund.reason,
        failure_reason: refund.failure_reason,
        gateway_refund_id: refund.gateway_refund_id,
        payout_method: refund.payout_method,
        payout_reference: refund.payout_reference,
        payout_proof: refund.payout_proof,
        requested_by: refund.requested_by,
        approved_by: refund.approved_by,
        processed_by: refund.processed_by,
        created_at_unix_ms: refund.created_at_unix_ms,
        updated_at_unix_secs: refund.updated_at_unix_secs,
        processed_at_unix_secs: refund.processed_at_unix_secs,
        completed_at_unix_secs: refund.completed_at_unix_secs,
    }
}

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
    object.insert("gateway_refund".to_string(), gateway_refund.proof.clone());
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
        Ok(value) if value.as_deref().is_none_or(is_safe_gateway_refund_id) => value,
        Ok(_) => {
            return Ok(build_admin_wallets_bad_request_response(
                "gateway_refund_id 格式无效",
            ))
        }
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
    let refund_before_complete = stored_refund_to_gateway(refund_before_complete);
    if !refund_before_complete.amount_usd.is_finite() || refund_before_complete.amount_usd <= 0.0 {
        return Ok(build_admin_wallets_bad_request_response("退款金额无效"));
    }
    if refund_before_complete.status == "succeeded" {
        if let Some(order_id) = refund_before_complete.payment_order_id.as_deref() {
            if let Err(err) = state
                .app()
                .reverse_referral_rewards_for_order(order_id, refund_before_complete.amount_usd)
                .await
            {
                warn!(
                    error = ?err,
                    order_id = %order_id,
                    refund_id = %refund_before_complete.id,
                    "failed to reconcile referral rewards for completed refund"
                );
                return Ok(build_admin_wallets_data_unavailable_response());
            }
        }
        let response = Json(json!({
            "refund": build_admin_wallet_refund_payload(
                &wallet,
                &owner,
                &refund_before_complete,
            ),
        }))
        .into_response();
        return Ok(attach_admin_audit_response(
            response,
            "admin_wallet_refund_completed",
            "complete_wallet_refund",
            "wallet_refund",
            &refund_id,
        ));
    }
    let mut gateway_refund_id = gateway_refund_id;
    let mut payout_proof = payload.payout_proof;
    if payload.gateway_refund {
        // A line-item refund in `offline_payout` mode has no provider-side
        // settlement contract.  Calling a gateway before recording evidence
        // would let `/fail` concurrently release the local reservation and
        // leave an external refund with no durable proof.  Keep the mode
        // constraint at the boundary, before any network request.
        if !gateway_refund_mode_allowed(&refund_before_complete.refund_mode) {
            return Ok(build_admin_wallets_bad_request_response(
                "只有原支付渠道退款可以调用支付网关",
            ));
        }
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
            crate::AdminWalletMutationOutcome::Invalid(_) => {
                return Ok(build_admin_wallets_bad_request_response(
                    "支付订单状态或数据无效",
                ))
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
                if result.is_pending() {
                    let persisted = match state
                        .app()
                        .update_admin_wallet_refund_gateway(
                            aether_data::repository::wallet::UpdateAdminWalletRefundGatewayInput {
                                wallet_id: wallet_id.clone(),
                                refund_id: refund_id.clone(),
                                gateway_refund_id: result.gateway_refund_id.clone(),
                                payout_proof: Some(result.proof.clone()),
                            },
                        )
                        .await?
                    {
                        Some(aether_data::repository::wallet::WalletMutationOutcome::Applied(
                            refund,
                        )) => refund,
                        Some(aether_data::repository::wallet::WalletMutationOutcome::NotFound) => {
                            return Ok(build_admin_wallet_refund_not_found_response())
                        }
                        Some(aether_data::repository::wallet::WalletMutationOutcome::Invalid(
                            detail,
                        )) => return Ok(build_admin_wallets_bad_request_response(detail)),
                        None => return Ok(build_admin_wallets_data_unavailable_response()),
                    };
                    let persisted = stored_refund_to_gateway(persisted);
                    let response = (
                        http::StatusCode::ACCEPTED,
                        Json(json!({
                            "refund": build_admin_wallet_refund_payload(&wallet, &owner, &persisted),
                            "gateway_refund": {
                                "id": result.gateway_refund_id,
                                "status": result.status,
                            },
                        })),
                    )
                        .into_response();
                    return Ok(attach_admin_audit_response(
                        response,
                        "admin_wallet_refund_pending",
                        "complete_wallet_refund",
                        "wallet_refund",
                        &refund_id,
                    ));
                }
                if !result.is_succeeded() {
                    return Ok(build_admin_wallets_bad_request_response("上游退款未成功"));
                }
                payout_proof = merge_gateway_refund_proof(payout_proof, Some(&result));

                // Persist the provider evidence before releasing the local refund reservation.
                // If the local completion transaction fails after a successful gateway call,
                // a retry can reuse the idempotent gateway identifier instead of issuing a
                // second refund with no durable proof of the first one.
                match state
                    .app()
                    .update_admin_wallet_refund_gateway(
                        aether_data::repository::wallet::UpdateAdminWalletRefundGatewayInput {
                            wallet_id: wallet_id.clone(),
                            refund_id: refund_id.clone(),
                            gateway_refund_id: result.gateway_refund_id.clone(),
                            payout_proof: payout_proof.clone(),
                        },
                    )
                    .await?
                {
                    Some(aether_data::repository::wallet::WalletMutationOutcome::Applied(_)) => {}
                    Some(aether_data::repository::wallet::WalletMutationOutcome::NotFound) => {
                        return Ok(build_admin_wallet_refund_not_found_response())
                    }
                    Some(aether_data::repository::wallet::WalletMutationOutcome::Invalid(
                        detail,
                    )) => return Ok(build_admin_wallets_bad_request_response(detail)),
                    None => return Ok(build_admin_wallets_data_unavailable_response()),
                }
            }
            Ok(None) => {
                return Ok(build_admin_wallets_bad_request_response(
                    "该支付方式不支持官方直连退款，请使用线下完成",
                ))
            }
            Err(detail) => {
                warn!(
                    error = %detail,
                    refund_id = %refund_id,
                    "direct payment gateway refund failed"
                );
                return Ok(build_admin_wallets_bad_request_response(
                    "支付网关退款请求失败",
                ));
            }
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
                    return Ok(build_admin_wallets_data_unavailable_response());
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
                "退款状态或参数无效".to_string()
            };
            Ok(build_admin_wallets_bad_request_response(detail))
        }
        crate::AdminWalletMutationOutcome::Unavailable => {
            Ok(build_admin_wallets_data_unavailable_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        gateway_refund_mode_allowed, is_safe_gateway_refund_id, merge_gateway_refund_proof,
    };
    use crate::handlers::shared::DirectGatewayRefundResult;
    use serde_json::json;

    #[test]
    fn gateway_refund_merge_replaces_legacy_raw_payload() {
        let existing = json!({
            "channel": "manual",
            "gateway_refund": {
                "payload": {
                    "authorization": "Bearer legacy-secret",
                    "payer": {"openid": "openid-secret"}
                }
            }
        });
        let result = DirectGatewayRefundResult {
            gateway_refund_id: "refund-1".to_string(),
            status: "success".to_string(),
            proof: json!({
                "gateway": "wxpay",
                "id": "refund-1",
                "status": "success",
                "order_no": "order-1",
                "refund_no": "request-1",
                "amount": 8.5,
                "currency": "CNY",
                "processed_at": "2026-08-27T12:00:00Z"
            }),
        };

        let merged = merge_gateway_refund_proof(Some(existing), Some(&result))
            .expect("gateway proof should be merged");
        assert_eq!(merged["channel"], "manual");
        assert_eq!(merged["gateway_refund"], result.proof);
        let encoded = merged.to_string();
        assert!(!encoded.contains("legacy-secret"));
        assert!(!encoded.contains("openid-secret"));
        assert!(!encoded.contains("payload"));
    }

    #[test]
    fn manual_gateway_refund_ids_use_the_same_strict_identifier_policy() {
        assert!(is_safe_gateway_refund_id("refund_123-ABC"));
        for value in [
            "Authorization: Bearer secret",
            "https://internal.example/refund?token=secret",
            "refund id",
            "payer/openid",
        ] {
            assert!(!is_safe_gateway_refund_id(value));
        }
        assert!(!is_safe_gateway_refund_id(&"a".repeat(129)));
    }

    #[test]
    fn gateway_refunds_are_limited_to_original_channel_mode() {
        assert!(gateway_refund_mode_allowed("original_channel"));
        assert!(gateway_refund_mode_allowed(" Original_Channel "));
        assert!(!gateway_refund_mode_allowed("offline_payout"));
        assert!(!gateway_refund_mode_allowed(""));
    }
}
