//! Candidate planning and provider request shaping for Codex Live.
//!
//! Live has its own endpoint and permission surface. Candidate selection,
//! model aliases and transport policy are shared with the ordinary scheduler,
//! but Responses body normalization and its WebSocket state machine never see
//! a Live protocol frame.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::http::header::{AUTHORIZATION, CONNECTION, CONTENT_TYPE, COOKIE, UPGRADE};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method};
use serde_json::json;
use sha2::{Digest, Sha256};
use url::{form_urlencoded, Url};

use crate::ai_serving::transport::ProviderOutboundRequestContext;
use crate::ai_serving::{
    build_standard_stream_plan_from_decision,
    maybe_build_pinned_stream_local_same_format_provider_decision_payload, AiExecutionDecision,
    AiStreamAttempt, ResponsesWebSocketPinnedCandidate,
};
use crate::control::GatewayControlDecision;
use crate::headers::request_origin_from_headers_and_remote_addr;
use crate::privacy::RedactionSessionSlot;
use crate::state::LocalExecutionRuntimeMissDiagnostic;
use crate::{AppState, GatewayError};

use super::protocol::{validate_model, LiveProtocolError, LiveRouteDialect};

pub(super) const LEGACY_LIVE_ALPHA_HEADER_VALUE: &str = "quicksilver=v2";
pub(super) const REALTIME_LIVE_ALPHA_HEADER_VALUE: &str = "quicksilver=v1";
const CHATGPT_ACCOUNT_ID_HEADER: &str = "chatgpt-account-id";
const CHATGPT_FEDRAMP_HEADER: &str = "x-openai-fedramp";
const CHATGPT_SESSION_ID_HEADER: &str = "x-session-id";
const OFFICIAL_CHATGPT_HOST: &str = "chatgpt.com";
const LIVE_ROUTING_FINGERPRINT_DOMAIN: &[u8] = b"aether-codex-live-routing-v1";
const MAX_LIVE_MODEL_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum LiveAuthMode {
    ApiKey,
    ChatGptOauth,
}

#[derive(Debug)]
pub(super) struct PlannedLiveCandidate {
    pub(super) execution: AiExecutionDecision,
    pub(super) pinned_candidate: ResponsesWebSocketPinnedCandidate,
    pub(super) provider_outbound_context: ProviderOutboundRequestContext,
    pub(super) client_model: String,
    pub(super) provider_model: String,
    pub(super) auth_mode: LiveAuthMode,
    /// Domain-separated digest of the stable upstream auth/account/origin
    /// identity. It deliberately excludes bearer tokens so an OAuth refresh
    /// does not invalidate an in-flight WebRTC call.
    pub(super) routing_fingerprint: String,
}

#[derive(Debug)]
pub(super) struct LiveCandidatePlanningOutcome {
    pub(super) candidate: Option<PlannedLiveCandidate>,
    pub(super) runtime_miss: Option<LocalExecutionRuntimeMissDiagnostic>,
}

/// Owns the request-scoped runtime-miss entry while Live planning is in
/// progress. The ordinary HTTP proxy consumes this entry after planning, but
/// Live has three specialized call sites and pinned sideband planning can be
/// cancelled when its attachment lease is lost. Clearing from `Drop` makes
/// that cancellation path safe as well as ordinary errors and early returns.
struct LiveRuntimeMissDiagnosticGuard<'a> {
    state: &'a AppState,
    trace_id: &'a str,
    armed: bool,
}

impl<'a> LiveRuntimeMissDiagnosticGuard<'a> {
    fn new(state: &'a AppState, trace_id: &'a str) -> Self {
        Self {
            state,
            trace_id,
            armed: true,
        }
    }

    fn take(mut self) -> Option<LocalExecutionRuntimeMissDiagnostic> {
        let diagnostic = self
            .state
            .take_local_execution_runtime_miss_diagnostic(self.trace_id);
        self.armed = false;
        diagnostic
    }
}

impl Drop for LiveRuntimeMissDiagnosticGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.state
                .clear_local_execution_runtime_miss_diagnostic(self.trace_id);
        }
    }
}

/// Cancellation-safe owner for the scheduler's distributed pool-key lease.
/// Live does not enter the ordinary HTTP/Responses attempt lifecycle, so it
/// must hold and release the lease explicitly for the call or socket lifetime.
pub(super) struct LivePoolLeaseGuard {
    state: AppState,
    report_context: Option<serde_json::Value>,
    renewal_task: Option<tokio::task::JoinHandle<()>>,
    healthy: Arc<AtomicBool>,
    armed: bool,
}

impl LivePoolLeaseGuard {
    pub(super) fn new(state: &AppState, candidate: &PlannedLiveCandidate) -> Self {
        let report_context = candidate.execution.report_context.clone();
        let lease = crate::orchestration::local_execution_candidate_metadata_from_report_context(
            report_context.as_ref(),
        )
        .pool_key_lease;
        let healthy = Arc::new(AtomicBool::new(true));
        let renewal_task = lease.map(|lease| {
            let runtime_state = Arc::clone(&state.runtime_state);
            let healthy = Arc::clone(&healthy);
            tokio::spawn(async move {
                let ttl = Duration::from_millis(lease.ttl_ms);
                let interval = Duration::from_millis((lease.ttl_ms / 3).max(1));
                loop {
                    tokio::time::sleep(interval).await;
                    match runtime_state.lock_renew(&lease, ttl).await {
                        Ok(true) => {}
                        Ok(false) | Err(_) => {
                            healthy.store(false, Ordering::Release);
                            return;
                        }
                    }
                }
            })
        });
        Self {
            state: state.clone(),
            report_context,
            renewal_task,
            healthy,
            armed: true,
        }
    }

    pub(super) fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }

    pub(super) async fn release(mut self) {
        if let Some(task) = self.renewal_task.take() {
            task.abort();
        }
        crate::orchestration::release_pool_key_lease_from_report_context(
            &self.state,
            self.report_context.as_ref(),
        )
        .await;
        self.armed = false;
    }
}

impl Drop for LivePoolLeaseGuard {
    fn drop(&mut self) {
        if let Some(task) = self.renewal_task.take() {
            task.abort();
        }
        if !self.armed {
            return;
        }
        let state = self.state.clone();
        let report_context = self.report_context.take();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                crate::orchestration::release_pool_key_lease_from_report_context(
                    &state,
                    report_context.as_ref(),
                )
                .await;
            });
        }
    }
}

