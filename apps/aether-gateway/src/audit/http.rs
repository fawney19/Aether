use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{AppState, GatewayError};

#[derive(Debug, Deserialize)]
pub(crate) struct GetRequestCandidateTraceQuery {
    pub(crate) attempted_only: Option<bool>,
}

pub(crate) async fn get_request_candidate_trace(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
    Query(query): Query<GetRequestCandidateTraceQuery>,
) -> Result<Json<crate::data::candidates::RequestCandidateTrace>, axum::response::Response> {
    let attempted_only = query.attempted_only.unwrap_or(false);
    let trace = state
        .data
        .read_request_candidate_trace(&request_id, attempted_only)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()).into_response())?;

    match trace {
        Some(mut trace) => {
            trace.sanitize_sensitive_diagnostics();
            Ok(Json(trace))
        }
        None => Err((
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({
                "error": {
                    "message": "Request not found",
                }
            })),
        )
            .into_response()),
    }
}

pub(crate) async fn get_decision_trace(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
    Query(query): Query<GetRequestCandidateTraceQuery>,
) -> Result<Json<crate::data::decision_trace::DecisionTrace>, axum::response::Response> {
    let attempted_only = query.attempted_only.unwrap_or(false);
    let trace = state
        .data
        .read_decision_trace(&request_id, attempted_only)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()).into_response())?;

    match trace {
        Some(mut trace) => {
            trace.sanitize_sensitive_diagnostics();
            Ok(Json(trace))
        }
        None => Err((
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({
                "error": {
                    "message": "Decision trace not found",
                }
            })),
        )
            .into_response()),
    }
}

pub(crate) async fn get_auth_api_key_snapshot(
    State(state): State<AppState>,
    Path((user_id, api_key_id)): Path<(String, String)>,
) -> Result<Json<crate::data::auth::GatewayAuthApiKeySnapshot>, axum::response::Response> {
    let snapshot = state
        .data
        .read_auth_api_key_snapshot(&user_id, &api_key_id, current_unix_secs())
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()).into_response())?;

    match snapshot {
        Some(snapshot) => Ok(Json(snapshot)),
        None => Err((
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({
                "error": {
                    "message": "Auth snapshot not found",
                }
            })),
        )
            .into_response()),
    }
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
