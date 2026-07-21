use super::cache_types::AdminMonitoringCacheAffinityRecord;
use crate::cache::SchedulerAffinityTarget;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
enum AdminMonitoringAffinityKeyKind {
    Cache,
    Scheduler,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedAdminMonitoringAffinityKey {
    affinity_key: String,
    api_format: String,
    model_name: String,
    client_family: Option<String>,
    session_hash: Option<String>,
}

fn split_admin_monitoring_api_format_and_model(
    segments: &[&str],
    key_kind: AdminMonitoringAffinityKeyKind,
) -> Option<(String, String)> {
    if segments.len() < 2 {
        return None;
    }

    let max_api_format_segments = segments.len().saturating_sub(1).min(3);
    for segment_count in (2..=max_api_format_segments).rev() {
        let candidate = segments[..segment_count]
            .iter()
            .map(|segment| segment.trim())
            .collect::<Vec<_>>()
            .join(":");
        if is_known_admin_monitoring_api_format(&candidate) {
            let model_name = segments[segment_count..]
                .iter()
                .map(|segment| segment.trim())
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>()
                .join(":");
            if model_name.is_empty() {
                return None;
            }
            return Some((candidate, model_name));
        }
    }

    if matches!(key_kind, AdminMonitoringAffinityKeyKind::Scheduler)
        && segments.len() >= 3
        && is_known_admin_monitoring_api_format_family(segments[0])
    {
        let api_format_kind = segments.get(1)?.trim();
        if api_format_kind.is_empty() {
            return None;
        }
        let model_name = segments[2..]
            .iter()
            .map(|segment| segment.trim())
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>()
            .join(":");
        if model_name.is_empty() {
            return None;
        }
        return Some((
            format!(
                "{}:{api_format_kind}",
                segments[0].trim().to_ascii_lowercase()
            ),
            model_name,
        ));
    }

    let api_format = segments.first()?.trim();
    if api_format.is_empty() {
        return None;
    }
    let model_name = segments[1..]
        .iter()
        .map(|segment| segment.trim())
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join(":");
    if model_name.is_empty() {
        return None;
    }

    Some((api_format.to_string(), model_name))
}

fn is_known_admin_monitoring_api_format_family(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "openai" | "claude" | "gemini" | "jina" | "doubao" | "aliyun"
    )
}

fn is_known_admin_monitoring_api_format(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "openai:chat"
            | "openai:responses"
            | "openai:responses:compact"
            | "openai:search"
            | "openai:image"
            | "openai:video"
            | "openai:embedding"
            | "openai:rerank"
            | "claude:messages"
            | "gemini:generate_content"
            | "gemini:video"
            | "gemini:files"
            | "gemini:embedding"
            | "jina:embedding"
            | "jina:rerank"
            | "doubao:embedding"
            | "aliyun:multimodal_embedding"
    )
}

fn parse_admin_monitoring_cache_affinity_key(
    raw_key: &str,
) -> Option<ParsedAdminMonitoringAffinityKey> {
    let parts = raw_key.split(':').collect::<Vec<_>>();
    let start = parts
        .iter()
        .position(|segment| *segment == "cache_affinity")?;
    let affinity_key = parts.get(start + 1)?.trim();
    if affinity_key.is_empty() {
        return None;
    }
    let remaining = parts.get(start + 2..)?;
    let (api_format, model_name) = split_admin_monitoring_api_format_and_model(
        remaining,
        AdminMonitoringAffinityKeyKind::Cache,
    )
    .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));
    Some(ParsedAdminMonitoringAffinityKey {
        affinity_key: affinity_key.to_string(),
        api_format,
        model_name,
        client_family: None,
        session_hash: None,
    })
}

