use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::shared::paths::{
    admin_clear_oauth_invalid_key_id, admin_provider_id_for_keys, admin_update_key_id,
};
use crate::handlers::admin::provider::shared::payloads::{
    AdminProviderKeyBatchDeleteRequest, AdminProviderKeyCreateRequest,
    AdminProviderKeyUpdateRequest,
};
use crate::handlers::admin::shared::build_admin_provider_key_response;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use super::super::write::keys::{
    build_admin_create_provider_key_record, build_admin_update_provider_key_record,
};

pub(super) async fn maybe_handle(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() == Some("endpoints_manage")
        && decision.route_kind.as_deref() == Some("update_key")
        && request_context.request_method == http::Method::PUT
        && request_context
            .request_path
            .starts_with("/api/admin/endpoints/keys/")
    {
        let Some(key_id) = admin_update_key_id(&request_context.request_path) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Key 不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体不能为空" })),
                )
                    .into_response(),
            ));
        };
        if !state.has_provider_catalog_data_reader() {
            return Ok(None);
        }
        let raw_value = match serde_json::from_slice::<serde_json::Value>(request_body) {
            Ok(value) => value,
            Err(_) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                    )
                        .into_response(),
                ));
            }
        };
        let Some(raw_payload) = raw_value.as_object().cloned() else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                )
                    .into_response(),
            ));
        };
        let payload = match serde_json::from_value::<AdminProviderKeyUpdateRequest>(raw_value) {
            Ok(payload) => payload,
            Err(_) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                    )
                        .into_response(),
                ));
            }
        };
        let Some(existing_key) = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Key {key_id} 不存在") })),
                )
                    .into_response(),
            ));
        };
        let Some(provider) = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&existing_key.provider_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Provider {} 不存在", existing_key.provider_id) })),
                )
                    .into_response(),
            ));
        };
        let updated_record = match build_admin_update_provider_key_record(
            state,
            &provider,
            &existing_key,
            &raw_payload,
            payload,
        )
        .await
        {
            Ok(record) => record,
            Err(detail) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": detail })),
                    )
                        .into_response(),
                ));
            }
        };
        let Some(updated) = state.update_provider_catalog_key(&updated_record).await? else {
            return Ok(None);
        };
        let now_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        return Ok(Some(
            Json(build_admin_provider_key_response(
                state,
                &updated,
                now_unix_secs,
            ))
            .into_response(),
        ));
    }

    if decision.route_family.as_deref() == Some("endpoints_manage")
        && decision.route_kind.as_deref() == Some("delete_key")
        && request_context.request_method == http::Method::DELETE
        && request_context
            .request_path
            .starts_with("/api/admin/endpoints/keys/")
    {
        let Some(key_id) = admin_update_key_id(&request_context.request_path) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Key 不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(_existing_key) = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Key {key_id} 不存在") })),
                )
                    .into_response(),
            ));
        };
        if !state.delete_provider_catalog_key(&key_id).await? {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Key {key_id} 不存在") })),
                )
                    .into_response(),
            ));
        }
        return Ok(Some(
            Json(json!({
                "message": format!("Key {key_id} 已删除")
            }))
            .into_response(),
        ));
    }

    if decision.route_family.as_deref() == Some("endpoints_manage")
        && decision.route_kind.as_deref() == Some("batch_delete_keys")
        && request_context.request_method == http::Method::POST
        && request_context.request_path == "/api/admin/endpoints/keys/batch-delete"
    {
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体不能为空" })),
                )
                    .into_response(),
            ));
        };
        let payload =
            match serde_json::from_slice::<AdminProviderKeyBatchDeleteRequest>(request_body) {
                Ok(payload) => payload,
                Err(_) => {
                    return Ok(Some(
                        (
                            http::StatusCode::BAD_REQUEST,
                            Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                        )
                            .into_response(),
                    ));
                }
            };
        if payload.ids.len() > 100 {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "ids 最多 100 个" })),
                )
                    .into_response(),
            ));
        }
        if payload.ids.is_empty() {
            return Ok(Some(
                Json(json!({
                    "success_count": 0,
                    "failed_count": 0,
                    "failed": []
                }))
                .into_response(),
            ));
        }

        let found_keys = state
            .read_provider_catalog_keys_by_ids(&payload.ids)
            .await?;
        let found_ids = found_keys
            .iter()
            .map(|key| key.id.clone())
            .collect::<BTreeSet<_>>();
        let mut failed = payload
            .ids
            .iter()
            .filter(|key_id| !found_ids.contains(*key_id))
            .map(|key_id| json!({ "id": key_id, "error": "not found" }))
            .collect::<Vec<_>>();

        let mut success_count = 0usize;
        for key_id in found_ids {
            if state.delete_provider_catalog_key(&key_id).await? {
                success_count += 1;
            } else {
                failed.push(json!({ "id": key_id, "error": "not found" }));
            }
        }

        return Ok(Some(
            Json(json!({
                "success_count": success_count,
                "failed_count": failed.len(),
                "failed": failed,
            }))
            .into_response(),
        ));
    }

    if decision.route_family.as_deref() == Some("endpoints_manage")
        && decision.route_kind.as_deref() == Some("clear_oauth_invalid")
        && request_context.request_method == http::Method::POST
        && request_context
            .request_path
            .starts_with("/api/admin/endpoints/keys/")
        && request_context
            .request_path
            .ends_with("/clear-oauth-invalid")
    {
        let Some(key_id) = admin_clear_oauth_invalid_key_id(&request_context.request_path) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Key 不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(key) = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Key {key_id} 不存在") })),
                )
                    .into_response(),
            ));
        };
        if key.oauth_invalid_at_unix_secs.is_none() {
            return Ok(Some(
                Json(json!({
                    "message": "该 Key 当前无失效标记，无需清除"
                }))
                .into_response(),
            ));
        }
        state
            .clear_provider_catalog_key_oauth_invalid_marker(&key_id)
            .await?;
        return Ok(Some(
            Json(json!({
                "message": "已清除 OAuth 失效标记"
            }))
            .into_response(),
        ));
    }

    if decision.route_family.as_deref() == Some("endpoints_manage")
        && decision.route_kind.as_deref() == Some("create_provider_key")
        && request_context.request_method == http::Method::POST
        && request_context
            .request_path
            .starts_with("/api/admin/endpoints/providers/")
        && request_context.request_path.ends_with("/keys")
    {
        let Some(provider_id) = admin_provider_id_for_keys(&request_context.request_path) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体不能为空" })),
                )
                    .into_response(),
            ));
        };
        if !state.has_provider_catalog_data_reader() {
            return Ok(None);
        }
        let payload = match serde_json::from_slice::<AdminProviderKeyCreateRequest>(request_body) {
            Ok(payload) => payload,
            Err(_) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                    )
                        .into_response(),
                ));
            }
        };
        let Some(provider) = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
                )
                    .into_response(),
            ));
        };
        let record = match build_admin_create_provider_key_record(state, &provider, payload).await {
            Ok(record) => record,
            Err(detail) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": detail })),
                    )
                        .into_response(),
                ));
            }
        };
        let Some(created) = state.create_provider_catalog_key(&record).await? else {
            return Ok(None);
        };
        let now_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        return Ok(Some(
            Json(build_admin_provider_key_response(
                state,
                &created,
                now_unix_secs,
            ))
            .into_response(),
        ));
    }

    Ok(None)
}
