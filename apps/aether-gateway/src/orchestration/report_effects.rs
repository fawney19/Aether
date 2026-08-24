use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;

use aether_admin::provider::quota as admin_provider_quota_pure;
use aether_data_contracts::repository::provider_catalog::ProviderCatalogKeyRuntimeMetadataUpdate;
use aether_provider_pool::grok_quota_window_key_for_model;
use aether_usage_runtime::{
    extract_gemini_file_mapping_entries, gemini_file_mapping_cache_key, normalize_gemini_file_name,
    report_request_id, GatewayStreamReportRequest, GatewaySyncReportRequest,
    GEMINI_FILE_MAPPING_TTL_SECONDS,
};
use base64::Engine as _;
use regex::Regex;
use serde_json::{json, Value};
use tracing::warn;
use uuid::Uuid;

use super::codex_quota_breaker::{
    install_codex_quota_exhaustion_breaker, log_codex_quota_breaker_install_failure,
};
use crate::clock::current_unix_secs;
use crate::handlers::shared::sync_provider_key_quota_status_snapshot;
use crate::log_ids::short_request_id;
use crate::{AppState, GatewayError};

const RUNTIME_METADATA_CAS_MAX_ATTEMPTS: usize = 16;
const CODEX_WEBSOCKET_RATE_LIMITS_REPORT_CONTEXT_FIELD: &str = "codex_websocket_rate_limits";
static GROK_CHINESE_WAIT_DURATION_RE: OnceLock<Regex> = OnceLock::new();
static GROK_ENGLISH_WAIT_DURATION_RE: OnceLock<Regex> = OnceLock::new();

fn upstream_metadata_namespace_value(
    upstream_metadata: Option<&Value>,
    namespace: &str,
) -> Option<Value> {
    upstream_metadata
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get(namespace))
        .cloned()
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum LocalReportEffect<'a> {
    Sync {
        payload: &'a GatewaySyncReportRequest,
    },
    Stream {
        payload: &'a GatewayStreamReportRequest,
    },
}

pub(crate) async fn apply_local_report_effect(state: &AppState, effect: LocalReportEffect<'_>) {
    match effect {
        LocalReportEffect::Sync { payload } => {
            apply_local_sync_report_effect(state, payload).await;
        }
        LocalReportEffect::Stream { payload } => {
            apply_local_stream_report_effect(state, payload).await;
        }
    }
}

fn report_context_key_id(report_context: Option<&Value>) -> Option<String> {
    report_context
        .and_then(|context| context.get("key_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn report_context_u64(report_context: Option<&Value>, key: &str) -> Option<u64> {
    report_context
        .and_then(|context| context.get(key))
        .and_then(admin_provider_quota_pure::coerce_json_u64)
}

fn report_context_string<'a>(report_context: Option<&'a Value>, key: &str) -> Option<&'a str> {
    report_context
        .and_then(|context| context.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn report_context_provider_response_headers(
    report_context: Option<&Value>,
) -> Option<BTreeMap<String, String>> {
    let headers = report_context
        .and_then(|context| context.get("provider_response_headers"))
        .and_then(Value::as_object)?;
    let mut out = BTreeMap::new();
    for (key, value) in headers {
        let Some(value) = value.as_str() else {
            continue;
        };
        out.insert(key.clone(), value.to_string());
    }
    (!out.is_empty()).then_some(out)
}

fn codex_websocket_quota_from_report_context(report_context: Option<&Value>) -> Option<Value> {
    report_context
        .and_then(|context| context.get(CODEX_WEBSOCKET_RATE_LIMITS_REPORT_CONTEXT_FIELD))
        .filter(|value| value.as_object().is_some_and(|object| !object.is_empty()))
        .cloned()
}
fn merge_metadata_object(
    current: Option<&Value>,
    section_key: &str,
    section_value: Value,
) -> Option<Value> {
    let mut merged = current
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    merged.insert(section_key.to_string(), section_value);
    Some(Value::Object(merged))
}

fn quota_status_snapshot_patch(status_snapshot: Option<&Value>) -> Value {
    let mut patch = serde_json::Map::new();
    if let Some(quota) = status_snapshot
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("quota"))
        .cloned()
    {
        patch.insert("quota".to_string(), quota);
    }
    Value::Object(patch)
}

fn grok_report_context_model(report_context: Option<&Value>) -> Option<String> {
    report_context
        .and_then(|context| context.get("mapped_model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn grok_chinese_wait_duration_regex() -> &'static Regex {
    GROK_CHINESE_WAIT_DURATION_RE.get_or_init(|| {
        Regex::new(r"(?i)(?:(\d+)\s*天)?\s*(?:(\d+)\s*(?:小时|小時))?\s*(?:(\d+)\s*分钟)?\s*(?:(\d+)\s*秒)?")
            .expect("grok Chinese wait duration regex should compile")
    })
}

fn grok_english_wait_duration_regex() -> &'static Regex {
    GROK_ENGLISH_WAIT_DURATION_RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?:(\d+)\s*(?:d|day|days))?\s*(?:(\d+)\s*(?:h|hour|hours))?\s*(?:(\d+)\s*(?:m|min|mins|minute|minutes))?\s*(?:(\d+)\s*(?:s|sec|secs|second|seconds))?",
        )
        .expect("grok English wait duration regex should compile")
    })
}

