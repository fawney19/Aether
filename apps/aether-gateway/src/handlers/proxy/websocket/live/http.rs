//! Authenticated WebRTC call creation for Codex Live.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use aether_contracts::{
    ExecutionPlan, ExecutionResponseBodyMode, ExecutionResult, EXECUTION_RESPONSE_BODY_MODE_HEADER,
};
use axum::body::{Body, Bytes};
use axum::http::{HeaderValue, Response, StatusCode};
use base64::Engine as _;
use serde_json::json;
use tracing::{info, warn};

use crate::ai_serving::build_standard_sync_plan_from_decision;
use crate::api::response::{
    build_client_response_from_parts, build_client_response_from_parts_with_mutator,
    build_local_auth_rejection_response, build_local_http_error_response_with_request_path,
};
use crate::control::{execution_plan_balance_capacity_rejection, GatewayPublicRequestContext};
use crate::execution_runtime::execute_execution_runtime_sync_plan_with_report_context;
use crate::handlers::proxy::websocket::responses::ResponsesWebSocketTurnAdmission;
use crate::{AppState, GatewayError};

use super::audit::{
    mark_live_call_create_report_context, LiveCallCreateAuditGuard,
    LIVE_CALL_CANDIDATE_UNAVAILABLE_MESSAGE,
};
use super::live_usage_accounting_is_safe;
use super::planner::{live_call_url, plan_live_candidate, LiveAuthMode, LivePoolLeaseGuard};
use super::protocol::{
    build_live_multipart, extract_call_id_from_location, parse_live_multipart, validate_model,
    validate_realtime_call_create_query, LiveRouteDialect,
};
use super::registry::{LiveCallBinding, LiveCallRegistry};

const MAX_LIVE_HTTP_BODY_BYTES: usize = 1024 * 1024;

pub(crate) async fn maybe_handle_live_http(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    parts: &http::request::Parts,
    body: Option<&Bytes>,
    remote_addr: &SocketAddr,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(dialect) = LiveRouteDialect::from_call_create_path(&request_context.request_path)
    else {
        return Ok(None);
    };
    if parts.method != http::Method::POST {
        return Ok(None);
    }
    // `/v1/realtime/calls` is shared with the ordinary OpenAI Realtime API.
    // The control-plane route classifier is the authority that distinguishes
    // Codex AVAS (`intent=quicksilver`) from a normal Realtime call.  Do not
    // let this path-only specialized hook steal an `openai:realtime` request
    // after classification has deliberately kept it on the generic proxy.
    if dialect == LiveRouteDialect::Realtime
        && !request_context
            .control_decision
            .as_ref()
            .and_then(|decision| decision.auth_endpoint_signature.as_deref())
            .is_some_and(|format| format.eq_ignore_ascii_case("codex:live"))
    {
        return Ok(None);
    }
    Box::pin(handle_live_http(
        state,
        request_context,
        parts,
        body,
        remote_addr,
        dialect,
    ))
    .await
}

