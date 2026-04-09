use super::super::shared::{
    admin_wallet_id_from_detail_path, build_admin_wallet_not_found_response,
    build_admin_wallet_summary_payload, build_admin_wallets_bad_request_response,
    resolve_admin_wallet_owner_summary,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};

pub(in super::super) async fn build_admin_wallet_detail_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(wallet_id) = admin_wallet_id_from_detail_path(request_context.path()) else {
        return Ok(build_admin_wallets_bad_request_response("wallet_id 无效"));
    };

    let Some(wallet) = state
        .find_wallet(aether_data::repository::wallet::WalletLookupKey::WalletId(
            &wallet_id,
        ))
        .await?
    else {
        return Ok(build_admin_wallet_not_found_response());
    };

    let owner = resolve_admin_wallet_owner_summary(state, &wallet).await?;
    let mut payload = build_admin_wallet_summary_payload(&wallet, &owner);
    if let Some(object) = payload.as_object_mut() {
        object.insert("pending_refund_count".to_string(), serde_json::Value::Null);
    }
    Ok(Json(payload).into_response())
}
