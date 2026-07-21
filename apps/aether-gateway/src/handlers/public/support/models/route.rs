use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow;
use axum::{body::Body, response::Response};
use serde_json::Value;
use tokio::time::timeout;
use tracing::warn;

use super::models_responses::{
    build_claude_model_detail_response, build_claude_models_list_response,
    build_codex_models_list_response, build_empty_models_list_response,
    build_gemini_model_detail_response, build_gemini_models_list_response,
    build_models_auth_error_response, build_models_not_found_response,
    build_openai_model_detail_response, build_openai_models_list_response,
};
use super::models_shared::{
    filter_eligible_model_rows, filter_rows_for_models, models_api_format, models_detail_id,
    models_query_api_formats,
};
use super::{query_param_value, AppState, GatewayPublicRequestContext};

#[cfg(not(test))]
const MODELS_ROUTE_READ_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
const MODELS_ROUTE_READ_TIMEOUT: Duration = Duration::from_millis(50);
const CODEX_MODELS_QUERY_API_FORMATS: &[&str] = &["openai:responses"];

async fn await_models_route_read<T, E, Fut>(operation: &'static str, future: Fut) -> Option<T>
where
    E: Debug,
    Fut: Future<Output = Result<T, E>>,
{
    match timeout(MODELS_ROUTE_READ_TIMEOUT, future).await {
        Ok(Ok(value)) => Some(value),
        Ok(Err(error)) => {
            warn!(
                event_name = "models_route_read_error",
                log_type = "ops",
                operation,
                error = ?error,
                "gateway local models route read failed"
            );
            None
        }
        Err(_) => {
            warn!(
                event_name = "models_route_read_timeout",
                log_type = "ops",
                operation,
                timeout_ms = MODELS_ROUTE_READ_TIMEOUT.as_millis() as u64,
                "gateway local models route read timed out"
            );
            None
        }
    }
}

fn build_models_read_fallback_response(
    request_context: &GatewayPublicRequestContext,
    api_format: &str,
) -> Response<Body> {
    let route_kind = request_context
        .control_decision
        .as_ref()
        .and_then(|decision| decision.route_kind.as_deref());
    match route_kind {
        Some("detail") => {
            let model_id = models_detail_id(&request_context.request_path)
                .unwrap_or_else(|| "unknown".to_string());
            build_models_not_found_response(&model_id, api_format)
        }
        _ => build_empty_models_list_response(api_format),
    }
}

fn sort_model_rows(
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
    rows
}

fn sort_and_dedup_model_rows(
    rows: Vec<StoredMinimalCandidateSelectionRow>,
) -> Vec<StoredMinimalCandidateSelectionRow> {
    let mut deduped = Vec::with_capacity(rows.len());
    let mut last_model_name: Option<String> = None;
    for row in sort_model_rows(rows) {
        if last_model_name.as_deref() == Some(row.global_model_name.as_str()) {
            continue;
        }
        last_model_name = Some(row.global_model_name.clone());
        deduped.push(row);
    }
    deduped
}

fn is_codex_models_api_format(api_format: &str) -> bool {
    crate::ai_serving::normalize_api_format_alias(api_format) == "openai:responses"
}

fn is_codex_provider_row(row: &StoredMinimalCandidateSelectionRow) -> bool {
    row.provider_type.trim().eq_ignore_ascii_case("codex")
}

fn codex_model_card_is_complete(card: &serde_json::Map<String, Value>) -> bool {
    card.get("slug").and_then(Value::as_str).is_some()
        && card.get("display_name").and_then(Value::as_str).is_some()
        && card
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .is_some()
        && card.get("shell_type").and_then(Value::as_str).is_some()
        && card.get("visibility").and_then(Value::as_str).is_some()
        && card
            .get("supported_in_api")
            .and_then(Value::as_bool)
            .is_some()
        && card.get("priority").and_then(Value::as_i64).is_some()
        && card
            .get("base_instructions")
            .and_then(Value::as_str)
            .is_some()
        && card
            .get("supports_reasoning_summary_parameter")
            .is_none_or(Value::is_boolean)
        && card
            .get("support_verbosity")
            .and_then(Value::as_bool)
            .is_some()
        && card
            .get("truncation_policy")
            .and_then(Value::as_object)
            .is_some()
        && card
            .get("supports_parallel_tool_calls")
            .and_then(Value::as_bool)
            .is_some()
        && card
            .get("experimental_supported_tools")
            .and_then(Value::as_array)
            .is_some()
}

fn project_codex_model_card(
    cached_models: &[Value],
    source_model: &str,
    global_model: &str,
) -> Option<Value> {
    let mut card = cached_models
        .iter()
        .find(|model| {
            model.get("id").and_then(Value::as_str) == Some(source_model)
                || model.get("slug").and_then(Value::as_str) == Some(source_model)
        })?
        .as_object()?
        .clone();
    if !codex_model_card_is_complete(&card) {
        return None;
    }

    card.remove("id");
    card.remove("api_formats");
    card.insert("slug".to_string(), Value::String(global_model.to_string()));
    Some(Value::Object(card))
}

async fn load_codex_model_cards(
    state: &AppState,
    rows: &[StoredMinimalCandidateSelectionRow],
) -> Vec<Value> {
    let cache_keys = rows
        .iter()
        .filter(|row| is_codex_provider_row(row))
        .map(|row| format!("upstream_models:{}:{}", row.provider_id, row.key_id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let cached_values = await_models_route_read(
        "codex_models_cache",
        state.runtime_state.kv_get_many(&cache_keys),
    )
    .await
    .unwrap_or_default();
    let cached_models_by_key = cache_keys
        .into_iter()
        .zip(cached_values)
        .filter_map(|(key, raw)| {
            let models = serde_json::from_str::<Vec<Value>>(raw.as_deref()?).ok()?;
            Some((key, models))
        })
        .collect::<BTreeMap<_, _>>();

    let mut seen_global_models = BTreeSet::new();
    let mut cards = Vec::new();
    for row in rows.iter().filter(|row| is_codex_provider_row(row)) {
        if seen_global_models.contains(&row.global_model_name) {
            continue;
        }
        let cache_key = format!("upstream_models:{}:{}", row.provider_id, row.key_id);
        let Some(cached_models) = cached_models_by_key.get(&cache_key) else {
            continue;
        };
        let source_model =
            aether_scheduler_core::select_provider_model_name(row, "openai:responses");
        let Some(card) = project_codex_model_card(
            cached_models,
            source_model.as_str(),
            row.global_model_name.as_str(),
        ) else {
            continue;
        };
        seen_global_models.insert(row.global_model_name.clone());
        cards.push(card);
    }
    cards
}

async fn list_model_rows_for_client_format(
    state: &AppState,
    api_format: &str,
    auth_snapshot: Option<&crate::data::auth::GatewayAuthApiKeySnapshot>,
) -> Option<Vec<StoredMinimalCandidateSelectionRow>> {
    let mut collected = Vec::new();
    let query_api_formats = if is_codex_models_api_format(api_format) {
        CODEX_MODELS_QUERY_API_FORMATS
    } else {
        models_query_api_formats(api_format)
    };
    for query_format in query_api_formats {
        let rows = await_models_route_read(
            "candidate_selection_by_api_format",
            state.list_minimal_candidate_selection_rows_for_api_format(query_format),
        )
        .await?;
        let mut filtered = if is_codex_models_api_format(api_format) {
            filter_eligible_model_rows(rows, auth_snapshot, query_format)
        } else {
            filter_rows_for_models(rows, auth_snapshot, query_format)
        };
        collected.append(&mut filtered);
    }
    if is_codex_models_api_format(api_format) {
        collected.retain(is_codex_provider_row);
        Some(sort_model_rows(collected))
    } else {
        Some(sort_and_dedup_model_rows(collected))
    }
}

async fn list_model_rows_for_client_format_and_global_model(
    state: &AppState,
    api_format: &str,
    global_model_name: &str,
    auth_snapshot: Option<&crate::data::auth::GatewayAuthApiKeySnapshot>,
) -> Option<Vec<StoredMinimalCandidateSelectionRow>> {
    let mut collected = Vec::new();
    for query_format in models_query_api_formats(api_format) {
        let rows = await_models_route_read(
            "candidate_selection_by_global_model",
            state.list_minimal_candidate_selection_rows_for_api_format_and_global_model(
                query_format,
                global_model_name,
            ),
        )
        .await?;
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
    let auth_snapshot = match await_models_route_read(
        "auth_api_key_snapshot",
        state.data.read_auth_api_key_snapshot(
            &auth_context.user_id,
            &auth_context.api_key_id,
            now_unix_secs,
        ),
    )
    .await
    {
        Some(snapshot) => snapshot,
        None => {
            return Some(build_models_read_fallback_response(
                request_context,
                api_format,
            ))
        }
    };
    let auth_snapshot = auth_snapshot.as_ref();

    match decision.route_kind.as_deref() {
        Some("list") => {
            let rows =
                match list_model_rows_for_client_format(state, api_format, auth_snapshot).await {
                    Some(rows) => rows,
                    None => {
                        return Some(build_models_read_fallback_response(
                            request_context,
                            api_format,
                        ))
                    }
                };
            if rows.is_empty() {
                return Some(build_empty_models_list_response(api_format));
            }
            if is_codex_models_api_format(api_format) {
                let models = load_codex_model_cards(state, &rows).await;
                return Some(build_codex_models_list_response(models));
            }
            let response = match api_format {
                "claude:messages" => {
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
                "gemini:generate_content" => {
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
            let rows = match list_model_rows_for_client_format_and_global_model(
                state,
                api_format,
                &model_id,
                auth_snapshot,
            )
            .await
            {
                Some(rows) => rows,
                None => {
                    return Some(build_models_read_fallback_response(
                        request_context,
                        api_format,
                    ))
                }
            };
            let Some(row) = rows.first() else {
                return Some(build_models_not_found_response(&model_id, api_format));
            };
            let response = match api_format {
                "claude:messages" => build_claude_model_detail_response(row),
                "gemini:generate_content" => build_gemini_model_detail_response(row),
                _ => build_openai_model_detail_response(row),
            };
            Some(response)
        }
        _ => Some(build_models_auth_error_response(api_format)),
    }
}
