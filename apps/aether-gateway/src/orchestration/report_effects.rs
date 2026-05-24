use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use aether_admin::provider::quota as admin_provider_quota_pure;
use aether_provider_pool::grok_quota_window_key_for_model;
use aether_usage_runtime::{
    extract_gemini_file_mapping_entries, gemini_file_mapping_cache_key, normalize_gemini_file_name,
    report_request_id, GatewayStreamReportRequest, GatewaySyncReportRequest,
    GEMINI_FILE_MAPPING_TTL_SECONDS,
};
use regex::Regex;
use serde_json::{json, Value};
use tracing::warn;
use uuid::Uuid;

use crate::clock::current_unix_secs;
use crate::handlers::shared::sync_provider_key_quota_status_snapshot;
use crate::log_ids::short_request_id;
use crate::{AppState, GatewayError};

const CODEX_QUOTA_CACHE_TTL_SECONDS: u64 = 30;
const CODEX_QUOTA_CACHE_MAX_ENTRIES: usize = 4096;

type HeaderFingerprintCache = Mutex<HashMap<String, (String, Instant)>>;

static CODEX_QUOTA_HEADER_FINGERPRINT_CACHE: OnceLock<HeaderFingerprintCache> = OnceLock::new();
static GROK_CHINESE_WAIT_DURATION_RE: OnceLock<Regex> = OnceLock::new();
static GROK_ENGLISH_WAIT_DURATION_RE: OnceLock<Regex> = OnceLock::new();

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

fn codex_quota_header_fingerprint_cache() -> &'static HeaderFingerprintCache {
    CODEX_QUOTA_HEADER_FINGERPRINT_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn report_context_key_id(report_context: Option<&Value>) -> Option<String> {
    report_context
        .and_then(|context| context.get("key_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
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

fn is_volatile_compare_field(key: &str) -> bool {
    key == "updated_at" || key.ends_with("_reset_seconds") || key.ends_with("_reset_after_seconds")
}

fn canonicalize_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_value).collect()),
        Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            let mut normalized = serde_json::Map::new();
            for (key, value) in entries {
                normalized.insert(key.clone(), canonicalize_value(value));
            }
            Value::Object(normalized)
        }
        _ => value.clone(),
    }
}

fn fingerprint_codex_payload(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    let mut entries = object
        .iter()
        .filter(|(key, _)| !is_volatile_compare_field(key))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(right.0));

    let mut normalized = serde_json::Map::new();
    for (key, value) in entries {
        normalized.insert(key.clone(), canonicalize_value(value));
    }
    serde_json::to_string(&Value::Object(normalized)).ok()
}

fn get_cached_codex_quota_fingerprint(key_id: &str, now: Instant) -> Option<String> {
    let mut cache = codex_quota_header_fingerprint_cache()
        .lock()
        .expect("codex realtime quota cache should lock");
    match cache.get(key_id) {
        Some((fingerprint, expires_at)) if *expires_at > now => Some(fingerprint.clone()),
        Some(_) => {
            cache.remove(key_id);
            None
        }
        None => None,
    }
}

fn set_cached_codex_quota_fingerprint(key_id: &str, fingerprint: String, now: Instant) {
    let mut cache = codex_quota_header_fingerprint_cache()
        .lock()
        .expect("codex realtime quota cache should lock");
    cache.insert(
        key_id.to_string(),
        (
            fingerprint,
            now.checked_add(Duration::from_secs(CODEX_QUOTA_CACHE_TTL_SECONDS))
                .unwrap_or(now),
        ),
    );

    cache.retain(|_, (_, expires_at)| *expires_at > now);
    if cache.len() <= CODEX_QUOTA_CACHE_MAX_ENTRIES {
        return;
    }

    let mut entries = cache
        .iter()
        .map(|(key, (_, expires_at))| (key.clone(), *expires_at))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.1);
    for (key, _) in entries
        .into_iter()
        .take(cache.len() - CODEX_QUOTA_CACHE_MAX_ENTRIES)
    {
        cache.remove(&key);
    }
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

    let Some(grok_bucket) = key
        .upstream_metadata
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("grok"))
        .and_then(Value::as_object)
        .cloned()
    else {
        return Ok(false);
    };

    let mut updated_grok_bucket = grok_bucket;
    let now_unix_secs = current_unix_secs();
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

    let updated_upstream_metadata = merge_metadata_object(
        key.upstream_metadata.as_ref(),
        "grok",
        Value::Object(updated_grok_bucket),
    );
    let updated_status_snapshot = sync_provider_key_quota_status_snapshot(
        key.status_snapshot.as_ref(),
        provider.provider_type.as_str(),
        updated_upstream_metadata.as_ref(),
        "report_effect",
    );
    let mut updated_key = key;
    updated_key.upstream_metadata = updated_upstream_metadata;
    updated_key.status_snapshot = updated_status_snapshot;
    updated_key.updated_at_unix_secs = Some(now_unix_secs);

    Ok(state
        .update_provider_catalog_key_runtime_state(&updated_key)
        .await?
        .is_some())
}