fn parse_admin_monitoring_scheduler_affinity_key(
    raw_key: &str,
) -> Option<ParsedAdminMonitoringAffinityKey> {
    let parts = raw_key.split(':').collect::<Vec<_>>();
    let start = parts
        .iter()
        .position(|segment| *segment == "scheduler_affinity")?;
    if parts.get(start + 1).is_some_and(|segment| *segment == "v2") {
        return parse_admin_monitoring_scheduler_affinity_v2_key(&parts, start);
    }

    let affinity_key = parts.get(start + 1)?.trim();
    if affinity_key.is_empty() {
        return None;
    }

    let remaining = parts.get(start + 2..)?;
    let (api_format, model_name) = split_admin_monitoring_api_format_and_model(
        remaining,
        AdminMonitoringAffinityKeyKind::Scheduler,
    )?;

    Some(ParsedAdminMonitoringAffinityKey {
        affinity_key: affinity_key.to_string(),
        api_format,
        model_name,
        client_family: None,
        session_hash: None,
    })
}

fn parse_admin_monitoring_scheduler_affinity_v2_key(
    parts: &[&str],
    start: usize,
) -> Option<ParsedAdminMonitoringAffinityKey> {
    let affinity_key = parts.get(start + 2)?.trim();
    if affinity_key.is_empty() {
        return None;
    }

    let remaining = parts.get(start + 3..)?;
    if remaining.len() < 4 {
        return None;
    }
    let session_hash = remaining.last()?.trim();
    let client_family = remaining.get(remaining.len().saturating_sub(2))?.trim();
    if session_hash.is_empty() || client_family.is_empty() {
        return None;
    }

    let api_model_segments = &remaining[..remaining.len() - 2];
    let (api_format, model_name) = split_admin_monitoring_api_format_and_model(
        api_model_segments,
        AdminMonitoringAffinityKeyKind::Scheduler,
    )?;

    Some(ParsedAdminMonitoringAffinityKey {
        affinity_key: affinity_key.to_string(),
        api_format,
        model_name,
        client_family: Some(client_family.to_ascii_lowercase()),
        session_hash: Some(session_hash.to_string()),
    })
}

pub(super) fn admin_monitoring_scheduler_affinity_cache_key(
    record: &AdminMonitoringCacheAffinityRecord,
) -> Option<String> {
    let affinity_key = record.affinity_key.trim();
    let api_format = record.api_format.trim().to_ascii_lowercase();
    let model_name = record.model_name.trim();
    if affinity_key.is_empty() || api_format.is_empty() || model_name.is_empty() {
        return None;
    }
    match (
        record
            .client_family
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
        record
            .session_hash
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    ) {
        (Some(client_family), Some(session_hash)) => Some(format!(
            "scheduler_affinity:v2:{affinity_key}:{api_format}:{model_name}:{client_family}:{session_hash}"
        )),
        _ => Some(format!(
            "scheduler_affinity:{affinity_key}:{api_format}:{model_name}"
        )),
    }
}

fn admin_monitoring_json_string_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Option<String> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn admin_monitoring_json_request_count(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Option<u64> {
    object.get("request_count").and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
    })
}

