use std::collections::BTreeMap;
use std::time::Duration;

use aether_data_contracts::repository::video_tasks::VideoTaskLookupKey;
use aether_usage_runtime::build_locally_actionable_report_context_from_video_task;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};
use tokio::time::sleep;
use uuid::Uuid;

use crate::request_candidate_runtime::resolve_locally_actionable_request_candidate_report_context;
use crate::video_tasks::{resolve_video_task_report_lookup, VideoTaskReportLookup};
use crate::AppState;

pub(crate) use aether_usage_runtime::report_context_is_locally_actionable;

const REQUEST_CANDIDATE_REPORT_CONTEXT_RETRY_ATTEMPTS: usize = 5;
const REQUEST_CANDIDATE_REPORT_CONTEXT_RETRY_DELAY_MS: u64 = 50;
const INTERNAL_REPORT_CAPABILITY_FIELD: &str = "_aether_internal_report_capability";
const INTERNAL_REPORT_CAPABILITY_KEY_PREFIX: &str = "internal:gateway:report-capability:";
const INTERNAL_REPORT_CAPABILITY_VERSION: u8 = 1;
const INTERNAL_REPORT_CAPABILITY_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const INTERNAL_REPORT_CAPABILITY_MINT_ATTEMPTS: usize = 4;
const PLAN_USAGE_RESERVATION_TOKEN_FIELD: &str = "plan_usage_reservation_token";
const PLAN_USAGE_RESERVATION_DEFERRED_FIELD: &str = "plan_usage_reservation_deferred";

/// Fields produced while observing an upstream response. Everything else in the
/// planner-issued context is immutable and covered by the capability digest.
///
/// This allowlist is intentionally top-level and fail-closed: adding a future
/// report-context side effect requires explicitly classifying it as an observation.
const INTERNAL_REPORT_OBSERVATION_FIELDS: &[&str] = &[
    "provider_response_headers",
    "provider_request_started_at_unix_ms",
    "provider_response_headers_observed_at_unix_ms",
    "provider_request_order_id",
    "client_response_status_code",
    "client_response_headers",
    "upstream_response",
    "error_flow",
    "transport_error",
    "input_tokens",
    "cache_creation_input_tokens",
    "cache_read_input_tokens",
    "kiro_simulated_cache_enabled",
    "stage_timings_ms",
    "db_timings_ms",
    "end_to_end_time_ms",
    "end_to_end_first_byte_time_ms",
    "windsurf_native_runtime",
    "windsurf_language_server_port",
];

#[derive(Debug, Serialize, Deserialize)]
struct InternalReportCapabilityRecord {
    version: u8,
    trace_id: String,
    report_scope: String,
    protected_context_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kiro_web_search_context_sha256: Option<String>,
}