fn grok_duration_capture_seconds(captures: regex::Captures<'_>) -> Option<u64> {
    let values = [1usize, 2, 3, 4]
        .into_iter()
        .map(|index| {
            captures
                .get(index)
                .and_then(|item| item.as_str().parse::<u64>().ok())
                .unwrap_or(0)
        })
        .collect::<Vec<_>>();
    let seconds = values[0]
        .saturating_mul(86_400)
        .saturating_add(values[1].saturating_mul(3_600))
        .saturating_add(values[2].saturating_mul(60))
        .saturating_add(values[3]);
    (seconds > 0).then_some(seconds)
}

fn grok_wait_duration_seconds_from_text(text: &str) -> Option<u64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    for captures in grok_chinese_wait_duration_regex().captures_iter(text) {
        if let Some(seconds) = grok_duration_capture_seconds(captures) {
            return Some(seconds);
        }
    }
    for captures in grok_english_wait_duration_regex().captures_iter(text) {
        if let Some(seconds) = grok_duration_capture_seconds(captures) {
            return Some(seconds);
        }
    }
    None
}

fn grok_response_error_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.trim().to_string()).filter(|text| !text.is_empty()),
        Value::Object(object) => {
            if let Some(error) = object.get("error") {
                if let Some(text) = grok_response_error_text(error) {
                    return Some(text);
                }
            }
            for key in ["message", "detail", "reason", "error"] {
                if let Some(text) = object
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                {
                    return Some(text.to_string());
                }
            }
            None
        }
        _ => None,
    }
}

fn grok_upstream_response_body(report_context: Option<&Value>) -> Option<&Value> {
    report_context
        .and_then(|context| context.get("upstream_response"))
        .and_then(|response| response.get("body"))
}

fn gemini_cli_credits_from_report_context(
    report_context: Option<&Value>,
    now_unix_secs: u64,
) -> Option<Value> {
    report_context
        .and_then(|context| context.get("gemini_cli_v1internal_credits"))
        .and_then(|value| {
            admin_provider_quota_pure::parse_gemini_cli_v1internal_credits_response(
                value,
                now_unix_secs,
            )
        })
}

fn gemini_cli_credits_from_stream_payload(
    payload: &GatewayStreamReportRequest,
    now_unix_secs: u64,
) -> Option<Value> {
    let body_base64 = payload.provider_body_base64.as_deref()?;
    let body = base64::engine::general_purpose::STANDARD
        .decode(body_base64)
        .ok()?;
    let text = std::str::from_utf8(&body).ok()?;
    let mut latest = None::<Value>;
    for raw_line in text.lines() {
        let line = raw_line.trim_matches('\r').trim();
        let data = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
        if data.is_empty() || data == "[DONE]" || data.starts_with(':') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(credits) =
            admin_provider_quota_pure::parse_gemini_cli_v1internal_credits_response(
                &value,
                now_unix_secs,
            )
        {
            latest = Some(credits);
        }
    }
    latest
}

fn codex_websocket_quota_from_stream_payload(
    payload: &GatewayStreamReportRequest,
    now_unix_secs: u64,
) -> Option<Value> {
    let body_base64 = payload.provider_body_base64.as_deref()?;
    let body = base64::engine::general_purpose::STANDARD
        .decode(body_base64)
        .ok()?;
    let text = std::str::from_utf8(&body).ok()?;
    let mut latest = None::<Value>;
    if let Ok(value) = serde_json::from_str::<Value>(text.trim()) {
        latest = admin_provider_quota_pure::parse_codex_websocket_rate_limits_response(
            &value,
            now_unix_secs,
        );
    }
    for raw_line in text.lines() {
        let line = raw_line.trim_matches('\r').trim();
        let data = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
        if data.is_empty() || data == "[DONE]" || data.starts_with(':') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(quota) = admin_provider_quota_pure::parse_codex_websocket_rate_limits_response(
            &value,
            now_unix_secs,
        ) {
            latest = Some(quota);
        }
    }
    latest
}

async fn sync_gemini_cli_credits_from_report(
    state: &AppState,
    report_context: Option<&Value>,
    credits: Option<Value>,
) -> Result<bool, GatewayError> {
    let Some(credits) = credits else {
        return Ok(false);
    };
    let key_id = match report_context_key_id(report_context) {
        Some(value) => value,
        None => return Ok(false),
    };
    let now_unix_secs = current_unix_secs();
    let Some(key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(false);
    };
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&key.provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(false);
    };
    if !provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("gemini_cli")
    {
        return Ok(false);
    }

    let expected_namespace_value =
        upstream_metadata_namespace_value(key.upstream_metadata.as_ref(), "gemini_cli");
    let mut gemini_cli_bucket = expected_namespace_value
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    gemini_cli_bucket.insert("credits".to_string(), credits.clone());
    gemini_cli_bucket.insert("updated_at".to_string(), json!(now_unix_secs));

    let namespace_value = Value::Object(gemini_cli_bucket);
    let updated_upstream_metadata = merge_metadata_object(
        key.upstream_metadata.as_ref(),
        "gemini_cli",
        namespace_value.clone(),
    );
    let updated_status_snapshot = sync_provider_key_quota_status_snapshot(
        key.status_snapshot.as_ref(),
        provider.provider_type.as_str(),
        updated_upstream_metadata.as_ref(),
        "report_effect",
    );
    let persisted = state
        .update_provider_catalog_key_runtime_metadata(&ProviderCatalogKeyRuntimeMetadataUpdate {
            key_id: key_id.clone(),
            namespace: "gemini_cli".to_string(),
            expected_upstream_metadata_value: expected_namespace_value,
            upstream_metadata_value: namespace_value,
            status_snapshot_patch: quota_status_snapshot_patch(updated_status_snapshot.as_ref()),
            updated_at_unix_secs: Some(now_unix_secs),
        })
        .await?;
    if persisted {
        return Ok(true);
    }
    // Credits returned by the provider are an authoritative snapshot; do not
    // replay it over a newer local namespace after a CAS conflict.
    Ok(false)
}

