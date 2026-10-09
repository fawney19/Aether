use super::super::shared::{
    admin_wallet_operator_id, admin_wallet_refund_ids_from_suffix_path,
    build_admin_wallet_not_found_response, build_admin_wallet_refund_not_found_response,
    build_admin_wallet_refund_payload, build_admin_wallet_summary_payload_with_package,
    build_admin_wallet_transaction_payload, build_admin_wallets_bad_request_response,
    build_admin_wallets_data_unavailable_response, resolve_admin_wallet_owner_summary,
    ADMIN_WALLETS_API_KEY_REFUND_DETAIL,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{attach_admin_audit_response, unix_secs_to_rfc3339};
use crate::GatewayError;
use aether_data::repository::wallet::stored_timestamp_unix_secs;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(in super::super) async fn build_admin_wallet_process_refund_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some((wallet_id, refund_id)) =
        admin_wallet_refund_ids_from_suffix_path(request_context.path(), "/process")
    else {
        return Ok(build_admin_wallets_bad_request_response(
            "wallet_id 或 refund_id 无效",
        ));
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

    #[derive(Default, serde::Deserialize)]
    struct Confirmation {
        referral_shortfall_confirmation: Option<String>,
    }
    let confirmation = match request_body.filter(|body| !body.is_empty()) {
        Some(body) => match serde_json::from_slice::<Confirmation>(body) {
            Ok(value) => value,
            Err(_) => return Ok(build_admin_wallets_bad_request_response("请求体格式无效")),
        },
        None => Confirmation::default(),
    };
    let Some(refund) = state
        .app()
        .find_wallet_refund(&wallet_id, &refund_id)
        .await?
    else {
        return Ok(build_admin_wallet_refund_not_found_response());
    };
    let refund = super::complete_refund::stored_refund_to_gateway(refund);
    if !matches!(refund.status.as_str(), "pending_approval" | "approved") {
        return Ok(build_admin_wallets_bad_request_response(
            "只有待审批退款可以处理",
        ));
    }
    let referral_preview = match super::referral_preview::check_referral_confirmation(
        state,
        request_context,
        &refund,
        "process",
        confirmation.referral_shortfall_confirmation.as_deref(),
    )
    .await?
    {
        Ok(preview) => preview,
        Err(response) => return Ok(response),
    };

    let operator_id = admin_wallet_operator_id(request_context);
    match state
        .admin_process_wallet_refund(&wallet_id, &refund_id, operator_id.as_deref())
        .await?
    {
        crate::AdminWalletMutationOutcome::Applied((wallet, refund, transaction)) => {
            let owner = resolve_admin_wallet_owner_summary(state, &wallet).await?;
            let wallet_payload =
                build_admin_wallet_summary_payload_with_package(state, &wallet, &owner).await?;
            let response = Json(json!({
                "wallet": wallet_payload,
                "refund": build_admin_wallet_refund_payload(&wallet, &owner, &refund),
                "transaction": build_admin_wallet_transaction_payload(
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
                    unix_secs_to_rfc3339(stored_timestamp_unix_secs(transaction.created_at_unix_ms)),
                ),
                "referral_preview": referral_preview,
            }))
            .into_response();
            Ok(attach_admin_audit_response(
                response,
                "admin_wallet_refund_processed",
                "process_wallet_refund",
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
