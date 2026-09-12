use super::analytics::admin_usage_api_key_names;
use super::analytics::admin_usage_provider_key_names;
use super::replay::{
    admin_usage_curl_headers, admin_usage_curl_url, admin_usage_headers_from_value,
    admin_usage_id_from_action_path, admin_usage_id_from_detail_path,
    admin_usage_resolve_body_value, admin_usage_resolve_request_capture_body,
    admin_usage_resolve_request_capture_body_for_item, build_admin_usage_curl_response,
    build_admin_usage_detail_payload, build_admin_usage_replay_response,
};
use super::summary_routes::{
    admin_usage_terminal_candidate_state_override, apply_admin_usage_state_override,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{attach_admin_audit_response, query_param_bool};
use crate::GatewayError;
use aether_admin::observability::usage::{
    admin_usage_bad_request_response, admin_usage_data_unavailable_response,
    admin_usage_provider_key_name, ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
};
use aether_data_contracts::repository::usage::{
    canonical_usage_body_ref_for, StoredRequestUsageAudit, StoredUsageBodyPayload,
    UsageBodyCaptureState, UsageBodyField, MAX_DECOMPRESSED_USAGE_JSON_BYTES,
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio::try_join;

#[derive(Default)]
struct AdminUsageDetailBodyValue {
    value: Option<Value>,
    error_code: Option<&'static str>,
}

impl AdminUsageDetailBodyValue {
    fn resolved(
        item: &StoredRequestUsageAudit,
        field: UsageBodyField,
        value: Option<Value>,
    ) -> Self {
        let missing = value.is_none()
            && item
                .body_capture_result(field, item.body_value(field))
                .available;
        Self {
            value,
            error_code: missing.then_some("missing"),
        }
    }
}

fn admin_usage_body_load_error_code(error: &GatewayError) -> &'static str {
    if let GatewayError::Internal(message) = error {
        if message.contains("decompressed usage json exceeds ")
            || message.contains("encoded usage json exceeds ")
        {
            return "too_large";
        }
        if message.contains("failed to decompress usage json:")
            || message.contains("failed to parse decompressed usage json:")
        {
            return "decode_failed";
        }
    }
    "storage_unavailable"
}

async fn resolve_admin_usage_detail_field(
    state: &AdminAppState<'_>,
    item: &StoredRequestUsageAudit,
    field: UsageBodyField,
    selected_field: Option<UsageBodyField>,
) -> AdminUsageDetailBodyValue {
    if selected_field.is_some_and(|selected| selected != field) {
        return AdminUsageDetailBodyValue::default();
    }
    if field == UsageBodyField::RequestBody {
        resolve_admin_usage_detail_request_body(state, item).await
    } else {
        resolve_admin_usage_detail_body_value(state, item, field).await
    }
}

async fn resolve_admin_usage_detail_request_body(
    state: &AdminAppState<'_>,
    item: &StoredRequestUsageAudit,
) -> AdminUsageDetailBodyValue {
    match admin_usage_resolve_request_capture_body_for_item(state, item, None).await {
        Ok(body) => AdminUsageDetailBodyValue::resolved(item, UsageBodyField::RequestBody, body),
        Err(err) => {
            tracing::warn!(
                error = ?err,
                usage_id = %item.id,
                request_id = %item.request_id,
                field = UsageBodyField::RequestBody.as_storage_field(),
                "failed to resolve admin usage detail body"
            );
            let value = admin_usage_resolve_request_capture_body(item, None);
            AdminUsageDetailBodyValue {
                error_code: value
                    .is_none()
                    .then(|| admin_usage_body_load_error_code(&err)),
                value,
            }
        }
    }
}

async fn resolve_admin_usage_detail_body_value(
    state: &AdminAppState<'_>,
    item: &StoredRequestUsageAudit,
    field: UsageBodyField,
) -> AdminUsageDetailBodyValue {
    let inline_body = item.body_value(field);
    match admin_usage_resolve_body_value(state, item, inline_body, field).await {
        Ok(body) => AdminUsageDetailBodyValue::resolved(item, field, body),
        Err(err) => {
            tracing::warn!(
                error = ?err,
                usage_id = %item.id,
                request_id = %item.request_id,
                field = field.as_storage_field(),
                "failed to resolve admin usage detail body"
            );
            let value = inline_body.cloned();
            AdminUsageDetailBodyValue {
                error_code: value
                    .is_none()
                    .then(|| admin_usage_body_load_error_code(&err)),
                value,
            }
        }
    }
}

async fn read_admin_usage_raw_body(
    state: &AdminAppState<'_>,
    item: &StoredRequestUsageAudit,
    field: UsageBodyField,
) -> Result<Option<StoredUsageBodyPayload>, GatewayError> {
    if matches!(
        item.body_state(field),
        Some(
            UsageBodyCaptureState::Disabled
                | UsageBodyCaptureState::Unavailable
                | UsageBodyCaptureState::None
        )
    ) {
        return Ok(None);
    }
    let inline_body = item.body_value(field);
    let prefer_inline = matches!(
        item.body_state(field),
        Some(UsageBodyCaptureState::Inline | UsageBodyCaptureState::Truncated)
    ) && inline_body.is_some();
    if !prefer_inline {
        if let Some(body_ref) = item
            .body_ref(field)
            .and_then(|reference| canonical_usage_body_ref_for(reference, &item.request_id, field))
        {
            if let Some(payload) = state.read_request_usage_body_payload(&body_ref).await? {
                return Ok(Some(payload));
            }
        }
    }
    let fallback = inline_body.cloned().or_else(|| {
        (field == UsageBodyField::RequestBody)
            .then(|| admin_usage_resolve_request_capture_body(item, None))
            .flatten()
    });
    fallback
        .map(|value| {
            serde_json::to_vec(&value)
                .map(StoredUsageBodyPayload::Json)
                .map_err(|error| GatewayError::Internal(error.to_string()))
        })
        .transpose()
}

async fn build_admin_usage_raw_body_response(
    state: &AdminAppState<'_>,
    item: &StoredRequestUsageAudit,
    field: UsageBodyField,
) -> Response<Body> {
    let result = read_admin_usage_raw_body(state, item, field).await;
    let mut response = match result {
        Ok(Some(payload)) => admin_usage_raw_payload_response(payload),
        Ok(None) => admin_usage_raw_body_error(http::StatusCode::NOT_FOUND, "missing"),
        Err(error) => {
            tracing::warn!(error = ?error, usage_id = %item.id, field = field.as_storage_field(), "failed to read admin usage raw body");
            let code = admin_usage_body_load_error_code(&error);
            admin_usage_raw_body_error(
                if code == "too_large" {
                    http::StatusCode::PAYLOAD_TOO_LARGE
                } else {
                    http::StatusCode::SERVICE_UNAVAILABLE
                },
                code,
            )
        }
    };
    let headers = response.headers_mut();
    headers.insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store, no-transform"),
    );
    headers.insert(
        "x-content-type-options",
        http::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "x-aether-body-field",
        http::HeaderValue::from_static(field.as_storage_field()),
    );
    if let Ok(value) = http::HeaderValue::from_str(&item.id) {
        headers.insert("x-aether-usage-id", value);
    }
    attach_admin_audit_response(
        response,
        "admin_usage_detail_viewed",
        "view_usage_detail",
        "usage_record",
        &item.id,
    )
}