pub(super) async fn plan_live_candidate(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    headers: &HeaderMap,
    remote_addr: &SocketAddr,
    client_model: &str,
    dialect: LiveRouteDialect,
    pinned_candidate: Option<&ResponsesWebSocketPinnedCandidate>,
    provider_outbound_context: Option<&ProviderOutboundRequestContext>,
) -> Result<LiveCandidatePlanningOutcome, GatewayError> {
    let diagnostic_guard = LiveRuntimeMissDiagnosticGuard::new(state, trace_id);
    let candidate = plan_live_candidate_inner(
        state,
        trace_id,
        decision,
        headers,
        remote_addr,
        client_model,
        dialect,
        pinned_candidate,
        provider_outbound_context,
    )
    .await?;
    Ok(LiveCandidatePlanningOutcome {
        candidate,
        runtime_miss: diagnostic_guard.take(),
    })
}

async fn plan_live_candidate_inner(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    headers: &HeaderMap,
    remote_addr: &SocketAddr,
    client_model: &str,
    dialect: LiveRouteDialect,
    pinned_candidate: Option<&ResponsesWebSocketPinnedCandidate>,
    provider_outbound_context: Option<&ProviderOutboundRequestContext>,
) -> Result<Option<PlannedLiveCandidate>, GatewayError> {
    if validate_model(client_model).is_err() || client_model.len() > MAX_LIVE_MODEL_BYTES {
        return Ok(None);
    }
    let mut parts = build_live_planning_parts(headers, remote_addr);
    let body = json!({"model": client_model, "input": []});
    if let Some(context) = provider_outbound_context {
        crate::ai_serving::codex_context::restore_codex_logical_turn_context(&mut parts, context);
    } else {
        crate::ai_serving::codex_context::install_codex_fingerprint_context_slot(&mut parts);
    }
    let execution = maybe_build_pinned_stream_local_same_format_provider_decision_payload(
        state,
        &parts,
        trace_id,
        decision,
        &body,
        crate::ai_serving::CODEX_LIVE_STREAM_PLAN_KIND,
        pinned_candidate
            .map(|pinned| (pinned.provider_id(), pinned.endpoint_id(), pinned.key_id())),
    )
    .await?;
    let Some(mut execution) = execution else {
        return Ok(None);
    };
    if execution
        .provider_api_format
        .as_deref()
        .map(crate::ai_serving::normalize_api_format_alias)
        .as_deref()
        != Some("codex:live")
    {
        crate::orchestration::release_pool_key_lease_from_report_context(
            state,
            execution.report_context.as_ref(),
        )
        .await;
        return Ok(None);
    }
    let Some(effective_auth_type) = execution
        .report_context
        .as_ref()
        .and_then(|context| context.get("upstream_credential_mode"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
    else {
        crate::orchestration::release_pool_key_lease_from_report_context(
            state,
            execution.report_context.as_ref(),
        )
        .await;
        return Ok(None);
    };
    let provider_type = execution
        .provider_type
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if !matches!(
        provider_type.to_ascii_lowercase().as_str(),
        "codex" | "openai" | "custom"
    ) {
        crate::orchestration::release_pool_key_lease_from_report_context(
            state,
            execution.report_context.as_ref(),
        )
        .await;
        return Ok(None);
    }
    let Some(pinned_candidate) = ResponsesWebSocketPinnedCandidate::from_decision(&execution)
    else {
        crate::orchestration::release_pool_key_lease_from_report_context(
            state,
            execution.report_context.as_ref(),
        )
        .await;
        return Ok(None);
    };
    let provider_model = execution
        .mapped_model
        .as_deref()
        .or(execution.model_name.as_deref())
        .map(str::trim)
        .filter(|model| validate_model(model).is_ok())
        .map(str::to_string);
    let Some(provider_model) = provider_model else {
        crate::orchestration::release_pool_key_lease_from_report_context(
            state,
            execution.report_context.as_ref(),
        )
        .await;
        return Ok(None);
    };
    let Some(auth_mode) = live_auth_mode(provider_type, effective_auth_type.as_str()) else {
        crate::orchestration::release_pool_key_lease_from_report_context(
            state,
            execution.report_context.as_ref(),
        )
        .await;
        return Ok(None);
    };
    apply_live_headers(&mut execution.provider_request_headers, trace_id, dialect);
    let routing_fingerprint =
        match live_routing_fingerprint(&execution, effective_auth_type.as_str(), auth_mode) {
            Ok(fingerprint) => fingerprint,
            Err(_) => {
                crate::orchestration::release_pool_key_lease_from_report_context(
                    state,
                    execution.report_context.as_ref(),
                )
                .await;
                return Ok(None);
            }
        };
    Ok(Some(PlannedLiveCandidate {
        execution,
        pinned_candidate,
        provider_outbound_context:
            crate::ai_serving::codex_context::resolve_codex_fingerprint_context(&parts, &body),
        client_model: client_model.to_string(),
        provider_model,
        auth_mode,
        routing_fingerprint,
    }))
}

pub(super) fn direct_live_websocket_url(
    candidate: &PlannedLiveCandidate,
) -> Result<String, LiveProtocolError> {
    direct_live_websocket_url_for_dialect(candidate, LiveRouteDialect::LegacyLive)
}

pub(super) fn direct_live_websocket_url_for_dialect(
    candidate: &PlannedLiveCandidate,
    dialect: LiveRouteDialect,
) -> Result<String, LiveProtocolError> {
    if candidate.auth_mode == LiveAuthMode::ChatGptOauth {
        return Err(LiveProtocolError::OauthDirectWebSocketUnsupported);
    }
    match dialect {
        LiveRouteDialect::LegacyLive => replace_live_suffix(
            candidate.execution.upstream_url.as_deref(),
            &["live"],
            Some(("model", candidate.provider_model.as_str())),
        ),
        LiveRouteDialect::Realtime => {
            // The endpoint is planned using the canonical codex:live `/live`
            // URL.  Direct Codex WebSocket clients use `/realtime` instead,
            // with the quicksilver intent and mapped provider model in the
            // query string.  Keep endpoint-owned query parameters while
            // replacing any duplicate transport selectors.
            let raw = replace_live_suffix(
                candidate.execution.upstream_url.as_deref(),
                &["realtime"],
                None,
            )?;
            let mut url =
                Url::parse(raw.as_str()).map_err(|_| LiveProtocolError::InvalidUpstreamUrl)?;
            replace_url_query_pair(&mut url, "intent", "quicksilver");
            replace_url_query_pair(&mut url, "model", candidate.provider_model.as_str());
            Ok(url.to_string())
        }
        LiveRouteDialect::RealtimeV2 => {
            // Realtime v2 is the current Codex default. It intentionally
            // omits the v1 quicksilver intent marker; remove any stale or
            // duplicate endpoint-owned intent before adding the mapped model.
            let raw = replace_live_suffix(
                candidate.execution.upstream_url.as_deref(),
                &["realtime"],
                None,
            )?;
            let mut url =
                Url::parse(raw.as_str()).map_err(|_| LiveProtocolError::InvalidUpstreamUrl)?;
            remove_url_query_pair(&mut url, "intent");
            replace_url_query_pair(&mut url, "model", candidate.provider_model.as_str());
            Ok(url.to_string())
        }
    }
}

pub(super) fn live_call_url(
    candidate: &PlannedLiveCandidate,
    dialect: LiveRouteDialect,
) -> Result<String, LiveProtocolError> {
    if dialect == LiveRouteDialect::RealtimeV2 {
        // Codex pins AVAS/WebRTC call creation to Realtime v1. Realtime v2 is
        // a direct WebSocket transport and has no call-create exchange.
        return Err(LiveProtocolError::InvalidCallLocation);
    }
    match candidate.auth_mode {
        LiveAuthMode::ApiKey => {
            let raw = replace_live_suffix(
                candidate.execution.upstream_url.as_deref(),
                match dialect {
                    LiveRouteDialect::LegacyLive => &["live"],
                    LiveRouteDialect::Realtime => &["realtime", "calls"],
                    LiveRouteDialect::RealtimeV2 => unreachable!("handled above"),
                },
                None,
            )?;
            if dialect == LiveRouteDialect::LegacyLive {
                return Ok(raw);
            }

            // Current Codex creates AVAS calls with these two transport
            // selectors. The scheduler intentionally plans against a synthetic
            // body and fixed URI, so reconstruct them here rather than trusting
            // arbitrary downstream query parameters. Replacement is
            // case-insensitive and preserves endpoint-owned query parameters.
            let mut url =
                Url::parse(raw.as_str()).map_err(|_| LiveProtocolError::InvalidUpstreamUrl)?;
            replace_url_query_pair(&mut url, "intent", "quicksilver");
            replace_url_query_pair(&mut url, "architecture", "avas");
            Ok(url.to_string())
        }
        LiveAuthMode::ChatGptOauth => {
            let source =
                validated_official_chatgpt_url(candidate.execution.upstream_url.as_deref())?;
            replace_live_suffix(
                Some(source.as_str()),
                &["realtime", "calls"],
                Some(("intent", "quicksilver")),
            )
        }
        .and_then(|raw| {
            let mut url =
                Url::parse(raw.as_str()).map_err(|_| LiveProtocolError::InvalidUpstreamUrl)?;
            replace_url_query_pair(&mut url, "architecture", "avas");
            Ok(url.to_string())
        }),
    }
}

pub(super) fn live_sideband_url(
    candidate: &PlannedLiveCandidate,
    call_id: &str,
    dialect: LiveRouteDialect,
) -> Result<String, LiveProtocolError> {
    super::protocol::validate_call_id(call_id)?;
    match candidate.auth_mode {
        LiveAuthMode::ApiKey => match dialect {
            LiveRouteDialect::LegacyLive => replace_live_suffix(
                candidate.execution.upstream_url.as_deref(),
                &["live", call_id],
                None,
            ),
            LiveRouteDialect::Realtime => {
                // AVAS sideband sockets use the same Codex quicksilver
                // discriminator as the call-creation and direct websocket
                // paths.  Keep endpoint-owned query parameters, but replace
                // any stale/duplicate transport selectors so the upstream
                // receives exactly one `intent=quicksilver` and `call_id`.
                let raw = replace_live_suffix(
                    candidate.execution.upstream_url.as_deref(),
                    &["realtime"],
                    None,
                )?;
                let mut url =
                    Url::parse(raw.as_str()).map_err(|_| LiveProtocolError::InvalidUpstreamUrl)?;
                replace_url_query_pair(&mut url, "intent", "quicksilver");
                replace_url_query_pair(&mut url, "call_id", call_id);
                Ok(url.to_string())
            }
            LiveRouteDialect::RealtimeV2 => {
                // V2 sideband joins (if used by a future client) carry only
                // the call id; unlike V1 they must not include quicksilver.
                let raw = replace_live_suffix(
                    candidate.execution.upstream_url.as_deref(),
                    &["realtime"],
                    None,
                )?;
                let mut url =
                    Url::parse(raw.as_str()).map_err(|_| LiveProtocolError::InvalidUpstreamUrl)?;
                remove_url_query_pair(&mut url, "intent");
                replace_url_query_pair(&mut url, "call_id", call_id);
                Ok(url.to_string())
            }
        },
        // The Codex ChatGPT call creation endpoint returns an OpenAI Realtime
        // call ID. Current Codex connects its sideband to this API origin even
        // when call creation used the ChatGPT OAuth backend.
        LiveAuthMode::ChatGptOauth => {
            validated_official_chatgpt_url(candidate.execution.upstream_url.as_deref())?;
            let mut url = Url::parse("https://api.openai.com/v1/live")
                .map_err(|_| LiveProtocolError::InvalidUpstreamUrl)?;
            // The sideband intentionally crosses from the official ChatGPT
            // backend to the OpenAI API origin. Do not copy backend query
            // parameters across that boundary; only the transport selectors
            // constructed below are valid on the sideband origin.
            match dialect {
                LiveRouteDialect::LegacyLive => {
                    url.path_segments_mut()
                        .map_err(|_| LiveProtocolError::InvalidUpstreamUrl)?
                        .push(call_id);
                }
                LiveRouteDialect::Realtime => {
                    url.set_path("/v1/realtime");
                    replace_url_query_pair(&mut url, "intent", "quicksilver");
                    replace_url_query_pair(&mut url, "call_id", call_id);
                }
                LiveRouteDialect::RealtimeV2 => {
                    url.set_path("/v1/realtime");
                    remove_url_query_pair(&mut url, "intent");
                    replace_url_query_pair(&mut url, "call_id", call_id);
                }
            }
            Ok(url.to_string())
        }
    }
}

pub(super) fn apply_live_headers(
    headers: &mut std::collections::BTreeMap<String, String>,
    seed: &str,
    dialect: LiveRouteDialect,
) {
    let alpha = match dialect {
        LiveRouteDialect::LegacyLive => LEGACY_LIVE_ALPHA_HEADER_VALUE,
        LiveRouteDialect::Realtime => REALTIME_LIVE_ALPHA_HEADER_VALUE,
        LiveRouteDialect::RealtimeV2 => "",
    };
    if dialect == LiveRouteDialect::RealtimeV2 {
        headers.retain(|name, _| !name.eq_ignore_ascii_case("openai-alpha"));
    } else {
        replace_header(headers, "openai-alpha", alpha);
    }
    let session_id = find_header(headers, "x-session-id")
        .or_else(|| find_header(headers, "thread-id"))
        .or_else(|| find_header(headers, "session-id"))
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| seed.to_string());
    replace_header(headers, "x-session-id", session_id.as_str());
}

pub(super) fn build_live_stream_admission_attempt(
    candidate: &PlannedLiveCandidate,
    headers: &HeaderMap,
    remote_addr: &SocketAddr,
    upstream_url: String,
) -> Result<Option<AiStreamAttempt>, GatewayError> {
    let mut parts = build_live_planning_parts(headers, remote_addr);
    crate::ai_serving::codex_context::restore_codex_logical_turn_context(
        &mut parts,
        &candidate.provider_outbound_context,
    );
    let body = json!({"model": candidate.client_model.as_str(), "input": []});
    let mut execution = candidate.execution.clone();
    execution.upstream_url = Some(upstream_url);
    execution.upstream_is_stream = true;
    build_standard_stream_plan_from_decision(&parts, &body, execution, false)
}

fn live_auth_mode(provider_type: &str, effective_auth_type: &str) -> Option<LiveAuthMode> {
    let provider_type = provider_type.trim();
    match effective_auth_type.trim().to_ascii_lowercase().as_str() {
        "api_key" | "bearer" => Some(LiveAuthMode::ApiKey),
        "oauth" if provider_type.eq_ignore_ascii_case("codex") => Some(LiveAuthMode::ChatGptOauth),
        _ => None,
    }
}

fn live_routing_fingerprint(
    decision: &AiExecutionDecision,
    effective_auth_type: &str,
    auth_mode: LiveAuthMode,
) -> Result<String, LiveProtocolError> {
    let raw_url = decision
        .upstream_url
        .as_deref()
        .ok_or(LiveProtocolError::MissingUpstreamUrl)?;
    let url = Url::parse(raw_url).map_err(|_| LiveProtocolError::InvalidUpstreamUrl)?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(LiveProtocolError::InvalidUpstreamUrl);
    }
    let path = url.path().trim_end_matches('/');
    let path_family = path
        .strip_suffix("/live")
        .ok_or(LiveProtocolError::InvalidUpstreamUrl)?;
    if auth_mode == LiveAuthMode::ChatGptOauth {
        validated_official_chatgpt_url(Some(raw_url))?;
    }

    let mut hasher = Sha256::new();
    hasher.update(LIVE_ROUTING_FINGERPRINT_DOMAIN);
    hasher.update([0]);
    for value in [
        effective_auth_type.trim(),
        url.scheme(),
        url.host_str().unwrap_or_default(),
        path_family,
    ] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    hasher.update(
        url.port_or_known_default()
            .unwrap_or_default()
            .to_be_bytes(),
    );
    hasher.update([0]);
    hasher.update(canonical_live_route_query(&url).as_bytes());
    hasher.update([0]);

    let session_id = find_header(
        &decision.provider_request_headers,
        CHATGPT_SESSION_ID_HEADER,
    )
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .ok_or(LiveProtocolError::InvalidUpstreamUrl)?;
    hasher.update(session_id.as_bytes());
    hasher.update([0]);

    if auth_mode == LiveAuthMode::ChatGptOauth {
        for (name, required) in [
            (CHATGPT_ACCOUNT_ID_HEADER, true),
            (CHATGPT_FEDRAMP_HEADER, false),
        ] {
            let value = find_header(&decision.provider_request_headers, name)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if required && value.is_none() {
                return Err(LiveProtocolError::InvalidUpstreamUrl);
            }
            hasher.update(value.unwrap_or_default().as_bytes());
            hasher.update([0]);
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn validated_official_chatgpt_url(raw: Option<&str>) -> Result<Url, LiveProtocolError> {
    let url = Url::parse(raw.ok_or(LiveProtocolError::MissingUpstreamUrl)?)
        .map_err(|_| LiveProtocolError::InvalidUpstreamUrl)?;
    let official = url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case(OFFICIAL_CHATGPT_HOST))
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && url.path().trim_end_matches('/').strip_suffix("/live") == Some("/backend-api/codex");
    if !official {
        return Err(LiveProtocolError::OauthUpstreamUnsupported);
    }
    Ok(url)
}

fn replace_live_suffix(
    raw: Option<&str>,
    suffix: &[&str],
    query: Option<(&str, &str)>,
) -> Result<String, LiveProtocolError> {
    let mut url = Url::parse(raw.ok_or(LiveProtocolError::MissingUpstreamUrl)?)
        .map_err(|_| LiveProtocolError::InvalidUpstreamUrl)?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(LiveProtocolError::InvalidUpstreamUrl);
    }
    if url.path_segments().and_then(Iterator::last) != Some("live") {
        return Err(LiveProtocolError::InvalidUpstreamUrl);
    }
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| LiveProtocolError::InvalidUpstreamUrl)?;
        path.pop_if_empty();
        path.pop();
        for segment in suffix {
            path.push(segment);
        }
    }
    if let Some((name, value)) = query {
        replace_url_query_pair(&mut url, name, value);
    }
    Ok(url.to_string())
}