pub(crate) async fn attach_internal_gateway_report_capability(
    state: &AppState,
    trace_id: &str,
    report_kind: Option<&str>,
    provider_request_headers: &BTreeMap<String, String>,
    report_context: &mut Option<Value>,
) -> Result<(), crate::GatewayError> {
    let Some(report_kind) = report_kind else {
        return Ok(());
    };
    let Some(report_scope) = internal_report_capability_scope(report_kind) else {
        return Ok(());
    };
    let Some(context) = report_context.as_mut().and_then(Value::as_object_mut) else {
        return Err(crate::GatewayError::Internal(
            "internal gateway report capability requires an object context".to_string(),
        ));
    };
    if context.contains_key(INTERNAL_REPORT_CAPABILITY_FIELD) {
        return Err(crate::GatewayError::Internal(
            "internal gateway planner produced a reserved report capability field".to_string(),
        ));
    }
    context.insert(
        "provider_request_headers".to_string(),
        serde_json::to_value(provider_request_headers)
            .map_err(|error| crate::GatewayError::Internal(error.to_string()))?,
    );

    let protected_context_sha256 = protected_internal_report_context_sha256(context)?;
    let kiro_web_search_context_sha256 = kiro_web_search_internal_report_context_sha256(context)?;
    let record = InternalReportCapabilityRecord {
        version: INTERNAL_REPORT_CAPABILITY_VERSION,
        trace_id: trace_id.to_string(),
        report_scope,
        protected_context_sha256,
        kiro_web_search_context_sha256,
    };
    let serialized = serde_json::to_string(&record)
        .map_err(|error| crate::GatewayError::Internal(error.to_string()))?;

    for _ in 0..INTERNAL_REPORT_CAPABILITY_MINT_ATTEMPTS {
        let capability = Uuid::new_v4().simple().to_string();
        let storage_key = internal_report_capability_storage_key(&capability);
        let inserted = state
            .runtime_state
            .kv_set_if_absent(
                &storage_key,
                serialized.clone(),
                INTERNAL_REPORT_CAPABILITY_TTL,
            )
            .await
            .map_err(|error| crate::GatewayError::Internal(error.to_string()))?;
        if inserted {
            context.insert(
                INTERNAL_REPORT_CAPABILITY_FIELD.to_string(),
                Value::String(capability),
            );
            return Ok(());
        }
    }

    Err(crate::GatewayError::Internal(
        "failed to allocate a unique internal gateway report capability".to_string(),
    ))
}

/// Validate and atomically consume a planner-issued report capability, then
/// return a context whose planner fields are equivalent after canonical JSON
/// normalization.
///
/// The internal request HMAC authenticates a peer, but a peer must not choose a
/// candidate, user, provider key, video task, or file mapping target. The opaque
/// capability is independent from diagnostic candidate persistence, so `terminal`
/// and `none` persistence modes remain functional.
pub(crate) async fn resolve_bound_internal_gateway_report_context(
    state: &AppState,
    trace_id: &str,
    report_kind: &str,
    report_context: Option<&Value>,
) -> Result<Option<Value>, crate::GatewayError> {
    let Some(context) = report_context.and_then(Value::as_object) else {
        return Ok(None);
    };
    let Some(capability) = context
        .get(INTERNAL_REPORT_CAPABILITY_FIELD)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    if Uuid::parse_str(capability).is_err() {
        return Ok(None);
    }
    if !internal_report_late_bound_settlement_fields_are_valid(context)
        || !internal_report_windsurf_observation_is_valid(context)
    {
        return Ok(None);
    }

    let storage_key = internal_report_capability_storage_key(capability);
    let Some(serialized) = state
        .runtime_state
        .kv_get(&storage_key)
        .await
        .map_err(|error| crate::GatewayError::Internal(error.to_string()))?
    else {
        return Ok(None);
    };
    let record: InternalReportCapabilityRecord = serde_json::from_str(&serialized)
        .map_err(|error| crate::GatewayError::Internal(error.to_string()))?;
    let protected_context_sha256 = protected_internal_report_context_sha256(context)?;
    let context_matches = protected_context_sha256 == record.protected_context_sha256
        || record.kiro_web_search_context_sha256.as_deref()
            == Some(protected_context_sha256.as_str());
    if record.version != INTERNAL_REPORT_CAPABILITY_VERSION
        || record.trace_id != trace_id
        || internal_report_capability_scope(report_kind).as_deref()
            != Some(record.report_scope.as_str())
        || !context_matches
    {
        return Ok(None);
    }

    // A report can mutate billing, provider health, file mappings, and video
    // tasks. Consume its capability so a signed peer cannot replay those side
    // effects. The second read is atomic: concurrent valid submissions have a
    // single winner, while invalid submissions above cannot burn the token.
    let claimed = state
        .runtime_state
        .kv_take(&storage_key)
        .await
        .map_err(|error| crate::GatewayError::Internal(error.to_string()))?;
    if claimed.as_deref() != Some(serialized.as_str()) {
        return Ok(None);
    }

    let mut resolved = context.clone();
    resolved.remove(INTERNAL_REPORT_CAPABILITY_FIELD);
    Ok(Some(Value::Object(resolved)))
}