async fn handle_live_http(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    parts: &http::request::Parts,
    body: Option<&Bytes>,
    remote_addr: &SocketAddr,
    dialect: LiveRouteDialect,
) -> Result<Option<Response<Body>>, GatewayError> {
    let mut call_audit = LiveCallCreateAuditGuard::new(
        state,
        request_context.control_decision.as_ref(),
        request_context.trace_id.as_str(),
        request_context.request_path.as_str(),
    );
    if dialect == LiveRouteDialect::Realtime {
        if let Err(error) = validate_realtime_call_create_query(parts.uri.query()) {
            return audited_local_live_error(
                &mut call_audit,
                request_context,
                error.status_code(),
                error.client_message(),
                error.code(),
            );
        }
    }
    let Some(control_decision) = request_context.control_decision.as_ref() else {
        return audited_local_live_error(
            &mut call_audit,
            request_context,
            StatusCode::NOT_FOUND,
            "Codex Live route is unavailable",
            "route_unavailable",
        );
    };
    if !live_usage_accounting_is_safe(control_decision) {
        return audited_local_live_error(
            &mut call_audit,
            request_context,
            StatusCode::NOT_IMPLEMENTED,
            "Codex Live is unavailable for finite-balance keys until Frameless usage settlement is supported",
            "finite_balance_unsupported",
        );
    }
    let Some(body) = body else {
        return audited_local_live_error(
            &mut call_audit,
            request_context,
            StatusCode::BAD_REQUEST,
            "Codex Live requires a multipart WebRTC offer",
            "request_body_missing",
        );
    };
    if body.len() > MAX_LIVE_HTTP_BODY_BYTES {
        return audited_local_live_error(
            &mut call_audit,
            request_context,
            StatusCode::PAYLOAD_TOO_LARGE,
            "Codex Live WebRTC offer exceeds the 1 MiB limit",
            "request_body_too_large",
        );
    }
    let content_type = parts
        .headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let offer = match parse_live_multipart(content_type, body.as_ref()) {
        Ok(offer) => offer,
        Err(error) => {
            return audited_local_live_error(
                &mut call_audit,
                request_context,
                error.status_code(),
                error.client_message(),
                "multipart_parse_failed",
            )
        }
    };
    let Some(client_model) = offer
        .session
        .get("model")
        .and_then(serde_json::Value::as_str)
    else {
        return audited_local_live_error(
            &mut call_audit,
            request_context,
            StatusCode::BAD_REQUEST,
            "Codex Live session.model must be a non-empty model identifier",
            "model_missing",
        );
    };
    if validate_model(client_model).is_err() {
        return audited_local_live_error(
            &mut call_audit,
            request_context,
            StatusCode::BAD_REQUEST,
            "Codex Live session.model must be a valid model identifier",
            "model_invalid",
        );
    }
    call_audit.set_validated_client_model(client_model);

    let planning = match plan_live_candidate(
        state,
        request_context.trace_id.as_str(),
        control_decision,
        &parts.headers,
        remote_addr,
        client_model,
        dialect,
        None,
    )
    .await
    {
        Ok(planning) => planning,
        Err(error) => {
            call_audit.fail(gateway_error_status(&error), "planning_failed");
            return Err(error);
        }
    };
    call_audit.set_runtime_miss(planning.runtime_miss.clone());
    let runtime_miss = planning.runtime_miss;
    let Some(mut candidate) = planning.candidate else {
        let auth_context = control_decision.auth_context.as_ref();
        warn!(
            event_name = "codex_live_call_candidate_unavailable",
            log_type = "ops",
            trace_id = %request_context.trace_id,
            transport = "webrtc",
            mode = "call_create",
            status_code = StatusCode::SERVICE_UNAVAILABLE.as_u16(),
            client_model,
            user_id = auth_context
                .map(|auth| auth.user_id.as_str())
                .unwrap_or("-"),
            api_key_id = auth_context
                .map(|auth| auth.api_key_id.as_str())
                .unwrap_or("-"),
            runtime_miss_reason = runtime_miss
                .as_ref()
                .map(|diagnostic| diagnostic.reason.as_str())
                .unwrap_or("unknown"),
            candidate_count = runtime_miss
                .as_ref()
                .and_then(|diagnostic| diagnostic.candidate_count)
                .unwrap_or(0),
            skipped_candidate_count = runtime_miss
                .as_ref()
                .and_then(|diagnostic| diagnostic.skipped_candidate_count)
                .unwrap_or(0),
            skip_reasons = runtime_miss
                .as_ref()
                .and_then(|diagnostic| diagnostic.skip_reasons_summary())
                .unwrap_or_default(),
            "Codex Live call creation has no eligible provider mapping"
        );
        return audited_local_live_error(
            &mut call_audit,
            request_context,
            StatusCode::SERVICE_UNAVAILABLE,
            LIVE_CALL_CANDIDATE_UNAVAILABLE_MESSAGE,
            "candidate_unavailable",
        );
    };
    let lease = LivePoolLeaseGuard::new(state, &candidate);
    let binding = LiveCallBinding::from_candidate(&candidate);
    let mut provider_session = offer.session.clone();
    provider_session
        .as_object_mut()
        .expect("validated Live session is a JSON object")
        .insert(
            "model".to_string(),
            serde_json::Value::String(candidate.provider_model.clone()),
        );
    let upstream_url = match live_call_url(&candidate, dialect) {
        Ok(url) => url,
        Err(error) => {
            lease.release().await;
            return audited_local_live_error(
                &mut call_audit,
                request_context,
                error.status_code(),
                error.client_message(),
                "upstream_url_invalid",
            );
        }
    };

    let (provider_content_type, provider_body_base64) = match build_live_call_provider_body(
        candidate.auth_mode,
        offer.sdp.as_str(),
        &provider_session,
    ) {
        Ok(body) => body,
        Err(error) => {
            lease.release().await;
            call_audit.fail(gateway_error_status(&error), "provider_body_build_failed");
            return Err(error);
        }
    };
    // The standard plan builder requires a JSON body marker even when the exact wire body is
    // carried as bytes. Keep only the mapped model here: retaining the SDP/session projection in
    // the decision would unnecessarily widen the surface for future logging or report changes.
    let provider_body_marker = json!({"model": candidate.provider_model.clone()});
    candidate.execution.upstream_url = Some(upstream_url);
    candidate.execution.provider_request_method = Some("POST".to_string());
    candidate.execution.provider_request_body = Some(provider_body_marker.clone());
    candidate.execution.provider_request_body_base64 = Some(provider_body_base64);
    candidate.execution.content_type = Some(provider_content_type.clone());
    candidate.execution.content_encoding = None;
    candidate.execution.request_gzip = None;
    candidate.execution.upstream_is_stream = false;
    prepare_live_call_request_headers(
        &mut candidate.execution.provider_request_headers,
        provider_content_type.as_str(),
    );
    candidate.execution.provider_request_headers.insert(
        EXECUTION_RESPONSE_BODY_MODE_HEADER.to_string(),
        ExecutionResponseBodyMode::PreserveBytes
            .as_str()
            .to_string(),
    );

    let mut attempt = match build_standard_sync_plan_from_decision(
        parts,
        &provider_body_marker,
        candidate.execution,
    ) {
        Ok(Some(attempt)) => attempt,
        Ok(None) => {
            lease.release().await;
            return audited_local_live_error(
                &mut call_audit,
                request_context,
                StatusCode::BAD_GATEWAY,
                "Codex Live provider request could not be built",
                "provider_plan_unavailable",
            );
        }
        Err(error) => {
            lease.release().await;
            call_audit.fail(gateway_error_status(&error), "provider_plan_build_failed");
            return Err(error);
        }
    };
    // The synchronous SDP exchange has an ordinary request lifecycle, but it
    // does not contain the media leg's token/cost usage. Keep the existing row
    // while making that boundary explicit and non-billable.
    mark_live_call_create_report_context(&mut attempt.report_context);
    call_audit.bind_attempt(&attempt);
    let balance_rejection = match execution_plan_balance_capacity_rejection(
        state,
        control_decision,
        &attempt.plan,
        attempt.report_context.as_ref(),
    )
    .await
    {
        Ok(rejection) => rejection,
        Err(error) => {
            lease.release().await;
            call_audit.fail(
                gateway_error_status(&error),
                "balance_capacity_check_failed",
            );
            return Err(error);
        }
    };
    if let Some(rejection) = balance_rejection {
        lease.release().await;
        let response = match build_local_auth_rejection_response(
            request_context.trace_id.as_str(),
            Some(control_decision),
            &rejection,
        ) {
            Ok(response) => response,
            Err(error) => {
                call_audit.fail(
                    gateway_error_status(&error),
                    "downstream_response_build_failed",
                );
                return Err(error);
            }
        };
        call_audit.fail(response.status(), "balance_capacity_rejected");
        return Ok(Some(response));
    }
    let admission = match ResponsesWebSocketTurnAdmission::acquire(
        state,
        &attempt.plan,
        request_context.trace_id.as_str(),
    )
    .await
    {
        Ok(admission) => admission,
        Err(error) => {
            lease.release().await;
            call_audit.fail(gateway_error_status(&error), "admission_failed");
            return Err(error);
        }
    };
    let result = execute_execution_runtime_sync_plan_with_report_context(
        state,
        Some(request_context.trace_id.as_str()),
        &attempt.plan,
        attempt.report_context.as_ref(),
    )
    .await;
    // These guards intentionally cover only the synchronous call-creation exchange. The
    // WebRTC media leg bypasses Aether, and neither the two-hour routing binding nor a sideband
    // attachment proves that media is still alive. Holding either guard for a guessed lifetime
    // would leak capacity or release it early without an authoritative upstream close signal.
    admission.release().await;
    let pool_lease_healthy = lease.is_healthy();
    lease.release().await;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            call_audit.fail(gateway_error_status(&error), "upstream_execute_failed");
            return Err(error);
        }
    };
    if !(200..300).contains(&result.status_code) {
        let response_body = match execution_result_body(&result) {
            Ok(body) => body,
            Err(error) => {
                call_audit.fail(StatusCode::BAD_GATEWAY, "upstream_error_body_unavailable");
                return Err(error);
            }
        };
        let downstream_headers =
            sanitized_live_response_headers(&result.headers, response_body.preserves_wire_encoding);
        warn!(
            event_name = "codex_live_call_upstream_failed",
            log_type = "ops",
            trace_id = %request_context.trace_id,
            provider_id = %attempt.plan.provider_id,
            endpoint_id = %attempt.plan.endpoint_id,
            key_id = %attempt.plan.key_id,
            status_code = result.status_code,
            elapsed_ms = result.telemetry.as_ref().and_then(|value| value.elapsed_ms),
            "Codex Live call creation failed upstream"
        );
        let response = match build_client_response_from_parts(
            result.status_code,
            &downstream_headers,
            Body::from(response_body.bytes),
            request_context.trace_id.as_str(),
            Some(control_decision),
        ) {
            Ok(response) => response,
            Err(error) => {
                call_audit.fail(
                    gateway_error_status(&error),
                    "downstream_response_build_failed",
                );
                return Err(error);
            }
        };
        call_audit.fail(response.status(), "upstream_rejected");
        return Ok(Some(response));
    }
    if !pool_lease_healthy {
        warn_live_call_orphaned(
            request_context,
            &attempt.plan,
            &result,
            "pool_lease_lost",
            None,
        );
        return audited_local_live_error(
            &mut call_audit,
            request_context,
            StatusCode::SERVICE_UNAVAILABLE,
            "Codex Live provider lease expired during call creation",
            "pool_lease_lost",
        );
    }
    let response_body = match execution_result_body(&result) {
        Ok(body) => body,
        Err(error) => {
            warn_live_call_orphaned(
                request_context,
                &attempt.plan,
                &result,
                "response_body_unavailable",
                None,
            );
            call_audit.fail(StatusCode::BAD_GATEWAY, "response_body_unavailable");
            return Err(error);
        }
    };
    let Some(location) = header_value(&result.headers, "location") else {
        warn_live_call_orphaned(
            request_context,
            &attempt.plan,
            &result,
            "location_missing",
            None,
        );
        return audited_local_live_error(
            &mut call_audit,
            request_context,
            StatusCode::BAD_GATEWAY,
            "Codex Live upstream response did not include a call location",
            "location_missing",
        );
    };
    let call_id = match extract_call_id_from_location(location) {
        Ok(call_id) => call_id,
        Err(error) => {
            warn_live_call_orphaned(
                request_context,
                &attempt.plan,
                &result,
                "location_invalid",
                Some(error.code()),
            );
            return audited_local_live_error(
                &mut call_audit,
                request_context,
                StatusCode::BAD_GATEWAY,
                error.client_message(),
                "location_invalid",
            );
        }
    };
    let Some(auth_context) = control_decision.auth_context.as_ref() else {
        warn_live_call_orphaned(
            request_context,
            &attempt.plan,
            &result,
            "auth_context_missing",
            None,
        );
        return audited_local_live_error(
            &mut call_audit,
            request_context,
            StatusCode::UNAUTHORIZED,
            "Codex Live requires an authenticated gateway API key",
            "auth_context_missing",
        );
    };
    let registry = LiveCallRegistry::new(std::sync::Arc::clone(&state.runtime_state));
    if let Err(error) = registry
        .register(
            auth_context.user_id.as_str(),
            auth_context.api_key_id.as_str(),
            call_id.as_str(),
            &binding,
        )
        .await
    {
        warn_live_call_orphaned(
            request_context,
            &attempt.plan,
            &result,
            "binding_failed",
            Some(error.kind()),
        );
        return audited_local_live_error(
            &mut call_audit,
            request_context,
            StatusCode::SERVICE_UNAVAILABLE,
            "Codex Live sideband binding is temporarily unavailable",
            "binding_failed",
        );
    }
    info!(
        event_name = "codex_live_call_created",
        log_type = "event",
        trace_id = %request_context.trace_id,
        provider_id = %attempt.plan.provider_id,
        endpoint_id = %attempt.plan.endpoint_id,
        key_id = %attempt.plan.key_id,
        client_model = %binding.client_model(),
        status_code = result.status_code,
        elapsed_ms = result.telemetry.as_ref().and_then(|value| value.elapsed_ms),
        usage_unavailable = true,
        "Codex Live created a bound WebRTC call"
    );
    let downstream_location = dialect.downstream_location(call_id.as_str());
    let downstream_headers =
        sanitized_live_response_headers(&result.headers, response_body.preserves_wire_encoding);
    let response = match build_client_response_from_parts_with_mutator(
        result.status_code,
        &downstream_headers,
        Body::from(response_body.bytes),
        request_context.trace_id.as_str(),
        Some(control_decision),
        |headers| {
            headers.insert(
                http::header::LOCATION,
                HeaderValue::from_str(downstream_location.as_str())
                    .map_err(|error| GatewayError::Internal(error.to_string()))?,
            );
            Ok(())
        },
    ) {
        Ok(response) => response,
        Err(error) => {
            warn_live_call_orphaned(
                request_context,
                &attempt.plan,
                &result,
                "downstream_response_build_failed",
                None,
            );
            call_audit.fail(
                gateway_error_status(&error),
                "downstream_response_build_failed",
            );
            return Err(error);
        }
    };
    call_audit.complete(response.status().as_u16(), "call_created");
    Ok(Some(response))
}

