use super::super::super::build_admin_users_bad_request_response;
use super::super::helpers::attach_audit_response;
use super::super::paths::admin_user_api_key_full_key_parts;

use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(crate) async fn build_admin_reveal_user_api_key_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some((user_id, key_id)) = admin_user_api_key_full_key_parts(request_context.path()) else {
        return Ok(build_admin_users_bad_request_response(
            "缺少 user_id 或 key_id",
        ));
    };

    let records = state
        .list_auth_api_key_export_records_by_user_ids(std::slice::from_ref(&user_id))
        .await?;
    let Some(record) = records
        .into_iter()
        .find(|record| record.api_key_id == key_id)
    else {
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": "API Key不存在或不属于该用户" })),
        )
            .into_response());
    };

    let Some(ciphertext) = record.key_encrypted.as_deref().map(str::trim) else {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "该密钥没有存储完整密钥信息" })),
        )
            .into_response());
    };
    if ciphertext.is_empty() {
        return Ok((
            http::StatusCode::BAD_REQUEST,
            Json(json!({ "detail": "该密钥没有存储完整密钥信息" })),
        )
            .into_response());
    }

    let Some(full_key) = state.decrypt_catalog_secret_with_fallbacks(ciphertext) else {
        return Ok((
            http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "detail": "解密密钥失败" })),
        )
            .into_response());
    };

    Ok(attach_audit_response(
        Json(json!({ "key": full_key })).into_response(),
        "admin_user_api_key_revealed",
        "reveal_user_api_key",
        "user_api_key",
        &key_id,
    ))
}
