use std::collections::BTreeSet;
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
    filter_eligible_model_rows_for_state, models_api_format, models_detail_id,
    models_query_api_formats,
};
use super::{query_param_value, AppState, GatewayPublicRequestContext};

#[cfg(not(test))]
const MODELS_ROUTE_READ_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
const MODELS_ROUTE_READ_TIMEOUT: Duration = Duration::from_secs(1);
const CODEX_MODELS_QUERY_API_FORMATS: &[&str] = &["openai:responses"];
const CODEX_MODELS_MAX_RESPONSE_MODELS: usize = 512;
const CODEX_MODELS_MAX_RESPONSE_JSON_BYTES: usize = 8 * 1024 * 1024;

fn codex_projected_catalog_fits_response_limits(cards: &[Value]) -> bool {
    if cards.len() > CODEX_MODELS_MAX_RESPONSE_MODELS {
        return false;
    }
    serde_json::to_vec(&serde_json::json!({ "models": cards }))
        .is_ok_and(|body| body.len() <= CODEX_MODELS_MAX_RESPONSE_JSON_BYTES)
}

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

async fn load_codex_model_cards(
    state: &AppState,
    rows: &[StoredMinimalCandidateSelectionRow],
    targets: &[crate::model_fetch::CodexCatalogTarget],
    client_version: crate::model_fetch::NormalizedCodexClientVersion,
) -> (Vec<Value>, Option<String>) {
    let catalogs = crate::model_fetch::load_codex_catalogs(state, targets, &client_version).await;
    for target in catalogs.stale_targets() {
        let state = state.clone();
        let target = target.clone();
        let client_version = client_version.clone();
        tokio::spawn(async move {
            crate::model_fetch::refresh_codex_catalog_target(&state, &target, &client_version)
                .await;
        });
    }
    if !catalogs.is_complete() {
        warn!(
            event_name = "codex_catalog_aggregate_incomplete",
            client_version = %client_version.as_str(),
            target_count = targets.len(),
            "Codex catalog aggregation was incomplete; serving cards from available last-known-good snapshots"
        );
    }
    let mut seen_global_models = BTreeSet::new();
    let possible_inference_catalogs = rows
        .iter()
        .filter(|row| is_codex_provider_row(row))
        .map(|row| (row.provider_id.clone(), row.key_id.clone()))
        .collect::<BTreeSet<_>>();
    let expected_global_models = rows
        .iter()
        .filter(|row| is_codex_provider_row(row))
        .map(|row| row.global_model_name.clone())
        .collect::<BTreeSet<_>>();
    let mut cards = Vec::new();
    for row in rows.iter().filter(|row| is_codex_provider_row(row)) {
        if seen_global_models.contains(&row.global_model_name) {
            continue;
        }
        let Some(snapshot) = catalogs.snapshot(&row.provider_id, &row.key_id) else {
            continue;
        };
        let source_model =
            aether_scheduler_core::select_provider_model_name(row, "openai:responses");
        let Some(card) = crate::ai_serving::project_codex_catalog_model_card(
            &snapshot.models,
            source_model.as_str(),
            row.global_model_name.as_str(),
        ) else {
            warn!(
                event_name = "codex_catalog_authorized_model_missing",
                provider_id = %row.provider_id,
                key_id = %row.key_id,
                client_version = %client_version.as_str(),
                source_model = %source_model,
                global_model = %row.global_model_name,
                "authorized Codex model was not present in this upstream catalog mapping"
            );
            continue;
        };
        seen_global_models.insert(row.global_model_name.clone());
        cards.push(card);
        if cards.len() > CODEX_MODELS_MAX_RESPONSE_MODELS {
            warn!(
                event_name = "codex_catalog_aggregate_model_limit",
                client_version = %client_version.as_str(),
                model_count = cards.len(),
                limit = CODEX_MODELS_MAX_RESPONSE_MODELS,
                "Codex projected catalog exceeded the aggregate model limit; returning an empty remote catalog"
            );
            return (Vec::new(), None);
        }
    }
    let missing_model_count = expected_global_models
        .difference(&seen_global_models)
        .count();
    if missing_model_count > 0 {
        warn!(
            event_name = "codex_catalog_authorized_models_incomplete",
            client_version = %client_version.as_str(),
            expected_model_count = expected_global_models.len(),
            projected_model_count = cards.len(),
            missing_model_count,
            "Codex upstream catalogs omitted authorized mappings; serving the available cards without fabricating missing model metadata"
        );
    }
    if !codex_projected_catalog_fits_response_limits(&cards) {
        warn!(
            event_name = "codex_catalog_aggregate_body_limit",
            client_version = %client_version.as_str(),
            model_count = cards.len(),
            limit_bytes = CODEX_MODELS_MAX_RESPONSE_JSON_BYTES,
            "Codex projected catalog exceeded the aggregate response body limit; returning an empty remote catalog"
        );
        return (Vec::new(), None);
    }
    if cards.is_empty() {
        return (cards, None);
    }
    let etag = if possible_inference_catalogs.len() == 1 {
        possible_inference_catalogs
            .iter()
            .next()
            .and_then(|(provider_id, key_id)| catalogs.snapshot(provider_id, key_id))
            .and_then(|snapshot| snapshot.etag.clone())
    } else {
        None
    };
    (cards, etag)
}

