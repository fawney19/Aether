use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::clock::current_unix_secs;
use crate::{AppState, GatewayError};
use aether_admin::provider::quota as admin_provider_quota_pure;
use serde_json::Value;

const CACHE_TTL_SECONDS: u64 = 30;
const CACHE_MAX_ENTRIES: usize = 4096;

type HeaderFingerprintCache = Mutex<HashMap<String, (String, Instant)>>;

static HEADER_FINGERPRINT_CACHE: OnceLock<HeaderFingerprintCache> = OnceLock::new();

fn header_fingerprint_cache() -> &'static HeaderFingerprintCache {
    HEADER_FINGERPRINT_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn report_context_key_id(report_context: Option<&Value>) -> Option<String> {
    report_context
        .and_then(|context| context.get("key_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
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

fn get_cached_fingerprint(key_id: &str, now: Instant) -> Option<String> {
    let mut cache = header_fingerprint_cache()
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

fn set_cached_fingerprint(key_id: &str, fingerprint: String, now: Instant) {
    let mut cache = header_fingerprint_cache()
        .lock()
        .expect("codex realtime quota cache should lock");
    cache.insert(
        key_id.to_string(),
        (
            fingerprint,
            now.checked_add(Duration::from_secs(CACHE_TTL_SECONDS))
                .unwrap_or(now),
        ),
    );

    cache.retain(|_, (_, expires_at)| *expires_at > now);
    if cache.len() <= CACHE_MAX_ENTRIES {
        return;
    }

    let mut entries = cache
        .iter()
        .map(|(key, (_, expires_at))| (key.clone(), *expires_at))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.1.cmp(&right.1));
    for (key, _) in entries.into_iter().take(cache.len() - CACHE_MAX_ENTRIES) {
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

pub(super) async fn sync_codex_quota_from_response_headers(
    state: &AppState,
    report_context: Option<&Value>,
    headers: &BTreeMap<String, String>,
) -> Result<bool, GatewayError> {
    let key_id = match report_context_key_id(report_context) {
        Some(value) => value,
        None => return Ok(false),
    };

    let now_unix_secs = current_unix_secs();
    let Some(parsed) = admin_provider_quota_pure::parse_codex_usage_headers(headers, now_unix_secs)
    else {
        return Ok(false);
    };
    let Some(incoming_fingerprint) = fingerprint_codex_payload(&parsed) else {
        return Ok(false);
    };

    let now = Instant::now();
    if get_cached_fingerprint(&key_id, now).as_deref() == Some(incoming_fingerprint.as_str()) {
        return Ok(false);
    }

    let Some(mut key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
        .await?
        .into_iter()
        .next()
    else {
        set_cached_fingerprint(&key_id, incoming_fingerprint, now);
        return Ok(false);
    };

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&key.provider_id))
        .await?
        .into_iter()
        .next()
    else {
        set_cached_fingerprint(&key_id, incoming_fingerprint, now);
        return Ok(false);
    };
    if !provider.provider_type.trim().eq_ignore_ascii_case("codex") {
        set_cached_fingerprint(&key_id, incoming_fingerprint, now);
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
        set_cached_fingerprint(&key_id, incoming_fingerprint, now);
        return Ok(false);
    };

    let mut merged_codex = current_codex
        .as_object()
        .cloned()
        .unwrap_or_else(serde_json::Map::new);
    let Some(parsed_object) = parsed.as_object() else {
        set_cached_fingerprint(&key_id, incoming_fingerprint, now);
        return Ok(false);
    };
    for (field, value) in parsed_object {
        merged_codex.insert(field.clone(), value.clone());
    }
    let merged_codex = Value::Object(merged_codex);
    let Some(merged_fingerprint) = fingerprint_codex_payload(&merged_codex) else {
        set_cached_fingerprint(&key_id, incoming_fingerprint, now);
        return Ok(false);
    };
    if current_fingerprint == merged_fingerprint {
        set_cached_fingerprint(&key_id, incoming_fingerprint, now);
        return Ok(false);
    }

    key.upstream_metadata =
        merge_metadata_object(key.upstream_metadata.as_ref(), "codex", merged_codex);
    key.updated_at_unix_secs = Some(now_unix_secs);

    let updated = state.update_provider_catalog_key(&key).await?.is_some();
    if updated {
        set_cached_fingerprint(&key_id, incoming_fingerprint, now);
    }
    Ok(updated)
}

#[cfg(test)]
pub(super) fn clear_codex_quota_fingerprint_cache() {
    if let Some(cache) = HEADER_FINGERPRINT_CACHE.get() {
        cache
            .lock()
            .expect("codex realtime quota cache should lock")
            .clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{clear_codex_quota_fingerprint_cache, sync_codex_quota_from_response_headers};
    use crate::data::GatewayDataState;
    use crate::AppState;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogReadRepository, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    };
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn build_state(repository: Arc<InMemoryProviderCatalogReadRepository>) -> AppState {
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_provider_catalog_repository_for_tests(repository),
            )
    }

    fn sample_provider(provider_id: &str, provider_type: &str) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            provider_id.to_string(),
            provider_type.to_string(),
            None,
            provider_type.to_string(),
        )
        .expect("provider should build")
    }

    fn sample_key(
        key_id: &str,
        provider_id: &str,
        upstream_metadata: Option<Value>,
    ) -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            key_id.to_string(),
            provider_id.to_string(),
            "default".to_string(),
            "bearer".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!(["openai:cli"])),
            "sk-codex-test".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build");
        key.upstream_metadata = upstream_metadata;
        key
    }

    fn paid_headers(
        primary_used_percent: &str,
        secondary_used_percent: &str,
        primary_reset_after_seconds: &str,
        secondary_reset_after_seconds: &str,
    ) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("x-codex-plan-type".to_string(), "team".to_string()),
            (
                "x-codex-primary-used-percent".to_string(),
                primary_used_percent.to_string(),
            ),
            (
                "x-codex-secondary-used-percent".to_string(),
                secondary_used_percent.to_string(),
            ),
            (
                "x-codex-primary-window-minutes".to_string(),
                "300".to_string(),
            ),
            (
                "x-codex-secondary-window-minutes".to_string(),
                "10080".to_string(),
            ),
            (
                "x-codex-primary-reset-after-seconds".to_string(),
                primary_reset_after_seconds.to_string(),
            ),
            (
                "x-codex-secondary-reset-after-seconds".to_string(),
                secondary_reset_after_seconds.to_string(),
            ),
            (
                "x-codex-primary-reset-at".to_string(),
                "1776148929".to_string(),
            ),
            (
                "x-codex-secondary-reset-at".to_string(),
                "1776657828".to_string(),
            ),
            (
                "x-codex-credits-has-credits".to_string(),
                "False".to_string(),
            ),
            ("x-codex-credits-balance".to_string(), "".to_string()),
            ("x-codex-credits-unlimited".to_string(), "False".to_string()),
        ])
    }

    #[tokio::test]
    async fn sync_codex_quota_updates_paid_windows_and_preserves_existing_fields() {
        clear_codex_quota_fingerprint_cache();

        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-codex", "codex")],
            Vec::new(),
            vec![sample_key(
                "key-codex-1",
                "provider-codex",
                Some(json!({
                    "codex": {
                        "legacy_marker": "keep-me",
                        "secondary_used_percent": 2.0
                    },
                    "other": {
                        "value": true
                    }
                })),
            )],
        ));
        let state = build_state(Arc::clone(&repository));

        let updated = sync_codex_quota_from_response_headers(
            &state,
            Some(&json!({
                "request_id": "req-codex-realtime-1",
                "key_id": "key-codex-1"
            })),
            &paid_headers("100", "31", "15160", "524059"),
        )
        .await
        .expect("codex realtime sync should succeed");

        assert!(updated);
        let reloaded = repository
            .list_keys_by_ids(&["key-codex-1".to_string()])
            .await
            .expect("keys should list");
        let codex = reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|metadata| metadata.get("codex"))
            .and_then(Value::as_object)
            .expect("codex metadata should exist");
        assert_eq!(codex.get("plan_type"), Some(&json!("team")));
        assert_eq!(codex.get("primary_used_percent"), Some(&json!(31.0)));
        assert_eq!(codex.get("secondary_used_percent"), Some(&json!(100.0)));
        assert_eq!(codex.get("legacy_marker"), Some(&json!("keep-me")));
        assert!(codex.get("updated_at").and_then(Value::as_u64).is_some());
        assert_eq!(
            reloaded[0]
                .upstream_metadata
                .as_ref()
                .and_then(Value::as_object)
                .and_then(|metadata| metadata.get("other")),
            Some(&json!({"value": true}))
        );
    }

    #[tokio::test]
    async fn sync_codex_quota_skips_non_codex_provider() {
        clear_codex_quota_fingerprint_cache();

        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-openai", "openai")],
            Vec::new(),
            vec![sample_key("key-openai-1", "provider-openai", None)],
        ));
        let state = build_state(Arc::clone(&repository));

        let updated = sync_codex_quota_from_response_headers(
            &state,
            Some(&json!({
                "request_id": "req-openai-realtime-1",
                "key_id": "key-openai-1"
            })),
            &paid_headers("100", "31", "15160", "524059"),
        )
        .await
        .expect("non-codex realtime sync should not fail");

        assert!(!updated);
        let reloaded = repository
            .list_keys_by_ids(&["key-openai-1".to_string()])
            .await
            .expect("keys should list");
        assert_eq!(reloaded[0].upstream_metadata, None);
    }

    #[tokio::test]
    async fn sync_codex_quota_skips_when_headers_do_not_contain_codex_metadata() {
        clear_codex_quota_fingerprint_cache();

        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-codex", "codex")],
            Vec::new(),
            vec![sample_key("key-codex-2", "provider-codex", None)],
        ));
        let state = build_state(Arc::clone(&repository));

        let updated = sync_codex_quota_from_response_headers(
            &state,
            Some(&json!({
                "request_id": "req-codex-realtime-2",
                "key_id": "key-codex-2"
            })),
            &BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        )
        .await
        .expect("empty codex headers should not fail");

        assert!(!updated);
        let reloaded = repository
            .list_keys_by_ids(&["key-codex-2".to_string()])
            .await
            .expect("keys should list");
        assert_eq!(reloaded[0].upstream_metadata, None);
    }

    #[tokio::test]
    async fn sync_codex_quota_cache_hit_skips_when_only_reset_countdown_changes() {
        clear_codex_quota_fingerprint_cache();

        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-codex", "codex")],
            Vec::new(),
            vec![sample_key("key-codex-3", "provider-codex", None)],
        ));
        let state = build_state(Arc::clone(&repository));
        let key_ids = ["key-codex-3".to_string()];

        let first_updated = sync_codex_quota_from_response_headers(
            &state,
            Some(&json!({
                "request_id": "req-codex-realtime-3",
                "key_id": "key-codex-3"
            })),
            &paid_headers("100", "31", "15160", "524059"),
        )
        .await
        .expect("first realtime sync should succeed");
        assert!(first_updated);
        let first_snapshot = repository
            .list_keys_by_ids(&key_ids)
            .await
            .expect("keys should list")
            .into_iter()
            .next()
            .expect("key should exist");

        let second_updated = sync_codex_quota_from_response_headers(
            &state,
            Some(&json!({
                "request_id": "req-codex-realtime-3",
                "key_id": "key-codex-3"
            })),
            &paid_headers("100", "31", "42", "84"),
        )
        .await
        .expect("second realtime sync should succeed");
        assert!(!second_updated);
        let second_snapshot = repository
            .list_keys_by_ids(&key_ids)
            .await
            .expect("keys should list")
            .into_iter()
            .next()
            .expect("key should exist");
        assert_eq!(
            first_snapshot.upstream_metadata,
            second_snapshot.upstream_metadata
        );
        assert_eq!(
            first_snapshot.updated_at_unix_secs,
            second_snapshot.updated_at_unix_secs
        );
    }

    #[tokio::test]
    async fn sync_codex_quota_updates_when_usage_percent_changes() {
        clear_codex_quota_fingerprint_cache();

        let repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-codex", "codex")],
            Vec::new(),
            vec![sample_key("key-codex-4", "provider-codex", None)],
        ));
        let state = build_state(Arc::clone(&repository));

        let first_updated = sync_codex_quota_from_response_headers(
            &state,
            Some(&json!({
                "request_id": "req-codex-realtime-4",
                "key_id": "key-codex-4"
            })),
            &paid_headers("98", "31", "15160", "524059"),
        )
        .await
        .expect("first realtime sync should succeed");
        assert!(first_updated);

        let second_updated = sync_codex_quota_from_response_headers(
            &state,
            Some(&json!({
                "request_id": "req-codex-realtime-4",
                "key_id": "key-codex-4"
            })),
            &paid_headers("100", "31", "15160", "524059"),
        )
        .await
        .expect("second realtime sync should succeed");
        assert!(second_updated);

        let reloaded = repository
            .list_keys_by_ids(&["key-codex-4".to_string()])
            .await
            .expect("keys should list");
        let codex = reloaded[0]
            .upstream_metadata
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|metadata| metadata.get("codex"))
            .and_then(Value::as_object)
            .expect("codex metadata should exist");
        assert_eq!(codex.get("secondary_used_percent"), Some(&json!(100.0)));
    }
}