fn gateway_error_status(error: &GatewayError) -> StatusCode {
    match error {
        GatewayError::UpstreamUnavailable { .. } | GatewayError::ControlUnavailable { .. } => {
            StatusCode::BAD_GATEWAY
        }
        GatewayError::LocalExecutionPlanningTimeout { .. } => StatusCode::GATEWAY_TIMEOUT,
        GatewayError::AdmissionTimeout { .. } => StatusCode::TOO_MANY_REQUESTS,
        GatewayError::Client { status, .. } => *status,
        GatewayError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn audited_local_live_error(
    audit: &mut LiveCallCreateAuditGuard,
    request_context: &GatewayPublicRequestContext,
    status: StatusCode,
    message: &str,
    termination: &'static str,
) -> Result<Option<Response<Body>>, GatewayError> {
    let response = match local_live_error(request_context, status, message) {
        Ok(response) => response,
        Err(error) => {
            audit.fail(
                gateway_error_status(&error),
                "downstream_response_build_failed",
            );
            return Err(error);
        }
    };
    audit.fail(response.status(), termination);
    Ok(Some(response))
}

fn local_live_error(
    request_context: &GatewayPublicRequestContext,
    status: StatusCode,
    message: &str,
) -> Result<Response<Body>, GatewayError> {
    build_local_http_error_response_with_request_path(
        request_context.trace_id.as_str(),
        request_context.control_decision.as_ref(),
        Some(request_context.request_path.as_str()),
        status,
        message,
    )
}

fn remove_headers(headers: &mut BTreeMap<String, String>, names: &[&str]) {
    headers.retain(|candidate, _| {
        !names
            .iter()
            .any(|name| candidate.eq_ignore_ascii_case(name))
    });
}

fn header_value<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn prepare_live_call_request_headers(headers: &mut BTreeMap<String, String>, content_type: &str) {
    remove_headers(
        headers,
        &[
            "content-type",
            "content-length",
            "content-encoding",
            "accept",
            "accept-encoding",
        ],
    );
    headers.insert("content-type".to_string(), content_type.to_string());
    headers.insert("accept".to_string(), "application/sdp".to_string());
    headers.insert("accept-encoding".to_string(), "identity".to_string());
}

fn build_live_call_provider_body(
    auth_mode: LiveAuthMode,
    sdp: &str,
    session: &serde_json::Value,
) -> Result<(String, String), GatewayError> {
    let (content_type, bytes) = match auth_mode {
        LiveAuthMode::ApiKey => {
            let (content_type, bytes) = build_live_multipart(sdp, session);
            (content_type, bytes)
        }
        LiveAuthMode::ChatGptOauth => {
            let bytes = serde_json::to_vec(&json!({"sdp": sdp, "session": session}))
                .map_err(|error| GatewayError::Internal(error.to_string()))?;
            ("application/json".to_string(), bytes)
        }
    };
    Ok((
        content_type,
        base64::engine::general_purpose::STANDARD.encode(bytes),
    ))
}

fn sanitized_live_response_headers(
    headers: &BTreeMap<String, String>,
    preserves_wire_encoding: bool,
) -> BTreeMap<String, String> {
    let mut sanitized = headers.clone();
    remove_headers(&mut sanitized, &["location", "set-cookie", "set-cookie2"]);
    if !preserves_wire_encoding {
        remove_headers(&mut sanitized, &["content-length", "content-encoding"]);
    }
    sanitized
}

fn warn_live_call_orphaned(
    request_context: &GatewayPublicRequestContext,
    plan: &ExecutionPlan,
    result: &ExecutionResult,
    reason: &'static str,
    error_kind: Option<&'static str>,
) {
    warn!(
        event_name = "codex_live_call_orphaned",
        log_type = "ops",
        trace_id = %request_context.trace_id,
        provider_id = %plan.provider_id,
        endpoint_id = %plan.endpoint_id,
        key_id = %plan.key_id,
        status_code = result.status_code,
        elapsed_ms = result.telemetry.as_ref().and_then(|value| value.elapsed_ms),
        reason,
        error_kind = error_kind.unwrap_or("none"),
        "Codex Live upstream call succeeded but could not be safely exposed downstream"
    );
}

struct LiveResponseBody {
    bytes: Vec<u8>,
    preserves_wire_encoding: bool,
}

fn execution_result_body(result: &ExecutionResult) -> Result<LiveResponseBody, GatewayError> {
    let Some(body) = result.body.as_ref() else {
        return Ok(LiveResponseBody {
            bytes: Vec::new(),
            preserves_wire_encoding: false,
        });
    };
    if let Some(encoded) = body.body_bytes_b64.as_deref() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| GatewayError::Internal(error.to_string()))?;
        return Ok(LiveResponseBody {
            bytes,
            preserves_wire_encoding: true,
        });
    }
    let bytes = body
        .json_body
        .as_ref()
        .map(serde_json::to_vec)
        .transpose()
        .map(|body| body.unwrap_or_default())
        .map_err(|error| GatewayError::Internal(error.to_string()))?;
    Ok(LiveResponseBody {
        bytes,
        preserves_wire_encoding: false,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use aether_contracts::{ExecutionPlan, ExecutionResult, ResponseBody};
    use aether_data::repository::usage::InMemoryUsageReadRepository;
    use aether_data_contracts::repository::usage::UsageReadRepository;
    use axum::body::to_bytes;
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::prelude::*;

    use crate::control::{GatewayControlAuthContext, GatewayControlDecision};

    use super::*;

    #[derive(Clone, Default)]
    struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

    struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

    impl SharedLogBuffer {
        fn lines(&self) -> Vec<serde_json::Value> {
            String::from_utf8(self.0.lock().expect("log buffer should lock").clone())
                .expect("logs should be valid UTF-8")
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| serde_json::from_str(line).expect("log line should be valid JSON"))
                .collect()
        }
    }

    impl std::io::Write for SharedLogWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer should lock")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for SharedLogBuffer {
        type Writer = SharedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedLogWriter(Arc::clone(&self.0))
        }
    }

    #[test]
    fn preserved_wire_bytes_win_over_the_json_projection() {
        let result = ExecutionResult {
            request_id: "request".to_string(),
            candidate_id: None,
            status_code: 201,
            headers: Default::default(),
            response_observation: None,
            body: Some(ResponseBody {
                json_body: Some(json!({"projected": true})),
                body_bytes_b64: Some(
                    base64::engine::general_purpose::STANDARD.encode(b"raw-sdp-answer"),
                ),
            }),
            telemetry: None,
            error: None,
        };
        let body = execution_result_body(&result).unwrap();
        assert_eq!(body.bytes, b"raw-sdp-answer");
        assert!(body.preserves_wire_encoding);
    }

    #[test]
    fn live_call_bodies_are_bytes_only_and_do_not_enter_report_context() {
        let session = json!({
            "model": "provider-live-model",
            "instructions": "opaque private instructions",
            "future_capability": {"enabled": true}
        });
        let sdp = "v=0\r\no=private-live-offer";
        let provider_body_marker = json!({"model": "provider-live-model"});

        for auth_mode in [LiveAuthMode::ApiKey, LiveAuthMode::ChatGptOauth] {
            let (content_type, encoded) =
                build_live_call_provider_body(auth_mode, sdp, &session).unwrap();
            let report_context = aether_ai_serving::augment_sync_report_context(
                Some(json!({"trace_id": "trace-live"})),
                &BTreeMap::new(),
                &provider_body_marker,
            )
            .unwrap()
            .unwrap();
            assert!(report_context.get("provider_request_body").is_none());
            assert!(!report_context.to_string().contains("private-live-offer"));
            assert!(!report_context
                .to_string()
                .contains("opaque private instructions"));

            let plan_body = aether_ai_serving::resolve_ai_passthrough_sync_request_body(
                Some(provider_body_marker.clone()),
                Some(encoded.clone()),
            );
            assert!(plan_body.json_body.is_none());
            assert_eq!(plan_body.body_bytes_b64.as_deref(), Some(encoded.as_str()));
            let usage_plan = ExecutionPlan {
                request_id: "trace-live".to_string(),
                candidate_id: Some("candidate-live".to_string()),
                provider_name: Some("codex".to_string()),
                provider_id: "provider-live".to_string(),
                endpoint_id: "endpoint-live".to_string(),
                key_id: "key-live".to_string(),
                method: "POST".to_string(),
                url: "https://api.openai.com/v1/live".to_string(),
                headers: BTreeMap::new(),
                content_type: Some(content_type.clone()),
                content_encoding: None,
                body: plan_body,
                stream: false,
                client_api_format: "openai:responses".to_string(),
                provider_api_format: "openai:responses".to_string(),
                model_name: Some("provider-live-model".to_string()),
                proxy: None,
                transport_profile: None,
                timeouts: None,
            };
            let usage_seed = aether_usage_runtime::build_terminal_usage_context_seed(
                &usage_plan,
                Some(&report_context),
            );
            assert!(usage_seed.provider_request.is_none());
            assert!(!usage_seed
                .request_metadata
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default()
                .contains("private-live-offer"));

            let wire = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap();
            match auth_mode {
                LiveAuthMode::ApiKey => {
                    let parsed = parse_live_multipart(content_type.as_str(), wire.as_slice())
                        .expect("API-key multipart should round-trip");
                    assert_eq!(parsed.sdp, sdp);
                    assert_eq!(parsed.session, session);
                }
                LiveAuthMode::ChatGptOauth => {
                    assert_eq!(content_type, "application/json");
                    let decoded: serde_json::Value = serde_json::from_slice(wire.as_slice())
                        .expect("OAuth JSON should round-trip");
                    assert_eq!(decoded["sdp"], sdp);
                    assert_eq!(decoded["session"], session);
                    assert_eq!(decoded["session"]["model"], "provider-live-model");
                    assert_eq!(
                        decoded["session"]["future_capability"],
                        json!({"enabled": true})
                    );
                }
            }
        }
    }

    #[test]
    fn live_call_request_headers_replace_stale_body_and_encoding_metadata() {
        let mut headers = BTreeMap::from([
            ("Content-Type".to_string(), "stale".to_string()),
            ("CONTENT-LENGTH".to_string(), "42".to_string()),
            ("Content-Encoding".to_string(), "gzip".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
            ("ACCEPT-ENCODING".to_string(), "br, gzip".to_string()),
            ("x-future".to_string(), "opaque".to_string()),
        ]);
        prepare_live_call_request_headers(&mut headers, "multipart/form-data; boundary=live-test");

        assert_eq!(
            header_value(&headers, "content-type"),
            Some("multipart/form-data; boundary=live-test")
        );
        assert_eq!(header_value(&headers, "accept"), Some("application/sdp"));
        assert_eq!(header_value(&headers, "accept-encoding"), Some("identity"));
        assert_eq!(header_value(&headers, "content-length"), None);
        assert_eq!(header_value(&headers, "content-encoding"), None);
        assert_eq!(headers.get("x-future").map(String::as_str), Some("opaque"));
    }

    #[test]
    fn live_response_headers_never_expose_upstream_location_or_cookies() {
        let headers = BTreeMap::from([
            (
                "Location".to_string(),
                "https://upstream/v1/live/secret".to_string(),
            ),
            ("SET-COOKIE".to_string(), "session=secret".to_string()),
            ("Set-Cookie2".to_string(), "legacy=secret".to_string()),
            ("Content-Length".to_string(), "128".to_string()),
            ("Content-Encoding".to_string(), "gzip".to_string()),
            ("x-future".to_string(), "opaque".to_string()),
        ]);

        let sanitized = sanitized_live_response_headers(&headers, true);
        assert_eq!(header_value(&sanitized, "location"), None);
        assert_eq!(header_value(&sanitized, "set-cookie"), None);
        assert_eq!(header_value(&sanitized, "set-cookie2"), None);
        assert_eq!(header_value(&sanitized, "content-length"), Some("128"));
        assert_eq!(header_value(&sanitized, "content-encoding"), Some("gzip"));
        assert_eq!(header_value(&sanitized, "x-future"), Some("opaque"));
    }

    #[test]
    fn rebuilt_live_response_body_drops_stale_length_and_encoding() {
        let headers = BTreeMap::from([
            ("content-length".to_string(), "128".to_string()),
            ("content-encoding".to_string(), "gzip".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ]);

        let sanitized = sanitized_live_response_headers(&headers, false);
        assert_eq!(header_value(&sanitized, "content-length"), None);
        assert_eq!(header_value(&sanitized, "content-encoding"), None);
        assert_eq!(
            header_value(&sanitized, "content-type"),
            Some("application/json")
        );
    }

    #[test]
    fn live_http_handler_future_stays_stack_bounded() {
        let state = AppState::new().expect("gateway state should build");
        let request_context = GatewayPublicRequestContext {
            trace_id: "trace-live-future-size".to_string(),
            request_method: http::Method::POST,
            request_path: "/v1/live".to_string(),
            request_query_string: None,
            request_content_type: None,
            host_header: None,
            control_decision: None,
        };
        let (parts, _) = http::Request::builder()
            .method(http::Method::POST)
            .uri("/v1/live")
            .body(())
            .expect("request should build")
            .into_parts();
        let remote_addr = "127.0.0.1:65002"
            .parse()
            .expect("remote address should parse");
        let future = maybe_handle_live_http(&state, &request_context, &parts, None, &remote_addr);
        let future_size = std::mem::size_of_val(&future);
        assert!(
            future_size <= 4 * 1024,
            "Live HTTP handler future grew to {future_size} bytes"
        );
    }

    #[tokio::test]
    async fn ordinary_openai_realtime_call_is_not_intercepted_by_codex_live() {
        let decision = GatewayControlDecision::synthetic(
            "/v1/realtime/calls",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("realtime".to_string()),
            Some("openai:realtime".to_string()),
        );
        let request_context = GatewayPublicRequestContext {
            trace_id: "trace-ordinary-realtime-call".to_string(),
            request_method: http::Method::POST,
            request_path: "/v1/realtime/calls".to_string(),
            request_query_string: None,
            request_content_type: Some("application/sdp".to_string()),
            host_header: None,
            control_decision: Some(decision),
        };
        let (parts, _) = http::Request::builder()
            .method(http::Method::POST)
            .uri("/v1/realtime/calls")
            .body(())
            .expect("request should build")
            .into_parts();

        let response = maybe_handle_live_http(
            &AppState::new().expect("gateway state should build"),
            &request_context,
            &parts,
            None,
            &"127.0.0.1:65003".parse().unwrap(),
        )
        .await
        .expect("ordinary Realtime hook check should succeed");

        assert!(response.is_none());
    }

    #[tokio::test]
    async fn codex_realtime_call_remains_on_the_live_handler() {
        let mut decision = GatewayControlDecision::synthetic(
            "/v1/realtime/calls",
            Some("ai_public".to_string()),
            Some("codex".to_string()),
            Some("live".to_string()),
            Some("codex:live".to_string()),
        );
        decision.auth_context = Some(GatewayControlAuthContext {
            user_id: "user-codex-realtime".to_string(),
            api_key_id: "key-codex-realtime".to_string(),
            username: Some("codex-realtime".to_string()),
            api_key_name: Some("codex-realtime".to_string()),
            balance_remaining: None,
            access_allowed: true,
            user_rate_limit: None,
            sell_rate_multiplier: 1.0,
            api_key_rate_limit: None,
            api_key_is_standalone: true,
            admin_bypass_limits: false,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
            billing_group_id: None,
            billing_group_name: None,
        });
        let request_context = GatewayPublicRequestContext {
            trace_id: "trace-codex-realtime-call".to_string(),
            request_method: http::Method::POST,
            request_path: "/v1/realtime/calls".to_string(),
            request_query_string: Some("intent=quicksilver&architecture=avas".to_string()),
            request_content_type: Some("multipart/form-data".to_string()),
            host_header: None,
            control_decision: Some(decision),
        };
        let (parts, _) = http::Request::builder()
            .method(http::Method::POST)
            .uri("/v1/realtime/calls?intent=quicksilver&architecture=avas")
            .body(())
            .expect("request should build")
            .into_parts();

        let response = maybe_handle_live_http(
            &AppState::new().expect("gateway state should build"),
            &request_context,
            &parts,
            None,
            &"127.0.0.1:65004".parse().unwrap(),
        )
        .await
        .expect("Codex Realtime hook check should succeed")
        .expect("Codex Realtime call must stay on the Live handler");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn finite_balance_post_live_fails_before_parsing_or_upstream_execution() {
        let mut decision = GatewayControlDecision::synthetic(
            "/v1/live",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("codex_live".to_string()),
            Some("openai:responses".to_string()),
        );
        decision.auth_context = Some(GatewayControlAuthContext {
            user_id: "user-finite".to_string(),
            api_key_id: "key-finite".to_string(),
            username: Some("finite".to_string()),
            api_key_name: Some("finite".to_string()),
            balance_remaining: Some(1.25),
            access_allowed: true,
            user_rate_limit: None,
            sell_rate_multiplier: 1.0,
            api_key_rate_limit: None,
            api_key_is_standalone: false,
            admin_bypass_limits: false,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
            billing_group_id: None,
            billing_group_name: None,
        });
        let request_context = GatewayPublicRequestContext {
            trace_id: "trace-live-finite".to_string(),
            request_method: http::Method::POST,
            request_path: "/v1/live".to_string(),
            request_query_string: None,
            request_content_type: None,
            host_header: None,
            control_decision: Some(decision),
        };
        let (parts, _) = http::Request::builder()
            .method(http::Method::POST)
            .uri("/v1/live")
            .body(())
            .unwrap()
            .into_parts();
        let response = maybe_handle_live_http(
            &AppState::new().expect("gateway state should build"),
            &request_context,
            &parts,
            None,
            &"127.0.0.1:65000".parse().unwrap(),
        )
        .await
        .unwrap()
        .expect("Live HTTP route must produce a local rejection");
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(body.as_ref()).contains("finite-balance"));
    }

    #[tokio::test]
    async fn post_live_without_a_candidate_returns_service_unavailable_and_clears_diagnostic() {
        let log_buffer = SharedLogBuffer::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_current_span(false)
                .with_span_list(false)
                .with_writer(log_buffer.clone())
                .with_filter(LevelFilter::WARN),
        );
        let dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&dispatch);
        let mut decision = GatewayControlDecision::synthetic(
            "/v1/live",
            Some("ai_public".to_string()),
            Some("codex".to_string()),
            Some("live".to_string()),
            Some("codex:live".to_string()),
        );
        decision.auth_context = Some(GatewayControlAuthContext {
            user_id: "user-live-unmapped".to_string(),
            api_key_id: "key-live-unmapped".to_string(),
            username: Some("unmapped".to_string()),
            api_key_name: Some("unmapped".to_string()),
            balance_remaining: None,
            access_allowed: true,
            user_rate_limit: None,
            sell_rate_multiplier: 1.0,
            api_key_rate_limit: None,
            api_key_is_standalone: true,
            admin_bypass_limits: false,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
            billing_group_id: None,
            billing_group_name: None,
        });
        let request_context = GatewayPublicRequestContext {
            trace_id: "trace-live-unmapped".to_string(),
            request_method: http::Method::POST,
            request_path: "/v1/live".to_string(),
            request_query_string: None,
            request_content_type: Some("multipart/form-data".to_string()),
            host_header: None,
            control_decision: Some(decision),
        };
        let (content_type, body) = build_live_multipart(
            "v=0\r\no=unmapped-live-offer",
            &json!({"model": "gpt-live-unmapped"}),
        );
        let (parts, _) = http::Request::builder()
            .method(http::Method::POST)
            .uri("/v1/live")
            .header(http::header::CONTENT_TYPE, content_type)
            .body(())
            .unwrap()
            .into_parts();
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let state = AppState::new()
            .expect("gateway state should build")
            .with_usage_data_repository_for_tests(Arc::clone(&usage_repository))
            .with_usage_runtime_for_tests(crate::usage::UsageRuntimeConfig {
                enabled: true,
                ..crate::usage::UsageRuntimeConfig::default()
            });
        state.set_local_execution_runtime_miss_diagnostic(
            request_context.trace_id.as_str(),
            crate::LocalExecutionRuntimeMissDiagnostic {
                reason: "test_sentinel".to_string(),
                ..Default::default()
            },
        );

        let response = maybe_handle_live_http(
            &state,
            &request_context,
            &parts,
            Some(&Bytes::from(body)),
            &"127.0.0.1:65001".parse().unwrap(),
        )
        .await
        .unwrap()
        .expect("Live HTTP route must reject an unmapped model locally");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        assert_eq!(
            response
                .headers()
                .get(crate::constants::TRACE_ID_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some(request_context.trace_id.as_str())
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(body.as_ref())
            .contains("No eligible Codex Live provider mapping is available"));
        assert!(state
            .take_local_execution_runtime_miss_diagnostic(request_context.trace_id.as_str())
            .is_none());

        let usage = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some(usage) = usage_repository
                    .find_by_request_id(request_context.trace_id.as_str())
                    .await
                    .expect("Live preflight usage read should succeed")
                {
                    break usage;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Live preflight rejection should persist a usage row before timeout");
        assert_eq!(usage.status, "failed");
        assert_eq!(usage.billing_status, "void");
        assert_eq!(usage.status_code, Some(503));
        assert_eq!(usage.request_type.as_deref(), Some("live"));
        assert_eq!(usage.api_format.as_deref(), Some("codex:live"));
        assert_eq!(usage.model, "gpt-live-unmapped");
        assert!(!usage.is_stream);
        assert!(!usage.is_websocket());
        assert_eq!(usage.websocket_transport(), None);
        assert!(!usage.usage_available());
        assert!(!usage.usage_pricing_available());
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.total_tokens, 0);
        assert_eq!(usage.total_cost_usd, 0.0);
        assert_eq!(usage.actual_total_cost_usd, 0.0);
        assert!(usage.request_headers.is_none());
        assert!(usage.request_body.is_none());
        assert!(usage.provider_request_headers.is_none());
        assert!(usage.provider_request_body.is_none());
        assert!(!serde_json::to_string(&usage)
            .expect("usage should serialize")
            .contains("unmapped-live-offer"));

        let logs = log_buffer.lines();
        let unavailable = logs
            .iter()
            .find(|entry| entry["event_name"] == "codex_live_call_candidate_unavailable")
            .expect("candidate miss should emit a dedicated structured log");
        assert_eq!(unavailable["status_code"], 503);
        assert_eq!(unavailable["client_model"], "gpt-live-unmapped");
        assert_eq!(unavailable["user_id"], "user-live-unmapped");
        assert_eq!(unavailable["api_key_id"], "key-live-unmapped");
        assert_eq!(unavailable["transport"], "webrtc");
        assert_eq!(unavailable["mode"], "call_create");
        assert!(!serde_json::to_string(&logs)
            .expect("logs should serialize")
            .contains("unmapped-live-offer"));
    }
}
