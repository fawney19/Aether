use crate::handlers::admin::provider::query::{
    models::{
        build_admin_provider_query_models_response,
        build_admin_provider_query_test_model_failover_local_response,
        build_admin_provider_query_test_model_local_response,
    },
    payload::{
        parse_admin_provider_query_body, provider_query_extract_failover_models,
        provider_query_extract_model, provider_query_extract_provider_id,
        provider_query_extract_request_id, provider_query_payload_keys,
    },
    response::{
        build_admin_provider_query_bad_request_response,
        ADMIN_PROVIDER_QUERY_FAILOVER_MODELS_REQUIRED_DETAIL,
        ADMIN_PROVIDER_QUERY_MODEL_REQUIRED_DETAIL,
        ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL,
    },
};
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::AdminRequestContext;
use crate::log_ids::short_request_id;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::Response,
};
use tracing::warn;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn maybe_build_admin_provider_query_route_response(
        &self,
        request_context: &AdminRequestContext<'_>,
        request_body: Option<&Bytes>,
    ) -> Result<Option<Response<Body>>, GatewayError> {
        let Some(decision) = request_context.decision() else {
            return Ok(None);
        };

        if decision.route_family.as_deref() != Some("provider_query_manage") {
            return Ok(None);
        }

        if request_context.method() != http::Method::POST {
            return Ok(None);
        }

        let payload = match parse_admin_provider_query_body(request_body) {
            Ok(value) => value,
            Err(response) => return Ok(Some(response)),
        };

        let route_kind = decision.route_kind.as_deref().unwrap_or("query_models");
        match route_kind {
            "query_models" => Ok(Some(
                build_admin_provider_query_models_response(self, &payload).await?,
            )),
            "test_model" => {
                let Some(_provider_id) = provider_query_extract_provider_id(&payload) else {
                    log_admin_provider_query_validation_failure(
                        request_context,
                        route_kind,
                        ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL,
                        &payload,
                    );
                    return Ok(Some(build_admin_provider_query_bad_request_response(
                        ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL,
                    )));
                };
                let Some(_model) = provider_query_extract_model(&payload) else {
                    log_admin_provider_query_validation_failure(
                        request_context,
                        route_kind,
                        ADMIN_PROVIDER_QUERY_MODEL_REQUIRED_DETAIL,
                        &payload,
                    );
                    return Ok(Some(build_admin_provider_query_bad_request_response(
                        ADMIN_PROVIDER_QUERY_MODEL_REQUIRED_DETAIL,
                    )));
                };
                Ok(Some(
                    build_admin_provider_query_test_model_local_response(self, &payload).await?,
                ))
            }
            "test_model_failover" => {
                let Some(_provider_id) = provider_query_extract_provider_id(&payload) else {
                    log_admin_provider_query_validation_failure(
                        request_context,
                        route_kind,
                        ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL,
                        &payload,
                    );
                    return Ok(Some(build_admin_provider_query_bad_request_response(
                        ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL,
                    )));
                };
                let failover_models = provider_query_extract_failover_models(&payload);
                if failover_models.is_empty() {
                    log_admin_provider_query_validation_failure(
                        request_context,
                        route_kind,
                        ADMIN_PROVIDER_QUERY_FAILOVER_MODELS_REQUIRED_DETAIL,
                        &payload,
                    );
                    return Ok(Some(build_admin_provider_query_bad_request_response(
                        ADMIN_PROVIDER_QUERY_FAILOVER_MODELS_REQUIRED_DETAIL,
                    )));
                }
                Ok(Some(
                    build_admin_provider_query_test_model_failover_local_response(self, &payload)
                        .await?,
                ))
            }
            _ => Ok(Some(
                build_admin_provider_query_models_response(self, &payload).await?,
            )),
        }
    }
}

fn log_admin_provider_query_validation_failure(
    request_context: &AdminRequestContext<'_>,
    route_kind: &str,
    detail: &'static str,
    payload: &serde_json::Value,
) {
    let provider_id =
        provider_query_extract_provider_id(payload).unwrap_or_else(|| "-".to_string());
    let model = provider_query_extract_model(payload).unwrap_or_else(|| "-".to_string());
    let request_id = provider_query_extract_request_id(payload).unwrap_or_else(|| "-".to_string());
    let request_id_for_log = short_request_id(request_id.as_str());
    let payload_keys = provider_query_payload_keys(payload);
    warn!(
        event_name = "admin_provider_query_request_rejected",
        log_type = "validation",
        route_kind,
        path = %request_context.path(),
        request_id = %request_id_for_log,
        provider_id = %provider_id,
        model = %model,
        payload_keys = ?payload_keys,
        detail,
        "admin provider query request rejected"
    );
}