fn replace_url_query_pair(url: &mut Url, name: &str, value: &str) {
    remove_url_query_pair(url, name);
    url.query_pairs_mut().append_pair(name, value);
}

fn remove_url_query_pair(url: &mut Url, name: &str) {
    let retained = url
        .query_pairs()
        .filter(|(candidate, _)| !candidate.eq_ignore_ascii_case(name))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    url.set_query(None);
    let mut query = url.query_pairs_mut();
    for (key, value) in retained {
        query.append_pair(key.as_str(), value.as_str());
    }
}

fn canonical_live_route_query(url: &Url) -> String {
    if url.query().is_none() {
        return String::new();
    }
    let mut pairs = url
        .query_pairs()
        .map(|(name, value)| {
            let value = if live_route_query_key_is_credential(name.as_ref()) {
                // Bind the authentication mechanism without pinning a rotating
                // credential value into the WebRTC call identity.
                String::new()
            } else {
                value.into_owned()
            };
            (name.into_owned(), value)
        })
        .collect::<Vec<_>>();
    pairs.sort_unstable();

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        serializer.append_pair(name.as_str(), value.as_str());
    }
    serializer.finish()
}

fn live_route_query_key_is_credential(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "key"
            | "api_key"
            | "api-key"
            | "x-api-key"
            | "x-goog-api-key"
            | "access_token"
            | "authorization"
            | "token"
            | "oauth_token"
            | "client_secret"
            | "secret_key"
            | "signature"
            | "sig"
    )
}