fn internal_report_capability_scope(report_kind: &str) -> Option<String> {
    let mut scope = report_kind.trim().to_ascii_lowercase();
    if scope.is_empty() || scope.len() > 160 {
        return None;
    }
    for suffix in ["_success", "_error", "_failed", "_cancelled", "_finalize"] {
        if let Some(value) = scope.strip_suffix(suffix) {
            scope = value.to_string();
            break;
        }
    }
    for suffix in ["_sync", "_stream"] {
        if let Some(value) = scope.strip_suffix(suffix) {
            scope = value.to_string();
            break;
        }
    }
    (!scope.is_empty()).then_some(scope)
}

fn internal_report_capability_storage_key(capability: &str) -> String {
    let digest = Sha256::digest(capability.as_bytes());
    format!("{INTERNAL_REPORT_CAPABILITY_KEY_PREFIX}{digest:x}")
}

fn protected_internal_report_context_sha256(
    context: &Map<String, Value>,
) -> Result<String, crate::GatewayError> {
    let mut protected = context.clone();
    protected.remove(INTERNAL_REPORT_CAPABILITY_FIELD);
    protected.remove(PLAN_USAGE_RESERVATION_TOKEN_FIELD);
    protected.remove(PLAN_USAGE_RESERVATION_DEFERRED_FIELD);
    for field in INTERNAL_REPORT_OBSERVATION_FIELDS {
        protected.remove(*field);
    }
    let canonical = canonicalize_internal_report_json(&Value::Object(protected));
    let encoded = serde_json::to_vec(&canonical)
        .map_err(|error| crate::GatewayError::Internal(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

/// Kiro MCP web-search execution emits a synthetic response that is already in
/// the client contract. The executor must therefore disable the planner's Kiro
/// envelope conversion before observing that response. Bind that exact, fixed
/// transformation when the capability is minted instead of making the affected
/// planner fields globally mutable.
fn kiro_web_search_internal_report_context_sha256(
    context: &Map<String, Value>,
) -> Result<Option<String>, crate::GatewayError> {
    let is_kiro_envelope = context
        .get("envelope_name")
        .and_then(Value::as_str)
        .is_some_and(|value| {
            value.eq_ignore_ascii_case(aether_provider_transport::kiro::KIRO_ENVELOPE_NAME)
        });
    if !is_kiro_envelope {
        return Ok(None);
    }

    let mut synthetic = context.clone();
    synthetic.insert("has_envelope".to_string(), Value::Bool(false));
    synthetic.insert("needs_conversion".to_string(), Value::Bool(false));
    synthetic.remove("envelope_name");
    synthetic.insert("kiro_web_search_mcp".to_string(), Value::Bool(true));
    protected_internal_report_context_sha256(&synthetic).map(Some)
}

/// HTTP candidate execution may create a plan-cost reservation only after the
/// planner has issued the report capability. Its opaque token is therefore
/// late-bound, but the peer cannot use it to select another request or user:
/// repository reconciliation also requires the capability-bound request and
/// subject identities. Deferred reconciliation is not a normal HTTP report
/// outcome and remains forbidden here; allowing it would let a peer strand a
/// reservation without submitting a terminal reconciliation.
fn internal_report_late_bound_settlement_fields_are_valid(context: &Map<String, Value>) -> bool {
    match (
        context.get(PLAN_USAGE_RESERVATION_TOKEN_FIELD),
        context.get(PLAN_USAGE_RESERVATION_DEFERRED_FIELD),
    ) {
        (None, None) => true,
        (Some(Value::String(token)), Some(Value::Bool(false))) => {
            Uuid::parse_str(token.trim()).is_ok()
        }
        _ => false,
    }
}

fn internal_report_windsurf_observation_is_valid(context: &Map<String, Value>) -> bool {
    match (
        context.get("windsurf_native_runtime"),
        context.get("windsurf_language_server_port"),
    ) {
        (None, None) => true,
        (Some(Value::Bool(true)), Some(Value::Number(port))) => port
            .as_u64()
            .is_some_and(|port| u16::try_from(port).is_ok() && port != 0),
        _ => false,
    }
}

fn canonicalize_internal_report_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(canonicalize_internal_report_json)
                .collect(),
        ),
        Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_unstable_by_key(|(left, _)| *left);
            Value::Object(Map::from_iter(entries.into_iter().map(|(key, value)| {
                (key.clone(), canonicalize_internal_report_json(value))
            })))
        }
        other => other.clone(),
    }
}