fn grok_quota_reset_after_seconds(
    body_json: Option<&Value>,
    report_context: Option<&Value>,
) -> Option<u64> {
    body_json
        .and_then(grok_response_error_text)
        .and_then(|text| grok_wait_duration_seconds_from_text(text.as_str()))
        .or_else(|| {
            grok_upstream_response_body(report_context)
                .and_then(grok_response_error_text)
                .and_then(|text| grok_wait_duration_seconds_from_text(text.as_str()))
        })
}

fn grok_apply_quota_feedback(
    bucket: &mut serde_json::Map<String, Value>,
    model: &str,
    status_code: u16,
    reset_after_seconds: Option<u64>,
    now_unix_secs: u64,
) -> bool {
    let Some(quota_key) = grok_quota_window_key_for_model(Some(model)) else {
        return false;
    };
    let quota_by_model = if bucket.contains_key("quota_by_model") {
        bucket.get_mut("quota_by_model")
    } else {
        bucket.get_mut("models")
    };
    let Some(window) = quota_by_model
        .and_then(Value::as_object_mut)
        .and_then(|models| models.get_mut(quota_key))
        .and_then(Value::as_object_mut)
    else {
        return false;
    };

    let total = window
        .get("total")
        .and_then(admin_provider_quota_pure::coerce_json_f64)
        .filter(|value| *value > 0.0);
    let current_remaining = window
        .get("remaining")
        .and_then(admin_provider_quota_pure::coerce_json_f64)
        .or(total)
        .unwrap_or(0.0);
    let next_remaining = match status_code {
        429 => 0.0,
        code if code >= 400 && reset_after_seconds.is_some() => 0.0,
        code if code < 300 => (current_remaining - 1.0).max(0.0),
        _ => return false,
    };

    window.insert("remaining".to_string(), json!(next_remaining));
    if let Some(total) = total {
        window.insert("total".to_string(), json!(total));
        window.insert(
            "remaining_fraction".to_string(),
            json!((next_remaining / total).clamp(0.0, 1.0)),
        );
        window.insert(
            "used_percent".to_string(),
            json!(((total - next_remaining).max(0.0) / total * 100.0).clamp(0.0, 100.0)),
        );
    } else if status_code == 429 {
        window.insert("remaining_fraction".to_string(), json!(0.0));
        window.insert("used_percent".to_string(), json!(100.0));
    }
    if let Some(reset_after_seconds) = reset_after_seconds.filter(|seconds| *seconds > 0) {
        let reset_at = now_unix_secs.saturating_add(reset_after_seconds);
        window.insert("reset_at".to_string(), json!(reset_at));
        window.insert("next_reset_at".to_string(), json!(reset_at));
        window.insert(
            "reset_after_seconds".to_string(),
            json!(reset_after_seconds),
        );
        window.insert("reset_at_source".to_string(), json!("grok_upstream_error"));
    }
    window.insert("is_exhausted".to_string(), json!(next_remaining <= 0.0));
    true
}

fn grok_mark_quota_bucket_updated(bucket: &mut serde_json::Map<String, Value>, now_unix_secs: u64) {
    bucket.insert("updated_at".to_string(), json!(now_unix_secs));
}

async fn sync_grok_quota_from_report_context(
    state: &AppState,
    report_context: Option<&Value>,
    status_code: u16,
    body_json: Option<&Value>,
) -> Result<bool, GatewayError> {
    let key_id = match report_context_key_id(report_context) {
        Some(value) => value,
        None => return Ok(false),
    };
    let Some(model) = grok_report_context_model(report_context) else {
        return Ok(false);
    };

    let now_unix_secs = current_unix_secs();
    for attempt in 0..RUNTIME_METADATA_CAS_MAX_ATTEMPTS {
        let Some(key) = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };
        let Some(provider) = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&key.provider_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };
        if !provider.provider_type.trim().eq_ignore_ascii_case("grok") {
            return Ok(false);
        }

        let expected_namespace_value =
            upstream_metadata_namespace_value(key.upstream_metadata.as_ref(), "grok");
        let Some(grok_bucket) = expected_namespace_value
            .as_ref()
            .and_then(Value::as_object)
            .cloned()
        else {
            return Ok(false);
        };

        let mut updated_grok_bucket = grok_bucket;
        if !grok_apply_quota_feedback(
            &mut updated_grok_bucket,
            model.as_str(),
            status_code,
            grok_quota_reset_after_seconds(body_json, report_context),
            now_unix_secs,
        ) {
            return Ok(false);
        }
        grok_mark_quota_bucket_updated(&mut updated_grok_bucket, now_unix_secs);

        let namespace_value = Value::Object(updated_grok_bucket);
        let updated_upstream_metadata = merge_metadata_object(
            key.upstream_metadata.as_ref(),
            "grok",
            namespace_value.clone(),
        );
        let updated_status_snapshot = sync_provider_key_quota_status_snapshot(
            key.status_snapshot.as_ref(),
            provider.provider_type.as_str(),
            updated_upstream_metadata.as_ref(),
            "report_effect",
        );
        let persisted = state
            .update_provider_catalog_key_runtime_metadata(
                &ProviderCatalogKeyRuntimeMetadataUpdate {
                    key_id: key_id.clone(),
                    namespace: "grok".to_string(),
                    expected_upstream_metadata_value: expected_namespace_value,
                    upstream_metadata_value: namespace_value,
                    status_snapshot_patch: quota_status_snapshot_patch(
                        updated_status_snapshot.as_ref(),
                    ),
                    updated_at_unix_secs: Some(now_unix_secs),
                },
            )
            .await?;
        if persisted {
            return Ok(true);
        }
        if attempt + 1 < RUNTIME_METADATA_CAS_MAX_ATTEMPTS {
            let backoff_us = 50_u64.saturating_mul((attempt + 1) as u64).min(1_000);
            tokio::time::sleep(Duration::from_micros(backoff_us)).await;
        }
    }
    Ok(false)
}