fn build_live_planning_parts(
    headers: &HeaderMap,
    remote_addr: &SocketAddr,
) -> http::request::Parts {
    let mut request = http::Request::builder()
        .method(Method::GET)
        .uri("/v1/live")
        .body(())
        .expect("the fixed Live planning request must be valid");
    *request.headers_mut() = sanitize_live_planning_headers(headers.clone());
    request
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    request
        .extensions_mut()
        .insert(request_origin_from_headers_and_remote_addr(
            headers,
            remote_addr,
        ));
    request
        .extensions_mut()
        .insert(RedactionSessionSlot::default());
    request.into_parts().0
}

fn sanitize_live_planning_headers(mut headers: HeaderMap) -> HeaderMap {
    let connection_names = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect::<Vec<_>>();
    for name in connection_names {
        headers.remove(name);
    }
    for name in [AUTHORIZATION, CONNECTION, COOKIE, UPGRADE] {
        headers.remove(name);
    }
    for name in [
        "api-key",
        "x-api-key",
        "x-goog-api-key",
        "proxy-authorization",
        "proxy-connection",
        "keep-alive",
    ] {
        headers.remove(name);
    }
    let websocket_headers = headers
        .keys()
        .filter(|name| name.as_str().starts_with("sec-websocket-"))
        .cloned()
        .collect::<Vec<_>>();
    for name in websocket_headers {
        headers.remove(name);
    }
    headers
}