async fn apply_local_sync_report_effect(state: &AppState, payload: &GatewaySyncReportRequest) {
    apply_local_gemini_file_mapping_report_effect(state, payload).await;
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
}

async fn apply_local_stream_report_effect(state: &AppState, payload: &GatewayStreamReportRequest) {
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
    let key_id = match report_context_key_id(report_context) {
        Some(value) => value,
        None => return Ok(false),
    };

    let now_unix_secs = current_unix_secs();
    let provider_headers = report_context_provider_response_headers(report_context);
    let parsed_from_provider_headers = provider_headers.as_ref().and_then(|headers| {
        admin_provider_quota_pure::parse_codex_usage_headers(headers, now_unix_secs)
    });
    let Some(parsed) = parsed_from_provider_headers
        .or_else(|| admin_provider_quota_pure::parse_codex_usage_headers(headers, now_unix_secs))
    else {
        return Ok(false);
    };
    let Some(incoming_fingerprint) = fingerprint_codex_payload(&parsed) else {
        return Ok(false);
    };

    let now = Instant::now();
    if get_cached_codex_quota_fingerprint(&key_id, now).as_deref()
        == Some(incoming_fingerprint.as_str())
    {
        return Ok(false);
    }

    let Some(key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
        .await?
        .into_iter()
        .next()
    else {
        set_cached_codex_quota_fingerprint(&key_id, incoming_fingerprint, now);
        return Ok(false);
    };

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&key.provider_id))
        .await?
        .into_iter()
        .next()
    else {
        set_cached_codex_quota_fingerprint(&key_id, incoming_fingerprint, now);
        return Ok(false);
    };
    if !provider.provider_type.trim().eq_ignore_ascii_case("codex") {
        set_cached_codex_quota_fingerprint(&key_id, incoming_fingerprint, now);
        return Ok(false);
    }

    let current_codex = key
        .upstream_metadata
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("codex"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(serde_json::Map::new);
    let current_codex = Value::Object(current_codex);
    let Some(current_fingerprint) = fingerprint_codex_payload(&current_codex) else {
        set_cached_codex_quota_fingerprint(&key_id, incoming_fingerprint, now);
        return Ok(false);
    };
    if current_fingerprint == incoming_fingerprint {
        set_cached_codex_quota_fingerprint(&key_id, incoming_fingerprint, now);
        return Ok(false);
    }

    let updated_upstream_metadata =
        merge_metadata_object(key.upstream_metadata.as_ref(), "codex", parsed);
    let updated_status_snapshot = sync_provider_key_quota_status_snapshot(
        key.status_snapshot.as_ref(),
        provider.provider_type.as_str(),
        updated_upstream_metadata.as_ref(),
        "response_headers",
    );
    let mut updated_key = key;
    updated_key.upstream_metadata = updated_upstream_metadata;
    updated_key.status_snapshot = updated_status_snapshot;
    updated_key.updated_at_unix_secs = Some(now_unix_secs);

    let updated = state
        .update_provider_catalog_key_runtime_state(&updated_key)
        .await?
        .is_some();
    if updated {
        set_cached_codex_quota_fingerprint(&key_id, incoming_fingerprint, now);
    }
    Ok(updated)
}

#[cfg(test)]
pub(crate) fn clear_local_report_effect_caches_for_tests() {
    if let Some(cache) = CODEX_QUOTA_HEADER_FINGERPRINT_CACHE.get() {
        cache
            .lock()
            .expect("codex realtime quota cache should lock")
            .clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