async fn apply_local_sync_report_effect(state: &AppState, payload: &GatewaySyncReportRequest) {
    apply_local_gemini_file_mapping_report_effect(state, payload).await;
    if (200..300).contains(&payload.status_code) {
        if let Err(err) = sync_codex_quota_from_response_headers(
            state,
            payload.report_context.as_ref(),
            &payload.headers,
        )
        .await
        {
            warn!(
                event_name = "codex_realtime_quota_sync_failed",
                log_type = "ops",
                report_kind = %payload.report_kind,
                report_request_id = %short_request_id(report_request_id(payload.report_context.as_ref())),
                error = ?err,
                "gateway failed to persist codex realtime quota from sync response headers"
            );
        }
    }
    if let Err(err) = sync_grok_quota_from_report_context(
        state,
        payload.report_context.as_ref(),
        payload.status_code,
        payload.body_json.as_ref(),
    )
    .await
    {
        warn!(
            event_name = "grok_realtime_quota_sync_failed",
            log_type = "ops",
            report_kind = %payload.report_kind,
            report_request_id = %short_request_id(report_request_id(payload.report_context.as_ref())),
            error = ?err,
            "gateway failed to persist grok realtime quota from sync response"
        );
    }
    let now_unix_secs = current_unix_secs();
    if let Err(err) = sync_gemini_cli_credits_from_report(
        state,
        payload.report_context.as_ref(),
        gemini_cli_credits_from_report_context(payload.report_context.as_ref(), now_unix_secs),
    )
    .await
    {
        warn!(
            event_name = "gemini_cli_realtime_credits_sync_failed",
            log_type = "ops",
            report_kind = %payload.report_kind,
            report_request_id = %short_request_id(report_request_id(payload.report_context.as_ref())),
            error = ?err,
            "gateway failed to persist gemini cli realtime credits from sync response"
        );
    }
}

async fn apply_local_stream_report_effect(state: &AppState, payload: &GatewayStreamReportRequest) {
    let websocket_quota_seen = match sync_codex_websocket_quota_from_stream_payload(state, payload)
        .await
    {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(err) => {
            warn!(
                event_name = "codex_realtime_quota_sync_failed",
                log_type = "ops",
                report_kind = %payload.report_kind,
                report_request_id = %short_request_id(report_request_id(payload.report_context.as_ref())),
                error = ?err,
                "gateway failed to persist Codex realtime quota from WebSocket response body"
            );
            false
        }
    };
    if !websocket_quota_seen && (200..300).contains(&payload.status_code) {
        if let Err(err) = sync_codex_quota_from_response_headers(
            state,
            payload.report_context.as_ref(),
            &payload.headers,
        )
        .await
        {
            warn!(
                event_name = "codex_realtime_quota_sync_failed",
                log_type = "ops",
                report_kind = %payload.report_kind,
                report_request_id = %short_request_id(report_request_id(payload.report_context.as_ref())),
                error = ?err,
                "gateway failed to persist codex realtime quota from stream response headers"
            );
        }
    }
    if let Err(err) = sync_grok_quota_from_report_context(
        state,
        payload.report_context.as_ref(),
        payload.status_code,
        grok_upstream_response_body(payload.report_context.as_ref()),
    )
    .await
    {
        warn!(
            event_name = "grok_realtime_quota_sync_failed",
            log_type = "ops",
            report_kind = %payload.report_kind,
            report_request_id = %short_request_id(report_request_id(payload.report_context.as_ref())),
            error = ?err,
            "gateway failed to persist grok realtime quota from stream response"
        );
    }
    let now_unix_secs = current_unix_secs();
    let credits =
        gemini_cli_credits_from_report_context(payload.report_context.as_ref(), now_unix_secs)
            .or_else(|| gemini_cli_credits_from_stream_payload(payload, now_unix_secs));
    if let Err(err) =
        sync_gemini_cli_credits_from_report(state, payload.report_context.as_ref(), credits).await
    {
        warn!(
            event_name = "gemini_cli_realtime_credits_sync_failed",
            log_type = "ops",
            report_kind = %payload.report_kind,
            report_request_id = %short_request_id(report_request_id(payload.report_context.as_ref())),
            error = ?err,
            "gateway failed to persist gemini cli realtime credits from stream response"
        );
    }
}

