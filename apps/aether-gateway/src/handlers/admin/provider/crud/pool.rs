use crate::handlers::admin::provider::shared::paths::{
    admin_provider_clear_pool_cooldown_parts, admin_provider_reset_pool_cost_parts,
};
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

enum AdminProviderPoolKeyLookup {
    ProviderMissing,
    KeyMissing,
    KeyName(String),
}

fn build_admin_provider_not_found_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

async fn lookup_admin_provider_pool_key_name(
    state: &AdminAppState<'_>,
    provider_id: &str,
    key_id: &str,
) -> Result<AdminProviderPoolKeyLookup, GatewayError> {
    let provider_id_owned = provider_id.to_string();
    let provider_exists = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id_owned))
        .await?
        .into_iter()
        .next()
        .is_some();
    if !provider_exists {
        return Ok(AdminProviderPoolKeyLookup::ProviderMissing);
    }

    let key = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider_id_owned))
        .await?
        .into_iter()
        .find(|key| key.id == key_id)
        .map(|key| key.name);

    Ok(match key {
        Some(name) => AdminProviderPoolKeyLookup::KeyName(name),
        None => AdminProviderPoolKeyLookup::KeyMissing,
    })
}

pub(crate) async fn maybe_build_local_admin_provider_pool_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    route_kind: Option<&str>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if route_kind == Some("clear_pool_cooldown") && request_context.method() == http::Method::POST {
        let Some((provider_id, key_id)) =
            admin_provider_clear_pool_cooldown_parts(request_context.path())
        else {
            return Ok(Some(build_admin_provider_not_found_response("Key 不存在")));
        };
        match lookup_admin_provider_pool_key_name(state, &provider_id, &key_id).await? {
            AdminProviderPoolKeyLookup::ProviderMissing => {
                return Ok(Some(build_admin_provider_not_found_response(format!(
                    "Provider {provider_id} 不存在"
                ))));
            }
            AdminProviderPoolKeyLookup::KeyMissing => {
                return Ok(Some(build_admin_provider_not_found_response(format!(
                    "Key {key_id} 不存在"
                ))));
            }
            AdminProviderPoolKeyLookup::KeyName(key_name) => {
                state
                    .clear_admin_provider_pool_cooldown(&provider_id, &key_id)
                    .await;
                return Ok(Some(attach_admin_audit_response(
                    Json(json!({
                        "message": format!("已清除 Key {} 的冷却状态", key_name),
                    }))
                    .into_response(),
                    "admin_provider_pool_cooldown_cleared",
                    "clear_provider_pool_cooldown",
                    "provider_key",
                    &key_id,
                )));
            }
        }
    }

    if route_kind == Some("reset_pool_cost") && request_context.method() == http::Method::POST {
        let Some((provider_id, key_id)) =
            admin_provider_reset_pool_cost_parts(request_context.path())
        else {
            return Ok(Some(build_admin_provider_not_found_response("Key 不存在")));
        };
        match lookup_admin_provider_pool_key_name(state, &provider_id, &key_id).await? {
            AdminProviderPoolKeyLookup::ProviderMissing => {
                return Ok(Some(build_admin_provider_not_found_response(format!(
                    "Provider {provider_id} 不存在"
                ))));
            }
            AdminProviderPoolKeyLookup::KeyMissing => {
                return Ok(Some(build_admin_provider_not_found_response(format!(
                    "Key {key_id} 不存在"
                ))));
            }
            AdminProviderPoolKeyLookup::KeyName(key_name) => {
                state
                    .reset_admin_provider_pool_cost(&provider_id, &key_id)
                    .await;
                return Ok(Some(attach_admin_audit_response(
                    Json(json!({
                        "message": format!("已重置 Key {} 的成本窗口", key_name),
                    }))
                    .into_response(),
                    "admin_provider_pool_cost_reset",
                    "reset_provider_pool_cost",
                    "provider_key",
                    &key_id,
                )));
            }
        }
    }

    Ok(None)
}