fn admin_monitoring_json_scheduler_affinity_epoch(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Option<u64> {
    object
        .get("scheduler_affinity_epoch")
        .and_then(serde_json::Value::as_u64)
}

pub(super) fn admin_monitoring_cache_affinity_record(
    raw_key: &str,
    raw_value: &str,
) -> Option<AdminMonitoringCacheAffinityRecord> {
    let payload = serde_json::from_str::<serde_json::Value>(raw_value).ok()?;
    let object = payload.as_object()?;
    let parsed = parse_admin_monitoring_cache_affinity_key(raw_key)?;
    let api_format = admin_monitoring_json_string_field(object, "api_format")
        .unwrap_or_else(|| parsed.api_format.clone());
    let model_name = admin_monitoring_json_string_field(object, "model_name")
        .unwrap_or_else(|| parsed.model_name.clone());
    let request_count = admin_monitoring_json_request_count(object);
    Some(AdminMonitoringCacheAffinityRecord {
        raw_key: raw_key.to_string(),
        affinity_key: parsed.affinity_key,
        api_format,
        model_name,
        client_family: admin_monitoring_json_string_field(object, "client_family")
            .map(|value| value.to_ascii_lowercase())
            .or(parsed.client_family),
        session_hash: admin_monitoring_json_string_field(object, "session_hash")
            .or(parsed.session_hash),
        provider_id: object
            .get("provider_id")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        endpoint_id: object
            .get("endpoint_id")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        key_id: object
            .get("key_id")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        created_at: object.get("created_at").cloned(),
        expire_at: object.get("expire_at").cloned(),
        request_count: request_count.unwrap_or(0),
        request_count_known: request_count.is_some(),
        scheduler_affinity_epoch: None,
    })
}

pub(super) fn admin_monitoring_scheduler_affinity_record(
    cache_key: &str,
    target: &SchedulerAffinityTarget,
    epoch: u64,
    age: Duration,
    ttl: Duration,
    now_unix_secs: u64,
) -> Option<AdminMonitoringCacheAffinityRecord> {
    let parsed = parse_admin_monitoring_scheduler_affinity_key(cache_key)?;
    let age_secs = age.as_secs();
    let created_at = now_unix_secs.saturating_sub(age_secs);
    let expire_at = created_at.saturating_add(ttl.as_secs());

    Some(AdminMonitoringCacheAffinityRecord {
        raw_key: cache_key.to_string(),
        affinity_key: parsed.affinity_key,
        api_format: parsed.api_format,
        model_name: parsed.model_name,
        client_family: parsed.client_family,
        session_hash: parsed.session_hash,
        provider_id: Some(target.provider_id.clone()),
        endpoint_id: Some(target.endpoint_id.clone()),
        key_id: Some(target.key_id.clone()),
        created_at: Some(serde_json::json!(created_at)),
        expire_at: Some(serde_json::json!(expire_at)),
        request_count: 0,
        request_count_known: false,
        scheduler_affinity_epoch: Some(epoch),
    })
}

pub(super) fn admin_monitoring_scheduler_affinity_record_from_raw(
    raw_key: &str,
    raw_value: &str,
) -> Option<AdminMonitoringCacheAffinityRecord> {
    let payload = serde_json::from_str::<serde_json::Value>(raw_value).ok()?;
    let object = payload.as_object()?;
    let parsed = parse_admin_monitoring_scheduler_affinity_key(raw_key)?;
    let api_format = admin_monitoring_json_string_field(object, "api_format")
        .unwrap_or_else(|| parsed.api_format.clone());
    let model_name = admin_monitoring_json_string_field(object, "model_name")
        .unwrap_or_else(|| parsed.model_name.clone());
    let request_count = admin_monitoring_json_request_count(object);

    Some(AdminMonitoringCacheAffinityRecord {
        raw_key: raw_key.to_string(),
        affinity_key: parsed.affinity_key,
        api_format,
        model_name,
        client_family: admin_monitoring_json_string_field(object, "client_family")
            .map(|value| value.to_ascii_lowercase())
            .or(parsed.client_family),
        session_hash: admin_monitoring_json_string_field(object, "session_hash")
            .or(parsed.session_hash),
        provider_id: object
            .get("provider_id")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        endpoint_id: object
            .get("endpoint_id")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        key_id: object
            .get("key_id")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned),
        created_at: object.get("created_at").cloned(),
        expire_at: object.get("expire_at").cloned(),
        request_count: request_count.unwrap_or(0),
        request_count_known: request_count.is_some(),
        scheduler_affinity_epoch: admin_monitoring_json_scheduler_affinity_epoch(object),
    })
}

pub(super) fn admin_monitoring_cache_affinity_record_identity(
    record: &AdminMonitoringCacheAffinityRecord,
) -> String {
    admin_monitoring_scheduler_affinity_cache_key(record).unwrap_or_else(|| record.raw_key.clone())
}

