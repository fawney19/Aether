use super::super::shared::{
    build_admin_wallets_bad_request_response, parse_admin_wallets_limit,
    parse_admin_wallets_offset, parse_admin_wallets_owner_type_filter,
    resolve_admin_wallet_owner_summary, wallet_owner_summary_from_fields,
    ADMIN_WALLETS_API_KEY_REFUND_DETAIL,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{query_param_value, unix_secs_to_rfc3339};
use crate::GatewayError;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(in super::super) async fn build_admin_wallet_refund_requests_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let query = request_context.query_string();
    let limit = match parse_admin_wallets_limit(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_wallets_bad_request_response(detail)),
    };
    let offset = match parse_admin_wallets_offset(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_wallets_bad_request_response(detail)),
    };
    let status = query_param_value(query, "status");
    let owner_type = parse_admin_wallets_owner_type_filter(query);
    if owner_type.as_deref() == Some("api_key") {
        return Ok(build_admin_wallets_bad_request_response(
            ADMIN_WALLETS_API_KEY_REFUND_DETAIL,
        ));
    }

    let (refunds, total) = state
        .list_admin_wallet_refund_requests(status.as_deref(), limit, offset)
        .await?;
    let mut items = Vec::with_capacity(refunds.len());
    for refund in refunds {
        let mut owner = wallet_owner_summary_from_fields(
            refund.wallet_user_id.as_deref(),
            refund.wallet_user_name.clone(),
            refund.wallet_api_key_id.as_deref(),
            refund.api_key_name.clone(),
        );
        if owner.owner_name.is_none() {
            if let Some(wallet) = state
                .find_wallet(aether_data::repository::wallet::WalletLookupKey::WalletId(
                    &refund.wallet_id,
                ))
                .await?
            {
                owner = resolve_admin_wallet_owner_summary(state, &wallet).await?;
            }
        }
        items.push(json!({
            "id": refund.id,
            "refund_no": refund.refund_no,
            "wallet_id": refund.wallet_id,
            "owner_type": owner.owner_type,
            "owner_name": owner.owner_name,
            "wallet_status": refund.wallet_status,
            "user_id": refund.user_id,
            "payment_order_id": refund.payment_order_id,
            "source_type": refund.source_type,
            "source_id": refund.source_id,
            "refund_mode": refund.refund_mode,
            "amount_usd": refund.amount_usd,
            "status": refund.status,
            "reason": refund.reason,
            "failure_reason": refund.failure_reason,
            "gateway_refund_id": refund.gateway_refund_id,
            "payout_method": refund.payout_method,
            "payout_reference": refund.payout_reference,
            "payout_proof": refund.payout_proof,
            "requested_by": refund.requested_by,
            "approved_by": refund.approved_by,
            "processed_by": refund.processed_by,
            "created_at": refund.created_at_unix_secs.and_then(unix_secs_to_rfc3339),
            "updated_at": refund.updated_at_unix_secs.and_then(unix_secs_to_rfc3339),
            "processed_at": refund.processed_at_unix_secs.and_then(unix_secs_to_rfc3339),
            "completed_at": refund.completed_at_unix_secs.and_then(unix_secs_to_rfc3339),
        }));
    }

    Ok(Json(json!({
        "items": items,
        "total": total,
        "limit": limit,
        "offset": offset,
    }))
    .into_response())
}