fn find_header<'a>(
    headers: &'a std::collections::BTreeMap<String, String>,
    name: &str,
) -> Option<&'a str> {
    headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn replace_header(
    headers: &mut std::collections::BTreeMap<String, String>,
    name: &str,
    value: &str,
) {
    headers.retain(|candidate, _| !candidate.eq_ignore_ascii_case(name));
    headers.insert(name.to_string(), value.to_string());
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    use super::*;

    #[test]
    fn live_runtime_miss_guard_returns_and_removes_the_diagnostic() {
        let state = AppState::new().expect("gateway state should build");
        let trace_id = "trace-live-planner-diagnostic";
        let expected = LocalExecutionRuntimeMissDiagnostic {
            reason: "no_eligible_candidate".to_string(),
            candidate_count: Some(2),
            ..Default::default()
        };
        state.set_local_execution_runtime_miss_diagnostic(trace_id, expected.clone());

        let diagnostic = LiveRuntimeMissDiagnosticGuard::new(&state, trace_id).take();

        assert_eq!(diagnostic, Some(expected));
        assert!(state
            .take_local_execution_runtime_miss_diagnostic(trace_id)
            .is_none());
    }

    #[tokio::test]
    async fn cancelling_live_planning_clears_the_runtime_miss_diagnostic() {
        let state = AppState::new().expect("gateway state should build");
        let trace_id = "trace-live-planner-cancelled";
        state.set_local_execution_runtime_miss_diagnostic(
            trace_id,
            LocalExecutionRuntimeMissDiagnostic {
                reason: "planning_started".to_string(),
                ..Default::default()
            },
        );

        let task_state = state.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _guard = LiveRuntimeMissDiagnosticGuard::new(&task_state, trace_id);
            let _ = ready_tx.send(());
            std::future::pending::<()>().await;
        });
        ready_rx
            .await
            .expect("planning task should install its diagnostic guard");
        task.abort();
        let _ = task.await;

        assert!(state
            .take_local_execution_runtime_miss_diagnostic(trace_id)
            .is_none());
    }

    fn candidate(url: &str, auth_mode: LiveAuthMode) -> PlannedLiveCandidate {
        let execution: AiExecutionDecision = serde_json::from_value(json!({
            "action": "stream",
            "provider_id": "provider-1",
            "endpoint_id": "endpoint-1",
            "key_id": "key-1",
            "upstream_url": url,
            "provider_type": "codex",
            "provider_request_headers": {"authorization": "Bearer secret"}
        }))
        .expect("decision should deserialize");
        PlannedLiveCandidate {
            execution,
            pinned_candidate: ResponsesWebSocketPinnedCandidate::new(
                "provider-1",
                "endpoint-1",
                "key-1",
            )
            .unwrap(),
            provider_outbound_context: ProviderOutboundRequestContext::new("test-live-turn", 1),
            client_model: "global-model".to_string(),
            provider_model: "provider-model".to_string(),
            auth_mode,
            routing_fingerprint: "0".repeat(64),
        }
    }

    fn transport_with_auth_override(
        default_auth_type: &str,
        auth_type_by_format: Option<serde_json::Value>,
    ) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "codex".to_string(),
                provider_type: "codex".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: false,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "codex:live".to_string(),
                api_family: Some("codex".to_string()),
                endpoint_kind: Some("live".to_string()),
                is_active: true,
                base_url: "https://chatgpt.com/backend-api/codex".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: default_auth_type.to_string(),
                is_active: true,
                api_formats: Some(vec!["codex:live".to_string()]),
                auth_type_by_format,
                allow_auth_channel_mismatch_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                upstream_metadata: None,
                decrypted_api_key: "rotating-secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    fn oauth_fingerprint_decision(
        token: &str,
        account_id: &str,
        fedramp: &str,
        session_id: &str,
    ) -> AiExecutionDecision {
        let mut decision = candidate(
            "https://chatgpt.com/backend-api/codex/live",
            LiveAuthMode::ChatGptOauth,
        )
        .execution;
        decision.provider_request_headers = BTreeMap::from([
            ("authorization".to_string(), format!("Bearer {token}")),
            ("chatgpt-account-id".to_string(), account_id.to_string()),
            ("x-openai-fedramp".to_string(), fedramp.to_string()),
            ("x-session-id".to_string(), session_id.to_string()),
        ]);
        decision
    }

    #[test]
    fn format_auth_override_selects_the_effective_live_auth_mode() {
        let overridden =
            transport_with_auth_override("oauth", Some(json!({"codex:live": "bearer"})));
        let effective =
            aether_provider_transport::auth::resolve_local_auth_type_for_transport_format(
                &overridden,
            );
        assert_eq!(effective, "bearer");
        assert_eq!(
            live_auth_mode(
                overridden.provider.provider_type.as_str(),
                effective.as_str()
            ),
            Some(LiveAuthMode::ApiKey)
        );

        let oauth = transport_with_auth_override("oauth", None);
        let effective =
            aether_provider_transport::auth::resolve_local_auth_type_for_transport_format(&oauth);
        assert_eq!(effective, "oauth");
        assert_eq!(
            live_auth_mode(oauth.provider.provider_type.as_str(), effective.as_str()),
            Some(LiveAuthMode::ChatGptOauth)
        );
    }

    #[test]
    fn derives_api_key_live_urls_preserves_query_and_replaces_the_mapped_model() {
        let mut candidate = candidate(
            "https://api.example.test/v1/live?api-version=2026-08-01&model=stale&MODEL=duplicate",
            LiveAuthMode::ApiKey,
        );
        candidate.provider_model = "upstream/model + future".to_string();
        let direct = Url::parse(direct_live_websocket_url(&candidate).unwrap().as_str()).unwrap();
        assert_eq!(direct.path(), "/v1/live");
        assert_eq!(
            direct.query_pairs().collect::<Vec<_>>(),
            vec![
                ("api-version".into(), "2026-08-01".into()),
                ("model".into(), "upstream/model + future".into()),
            ]
        );
        assert_eq!(
            direct
                .query_pairs()
                .filter(|(name, _)| name.eq_ignore_ascii_case("model"))
                .count(),
            1
        );
        assert!(!direct.as_str().contains("global-model"));
        assert_eq!(
            live_call_url(&candidate, LiveRouteDialect::LegacyLive).unwrap(),
            "https://api.example.test/v1/live?api-version=2026-08-01&model=stale&MODEL=duplicate"
        );
        assert_eq!(
            live_sideband_url(&candidate, "rtc_abc-123", LiveRouteDialect::LegacyLive).unwrap(),
            "https://api.example.test/v1/live/rtc_abc-123?api-version=2026-08-01&model=stale&MODEL=duplicate"
        );
        assert_eq!(
            live_call_url(&candidate, LiveRouteDialect::Realtime).unwrap(),
            "https://api.example.test/v1/realtime/calls?api-version=2026-08-01&model=stale&MODEL=duplicate&intent=quicksilver&architecture=avas"
        );
        assert_eq!(
            live_sideband_url(&candidate, "rtc_abc-123", LiveRouteDialect::Realtime).unwrap(),
            "https://api.example.test/v1/realtime?api-version=2026-08-01&model=stale&MODEL=duplicate&intent=quicksilver&call_id=rtc_abc-123"
        );
    }

    #[test]
    fn derives_direct_realtime_websocket_url_with_mapped_model_and_intent() {
        let mut candidate = candidate(
            "https://api.example.test/v1/live?api-version=2026-08-01&intent=stale&INTENT=duplicate&model=stale&MODEL=duplicate&deployment=primary",
            LiveAuthMode::ApiKey,
        );
        candidate.provider_model = "upstream/model + future".to_string();
        let direct = Url::parse(
            direct_live_websocket_url_for_dialect(&candidate, LiveRouteDialect::Realtime)
                .expect("direct Realtime URL should build")
                .as_str(),
        )
        .expect("direct Realtime URL should parse");
        assert_eq!(direct.path(), "/v1/realtime");
        assert_eq!(
            direct.query_pairs().collect::<Vec<_>>(),
            vec![
                ("api-version".into(), "2026-08-01".into()),
                ("deployment".into(), "primary".into()),
                ("intent".into(), "quicksilver".into()),
                ("model".into(), "upstream/model + future".into()),
            ]
        );
        assert_eq!(
            direct
                .query_pairs()
                .filter(|(name, _)| name.eq_ignore_ascii_case("intent"))
                .count(),
            1
        );
        assert_eq!(
            direct
                .query_pairs()
                .filter(|(name, _)| name.eq_ignore_ascii_case("model"))
                .count(),
            1
        );
    }

    #[test]
    fn derives_direct_realtime_v2_websocket_url_without_intent() {
        let mut candidate = candidate(
            "https://api.example.test/v1/live?api-version=2026-08-01&intent=stale&INTENT=duplicate&model=stale&MODEL=duplicate&deployment=primary",
            LiveAuthMode::ApiKey,
        );
        candidate.provider_model = "upstream/model + future".to_string();
        let direct = Url::parse(
            direct_live_websocket_url_for_dialect(&candidate, LiveRouteDialect::RealtimeV2)
                .expect("direct Realtime v2 URL should build")
                .as_str(),
        )
        .expect("direct Realtime v2 URL should parse");
        assert_eq!(direct.path(), "/v1/realtime");
        assert_eq!(
            direct.query_pairs().collect::<Vec<_>>(),
            vec![
                ("api-version".into(), "2026-08-01".into()),
                ("deployment".into(), "primary".into()),
                ("model".into(), "upstream/model + future".into()),
            ]
        );
        assert_eq!(
            direct
                .query_pairs()
                .filter(|(name, _)| name.eq_ignore_ascii_case("intent"))
                .count(),
            0
        );
        assert_eq!(
            direct
                .query_pairs()
                .filter(|(name, _)| name.eq_ignore_ascii_case("model"))
                .count(),
            1
        );
    }

    #[test]
    fn realtime_api_key_call_url_replaces_duplicate_transport_selectors() {
        let candidate = candidate(
            "https://api.example.test/v1/live?intent=stale&INTENT=duplicate&architecture=stale&ARCHITECTURE=duplicate&deployment=primary",
            LiveAuthMode::ApiKey,
        );
        let url = Url::parse(
            live_call_url(&candidate, LiveRouteDialect::Realtime)
                .expect("Realtime API-key call URL should build")
                .as_str(),
        )
        .expect("Realtime API-key call URL should parse");
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![
                ("deployment".into(), "primary".into()),
                ("intent".into(), "quicksilver".into()),
                ("architecture".into(), "avas".into()),
            ]
        );
        assert_eq!(
            url.query_pairs()
                .filter(|(name, _)| name.eq_ignore_ascii_case("intent"))
                .count(),
            1
        );
        assert_eq!(
            url.query_pairs()
                .filter(|(name, _)| name.eq_ignore_ascii_case("architecture"))
                .count(),
            1
        );
    }

    #[test]
    fn realtime_sideband_url_replaces_duplicate_transport_selectors() {
        let candidate = candidate(
            "https://api.example.test/v1/live?trace=1&intent=stale&INTENT=duplicate&call_id=old&CALL_ID=duplicate",
            LiveAuthMode::ApiKey,
        );
        let url = Url::parse(
            live_sideband_url(&candidate, "rtc_current", LiveRouteDialect::Realtime)
                .expect("Realtime sideband URL should build")
                .as_str(),
        )
        .expect("Realtime sideband URL should parse");
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![
                ("trace".into(), "1".into()),
                ("intent".into(), "quicksilver".into()),
                ("call_id".into(), "rtc_current".into()),
            ]
        );
        assert_eq!(
            url.query_pairs()
                .filter(|(name, _)| name.eq_ignore_ascii_case("intent"))
                .count(),
            1
        );
        assert_eq!(
            url.query_pairs()
                .filter(|(name, _)| name.eq_ignore_ascii_case("call_id"))
                .count(),
            1
        );
    }

    #[test]
    fn derives_chatgpt_call_and_official_sideband_urls() {
        let candidate = candidate(
            "https://chatgpt.com/backend-api/codex/live?api-version=2026-08-01&intent=stale&INTENT=duplicate&architecture=stale&ARCHITECTURE=duplicate",
            LiveAuthMode::ChatGptOauth,
        );
        let call = Url::parse(
            live_call_url(&candidate, LiveRouteDialect::Realtime)
                .unwrap()
                .as_str(),
        )
        .unwrap();
        assert_eq!(call.path(), "/backend-api/codex/realtime/calls");
        assert_eq!(
            call.query_pairs().collect::<Vec<_>>(),
            vec![
                ("api-version".into(), "2026-08-01".into()),
                ("intent".into(), "quicksilver".into()),
                ("architecture".into(), "avas".into()),
            ]
        );
        assert_eq!(
            call.query_pairs()
                .filter(|(name, _)| name.eq_ignore_ascii_case("intent"))
                .count(),
            1
        );
        assert_eq!(
            call.query_pairs()
                .filter(|(name, _)| name.eq_ignore_ascii_case("architecture"))
                .count(),
            1
        );
        assert_eq!(
            live_sideband_url(&candidate, "rtc_call_1", LiveRouteDialect::LegacyLive).unwrap(),
            "https://api.openai.com/v1/live/rtc_call_1"
        );
        assert_eq!(
            live_sideband_url(&candidate, "rtc_call_1", LiveRouteDialect::Realtime).unwrap(),
            "https://api.openai.com/v1/realtime?intent=quicksilver&call_id=rtc_call_1"
        );
        let oauth_sideband = Url::parse(
            live_sideband_url(&candidate, "rtc_call_1", LiveRouteDialect::Realtime)
                .unwrap()
                .as_str(),
        )
        .unwrap();
        assert_eq!(
            oauth_sideband
                .query_pairs()
                .filter(|(name, _)| name.eq_ignore_ascii_case("intent"))
                .count(),
            1
        );
        assert_eq!(
            oauth_sideband
                .query_pairs()
                .filter(|(name, _)| name.eq_ignore_ascii_case("call_id"))
                .count(),
            1
        );
        assert_eq!(
            direct_live_websocket_url(&candidate),
            Err(LiveProtocolError::OauthDirectWebSocketUnsupported)
        );
    }

    #[test]
    fn chatgpt_oauth_sideband_does_not_copy_cross_origin_query() {
        let candidate = candidate(
            "https://chatgpt.com/backend-api/codex/live?api-version=2026-08-01&deployment=backend-only&access_token=must-not-cross-origin&X-Api-Key=must-not-cross-origin",
            LiveAuthMode::ChatGptOauth,
        );
        assert_eq!(
            live_sideband_url(&candidate, "rtc_call_1", LiveRouteDialect::LegacyLive).unwrap(),
            "https://api.openai.com/v1/live/rtc_call_1"
        );
        assert_eq!(
            live_sideband_url(&candidate, "rtc_call_1", LiveRouteDialect::Realtime).unwrap(),
            "https://api.openai.com/v1/realtime?intent=quicksilver&call_id=rtc_call_1"
        );
    }

    #[test]
    fn chatgpt_oauth_live_fails_closed_for_custom_backend_origins() {
        let candidate = candidate(
            "https://relay.example/backend-api/codex/live",
            LiveAuthMode::ChatGptOauth,
        );
        assert_eq!(
            live_call_url(&candidate, LiveRouteDialect::Realtime),
            Err(LiveProtocolError::OauthUpstreamUnsupported)
        );
        assert_eq!(
            live_sideband_url(&candidate, "rtc_custom_backend", LiveRouteDialect::Realtime),
            Err(LiveProtocolError::OauthUpstreamUnsupported)
        );
        assert_eq!(
            live_routing_fingerprint(&candidate.execution, "oauth", LiveAuthMode::ChatGptOauth),
            Err(LiveProtocolError::OauthUpstreamUnsupported)
        );
    }

    #[test]
    fn routing_fingerprint_ignores_token_rotation_but_binds_oauth_identity() {
        let baseline =
            oauth_fingerprint_decision("access-token-1", "account-1", "true", "session-1");
        let refreshed =
            oauth_fingerprint_decision("access-token-2", "account-1", "true", "session-1");
        let fingerprint =
            live_routing_fingerprint(&baseline, "oauth", LiveAuthMode::ChatGptOauth).unwrap();
        assert_eq!(
            live_routing_fingerprint(&refreshed, "oauth", LiveAuthMode::ChatGptOauth).unwrap(),
            fingerprint,
            "access-token refresh must not invalidate an established sideband binding"
        );

        for changed in [
            oauth_fingerprint_decision("access-token-2", "account-2", "true", "session-1"),
            oauth_fingerprint_decision("access-token-2", "account-1", "false", "session-1"),
            oauth_fingerprint_decision("access-token-2", "account-1", "true", "session-2"),
        ] {
            assert_ne!(
                live_routing_fingerprint(&changed, "oauth", LiveAuthMode::ChatGptOauth).unwrap(),
                fingerprint
            );
        }
    }

    #[test]
    fn routing_fingerprint_binds_api_key_origin_without_hashing_the_token() {
        let mut first = candidate("https://api-a.example/v1/live", LiveAuthMode::ApiKey).execution;
        first.provider_request_headers.extend([
            ("authorization".to_string(), "Bearer token-1".to_string()),
            ("x-session-id".to_string(), "session-1".to_string()),
        ]);
        let mut refreshed = first.clone();
        refreshed
            .provider_request_headers
            .insert("authorization".to_string(), "Bearer token-2".to_string());
        assert_eq!(
            live_routing_fingerprint(&first, "bearer", LiveAuthMode::ApiKey).unwrap(),
            live_routing_fingerprint(&refreshed, "bearer", LiveAuthMode::ApiKey).unwrap()
        );

        let mut changed_origin =
            candidate("https://api-b.example/v1/live", LiveAuthMode::ApiKey).execution;
        changed_origin
            .provider_request_headers
            .insert("x-session-id".to_string(), "session-1".to_string());
        assert_ne!(
            live_routing_fingerprint(&first, "bearer", LiveAuthMode::ApiKey).unwrap(),
            live_routing_fingerprint(&changed_origin, "bearer", LiveAuthMode::ApiKey).unwrap()
        );

        let mut changed_session = refreshed;
        changed_session
            .provider_request_headers
            .insert("x-session-id".to_string(), "session-2".to_string());
        assert_ne!(
            live_routing_fingerprint(&first, "bearer", LiveAuthMode::ApiKey).unwrap(),
            live_routing_fingerprint(&changed_session, "bearer", LiveAuthMode::ApiKey).unwrap()
        );

        let missing_session =
            candidate("https://api-a.example/v1/live", LiveAuthMode::ApiKey).execution;
        assert_eq!(
            live_routing_fingerprint(&missing_session, "bearer", LiveAuthMode::ApiKey),
            Err(LiveProtocolError::InvalidUpstreamUrl)
        );
    }

    #[test]
    fn routing_fingerprint_canonicalizes_safe_query_and_ignores_query_credentials() {
        let mut baseline = candidate(
            "https://api-a.example/v1/live?api-version=2026-08-01&deployment=primary&alt=sse&token=secret-1&key=secret-1",
            LiveAuthMode::ApiKey,
        )
        .execution;
        baseline
            .provider_request_headers
            .insert("x-session-id".to_string(), "session-1".to_string());
        let fingerprint =
            live_routing_fingerprint(&baseline, "bearer", LiveAuthMode::ApiKey).unwrap();

        let mut reordered = candidate(
            "https://api-a.example/v1/live?key=secret-2&alt=sse&token=secret-2&deployment=primary&api-version=2026-08-01",
            LiveAuthMode::ApiKey,
        )
        .execution;
        reordered
            .provider_request_headers
            .insert("x-session-id".to_string(), "session-1".to_string());
        assert_eq!(
            live_routing_fingerprint(&reordered, "bearer", LiveAuthMode::ApiKey).unwrap(),
            fingerprint,
            "query order and query credentials must not change the stable route identity"
        );

        let mut changed_route = candidate(
            "https://api-a.example/v1/live?api-version=2026-08-01&deployment=secondary&alt=sse&token=secret-2&key=secret-2",
            LiveAuthMode::ApiKey,
        )
        .execution;
        changed_route
            .provider_request_headers
            .insert("x-session-id".to_string(), "session-1".to_string());
        assert_ne!(
            live_routing_fingerprint(&changed_route, "bearer", LiveAuthMode::ApiKey).unwrap(),
            fingerprint,
            "a non-sensitive endpoint route query change must invalidate the binding"
        );
    }

    #[test]
    fn live_headers_force_the_route_dialect_and_preserve_a_stable_session_identity() {
        let mut headers = BTreeMap::from([
            ("OpenAI-Alpha".to_string(), "wrong".to_string()),
            ("thread-id".to_string(), "thread-stable".to_string()),
        ]);
        apply_live_headers(&mut headers, "trace-fallback", LiveRouteDialect::Realtime);
        assert_eq!(
            headers.get("openai-alpha").map(String::as_str),
            Some(REALTIME_LIVE_ALPHA_HEADER_VALUE)
        );
        assert_eq!(
            headers.get("x-session-id").map(String::as_str),
            Some("thread-stable")
        );
        assert_eq!(
            headers
                .keys()
                .filter(|name| name.eq_ignore_ascii_case("openai-alpha"))
                .count(),
            1
        );

        let mut legacy_session_header = BTreeMap::from([
            ("Session-Id".to_string(), "legacy-stable".to_string()),
            ("chatgpt-account-id".to_string(), "account-1".to_string()),
        ]);
        apply_live_headers(
            &mut legacy_session_header,
            "trace-fallback",
            LiveRouteDialect::LegacyLive,
        );
        assert_eq!(
            legacy_session_header
                .get("openai-alpha")
                .map(String::as_str),
            Some(LEGACY_LIVE_ALPHA_HEADER_VALUE)
        );
        assert_eq!(
            legacy_session_header
                .get("x-session-id")
                .map(String::as_str),
            Some("legacy-stable")
        );
        assert_eq!(
            legacy_session_header
                .get("chatgpt-account-id")
                .map(String::as_str),
            Some("account-1")
        );

        let mut v2_headers = BTreeMap::from([
            ("OpenAI-Alpha".to_string(), "stale".to_string()),
            ("thread-id".to_string(), "v2-thread".to_string()),
        ]);
        apply_live_headers(
            &mut v2_headers,
            "trace-fallback",
            LiveRouteDialect::RealtimeV2,
        );
        assert!(v2_headers
            .keys()
            .all(|name| !name.eq_ignore_ascii_case("openai-alpha")));
        assert_eq!(
            v2_headers.get("x-session-id").map(String::as_str),
            Some("v2-thread")
        );
    }

    #[test]
    fn live_urls_reject_credentials_invalid_suffixes_and_call_ids() {
        let credentials = candidate("https://token@example.test/v1/live", LiveAuthMode::ApiKey);
        assert_eq!(
            direct_live_websocket_url(&credentials),
            Err(LiveProtocolError::InvalidUpstreamUrl)
        );

        let wrong_suffix = candidate(
            "https://api.example.test/v1/chat/completions",
            LiveAuthMode::ApiKey,
        );
        assert_eq!(
            live_call_url(&wrong_suffix, LiveRouteDialect::LegacyLive),
            Err(LiveProtocolError::InvalidUpstreamUrl)
        );
        assert_eq!(
            live_sideband_url(&wrong_suffix, "rtc/escape", LiveRouteDialect::LegacyLive),
            Err(LiveProtocolError::InvalidCallId)
        );

        let fragment = candidate(
            "https://api.example.test/v1/live#not-sent-upstream",
            LiveAuthMode::ApiKey,
        );
        assert_eq!(
            direct_live_websocket_url(&fragment),
            Err(LiveProtocolError::InvalidUpstreamUrl)
        );
        assert_eq!(
            live_call_url(&fragment, LiveRouteDialect::LegacyLive),
            Err(LiveProtocolError::InvalidUpstreamUrl)
        );

        let mut fragment_fingerprint = fragment.execution;
        fragment_fingerprint
            .provider_request_headers
            .insert("x-session-id".to_string(), "session-1".to_string());
        assert_eq!(
            live_routing_fingerprint(&fragment_fingerprint, "bearer", LiveAuthMode::ApiKey),
            Err(LiveProtocolError::InvalidUpstreamUrl)
        );
    }
}
