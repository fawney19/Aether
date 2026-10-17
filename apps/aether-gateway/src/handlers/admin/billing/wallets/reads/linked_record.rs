use super::super::shared::{
    admin_wallet_payout_proof_projection, build_admin_wallet_not_found_response,
    build_admin_wallets_bad_request_response, resolve_admin_wallet_owner_summary,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::unix_secs_to_rfc3339;
use crate::GatewayError;
use aether_data::repository::wallet::{stored_timestamp_unix_secs, WalletLookupKey};
use axum::{
    body::Body,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(in super::super) async fn build_admin_wallet_linked_record_response(
    state: &AdminAppState<'_>,
    context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let parts = context
        .path()
        .trim_end_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    if parts.len() != 7 || parts[4].is_empty() || parts[6].is_empty() {
        return Ok(build_admin_wallets_bad_request_response("钱包记录 ID 无效"));
    }
    let Some(wallet) = state
        .find_wallet(WalletLookupKey::WalletId(parts[4]))
        .await?
    else {
        return Ok(build_admin_wallet_not_found_response());
    };
    let owner = resolve_admin_wallet_owner_summary(state, &wallet).await?;
    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"detail":"Wallet record not found"})),
        )
            .into_response()
    };
    if parts[5] == "refunds" {
        let Some(refund) = state.find_wallet_refund(&wallet.id, parts[6]).await? else {
            return Ok(not_found());
        };
        let mut payload =
            serde_json::to_value(&refund).map_err(|err| GatewayError::Internal(err.to_string()))?;
        let obj = payload
            .as_object_mut()
            .expect("wallet refund serializes as object");
        for key in [
            "created_at_unix_ms",
            "updated_at_unix_secs",
            "processed_at_unix_secs",
            "completed_at_unix_secs",
        ] {
            obj.remove(key);
        }
        obj.insert("owner_type".into(), json!(owner.owner_type));
        obj.insert("owner_name".into(), json!(owner.owner_name));
        obj.insert("wallet_status".into(), json!(wallet.status));
        obj.insert(
            "payout_proof".into(),
            json!(admin_wallet_payout_proof_projection(
                refund.payout_proof.as_ref()
            )),
        );
        obj.insert(
            "created_at".into(),
            json!(unix_secs_to_rfc3339(stored_timestamp_unix_secs(
                refund.created_at_unix_ms
            ))),
        );
        obj.insert(
            "updated_at".into(),
            json!(unix_secs_to_rfc3339(refund.updated_at_unix_secs)),
        );
        obj.insert(
            "processed_at".into(),
            json!(refund.processed_at_unix_secs.and_then(unix_secs_to_rfc3339)),
        );
        obj.insert(
            "completed_at".into(),
            json!(refund.completed_at_unix_secs.and_then(unix_secs_to_rfc3339)),
        );
        return Ok(Json(json!({"refund":payload})).into_response());
    }
    let Some(transaction) = state
        .app()
        .find_admin_wallet_transaction(&wallet.id, parts[6])
        .await?
    else {
        return Ok(not_found());
    };
    let mut payload = serde_json::to_value(&transaction)
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let obj = payload
        .as_object_mut()
        .expect("wallet transaction serializes as object");
    obj.remove("created_at_unix_ms");
    obj.insert("owner_type".into(), json!(owner.owner_type));
    obj.insert("owner_name".into(), json!(owner.owner_name));
    obj.insert("wallet_status".into(), json!(wallet.status));
    obj.insert(
        "created_at".into(),
        json!(transaction
            .created_at_unix_ms
            .map(stored_timestamp_unix_secs)
            .and_then(unix_secs_to_rfc3339)),
    );
    Ok(Json(json!({"transaction":payload})).into_response())
}