pub(super) fn clear_admin_monitoring_scheduler_affinity_entries(
    state: &AdminAppState<'_>,
    records: &[AdminMonitoringCacheAffinityRecord],
) {
    let mut scheduler_keys = std::collections::BTreeSet::new();
    for record in records {
        if record.raw_key.contains("scheduler_affinity:") {
            scheduler_keys.insert(record.raw_key.clone());
        }
        if let Some(cache_key) = admin_monitoring_scheduler_affinity_cache_key(record) {
            scheduler_keys.insert(cache_key);
        }
    }
    for scheduler_key in scheduler_keys {
        let _ = state
            .as_ref()
            .remove_scheduler_affinity_cache_entry(&scheduler_key);
    }
}

#[cfg(test)]
pub(super) fn delete_admin_monitoring_cache_affinity_entries_for_tests(
    state: &AdminAppState<'_>,
    raw_keys: &[String],
) -> usize {
    state
        .as_ref()
        .remove_admin_monitoring_cache_affinity_entries_for_tests(raw_keys)
}

#[cfg(not(test))]
pub(super) fn delete_admin_monitoring_cache_affinity_entries_for_tests(
    _state: &AdminAppState<'_>,
    _raw_keys: &[String],
) -> usize {
    0
}

pub(super) async fn delete_admin_monitoring_cache_affinity_raw_keys(
    state: &AdminAppState<'_>,
    raw_keys: &[String],
) -> Result<usize, GatewayError> {
    if raw_keys.is_empty() {
        return Ok(0);
    }

    let deleted = state
        .runtime_state()
        .kv_delete_many(raw_keys)
        .await
        .map_err(|err| GatewayError::Internal(format!("runtime cache delete failed: {err}")))?;
    if deleted > 0 {
        return Ok(deleted);
    }

    Ok(delete_admin_monitoring_cache_affinity_entries_for_tests(
        state, raw_keys,
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        admin_monitoring_scheduler_affinity_record_from_raw,
        parse_admin_monitoring_cache_affinity_key, parse_admin_monitoring_scheduler_affinity_key,
    };
    use serde_json::json;

    #[test]
    fn parses_legacy_scheduler_affinity_key_with_multi_segment_api_format() {
        let parsed = parse_admin_monitoring_scheduler_affinity_key(
            "scheduler_affinity:user-key-1:openai:chat:gpt-4.1",
        )
        .expect("legacy scheduler key should parse");

        assert_eq!(parsed.affinity_key, "user-key-1");
        assert_eq!(parsed.api_format, "openai:chat");
        assert_eq!(parsed.model_name, "gpt-4.1");
        assert_eq!(parsed.client_family, None);
        assert_eq!(parsed.session_hash, None);
    }

    #[test]
    fn parses_v2_scheduler_affinity_key_with_client_family() {
        let parsed = parse_admin_monitoring_scheduler_affinity_key(
            "scheduler_affinity:v2:user-key-1:openai:responses:gpt-5.5:codex:d35efdccd572e9c5430e17d663dfde4cce83ea7e6665ac332f565780c98b1dff",
        )
        .expect("v2 scheduler key should parse");

        assert_eq!(parsed.affinity_key, "user-key-1");
        assert_eq!(parsed.api_format, "openai:responses");
        assert_eq!(parsed.model_name, "gpt-5.5");
        assert_eq!(parsed.client_family.as_deref(), Some("codex"));
        assert_eq!(
            parsed.session_hash.as_deref(),
            Some("d35efdccd572e9c5430e17d663dfde4cce83ea7e6665ac332f565780c98b1dff")
        );
    }

    #[test]
    fn parses_v2_scheduler_affinity_key_for_openai_search() {
        let parsed = parse_admin_monitoring_scheduler_affinity_key(
            "scheduler_affinity:v2:user-key-1:openai:search:gpt-5.6-sol:codex:sessionhash",
        )
        .expect("Search scheduler key should parse");

        assert_eq!(parsed.affinity_key, "user-key-1");
        assert_eq!(parsed.api_format, "openai:search");
        assert_eq!(parsed.model_name, "gpt-5.6-sol");
        assert_eq!(parsed.client_family.as_deref(), Some("codex"));
        assert_eq!(parsed.session_hash.as_deref(), Some("sessionhash"));
    }

    #[test]
    fn parses_v2_scheduler_affinity_key_with_three_segment_api_format() {
        let parsed = parse_admin_monitoring_scheduler_affinity_key(
            "scheduler_affinity:v2:user-key-1:openai:responses:compact:gpt-5.5:opencode:sessionhash",
        )
        .expect("compact v2 scheduler key should parse");

        assert_eq!(parsed.affinity_key, "user-key-1");
        assert_eq!(parsed.api_format, "openai:responses:compact");
        assert_eq!(parsed.model_name, "gpt-5.5");
        assert_eq!(parsed.client_family.as_deref(), Some("opencode"));
        assert_eq!(parsed.session_hash.as_deref(), Some("sessionhash"));
    }

    #[test]
    fn parses_scheduler_affinity_key_with_unregistered_two_segment_api_format() {
        let parsed = parse_admin_monitoring_scheduler_affinity_key(
            "scheduler_affinity:v2:user-key-1:openai:image:gpt-image-1:opencode:sessionhash",
        )
        .expect("image v2 scheduler key should parse");

        assert_eq!(parsed.affinity_key, "user-key-1");
        assert_eq!(parsed.api_format, "openai:image");
        assert_eq!(parsed.model_name, "gpt-image-1");
        assert_eq!(parsed.client_family.as_deref(), Some("opencode"));
        assert_eq!(parsed.session_hash.as_deref(), Some("sessionhash"));
    }

    #[test]
    fn keeps_legacy_cache_affinity_single_segment_api_format() {
        let parsed = parse_admin_monitoring_cache_affinity_key(
            "cache_affinity:user-key-1:openai:model-alpha",
        )
        .expect("cache affinity key should parse");

        assert_eq!(parsed.affinity_key, "user-key-1");
        assert_eq!(parsed.api_format, "openai");
        assert_eq!(parsed.model_name, "model-alpha");
    }

    #[test]
    fn scheduler_raw_record_exposes_client_family_and_known_count() {
        let raw_value = json!({
            "provider_id": "provider-1",
            "endpoint_id": "endpoint-1",
            "key_id": "provider-key-1",
            "created_at": 1710000000u64,
            "expire_at": 1710000300u64,
            "request_count": 3u64,
        })
        .to_string();

        let record = admin_monitoring_scheduler_affinity_record_from_raw(
            "scheduler_affinity:v2:user-key-1:openai:responses:gpt-5.5:codex:sessionhash",
            &raw_value,
        )
        .expect("raw scheduler affinity should parse");

        assert_eq!(record.affinity_key, "user-key-1");
        assert_eq!(record.api_format, "openai:responses");
        assert_eq!(record.model_name, "gpt-5.5");
        assert_eq!(record.client_family.as_deref(), Some("codex"));
        assert_eq!(record.session_hash.as_deref(), Some("sessionhash"));
        assert_eq!(record.request_count, 3);
        assert!(record.request_count_known);
    }

    #[test]
    fn scheduler_raw_record_treats_zero_request_count_as_known() {
        let raw_value = json!({
            "provider_id": "provider-1",
            "endpoint_id": "endpoint-1",
            "key_id": "provider-key-1",
            "created_at": 1710000000u64,
            "expire_at": 1710000300u64,
            "request_count": 0u64,
        })
        .to_string();

        let record = admin_monitoring_scheduler_affinity_record_from_raw(
            "scheduler_affinity:v2:user-key-1:openai:responses:gpt-5.5:codex:sessionhash",
            &raw_value,
        )
        .expect("raw scheduler affinity should parse");

        assert_eq!(record.request_count, 0);
        assert!(record.request_count_known);
    }
}