pub(crate) async fn resolve_locally_actionable_report_context(
    state: &AppState,
    report_context: Option<&Value>,
) -> Option<Value> {
    let context = report_context?.clone();
    if report_context_is_locally_actionable(Some(&context)) {
        return Some(context);
    }

    if let Some(resolved) =
        resolve_locally_actionable_request_candidate_report_context_with_retry(state, &context)
            .await
    {
        return Some(resolved);
    }

    let context = resolve_locally_actionable_report_context_from_video_task(state, &context)
        .await
        .unwrap_or(context);

    if let Some(resolved) =
        resolve_locally_actionable_request_candidate_report_context_with_retry(state, &context)
            .await
    {
        return Some(resolved);
    }

    report_context_is_locally_actionable(Some(&context)).then_some(context)
}

async fn resolve_locally_actionable_request_candidate_report_context_with_retry(
    state: &AppState,
    context: &Value,
) -> Option<Value> {
    if context
        .get("request_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        return None;
    }

    for attempt in 0..=REQUEST_CANDIDATE_REPORT_CONTEXT_RETRY_ATTEMPTS {
        if let Some(resolved) =
            resolve_locally_actionable_request_candidate_report_context(state, context).await
        {
            return Some(resolved);
        }

        if attempt < REQUEST_CANDIDATE_REPORT_CONTEXT_RETRY_ATTEMPTS {
            sleep(Duration::from_millis(
                REQUEST_CANDIDATE_REPORT_CONTEXT_RETRY_DELAY_MS,
            ))
            .await;
        }
    }

    None
}

async fn resolve_locally_actionable_report_context_from_video_task(
    state: &AppState,
    context: &Value,
) -> Option<Value> {
    let requested_user_id = requested_report_user_id(context)?;
    let task = match resolve_video_task_report_lookup(context)? {
        VideoTaskReportLookup::Lookup(lookup) => {
            let task = state.data.find_video_task(lookup).await.ok()??;
            if !video_task_matches_requested_user(&task, requested_user_id) {
                return None;
            }
            task
        }
        VideoTaskReportLookup::TaskIdOrExternal { task_id, user_id } => {
            if let Some(task) = state
                .data
                .find_video_task(VideoTaskLookupKey::Id(task_id))
                .await
                .ok()?
                .filter(|task| video_task_matches_requested_user(task, requested_user_id))
            {
                task
            } else {
                let user_id = user_id?;
                let task = state
                    .data
                    .find_video_task(VideoTaskLookupKey::UserExternal {
                        user_id,
                        external_task_id: task_id,
                    })
                    .await
                    .ok()??;
                if !video_task_matches_requested_user(&task, requested_user_id) {
                    return None;
                }
                task
            }
        }
    };

    build_locally_actionable_report_context_from_video_task(context, &task)
}

fn requested_report_user_id(context: &Value) -> Option<Option<&str>> {
    match context.get("user_id") {
        None => Some(None),
        Some(value) => value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(Some),
    }
}

fn video_task_matches_requested_user(
    task: &aether_data_contracts::repository::video_tasks::StoredVideoTask,
    requested_user_id: Option<&str>,
) -> bool {
    let Some(requested_user_id) = requested_user_id else {
        return true;
    };
    task.user_id.as_deref().map(str::trim) == Some(requested_user_id)
}
