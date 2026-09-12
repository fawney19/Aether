use super::super::shared::{
    build_admin_wallets_bad_request_response, enrich_admin_wallet_package_summary,
    parse_admin_wallets_limit, parse_admin_wallets_offset, parse_admin_wallets_owner_type_filter,
    wallet_owner_summary_from_fields,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{query_param_value, unix_secs_to_rfc3339};
use crate::GatewayError;
use aether_data::repository::wallet::stored_timestamp_unix_secs;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(in super::super) async fn build_admin_wallet_list_response(
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

    let (wallets, total) = state
        .list_admin_wallets(status.as_deref(), owner_type.as_deref(), limit, offset)
        .await?;
    let mut items = Vec::with_capacity(wallets.len());
    for wallet in wallets {
        let owner = wallet_owner_summary_from_fields(
            wallet.user_id.as_deref(),
            wallet.user_name.clone(),
            wallet.api_key_id.as_deref(),
            wallet.api_key_name.clone(),
        );
        let user_id = wallet.user_id.clone();
        let wallet_balance = wallet.balance + wallet.gift_balance;
        let unlimited = wallet.limit_mode.eq_ignore_ascii_case("unlimited");
        let mut payload = json!({
            "id": wallet.id,
            "user_id": wallet.user_id,
            "api_key_id": wallet.api_key_id,
            "owner_type": owner.owner_type,
            "owner_name": owner.owner_name,
            "balance": wallet_balance,
            "recharge_balance": wallet.balance,
            "gift_balance": wallet.gift_balance,
            "refundable_balance": wallet.balance,
            "currency": wallet.currency,
            "status": wallet.status,
            "limit_mode": wallet.limit_mode.clone(),
            "unlimited": unlimited,
            "total_recharged": wallet.total_recharged,
            "total_consumed": wallet.total_consumed,
            "total_refunded": wallet.total_refunded,
            "total_adjusted": wallet.total_adjusted,
            "created_at": wallet
                .created_at_unix_ms
                .map(stored_timestamp_unix_secs)
                .and_then(unix_secs_to_rfc3339),
            "updated_at": wallet.updated_at_unix_secs.and_then(unix_secs_to_rfc3339),
        });
        enrich_admin_wallet_package_summary(
            state,
            &mut payload,
            user_id.as_deref(),
            wallet_balance,
            unlimited,
        )
        .await?;
        items.push(payload);
    }

    Ok(Json(json!({
        "items": items,
        "total": total,
        "limit": limit,
        "offset": offset,
    }))
    .into_response())
}