struct ModelRowsForClientFormat {
    rows: Vec<StoredMinimalCandidateSelectionRow>,
    codex_catalog_targets: Vec<crate::model_fetch::CodexCatalogTarget>,
}

async fn list_model_rows_for_client_format(
    state: &AppState,
    api_format: &str,
    auth_snapshot: Option<&crate::data::auth::GatewayAuthApiKeySnapshot>,
) -> Option<ModelRowsForClientFormat> {
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
        let mut filtered = await_models_route_read(
            "candidate_selection_pool_key_policy",
            filter_eligible_model_rows_for_state(state, rows, auth_snapshot, query_format),
        )
        .await?;
        collected.append(&mut filtered);
    }
    if is_codex_models_api_format(api_format) {
        collected.retain(is_codex_provider_row);
        let codex_catalog_targets = crate::model_fetch::codex_catalog_targets(&collected);
        Some(ModelRowsForClientFormat {
            rows: sort_model_rows(collected),
            codex_catalog_targets,
        })
    } else {
        Some(ModelRowsForClientFormat {
            rows: sort_and_dedup_model_rows(collected),
            codex_catalog_targets: Vec::new(),
        })
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
        let mut filtered = await_models_route_read(
            "candidate_selection_pool_key_policy",
            filter_eligible_model_rows_for_state(state, rows, auth_snapshot, query_format),
        )
        .await?;
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
    if !auth_context.access_allowed || auth_context.local_rejection.is_some() {
        return Some(build_models_auth_error_response(api_format));
    }
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
    let Some(auth_snapshot) = auth_snapshot.as_ref() else {
        warn!(
            event_name = "models_route_auth_snapshot_missing",
            user_id = %auth_context.user_id,
            api_key_id = %auth_context.api_key_id,
            "gateway models route rejected a request whose authenticated API key snapshot disappeared"
        );
        return Some(build_models_auth_error_response(api_format));
    };
    if !auth_snapshot.currently_usable {
        return Some(build_models_auth_error_response(api_format));
    }
    let auth_snapshot = Some(auth_snapshot);

    match decision.route_kind.as_deref() {
        Some("list") => {
            let listed =
                match list_model_rows_for_client_format(state, api_format, auth_snapshot).await {
                    Some(rows) => rows,
                    None => {
                        return Some(build_models_read_fallback_response(
                            request_context,
                            api_format,
                        ))
                    }
                };
            let rows = listed.rows;
            if rows.is_empty() {
                return Some(build_empty_models_list_response(api_format));
            }
            if is_codex_models_api_format(api_format) {
                let raw_client_version = query_param_value(
                    request_context.request_query_string.as_deref(),
                    "client_version",
                );
                let client_version = crate::model_fetch::normalize_codex_client_version(
                    raw_client_version.as_deref(),
                );
                if client_version.used_fallback() {
                    warn!(
                        event_name = "codex_catalog_invalid_client_version",
                        raw_length = raw_client_version.as_ref().map_or(0, String::len),
                        fallback_version = %client_version.as_str(),
                        "invalid Codex client_version used the bounded fallback version"
                    );
                }
                let (models, etag) = load_codex_model_cards(
                    state,
                    &rows,
                    &listed.codex_catalog_targets,
                    client_version,
                )
                .await;
                return Some(build_codex_models_list_response(models, etag.as_deref()));
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        codex_projected_catalog_fits_response_limits, CODEX_MODELS_MAX_RESPONSE_JSON_BYTES,
        CODEX_MODELS_MAX_RESPONSE_MODELS,
    };

    #[test]
    fn projected_codex_catalog_enforces_aggregate_count_and_body_limits() {
        assert!(codex_projected_catalog_fits_response_limits(&[json!({
            "slug": "gpt-future-dynamic",
            "model_messages": {"instructions_template": "opaque"}
        })]));

        let too_many = (0..=CODEX_MODELS_MAX_RESPONSE_MODELS)
            .map(|index| json!({"slug": format!("gpt-future-{index}")}))
            .collect::<Vec<_>>();
        assert!(!codex_projected_catalog_fits_response_limits(&too_many));

        let oversized = vec![json!({
            "slug": "gpt-future-oversized",
            "future_capability": "x".repeat(CODEX_MODELS_MAX_RESPONSE_JSON_BYTES)
        })];
        assert!(!codex_projected_catalog_fits_response_limits(&oversized));
    }
}
