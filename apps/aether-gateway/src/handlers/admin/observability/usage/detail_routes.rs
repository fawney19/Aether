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
use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UsageBodyField};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio::try_join;

struct AdminUsageDetailBodyValue {
    value: Option<Value>,
    load_failed: bool,
}

async fn resolve_admin_usage_detail_request_body(
    state: &AdminAppState<'_>,
    item: &StoredRequestUsageAudit,
) -> AdminUsageDetailBodyValue {
    match admin_usage_resolve_request_capture_body_for_item(state, item, None).await {
        Ok(body) => AdminUsageDetailBodyValue {
            value: body,
            load_failed: false,
        },
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
                load_failed: value.is_none(),
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
        Ok(body) => AdminUsageDetailBodyValue {
            value: body,
            load_failed: false,
        },
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
                load_failed: value.is_none(),
                value,
            }
        }
    }
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

            let Some(item) = state.find_request_usage_by_id(&usage_id).await? else {
                return Ok(Some(
                    (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({ "detail": "Usage record not found" })),
                    )
                        .into_response(),
                ));
            };

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
            let request_body = if include_bodies {
                let (request_body, provider_request_body, response_body, client_response_body) = tokio::join!(
                    resolve_admin_usage_detail_request_body(state, &item),
                    resolve_admin_usage_detail_body_value(
                        state,
                        &item,
                        UsageBodyField::ProviderRequestBody,
                    ),
                    resolve_admin_usage_detail_body_value(
                        state,
                        &item,
                        UsageBodyField::ResponseBody,
                    ),
                    resolve_admin_usage_detail_body_value(
                        state,
                        &item,
                        UsageBodyField::ClientResponseBody,
                    ),
                );
                for (field, resolved) in [
                    (UsageBodyField::RequestBody, &request_body),
                    (UsageBodyField::ProviderRequestBody, &provider_request_body),
                    (UsageBodyField::ResponseBody, &response_body),
                    (UsageBodyField::ClientResponseBody, &client_response_body),
                ] {
                    if resolved.load_failed {
                        body_load_errors.insert(field.as_storage_field().to_string(), json!(true));
                    }
                }
                detail_item.provider_request_body = provider_request_body.value;
                detail_item.response_body = response_body.value;
                detail_item.client_response_body = client_response_body.value;
                request_body.value
            } else {
                None
            };
            if include_bodies {
                // request_body 已通过 request capture 解析；其余 detached body 在上方并行加载。
            }
            let default_headers = admin_usage_curl_headers();
            let mut payload = build_admin_usage_detail_payload(
                &detail_item,
                &users_by_id,
                &api_key_names,
                state.has_auth_user_data_reader(),
                state.has_auth_api_key_data_reader(),
                provider_key_name.as_deref(),
                include_bodies,
                request_body,
                &default_headers,
            );
            payload["body_load_errors"] = if include_bodies && !body_load_errors.is_empty() {
                Value::Object(body_load_errors)
            } else {
                Value::Null
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
