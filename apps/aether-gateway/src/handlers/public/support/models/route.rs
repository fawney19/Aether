use std::time::{SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow;
use axum::{body::Body, response::Response};

use super::models_responses::{
    build_claude_model_detail_response, build_claude_models_list_response,
    build_empty_models_list_response, build_gemini_model_detail_response,
    build_gemini_models_list_response, build_models_auth_error_response,
    build_models_not_found_response, build_openai_model_detail_response,
    build_openai_models_list_response,
};
use super::models_shared::{
    filter_rows_for_models, models_api_format, models_detail_id, models_query_api_formats,
};
use super::{query_param_value, AppState, GatewayPublicRequestContext};

fn sort_and_dedup_model_rows(
    mut rows: Vec<StoredMinimalCandidateSelectionRow>,
) -> Vec<StoredMinimalCandidateSelectionRow> {
    rows.sort_by(|left, right| {
        left.global_model_name
            .cmp(&right.global_model_name)
            .then(left.provider_priority.cmp(&right.provider_priority))
            .then(left.key_internal_priority.cmp(&right.key_internal_priority))
            .then(left.provider_id.cmp(&right.provider_id))
            .then(left.endpoint_id.cmp(&right.endpoint_id))
            .then(left.key_id.cmp(&right.key_id))
            .then(left.model_id.cmp(&right.model_id))
    });
    let mut deduped = Vec::with_capacity(rows.len());
    let mut last_model_name: Option<String> = None;
    for row in rows {
        if last_model_name.as_deref() == Some(row.global_model_name.as_str()) {
            continue;
        }
        last_model_name = Some(row.global_model_name.clone());
        deduped.push(row);
    }
    deduped
}

async fn list_model_rows_for_client_format(
    state: &AppState,
    api_format: &str,
    auth_snapshot: Option<&crate::data::auth::GatewayAuthApiKeySnapshot>,
) -> Option<Vec<StoredMinimalCandidateSelectionRow>> {
    let mut collected = Vec::new();
    for query_format in models_query_api_formats(api_format) {
        let rows = state
            .list_minimal_candidate_selection_rows_for_api_format(query_format)
            .await
            .ok()?;
        let mut filtered = filter_rows_for_models(rows, auth_snapshot, query_format);
        collected.append(&mut filtered);
    }
    Some(sort_and_dedup_model_rows(collected))
}

async fn list_model_rows_for_client_format_and_global_model(
    state: &AppState,
    api_format: &str,
    global_model_name: &str,
    auth_snapshot: Option<&crate::data::auth::GatewayAuthApiKeySnapshot>,
) -> Option<Vec<StoredMinimalCandidateSelectionRow>> {
    let mut collected = Vec::new();
    for query_format in models_query_api_formats(api_format) {
        let rows = state
            .list_minimal_candidate_selection_rows_for_api_format_and_global_model(
                query_format,
                global_model_name,
            )
            .await
            .ok()?;
        let mut filtered = filter_rows_for_models(rows, auth_snapshot, query_format);
        collected.append(&mut filtered);
    }
    Some(sort_and_dedup_model_rows(collected))
}

pub(super) async fn maybe_build_local_models_route_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("models") {
        return None;
    }
    let api_format = models_api_format(request_context)?;
    if !state.has_minimal_candidate_selection_reader() {
        return None;
    }

    let auth_context = decision.auth_context.as_ref()?;
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let auth_snapshot = state
        .data
        .read_auth_api_key_snapshot(
            &auth_context.user_id,
            &auth_context.api_key_id,
            now_unix_secs,
        )
        .await
        .ok()
        .flatten();
    let auth_snapshot = auth_snapshot.as_ref();

    match decision.route_kind.as_deref() {
        Some("list") => {
            let rows = list_model_rows_for_client_format(state, api_format, auth_snapshot).await?;
            if rows.is_empty() {
                return Some(build_empty_models_list_response(api_format));
            }
            let response = match api_format {
                "claude:chat" => {
                    let before_id = query_param_value(
                        request_context.request_query_string.as_deref(),
                        "before_id",
                    );
                    let after_id = query_param_value(
                        request_context.request_query_string.as_deref(),
                        "after_id",
                    );
                    let limit =
                        query_param_value(request_context.request_query_string.as_deref(), "limit")
                            .and_then(|value| value.parse::<usize>().ok())
                            .filter(|value| *value > 0)
                            .unwrap_or(20);
                    build_claude_models_list_response(
                        &rows,
                        before_id.as_deref(),
                        after_id.as_deref(),
                        limit,
                    )
                }
                "gemini:chat" => {
                    let page_size = query_param_value(
                        request_context.request_query_string.as_deref(),
                        "pageSize",
                    )
                    .and_then(|value| value.parse::<usize>().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(50);
                    let page_token = query_param_value(
                        request_context.request_query_string.as_deref(),
                        "pageToken",
                    );
                    build_gemini_models_list_response(&rows, page_size, page_token.as_deref())
                }
                _ => build_openai_models_list_response(&rows),
            };
            Some(response)
        }
        Some("detail") => {
            let model_id = models_detail_id(&request_context.request_path)?;
            let rows = list_model_rows_for_client_format_and_global_model(
                state,
                api_format,
                &model_id,
                auth_snapshot,
            )
            .await?;
            let Some(row) = rows.first() else {
                return Some(build_models_not_found_response(&model_id, api_format));
            };
            let response = match api_format {
                "claude:chat" => build_claude_model_detail_response(row),
                "gemini:chat" => build_gemini_model_detail_response(row),
                _ => build_openai_model_detail_response(row),
            };
            Some(response)
        }
        _ => Some(build_models_auth_error_response(api_format)),
    }
}
