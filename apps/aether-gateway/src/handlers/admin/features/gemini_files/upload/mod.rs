use super::{admin_gemini_files_error_response, ADMIN_GEMINI_FILES_DATA_UNAVAILABLE_DETAIL};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::is_admin_gemini_files_upload_root;
use crate::GatewayError;
use axum::body::{Body, Bytes};
use axum::http::{self, Response};
use axum::response::IntoResponse;
use axum::Json;

mod request;
mod stage;
mod support;

pub(super) async fn maybe_build_local_admin_gemini_files_upload_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    match request_context
        .decision()
        .and_then(|decision| decision.route_kind.as_deref())
    {
        Some("upload")
            if request_context.method() == http::Method::POST
                && is_admin_gemini_files_upload_root(request_context.path()) =>
        {
            if !state.has_gemini_file_mapping_data_writer() {
                return Ok(Some(admin_gemini_files_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    ADMIN_GEMINI_FILES_DATA_UNAVAILABLE_DETAIL,
                )));
            }
            let upload = match request::admin_gemini_files_parse_upload_request(
                state,
                request_context,
                request_body,
            ) {
                Ok(upload) => upload,
                Err(detail) => {
                    return Ok(Some(admin_gemini_files_error_response(
                        http::StatusCode::BAD_REQUEST,
                        detail,
                    )));
                }
            };
            let key_ids = support::admin_gemini_files_query_key_ids(state, request_context);
            if key_ids.is_empty() {
                return Ok(Some(admin_gemini_files_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "key_ids 不能为空",
                )));
            }
            let response = stage::admin_gemini_files_upload_across_keys(
                state,
                "",
                request_context.trace_id(),
                &upload,
                &key_ids,
            )
            .await?;
            Ok(Some(Json(response).into_response()))
        }
        _ => Ok(None),
    }
}
