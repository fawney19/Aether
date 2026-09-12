use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::{AppState, GatewayError};
use aether_data::repository::audit::RequestAuditBundle;
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;

#[derive(Debug, Deserialize)]
pub(crate) struct GetRequestAuditBundleQuery {
    pub(crate) attempted_only: Option<bool>,
}

pub(crate) async fn get_request_usage_audit(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
) -> Result<Json<StoredRequestUsageAudit>, axum::response::Response> {
    let usage = state
        .data
        .read_request_usage_audit(&request_id)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()).into_response())?;

    match usage {
        Some(usage) => Ok(Json(usage)),
        None => Err((
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({
                "error": {
                    "message": "Request usage not found",
                }
            })),
        )
            .into_response()),
    }
}

pub(crate) async fn get_request_audit_bundle(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
    Query(query): Query<GetRequestAuditBundleQuery>,
) -> Result<Json<RequestAuditBundle>, axum::response::Response> {
    let attempted_only = query.attempted_only.unwrap_or(false);
    let bundle = state
        .data
        .read_request_audit_bundle(&request_id, attempted_only, current_unix_secs())
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()).into_response())?;

    match bundle {
        Some(mut bundle) => {
            if let Some(trace) = bundle.decision_trace.as_mut() {
                trace.sanitize_sensitive_diagnostics();
            }
            Ok(Json(bundle))
        }
        None => Err((
            axum::http::StatusCode::NOT_FOUND,
            Json(json!({
                "error": {
                    "message": "Request audit bundle not found",
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
