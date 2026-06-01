use super::{
    admin_pool_provider_id_from_path, build_admin_pool_error_response,
    resolve_admin_pool_selection_snapshot_key_ids, AdminPoolBatchActionRequest,
    AdminPoolSelectionSnapshotReference, ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
    ADMIN_POOL_PROVIDER_CATALOG_WRITER_UNAVAILABLE_DETAIL,
    ADMIN_POOL_SELECTION_SNAPSHOT_ITEM_PAGE_SIZE,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_admin::provider::pool::{
    self as admin_provider_pool_pure, AdminPoolBatchSelectionRequest,
};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};

fn admin_pool_batch_snapshot_reference(
    payload: &AdminPoolBatchActionRequest,
) -> Result<Option<AdminPoolSelectionSnapshotReference>, Response<Body>> {
    let Some(selection) = payload.selection.as_ref() else {
        return Ok(None);
    };
    if !payload.key_ids.is_empty() {
        return Err(build_admin_pool_error_response(
            http::StatusCode::BAD_REQUEST,
            "selection and key_ids cannot be used together",
        ));
    }

    match selection {
        AdminPoolBatchSelectionRequest::Snapshot {
            snapshot_id,
            expected_total,
        } => {
            let snapshot_id = snapshot_id.trim();
            if snapshot_id.is_empty() {
                return Err(build_admin_pool_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "selection.snapshot_id should not be empty",
                ));
            }
            let Some(expected_total) = expected_total else {
                return Err(build_admin_pool_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "selection.expected_total is required",
                ));
            };
            Ok(Some(AdminPoolSelectionSnapshotReference {
                snapshot_id: snapshot_id.to_string(),
                expected_total: *expected_total,
            }))
        }
    }
}

async fn build_admin_pool_snapshot_batch_action_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    provider_id: &str,
    payload: AdminPoolBatchActionRequest,
    reference: AdminPoolSelectionSnapshotReference,
) -> Result<Response<Body>, GatewayError> {
    let key_ids = match resolve_admin_pool_selection_snapshot_key_ids(
        state,
        request_context,
        provider_id,
        &reference,
    )
    .await?
    {
        Ok(key_ids) => key_ids,
        Err(response) => return Ok(response),
    };

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id.to_string()))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::NOT_FOUND,
            format!("Provider {provider_id} 不存在"),
        ));
    };

    let mut affected = 0usize;
    let mut action_label = None;
    for chunk in key_ids.chunks(ADMIN_POOL_SELECTION_SNAPSHOT_ITEM_PAGE_SIZE) {
        let chunk_payload = AdminPoolBatchActionRequest {
            key_ids: chunk.to_vec(),
            selection: None,
            action: payload.action.clone(),
            payload: payload.payload.clone(),
        };
        let plan = match admin_provider_pool_pure::build_admin_pool_batch_action_plan(chunk_payload)
        {
            Ok(plan) => plan,
            Err(detail) => {
                return Ok(build_admin_pool_error_response(
                    http::StatusCode::BAD_REQUEST,
                    detail,
                ));
            }
        };
        action_label.get_or_insert(plan.action_label);
        affected = affected.saturating_add(
            state
                .execute_admin_pool_batch_action_plan(&provider, plan)
                .await?,
        );
    }

    let action_label = action_label.unwrap_or("processed");
    Ok(Json(
        admin_provider_pool_pure::build_admin_pool_batch_action_result_payload(
            affected,
            action_label,
        ),
    )
    .into_response())
}

pub(super) async fn build_admin_pool_batch_action_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
        ));
    }
    if !state.has_provider_catalog_data_writer() {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            ADMIN_POOL_PROVIDER_CATALOG_WRITER_UNAVAILABLE_DETAIL,
        ));
    }

    let Some(provider_id) = admin_pool_provider_id_from_path(request_context.path()) else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let payload = match request_body {
        Some(body) if !body.is_empty() => {
            match serde_json::from_slice::<AdminPoolBatchActionRequest>(body) {
                Ok(value) => value,
                Err(_) => {
                    return Ok(build_admin_pool_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "Invalid JSON request body",
                    ));
                }
            }
        }
        _ => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                "Invalid JSON request body",
            ));
        }
    };
    let reference = match admin_pool_batch_snapshot_reference(&payload) {
        Ok(reference) => reference,
        Err(response) => return Ok(response),
    };
    if let Some(reference) = reference {
        return build_admin_pool_snapshot_batch_action_response(
            state,
            request_context,
            &provider_id,
            payload,
            reference,
        )
        .await;
    }

    state
        .build_admin_pool_batch_action_response(&provider_id, payload)
        .await
}
