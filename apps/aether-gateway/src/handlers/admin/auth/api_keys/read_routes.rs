use super::shared::{
    admin_api_keys_id_from_path, admin_api_keys_parse_limit, admin_api_keys_parse_skip,
    build_admin_api_key_detail_payload, build_admin_api_key_list_item_payload,
    build_admin_api_keys_bad_request_response, build_admin_api_keys_data_unavailable_response,
    build_admin_api_keys_not_found_response,
};
use super::{decrypt_catalog_secret_with_fallbacks, query_param_bool, query_param_optional_bool};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::Instant;
use tracing::info;

pub(super) async fn build_admin_list_api_keys_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let handler_started_at = Instant::now();
    let query = request_context.query_string();
    let skip = match admin_api_keys_parse_skip(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
    };
    let limit = match admin_api_keys_parse_limit(query) {
        Ok(value) => value,
        Err(detail) => return Ok(build_admin_api_keys_bad_request_response(detail)),
    };
    let is_active = query_param_optional_bool(query, "is_active");

    let list_query = aether_data::repository::auth::StandaloneApiKeyExportListQuery {
        skip,
        limit,
        is_active,
    };
    let count_and_page_started_at = Instant::now();
    let (total, paged_records) = tokio::try_join!(
        state.count_auth_api_key_export_standalone_records(is_active),
        state.list_auth_api_key_export_standalone_records_page(&list_query),
    )?;
    let count_and_page_ms = count_and_page_started_at.elapsed().as_millis() as u64;
    let api_key_ids = paged_records
        .iter()
        .map(|record| record.api_key_id.clone())
        .collect::<Vec<_>>();
    let wallet_lookup_started_at = Instant::now();
    let wallets_by_api_key_id = state
        .list_wallet_snapshots_by_api_key_ids(&api_key_ids)
        .await?
        .into_iter()
        .filter_map(|wallet| {
            wallet
                .api_key_id
                .clone()
                .map(|api_key_id| (api_key_id, wallet))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let wallet_lookup_ms = wallet_lookup_started_at.elapsed().as_millis() as u64;

    let api_keys = paged_records
        .iter()
        .map(|record| {
            build_admin_api_key_list_item_payload(
                state,
                record,
                wallets_by_api_key_id.get(&record.api_key_id),
            )
        })
        .collect::<Vec<_>>();

    info!(
        event_name = "admin_api_keys_list_timing",
        log_type = "event",
        trace_id = request_context.trace_id.as_str(),
        returned_items = api_keys.len(),
        total,
        count_and_page_ms,
        wallet_lookup_ms,
        handler_ms = handler_started_at.elapsed().as_millis() as u64,
        "measured admin api keys list handler timing"
    );

    Ok(Json(json!({
        "api_keys": api_keys,
        "total": total as usize,
        "limit": limit,
        "skip": skip,
    }))
    .into_response())
}

pub(super) async fn build_admin_api_key_detail_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(api_key_id) = admin_api_keys_id_from_path(request_context.path()) else {
        return Ok(build_admin_api_keys_data_unavailable_response());
    };

    if state
        .list_auth_api_key_snapshots_by_ids(std::slice::from_ref(&api_key_id))
        .await?
        .into_iter()
        .any(|snapshot| snapshot.api_key_id == api_key_id && !snapshot.api_key_is_standalone)
    {
        return Ok(build_admin_api_keys_bad_request_response(
            "仅支持查看独立密钥",
        ));
    }

    let Some(record) = state
        .find_auth_api_key_export_standalone_record_by_id(&api_key_id)
        .await?
    else {
        return Ok(build_admin_api_keys_not_found_response());
    };

    if query_param_bool(request_context.query_string(), "include_key", false) {
        let Some(ciphertext) = record
            .key_encrypted
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(build_admin_api_keys_bad_request_response(
                "该密钥没有存储完整密钥信息",
            ));
        };
        let Some(key) = decrypt_catalog_secret_with_fallbacks(state.encryption_key(), ciphertext)
        else {
            return Ok((
                http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "detail": "解密密钥失败" })),
            )
                .into_response());
        };
        return Ok(attach_admin_audit_response(
            Json(json!({ "key": key })).into_response(),
            "admin_standalone_api_key_revealed",
            "reveal_standalone_api_key",
            "api_key",
            &api_key_id,
        ));
    }

    let wallet = state
        .list_wallet_snapshots_by_api_key_ids(std::slice::from_ref(&api_key_id))
        .await?
        .into_iter()
        .find(|wallet| wallet.api_key_id.as_deref() == Some(api_key_id.as_str()));

    Ok(Json(build_admin_api_key_detail_payload(
        state,
        &record,
        wallet.as_ref(),
    ))
    .into_response())
}
