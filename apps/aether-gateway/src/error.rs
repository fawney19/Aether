use axum::body::Body;
use axum::http::{Response, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;
use tracing::warn;

use crate::ai_serving::AiSurfaceFinalizeError;
use crate::constants::*;
use crate::insert_header_if_missing;

#[derive(Debug)]
pub(crate) enum GatewayError {
    UpstreamUnavailable { trace_id: String, message: String },
    ControlUnavailable { trace_id: String, message: String },
    Client { status: StatusCode, message: String },
    Internal(String),
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response<Body> {
        match self {
            Self::UpstreamUnavailable { trace_id, message } => {
                warn!(trace_id = %trace_id, error = %message, "gateway proxy unavailable");
                let body = Json(json!({
                    "error": {
                        "message": "gateway proxy unavailable",
                        "trace_id": trace_id,
                    }
                }));
                let mut response = (StatusCode::BAD_GATEWAY, body).into_response();
                let _ =
                    insert_header_if_missing(response.headers_mut(), TRACE_ID_HEADER, &trace_id);
                let _ = insert_header_if_missing(
                    response.headers_mut(),
                    GATEWAY_HEADER,
                    "rust-phase3b",
                );
                response
            }
            Self::ControlUnavailable { trace_id, message } => {
                warn!(trace_id = %trace_id, error = %message, "gateway control unavailable");
                let body = Json(json!({
                    "error": {
                        "message": "gateway control unavailable",
                        "trace_id": trace_id,
                    }
                }));
                let mut response = (StatusCode::BAD_GATEWAY, body).into_response();
                let _ =
                    insert_header_if_missing(response.headers_mut(), TRACE_ID_HEADER, &trace_id);
                let _ = insert_header_if_missing(
                    response.headers_mut(),
                    GATEWAY_HEADER,
                    "rust-phase3b",
                );
                response
            }
            Self::Client { status, message } => (
                status,
                Json(json!({
                    "error": {
                        "message": message,
                    }
                })),
            )
                .into_response(),
            Self::Internal(message) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": {
                        "message": message,
                    }
                })),
            )
                .into_response(),
        }
    }
}

impl From<AiSurfaceFinalizeError> for GatewayError {
    fn from(error: AiSurfaceFinalizeError) -> Self {
        GatewayError::Internal(error.0)
    }
}