async fn apply_local_gemini_file_mapping_report_effect(
    state: &AppState,
    payload: &GatewaySyncReportRequest,
) {
    match payload.report_kind.as_str() {
        "gemini_files_store_mapping" => {
            if payload.status_code >= 300 {
                return;
            }

            let key_id = payload
                .report_context
                .as_ref()
                .and_then(|context| context.get("file_key_id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let user_id = payload
                .report_context
                .as_ref()
                .and_then(|context| context.get("user_id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let Some(key_id) = key_id else {
                return;
            };

            for entry in extract_gemini_file_mapping_entries(payload) {
                if let Err(err) = store_local_gemini_file_mapping(
                    state,
                    entry.file_name.as_str(),
                    key_id,
                    user_id,
                    entry.display_name.as_deref(),
                    entry.mime_type.as_deref(),
                )
                .await
                {
                    warn!(
                        event_name = "gemini_file_mapping_store_failed",
                        log_type = "ops",
                        report_kind = %payload.report_kind,
                        report_request_id = %short_request_id(report_request_id(payload.report_context.as_ref())),
                        file_name = %entry.file_name,
                        error = ?err,
                        "gateway failed to persist gemini file mapping locally"
                    );
                }
            }
        }
        "gemini_files_delete_mapping" if payload.status_code < 300 => {
            let file_name = payload
                .report_context
                .as_ref()
                .and_then(|context| context.get("file_name"))
                .and_then(Value::as_str)
                .and_then(normalize_gemini_file_name);
            let Some(file_name) = file_name else {
                return;
            };

            if let Err(err) = delete_local_gemini_file_mapping(state, file_name.as_str()).await {
                warn!(
                    event_name = "gemini_file_mapping_delete_failed",
                    log_type = "ops",
                    report_kind = %payload.report_kind,
                    report_request_id = %short_request_id(report_request_id(payload.report_context.as_ref())),
                    file_name = %file_name,
                    error = ?err,
                    "gateway failed to delete gemini file mapping locally"
                );
            }
        }
        _ => {}
    }
}

pub(crate) async fn store_local_gemini_file_mapping(
    state: &AppState,
    file_name: &str,
    key_id: &str,
    user_id: Option<&str>,
    display_name: Option<&str>,
    mime_type: Option<&str>,
) -> Result<(), GatewayError> {
    let Some(file_name) = normalize_gemini_file_name(file_name) else {
        return Ok(());
    };
    let expires_at_unix_secs = current_unix_secs().saturating_add(GEMINI_FILE_MAPPING_TTL_SECONDS);

    let _stored = state
        .upsert_gemini_file_mapping(
            aether_data::repository::gemini_file_mappings::UpsertGeminiFileMappingRecord {
                id: Uuid::new_v4().to_string(),
                file_name: file_name.clone(),
                key_id: key_id.to_string(),
                user_id: user_id.map(ToOwned::to_owned),
                display_name: display_name.map(ToOwned::to_owned),
                mime_type: mime_type.map(ToOwned::to_owned),
                source_hash: None,
                expires_at_unix_secs,
            },
        )
        .await?;
    state
        .cache_set_string_with_ttl(
            gemini_file_mapping_cache_key(file_name.as_str()).as_str(),
            key_id,
            GEMINI_FILE_MAPPING_TTL_SECONDS,
        )
        .await?;
    Ok(())
}

async fn delete_local_gemini_file_mapping(
    state: &AppState,
    file_name: &str,
) -> Result<(), GatewayError> {
    let Some(file_name) = normalize_gemini_file_name(file_name) else {
        return Ok(());
    };

    let _deleted = state
        .delete_gemini_file_mapping_by_file_name(file_name.as_str())
        .await?;
    state
        .cache_delete_key(gemini_file_mapping_cache_key(file_name.as_str()).as_str())
        .await?;
    Ok(())
}

async fn sync_codex_quota_from_response_headers(
    state: &AppState,
    report_context: Option<&Value>,
    headers: &BTreeMap<String, String>,
) -> Result<bool, GatewayError> {
    let now_unix_secs = current_unix_secs();
    let observed_at_unix_secs = report_context_u64(
        report_context,
        "provider_response_headers_observed_at_unix_ms",
    )
    .map(|value| value / 1_000)
    .filter(|value| *value > 0)
    .unwrap_or(now_unix_secs);
    let request_started_at_unix_ms =
        report_context_u64(report_context, "provider_request_started_at_unix_ms");
    let request_order_id = report_context_string(report_context, "provider_request_order_id");
    let observed_reset_generation =
        report_context_u64(report_context, "codex_quota_reset_generation");
    let observed_credential_generation =
        report_context_string(report_context, "codex_credential_generation");
    let provider_headers = report_context_provider_response_headers(report_context);
    let parsed_from_provider_headers = provider_headers.as_ref().and_then(|headers| {
        admin_provider_quota_pure::parse_codex_usage_headers(headers, observed_at_unix_secs)
    });
    let Some(parsed) = parsed_from_provider_headers.or_else(|| {
        admin_provider_quota_pure::parse_codex_usage_headers(headers, observed_at_unix_secs)
    }) else {
        return Ok(false);
    };
    sync_codex_quota_metadata_with_observation(
        state,
        report_context,
        parsed,
        "response_headers",
        observed_at_unix_secs,
        request_started_at_unix_ms,
        request_order_id,
        observed_reset_generation,
        observed_credential_generation,
    )
    .await
}

async fn sync_codex_websocket_quota_from_stream_payload(
    state: &AppState,
    payload: &GatewayStreamReportRequest,
) -> Result<Option<bool>, GatewayError> {
    let now_unix_secs = current_unix_secs();
    let parsed = codex_websocket_quota_from_report_context(payload.report_context.as_ref())
        .or_else(|| codex_websocket_quota_from_stream_payload(payload, now_unix_secs));
    let Some(parsed) = parsed else {
        return Ok(None);
    };
    Ok(Some(
        sync_codex_quota_metadata(
            state,
            payload.report_context.as_ref(),
            parsed,
            "websocket_response_body",
        )
        .await?,
    ))
}

async fn sync_codex_quota_metadata(
    state: &AppState,
    report_context: Option<&Value>,
    parsed: Value,
    source: &'static str,
) -> Result<bool, GatewayError> {
    let observed_at_unix_secs = parsed
        .get("updated_at")
        .and_then(admin_provider_quota_pure::coerce_json_u64)
        .filter(|value| *value > 0)
        .unwrap_or_else(current_unix_secs);
    let request_started_at_unix_ms =
        report_context_u64(report_context, "provider_request_started_at_unix_ms");
    let request_order_id = report_context_string(report_context, "provider_request_order_id");
    let observed_reset_generation =
        report_context_u64(report_context, "codex_quota_reset_generation");
    let observed_credential_generation =
        report_context_string(report_context, "codex_credential_generation");
    sync_codex_quota_metadata_with_observation(
        state,
        report_context,
        parsed,
        source,
        observed_at_unix_secs,
        request_started_at_unix_ms,
        request_order_id,
        observed_reset_generation,
        observed_credential_generation,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn sync_codex_quota_metadata_with_observation(
    state: &AppState,
    report_context: Option<&Value>,
    parsed: Value,
    source: &'static str,
    observed_at_unix_secs: u64,
    request_started_at_unix_ms: Option<u64>,
    request_order_id: Option<&str>,
    observed_reset_generation: Option<u64>,
    observed_credential_generation: Option<&str>,
) -> Result<bool, GatewayError> {
    if admin_provider_quota_pure::codex_rate_limit_metadata_exhausted(&parsed) {
        if let Err(error) =
            install_codex_quota_exhaustion_breaker(state, report_context, &parsed, source).await
        {
            log_codex_quota_breaker_install_failure(&error);
        }
    }

    let key_id = match report_context_key_id(report_context) {
        Some(value) => value,
        None => return Ok(false),
    };
    // Runtime headers can be partial (for example only the primary window),
    // so absence never authoritatively removes another stored window.
    let coverage = admin_provider_quota_pure::CodexQuotaWindowCoverage::Patch;

    for attempt in 0..RUNTIME_METADATA_CAS_MAX_ATTEMPTS {
        let Some(key) = state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };

        let Some(provider) = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&key.provider_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };
        if !provider.provider_type.trim().eq_ignore_ascii_case("codex") {
            return Ok(false);
        }

        let expected_namespace_value =
            upstream_metadata_namespace_value(key.upstream_metadata.as_ref(), "codex");
        let Some(outcome) = admin_provider_quota_pure::merge_codex_quota_metadata_snapshot(
            expected_namespace_value.as_ref(),
            &parsed,
            admin_provider_quota_pure::CodexQuotaMergeContext {
                observed_at_unix_secs,
                request_started_at_unix_ms,
                request_order_id,
                observed_reset_generation,
                authoritative_reset_generation: None,
                observed_credential_generation,
                account_reset_fence_id: None,
                coverage,
            },
        ) else {
            return Ok(false);
        };
        if !outcome.changed {
            return Ok(false);
        }
        let next_codex = outcome.metadata;
        let updated_upstream_metadata =
            merge_metadata_object(key.upstream_metadata.as_ref(), "codex", next_codex.clone());
        let updated_status_snapshot = sync_provider_key_quota_status_snapshot(
            key.status_snapshot.as_ref(),
            provider.provider_type.as_str(),
            updated_upstream_metadata.as_ref(),
            source,
        );
        let updated = state
            .update_provider_catalog_key_runtime_metadata(
                &ProviderCatalogKeyRuntimeMetadataUpdate {
                    key_id: key_id.clone(),
                    namespace: "codex".to_string(),
                    expected_upstream_metadata_value: expected_namespace_value,
                    upstream_metadata_value: next_codex,
                    status_snapshot_patch: quota_status_snapshot_patch(
                        updated_status_snapshot.as_ref(),
                    ),
                    updated_at_unix_secs: Some(observed_at_unix_secs),
                },
            )
            .await?;
        if updated {
            return Ok(true);
        }
        if attempt + 1 < RUNTIME_METADATA_CAS_MAX_ATTEMPTS {
            let backoff_us = 50_u64.saturating_mul((attempt + 1) as u64).min(1_000);
            tokio::time::sleep(Duration::from_micros(backoff_us)).await;
        }
    }
    Ok(false)
}

pub(crate) async fn sync_codex_websocket_quota_metadata(
    state: &AppState,
    report_context: Option<&Value>,
    parsed: Value,
) -> Result<bool, GatewayError> {
    sync_codex_quota_metadata(state, report_context, parsed, "websocket_response_body").await
}

#[cfg(test)]
pub(crate) fn clear_local_report_effect_caches_for_tests() {}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use serde_json::json;
    use std::sync::Arc;

    use crate::data::GatewayDataState;

    fn codex_headers(used_percent: f64, reset_at: u64) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("x-codex-plan-type".to_string(), "free".to_string()),
            (
                "x-codex-primary-used-percent".to_string(),
                used_percent.to_string(),
            ),
            ("x-codex-primary-reset-at".to_string(), reset_at.to_string()),
            (
                "x-codex-primary-window-minutes".to_string(),
                "300".to_string(),
            ),
        ])
    }

    fn codex_test_state(key_id: &str) -> AppState {
        let provider = StoredProviderCatalogProvider::new(
            "codex-provider".to_string(),
            "Codex".to_string(),
            None,
            "codex".to_string(),
        )
        .expect("provider should build");
        let key = StoredProviderCatalogKey::new(
            key_id.to_string(),
            provider.id.clone(),
            "Codex Key".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider],
            vec![],
            vec![key],
        ));
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository),
            )
    }

    async fn stored_codex_metadata(state: &AppState, key_id: &str) -> Value {
        state
            .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .and_then(|key| key.upstream_metadata)
            .and_then(|metadata| metadata.get("codex").cloned())
            .expect("codex metadata should exist")
    }

    #[tokio::test]
    async fn out_of_order_codex_quota_reports_keep_max_usage_and_allow_next_reset() {
        clear_local_report_effect_caches_for_tests();
        let key_id = "codex-out-of-order-quota-key";
        let state = codex_test_state(key_id);
        let report_context = json!({"key_id": key_id});
        let reset_at = 2_000_000_000u64;
        let higher = codex_headers(60.0, reset_at);
        let lower = codex_headers(50.0, reset_at);

        let (higher_result, lower_result) = tokio::join!(
            sync_codex_quota_from_response_headers(&state, Some(&report_context), &higher),
            sync_codex_quota_from_response_headers(&state, Some(&report_context), &lower),
        );
        higher_result.expect("higher quota report should complete");
        lower_result.expect("lower quota report should complete");

        let stored = stored_codex_metadata(&state, key_id).await;
        assert_eq!(stored["primary_used_percent"], json!(60.0));
        assert_eq!(stored["primary_reset_at"], json!(reset_at));

        let next_reset_at = reset_at + 18_000;
        assert!(sync_codex_quota_from_response_headers(
            &state,
            Some(&report_context),
            &codex_headers(2.0, next_reset_at),
        )
        .await
        .expect("new reset quota report should complete"));
        let reset = stored_codex_metadata(&state, key_id).await;
        assert_eq!(reset["primary_used_percent"], json!(2.0));
        assert_eq!(reset["primary_reset_at"], json!(next_reset_at));
    }

    #[tokio::test]
    async fn non_success_sync_report_does_not_persist_codex_quota_headers() {
        let key_id = "codex-non-success-sync-report";
        let state = codex_test_state(key_id);
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-non-success-sync".to_string(),
            report_kind: "openai_responses_sync_error".to_string(),
            report_context: Some(json!({"key_id": key_id})),
            status_code: 401,
            headers: codex_headers(85.0, 2_000_000_000),
            body_json: None,
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        };

        apply_local_report_effect(&state, LocalReportEffect::Sync { payload: &payload }).await;

        let stored = state
            .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert!(stored.upstream_metadata.is_none());
    }

    #[tokio::test]
    async fn non_success_stream_report_does_not_persist_codex_quota_headers() {
        let key_id = "codex-non-success-stream-report";
        let state = codex_test_state(key_id);
        let payload = GatewayStreamReportRequest {
            trace_id: "trace-non-success-stream".to_string(),
            report_kind: "openai_responses_stream_error".to_string(),
            report_context: Some(json!({"key_id": key_id})),
            status_code: 429,
            headers: codex_headers(95.0, 2_000_000_000),
            provider_body_base64: None,
            provider_body_state: None,
            client_body_base64: None,
            client_body_state: None,
            terminal_summary: None,
            telemetry: None,
        };

        apply_local_report_effect(&state, LocalReportEffect::Stream { payload: &payload }).await;

        let stored = state
            .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert!(stored.upstream_metadata.is_none());
    }

    #[tokio::test]
    async fn gemini_report_metadata_write_preserves_adaptive_and_other_provider_state() {
        let provider = StoredProviderCatalogProvider::new(
            "gemini-provider".to_string(),
            "Gemini CLI".to_string(),
            None,
            "gemini_cli".to_string(),
        )
        .expect("provider should build");
        let mut key = StoredProviderCatalogKey::new(
            "gemini-key".to_string(),
            "gemini-provider".to_string(),
            "Gemini Key".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.learned_rpm_limit = Some(12);
        key.rpm_429_count = Some(3);
        key.upstream_metadata = Some(json!({
            "gemini_cli": {"credits":{"remaining":9}},
            "codex": {"remaining":7}
        }));
        key.status_snapshot = Some(json!({
            "quota": {"source":"old"},
            "observation_count": 4,
            "learning_confidence": 0.7,
            "oauth": {"invalid":false}
        }));
        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider],
            vec![],
            vec![key],
        ));
        let state = AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository),
            );

        assert!(sync_gemini_cli_credits_from_report(
            &state,
            Some(&json!({"key_id":"gemini-key"})),
            Some(json!({"remaining":3,"total":10}))
        )
        .await
        .expect("report metadata should update"));

        let stored = state
            .read_provider_catalog_keys_by_ids(&["gemini-key".to_string()])
            .await
            .expect("key should reload")
            .pop()
            .expect("key should exist");
        assert_eq!(stored.learned_rpm_limit, Some(12));
        assert_eq!(stored.rpm_429_count, Some(3));
        assert_eq!(
            stored.upstream_metadata.as_ref().unwrap()["codex"],
            json!({"remaining":7})
        );
        assert_eq!(
            stored.upstream_metadata.as_ref().unwrap()["gemini_cli"]["credits"]["remaining"],
            json!(3)
        );
        let status = stored.status_snapshot.expect("status should exist");
        assert_eq!(status["observation_count"], json!(4));
        assert_eq!(status["learning_confidence"], json!(0.7));
        assert_eq!(status["oauth"], json!({"invalid":false}));
        assert_eq!(status["quota"]["provider_type"], json!("gemini_cli"));
    }

    #[test]
    fn grok_quota_feedback_decrements_the_matching_window() {
        let mut bucket = json!({
            "quota_by_model": {
                "quota_fast": {
                    "display_name": "fast",
                    "remaining": 30.0,
                    "total": 30.0,
                    "remaining_fraction": 1.0,
                    "used_percent": 0.0,
                    "is_exhausted": false
                }
            }
        })
        .as_object()
        .cloned()
        .expect("bucket should be object");

        assert!(grok_apply_quota_feedback(
            &mut bucket,
            "grok-4.20-fast",
            200,
            None,
            1_700_000_000
        ));

        let fast = bucket
            .get("quota_by_model")
            .and_then(Value::as_object)
            .and_then(|models| models.get("quota_fast"))
            .and_then(Value::as_object)
            .expect("fast window should exist");
        assert_eq!(fast.get("remaining"), Some(&json!(29.0)));
        assert_eq!(fast.get("remaining_fraction"), Some(&json!(29.0 / 30.0)));
        assert_eq!(fast.get("used_percent"), Some(&json!(100.0 / 30.0)));
        assert_eq!(fast.get("is_exhausted"), Some(&json!(false)));
    }

    #[test]
    fn grok_report_context_model_requires_mapped_model() {
        assert_eq!(
            grok_report_context_model(Some(&json!({
                "model": "grok-4.20-0309-reasoning"
            }))),
            None
        );
        assert_eq!(
            grok_report_context_model(Some(&json!({
                "mapped_model": "grok-4.20-fast"
            }))),
            Some("grok-4.20-fast".to_string())
        );
    }

    #[test]
    fn grok_quota_feedback_zeros_rate_limited_window() {
        let mut bucket = json!({
            "quota_by_model": {
                "quota_fast": {
                    "display_name": "fast",
                    "remaining": 1.0,
                    "total": 30.0,
                    "remaining_fraction": 1.0 / 30.0,
                    "used_percent": 29.0 / 30.0 * 100.0,
                    "is_exhausted": false
                }
            }
        })
        .as_object()
        .cloned()
        .expect("bucket should be object");

        assert!(grok_apply_quota_feedback(
            &mut bucket,
            "grok-4.20-fast",
            429,
            None,
            1_700_000_000
        ));

        let fast = bucket
            .get("quota_by_model")
            .and_then(Value::as_object)
            .and_then(|models| models.get("quota_fast"))
            .and_then(Value::as_object)
            .expect("fast window should exist");
        assert_eq!(fast.get("remaining"), Some(&json!(0.0)));
        assert_eq!(fast.get("remaining_fraction"), Some(&json!(0.0)));
        assert_eq!(fast.get("used_percent"), Some(&json!(100.0)));
        assert_eq!(fast.get("is_exhausted"), Some(&json!(true)));
    }

    #[test]
    fn grok_quota_feedback_records_reset_after_when_upstream_mentions_wait_time() {
        let mut bucket = json!({
            "quota_by_model": {
                "quota_fast": {
                    "display_name": "fast",
                    "remaining": 1.0,
                    "total": 30.0,
                    "remaining_fraction": 1.0 / 30.0,
                    "used_percent": 29.0 / 30.0 * 100.0,
                    "is_exhausted": false,
                    "reset_at": 10,
                    "next_reset_at": 10
                }
            }
        })
        .as_object()
        .cloned()
        .expect("bucket should be object");

        let parsed = grok_quota_reset_after_seconds(
            Some(&json!({
                "error": {
                    "message": "升级到 SuperGrok 获得更高使用上限，或等待 6小时 13分钟。"
                }
            })),
            None,
        );

        assert_eq!(parsed, Some(22_380));
        assert!(grok_apply_quota_feedback(
            &mut bucket,
            "grok-4.20-fast",
            503,
            parsed,
            1_700_000_000
        ));

        let fast = bucket
            .get("quota_by_model")
            .and_then(Value::as_object)
            .and_then(|models| models.get("quota_fast"))
            .and_then(Value::as_object)
            .expect("fast window should exist");
        assert_eq!(fast.get("remaining"), Some(&json!(0.0)));
        assert_eq!(fast.get("is_exhausted"), Some(&json!(true)));
        assert_eq!(fast.get("reset_after_seconds"), Some(&json!(22_380u64)));
        assert_eq!(fast.get("reset_at"), Some(&json!(1_700_022_380u64)));
        assert_eq!(fast.get("next_reset_at"), Some(&json!(1_700_022_380u64)));
        assert_eq!(
            fast.get("reset_at_source"),
            Some(&json!("grok_upstream_error"))
        );
    }

    #[test]
    fn grok_realtime_quota_bucket_updates_observed_timestamp() {
        let mut bucket = json!({
            "updated_at": 1_600_000_000u64,
            "quota_by_model": {
                "quota_fast": {
                    "display_name": "fast",
                    "remaining": 1.0,
                    "total": 30.0,
                    "remaining_fraction": 1.0 / 30.0,
                    "used_percent": 29.0 / 30.0 * 100.0,
                    "is_exhausted": false
                }
            }
        })
        .as_object()
        .cloned()
        .expect("bucket should be object");

        grok_mark_quota_bucket_updated(&mut bucket, 1_700_000_000);

        assert_eq!(bucket.get("updated_at"), Some(&json!(1_700_000_000u64)));
    }

    #[test]
    fn grok_wait_duration_parser_handles_english_and_chinese_messages() {
        assert_eq!(
            grok_wait_duration_seconds_from_text("wait 6h 13m"),
            Some(22_380)
        );
        assert_eq!(
            grok_wait_duration_seconds_from_text("等待 6小时13分钟"),
            Some(22_380)
        );
        assert_eq!(
            grok_wait_duration_seconds_from_text("no duration here"),
            None
        );
    }
}
