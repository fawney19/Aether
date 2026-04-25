use super::responses::build_admin_providers_data_unavailable_response;
use crate::handlers::admin::provider::shared::paths::{
    admin_provider_id_for_manage_path, is_admin_providers_root,
};
use crate::handlers::admin::provider::shared::payloads::{
    AdminProviderCreateRequest, AdminProviderUpdatePatch,
};
use crate::handlers::admin::provider::write::provider::{
    reconcile_admin_fixed_provider_template_endpoints, reconcile_admin_fixed_provider_template_keys,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

fn build_admin_provider_bad_request_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

fn build_admin_provider_not_found_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

pub(crate) async fn maybe_build_local_admin_provider_writes_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
    route_kind: Option<&str>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if route_kind == Some("create_provider")
        && request_context.method() == http::Method::POST
        && is_admin_providers_root(request_context.path())
    {
        let Some(request_body) = request_body else {
            return Ok(Some(build_admin_provider_bad_request_response(
                "请求体不能为空",
            )));
        };
        if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        }
        let payload = match serde_json::from_slice::<AdminProviderCreateRequest>(request_body) {
            Ok(payload) => payload,
            Err(_) => {
                return Ok(Some(build_admin_provider_bad_request_response(
                    "请求体必须是合法的 JSON 对象",
                )));
            }
        };
        let (record, shift_existing_priorities_from) =
            match state.build_admin_create_provider_record(payload).await {
                Ok(record) => record,
                Err(message) => {
                    return Ok(Some(build_admin_provider_bad_request_response(message)));
                }
            };
        let Some(created_provider) = state
            .create_provider_catalog_provider(&record, shift_existing_priorities_from)
            .await?
        else {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        };

        if state
            .fixed_provider_template(&created_provider.provider_type)
            .is_some()
        {
            reconcile_admin_fixed_provider_template_endpoints(state, &created_provider).await?;
            reconcile_admin_fixed_provider_template_keys(state, &created_provider).await?;
        }
        return Ok(Some(attach_admin_audit_response(
            Json(json!({
                "id": created_provider.id,
                "name": created_provider.name,
                "message": "提供商创建成功",
            }))
            .into_response(),
            "admin_provider_created",
            "create_provider",
            "provider",
            &created_provider.id,
        )));
    }

    if route_kind == Some("update_provider")
        && request_context.method() == http::Method::PATCH
        && request_context.path().starts_with("/api/admin/providers/")
    {
        let Some(provider_id) = admin_provider_id_for_manage_path(request_context.path()) else {
            return Ok(Some(build_admin_provider_not_found_response(
                "Provider 不存在",
            )));
        };
        let Some(request_body) = request_body else {
            return Ok(Some(build_admin_provider_bad_request_response(
                "请求体不能为空",
            )));
        };
        if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        }
        let raw_value = match serde_json::from_slice::<serde_json::Value>(request_body) {
            Ok(value) => value,
            Err(_) => {
                return Ok(Some(build_admin_provider_bad_request_response(
                    "请求体必须是合法的 JSON 对象",
                )));
            }
        };
        let Some(raw_payload) = raw_value.as_object().cloned() else {
            return Ok(Some(build_admin_provider_bad_request_response(
                "请求体必须是合法的 JSON 对象",
            )));
        };
        let patch = match AdminProviderUpdatePatch::from_object(raw_payload) {
            Ok(patch) => patch,
            Err(_) => {
                return Ok(Some(build_admin_provider_bad_request_response(
                    "请求体必须是合法的 JSON 对象",
                )));
            }
        };
        let Some(existing_provider) = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(build_admin_provider_not_found_response(format!(
                "Provider {provider_id} 不存在"
            ))));
        };
        let updated_record = match state
            .build_admin_update_provider_record(&existing_provider, patch)
            .await
        {
            Ok(record) => record,
            Err(detail) => return Ok(Some(build_admin_provider_bad_request_response(detail))),
        };
        let Some(_updated) = state
            .update_provider_catalog_provider(&updated_record)
            .await?
        else {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        };
        if state
            .fixed_provider_template(&updated_record.provider_type)
            .is_some()
        {
            reconcile_admin_fixed_provider_template_endpoints(state, &updated_record).await?;
            reconcile_admin_fixed_provider_template_keys(state, &updated_record).await?;
        }
        return Ok(Some(
            match state
                .build_admin_provider_summary_payload(&provider_id)
                .await
            {
                Some(payload) => attach_admin_audit_response(
                    Json(payload).into_response(),
                    "admin_provider_updated",
                    "update_provider",
                    "provider",
                    &provider_id,
                ),
                None => build_admin_provider_not_found_response(format!(
                    "Provider {provider_id} 不存在"
                )),
            },
        ));
    }

    Ok(None)
}