fn admin_usage_raw_payload_response(payload: StoredUsageBodyPayload) -> Response<Body> {
    let (encoding, bytes, limit) = match payload {
        StoredUsageBodyPayload::Gzip(bytes) => (
            "gzip",
            bytes,
            MAX_DECOMPRESSED_USAGE_JSON_BYTES + 1024 * 1024,
        ),
        StoredUsageBodyPayload::Json(bytes) => ("json", bytes, MAX_DECOMPRESSED_USAGE_JSON_BYTES),
    };
    if bytes.len() > limit {
        admin_usage_raw_body_error(http::StatusCode::PAYLOAD_TOO_LARGE, "too_large")
    } else {
        (
            [
                ("content-type", "application/octet-stream"),
                ("content-encoding", "identity"),
                ("x-aether-body-encoding", encoding),
            ],
            bytes,
        )
            .into_response()
    }
}

fn admin_usage_raw_body_error(status: http::StatusCode, code: &'static str) -> Response<Body> {
    (
        status,
        [("x-aether-body-error", code)],
        Json(json!({ "body_load_error_code": code })),
    )
        .into_response()
}

pub(super) async fn maybe_build_local_admin_usage_detail_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let route_kind = request_context
        .control_decision
        .as_ref()
        .and_then(|decision| decision.route_kind.as_deref());

    match route_kind {
        Some("curl")
            if request_context.request_method == http::Method::GET
                && request_context
                    .request_path
                    .starts_with("/api/admin/usage/")
                && request_context.request_path.ends_with("/curl") =>
        {
            if !state.has_usage_data_reader() {
                return Ok(Some(admin_usage_data_unavailable_response(
                    ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
                )));
            }

            let Some(usage_id) =
                admin_usage_id_from_action_path(&request_context.request_path, "/curl")
            else {
                return Ok(Some(admin_usage_bad_request_response("usage_id 无效")));
            };

            let Some(item) = state.find_request_usage_by_id(&usage_id).await? else {
                return Ok(Some(
                    (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({ "detail": "Usage record not found" })),
                    )
                        .into_response(),
                ));
            };

            let endpoint = if let Some(endpoint_id) = item.provider_endpoint_id.as_ref() {
                state
                    .read_provider_catalog_endpoints_by_ids(std::slice::from_ref(endpoint_id))
                    .await?
                    .into_iter()
                    .next()
            } else {
                None
            };
            let url = endpoint
                .as_ref()
                .map(|endpoint| admin_usage_curl_url(state, endpoint, &item));
            let headers_json = item
                .provider_request_headers
                .clone()
                .or_else(|| item.request_headers.clone());
            let headers = headers_json
                .as_ref()
                .and_then(admin_usage_headers_from_value)
                .filter(|headers| !headers.is_empty())
                .unwrap_or_else(admin_usage_curl_headers);
            let provider_request_body = admin_usage_resolve_body_value(
                state,
                &item,
                item.provider_request_body.as_ref(),
                UsageBodyField::ProviderRequestBody,
            )
            .await?;
            let request_body = admin_usage_resolve_body_value(
                state,
                &item,
                item.request_body.as_ref(),
                UsageBodyField::RequestBody,
            )
            .await?;
            let body = provider_request_body
                .or(request_body)
                .or_else(|| admin_usage_resolve_request_capture_body(&item, None));
            return Ok(Some(attach_admin_audit_response(
                build_admin_usage_curl_response(&item, url, headers_json, &headers, body.as_ref()),
                "admin_usage_curl_viewed",
                "view_usage_curl_replay",
                "usage_record",
                &item.id,
            )));
        }
        Some("replay") => {
            let mut response =
                build_admin_usage_replay_response(state, request_context, request_body).await?;
            if response.status().is_success() {
                if let Some(usage_id) =
                    admin_usage_id_from_action_path(&request_context.request_path, "/replay")
                {
                    response = attach_admin_audit_response(
                        response,
                        "admin_usage_replay_plan_generated",
                        "generate_usage_replay_plan",
                        "usage_record",
                        &usage_id,
                    );
                }
            }
            return Ok(Some(response));
        }
        Some("detail")
            if request_context.request_method == http::Method::GET
                && request_context
                    .request_path
                    .starts_with("/api/admin/usage/") =>
        {
            if !state.has_usage_data_reader() {
                return Ok(Some(admin_usage_data_unavailable_response(
                    ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
                )));
            }

            let Some(usage_id) = admin_usage_id_from_detail_path(&request_context.request_path)
            else {
                return Ok(Some(admin_usage_bad_request_response("usage_id 无效")));
            };
            let include_bodies = query_param_bool(
                request_context.request_query_string.as_deref(),
                "include_bodies",
                true,
            );
            let body_field =
                match request_context
                    .request_query_string
                    .as_deref()
                    .and_then(|query| {
                        url::form_urlencoded::parse(query.as_bytes())
                            .find(|(key, _)| key == "body_field")
                            .map(|(_, value)| value.into_owned())
                    }) {
                    Some(value) => {
                        match UsageBodyField::from_storage_field(value.trim()) {
                            Some(field) if include_bodies => Some(field),
                            _ => return Ok(Some(admin_usage_bad_request_response(
                                "body_field 必须是有效的正文字段，且 include_bodies 必须为 true",
                            ))),
                        }
                    }
                    None => None,
                };

            let Some(item) = state.find_request_usage_by_id(&usage_id).await? else {
                return Ok(Some(
                    (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({ "detail": "Usage record not found" })),
                    )
                        .into_response(),
                ));
            };

            let body_format = request_context
                .request_query_string
                .as_deref()
                .and_then(|query| {
                    url::form_urlencoded::parse(query.as_bytes())
                        .find(|(key, _)| key == "body_format")
                        .map(|(_, value)| value.into_owned())
                });
            if let Some(format) = body_format {
                if format != "raw" || body_field.is_none() {
                    return Ok(Some(admin_usage_bad_request_response(
                        "body_format=raw 必须指定 body_field",
                    )));
                }
                if let Some(field) = body_field {
                    return Ok(Some(
                        build_admin_usage_raw_body_response(state, &item, field).await,
                    ));
                }
            }

            let user_ids = item.user_id.clone().into_iter().collect::<Vec<_>>();
            let (users_by_id, provider_key_names, api_key_names): (
                BTreeMap<String, aether_data::repository::users::StoredUserSummary>,
                BTreeMap<String, String>,
                BTreeMap<String, String>,
            ) = try_join!(
                state.resolve_auth_user_summaries_by_ids(&user_ids),
                admin_usage_provider_key_names(state, std::slice::from_ref(&item)),
                admin_usage_api_key_names(state, std::slice::from_ref(&item)),
            )?;
            let provider_key_name = admin_usage_provider_key_name(&item, &provider_key_names);

            let mut detail_item = item.clone();
            if matches!(detail_item.status.as_str(), "pending" | "streaming")
                && state.has_request_candidate_data_reader()
            {
                let candidates = state
                    .app()
                    .read_request_candidates_by_request_id(&detail_item.request_id)
                    .await?;
                if let Some(override_payload) =
                    admin_usage_terminal_candidate_state_override(&candidates)
                {
                    apply_admin_usage_state_override(&mut detail_item, &override_payload);
                }
            }
            let mut body_load_errors = serde_json::Map::new();
            let mut body_load_error_codes = serde_json::Map::new();
            let mut request_body = if include_bodies {
                let (request_body, provider_request_body, response_body, client_response_body) = tokio::join!(
                    resolve_admin_usage_detail_field(
                        state,
                        &item,
                        UsageBodyField::RequestBody,
                        body_field
                    ),
                    resolve_admin_usage_detail_field(
                        state,
                        &item,
                        UsageBodyField::ProviderRequestBody,
                        body_field,
                    ),
                    resolve_admin_usage_detail_field(
                        state,
                        &item,
                        UsageBodyField::ResponseBody,
                        body_field,
                    ),
                    resolve_admin_usage_detail_field(
                        state,
                        &item,
                        UsageBodyField::ClientResponseBody,
                        body_field,
                    ),
                );
                for (field, resolved) in [
                    (UsageBodyField::RequestBody, &request_body),
                    (UsageBodyField::ProviderRequestBody, &provider_request_body),
                    (UsageBodyField::ResponseBody, &response_body),
                    (UsageBodyField::ClientResponseBody, &client_response_body),
                ] {
                    if let Some(error_code) = resolved.error_code {
                        body_load_errors.insert(field.as_storage_field().to_string(), json!(true));
                        body_load_error_codes
                            .insert(field.as_storage_field().to_string(), json!(error_code));
                    }
                }
                if body_field.is_none_or(|field| field == UsageBodyField::ProviderRequestBody) {
                    detail_item.provider_request_body = provider_request_body.value;
                }
                if body_field.is_none_or(|field| field == UsageBodyField::ResponseBody) {
                    detail_item.response_body = response_body.value;
                }
                if body_field.is_none_or(|field| field == UsageBodyField::ClientResponseBody) {
                    detail_item.client_response_body = client_response_body.value;
                }
                request_body.value
            } else {
                None
            };
            let default_headers = admin_usage_curl_headers();
            let mut payload = build_admin_usage_detail_payload(
                &detail_item,
                &users_by_id,
                &api_key_names,
                state.has_auth_user_data_reader(),
                state.has_auth_api_key_data_reader(),
                provider_key_name.as_deref(),
                include_bodies && body_field.is_none(),
                if body_field.is_none() {
                    request_body.take()
                } else {
                    None
                },
                &default_headers,
            );
            if let Some(field) = body_field {
                payload[field.as_storage_field()] = match field {
                    UsageBodyField::RequestBody => request_body,
                    UsageBodyField::ProviderRequestBody => detail_item.provider_request_body.take(),
                    UsageBodyField::ResponseBody => detail_item.response_body.take(),
                    UsageBodyField::ClientResponseBody => detail_item.client_response_body.take(),
                }
                .unwrap_or(Value::Null);
            }
            payload["body_load_errors"] = if include_bodies && !body_load_errors.is_empty() {
                Value::Object(body_load_errors)
            } else {
                Value::Null
            };
            payload["body_load_error_codes"] = if body_load_error_codes.is_empty() {
                Value::Null
            } else {
                Value::Object(body_load_error_codes)
            };

            return Ok(Some(attach_admin_audit_response(
                Json(payload).into_response(),
                "admin_usage_detail_viewed",
                "view_usage_detail",
                "usage_record",
                &item.id,
            )));
        }
        _ => {}
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::admin_usage_body_load_error_code;
    use crate::GatewayError;

    #[tokio::test]
    async fn admin_usage_raw_body_does_not_decode_or_reencode_stored_bytes() {
        use super::{admin_usage_raw_payload_response, StoredUsageBodyPayload};
        for (payload, encoding, expected) in [
            (
                StoredUsageBodyPayload::Gzip(vec![31, 139, 8, 0, 1]),
                "gzip",
                vec![31, 139, 8, 0, 1],
            ),
            (
                StoredUsageBodyPayload::Json(b"{ \"untouched\" : true }".to_vec()),
                "json",
                b"{ \"untouched\" : true }".to_vec(),
            ),
        ] {
            let response = admin_usage_raw_payload_response(payload);
            assert_eq!(response.headers()["content-encoding"], "identity");
            assert_eq!(response.headers()["x-aether-body-encoding"], encoding);
            let bytes = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            assert_eq!(bytes.as_ref(), expected.as_slice());
        }
    }

    #[test]
    fn body_load_errors_expose_safe_codes_instead_of_internal_messages() {
        for (message, expected) in [
            (
                "unexpected database value: decompressed usage json exceeds 67108864 bytes",
                "too_large",
            ),
            (
                "failed to decompress usage json: invalid gzip header",
                "decode_failed",
            ),
            (
                "failed to parse decompressed usage json: invalid JSON",
                "decode_failed",
            ),
            (
                "postgres error: private connection details",
                "storage_unavailable",
            ),
        ] {
            assert_eq!(
                admin_usage_body_load_error_code(&GatewayError::Internal(message.to_string())),
                expected
            );
        }
    }
}
