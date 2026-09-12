use std::time::{SystemTime, UNIX_EPOCH};

use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use serde_json::{json, Map, Value};

use crate::provider::ProviderPoolMemberInput;
use crate::service::ProviderPoolService;

pub fn provider_pool_key_account_quota_exhausted(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> bool {
    let adapter = ProviderPoolService::with_builtin_adapters().adapter(provider_type);
    adapter.quota_exhausted(&ProviderPoolMemberInput {
        provider_type,
        key,
        auth_config: None,
        provider_model_name: None,
    })
}

pub fn provider_pool_key_quota_hard_blocked(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> bool {
    let adapter = ProviderPoolService::with_builtin_adapters().adapter(provider_type);
    adapter.quota_hard_blocked(&ProviderPoolMemberInput {
        provider_type,
        key,
        auth_config: None,
        provider_model_name: None,
    })
}

/// Model-aware hard-block lookup for pre-scheduler candidate filtering.  Some
/// providers expose permanent account flags alongside independent model
/// buckets; adapters can suppress the account flag when the selected model
/// has its own usable quota.
pub fn provider_pool_key_model_quota_hard_blocked(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    provider_model_name: &str,
) -> bool {
    let adapter = ProviderPoolService::with_builtin_adapters().adapter(provider_type);
    adapter.quota_hard_blocked(&ProviderPoolMemberInput {
        provider_type,
        key,
        auth_config: None,
        provider_model_name: Some(provider_model_name),
    })
}

pub fn provider_pool_member_quota_snapshot<'a>(
    key: &'a StoredProviderCatalogKey,
    provider_type: &str,
) -> Option<&'a Map<String, Value>> {
    let quota_snapshot = key
        .status_snapshot
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("quota"))
        .and_then(Value::as_object)?;
    provider_pool_quota_snapshot_matches_provider(quota_snapshot, provider_type)
        .then_some(quota_snapshot)
}

/// Resolve exhaustion for the quota bucket applicable to one provider model.
///
/// Providers are free to expose quota windows in different shapes.  Newer
/// snapshots should put an explicit `model`/`models` (or `quota_group`) on a
/// window; legacy Codex snapshots use a family prefix in `code` (for example
/// `spark_5h`).  We deliberately do not name any product or model here: the
/// resolver compares the metadata supplied by the provider with the selected
/// model and only falls back to account-level exhaustion when no model bucket
/// can be identified.
pub(crate) fn provider_pool_model_quota_exhausted(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    provider_model_name: &str,
) -> Option<bool> {
    let requested = provider_pool_identifier_tokens(provider_model_name);
    if requested.is_empty() {
        return None;
    }

    // Prefer the materialized status snapshot, but also inspect the raw
    // provider metadata.  A quota refresh and a request can race, leaving the
    // latter newer than the snapshot; resolving both avoids falling back to an
    // account-wide signal and incorrectly blocking an unrelated model bucket.
    let sources = [
        provider_pool_member_quota_snapshot(key, provider_type),
        provider_pool_metadata_bucket(key.upstream_metadata.as_ref(), provider_type),
    ];
    let mut resolved = None::<(Option<u64>, bool)>;
    for source in sources.into_iter().flatten() {
        let windows = provider_pool_collect_quota_windows(source);
        if windows.is_empty() {
            continue;
        }
        let observed_at = provider_pool_timestamp_unix_secs(source.get("observed_at"))
            .or_else(|| provider_pool_timestamp_unix_secs(source.get("updated_at")));
        let model_matches = windows
            .iter()
            .filter(|window| {
                provider_pool_window_explicitly_matches_model(window, provider_model_name)
            })
            .collect::<Vec<_>>();
        if !model_matches.is_empty() {
            let exhausted =
                provider_pool_explicit_model_windows_exhausted(model_matches, observed_at);
            if resolved.is_none()
                || provider_pool_should_replace_model_quota_resolution(
                    resolved.as_ref().and_then(|(observed_at, _)| *observed_at),
                    observed_at,
                )
            {
                resolved = Some((observed_at, exhausted));
            }
            continue;
        }

        // Legacy snapshots may not carry a model field.  Match an opaque
        // family token (the prefix before `_`/`:` in `code`, or an explicit
        // family key) against the model's tokens.  This keeps independent
        // windows isolated without baking in names such as "spark".
        let family_matches = windows
            .iter()
            .filter(|window| provider_pool_window_family_matches_model(window, &requested))
            .collect::<Vec<_>>();
        if !family_matches.is_empty() {
            let exhausted = provider_pool_any_window_exhausted(family_matches, observed_at);
            if resolved.is_none()
                || provider_pool_should_replace_model_quota_resolution(
                    resolved.as_ref().and_then(|(observed_at, _)| *observed_at),
                    observed_at,
                )
            {
                resolved = Some((observed_at, exhausted));
            }
            continue;
        }

        // Account-scoped windows (for example the ordinary weekly and short
        // windows emitted by Codex) apply to every model that has no more
        // specific family.  Restrict this fallback to well-known structural
        // window names; opaque family names such as `alpha_weekly` must not
        // accidentally make an unrelated model look schedulable.
        let generic_matches = windows
            .iter()
            .filter(|window| provider_pool_window_is_generic(window))
            .collect::<Vec<_>>();
        if !generic_matches.is_empty() {
            let exhausted = provider_pool_any_window_exhausted(generic_matches, observed_at);
            if resolved.is_none()
                || provider_pool_should_replace_model_quota_resolution(
                    resolved.as_ref().and_then(|(observed_at, _)| *observed_at),
                    observed_at,
                )
            {
                resolved = Some((observed_at, exhausted));
            }
        }
    }

    resolved.map(|(_, exhausted)| exhausted)
}

fn provider_pool_should_replace_model_quota_resolution(
    previous_observed_at: Option<u64>,
    next_observed_at: Option<u64>,
) -> bool {
    match (previous_observed_at, next_observed_at) {
        (Some(previous), Some(next)) => next >= previous,
        (None, Some(_)) => true,
        (Some(_), None) => false,
        // Preserve source order when neither side carries freshness metadata;
        // the materialized status snapshot is preferred over raw metadata.
        (None, None) => false,
    }
}

/// Public adapter-independent model quota lookup used by schedulers that need
/// to prefilter candidates before constructing provider-pool signals.
pub fn provider_pool_key_model_quota_exhausted(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    provider_model_name: &str,
) -> Option<bool> {
    provider_pool_model_quota_exhausted(key, provider_type, provider_model_name)
}

fn provider_pool_explicit_model_windows_exhausted(
    windows: Vec<&Map<String, Value>>,
    snapshot_observed_at: Option<u64>,
) -> bool {
    let now_unix_secs = provider_pool_current_unix_secs();

    // Explicit model buckets represent independent windows for one model. The
    // model remains usable while at least one of those windows still has
    // capacity, so exhaustion is reported only when all are exhausted.
    windows.iter().all(|window| {
        provider_pool_quota_window_is_exhausted(window)
            && !now_unix_secs.is_some_and(|now| {
                provider_pool_reset_deadline_elapsed(window, snapshot_observed_at, now)
            })
    })
}

fn provider_pool_any_window_exhausted(
    windows: Vec<&Map<String, Value>>,
    snapshot_observed_at: Option<u64>,
) -> bool {
    let now_unix_secs = provider_pool_current_unix_secs();
    windows.iter().any(|window| {
        provider_pool_quota_window_is_exhausted(window)
            && !now_unix_secs.is_some_and(|now| {
                provider_pool_reset_deadline_elapsed(window, snapshot_observed_at, now)
            })
    })
}

fn provider_pool_window_is_generic(window: &Map<String, Value>) -> bool {
    if provider_pool_window_has_explicit_model(window)
        || window
            .get("scope")
            .and_then(Value::as_str)
            .is_some_and(|scope| scope.trim().eq_ignore_ascii_case("model"))
    {
        return false;
    }

    let code = window
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let family = code
        .split_once(['_', ':', '/'])
        .map(|(prefix, _)| prefix)
        .unwrap_or(code)
        .trim()
        .to_ascii_lowercase();
    if family.is_empty() {
        return window
            .get("scope")
            .and_then(Value::as_str)
            .is_some_and(|scope| scope.trim().eq_ignore_ascii_case("account"));
    }
    [
        "weekly",
        "5h",
        "daily",
        "monthly",
        "primary",
        "secondary",
        "account",
        "quota",
        "window",
        "rate",
        "reset",
    ]
    .contains(&family.as_str())
}

fn provider_pool_window_explicitly_matches_model(
    window: &Map<String, Value>,
    requested_model: &str,
) -> bool {
    let requested = provider_pool_normalize_identifier(requested_model);
    let requested_tokens = provider_pool_identifier_tokens(requested_model);
    if requested.is_empty() {
        return false;
    }
    [
        "model",
        "model_name",
        "model_id",
        "quota_model",
        "quota_model_name",
        "target_model",
        "limit_name",
    ]
    .iter()
    .filter_map(|key| window.get(*key))
    .any(|value| match value {
        Value::String(value) => {
            provider_pool_identifiers_match(&requested, value, &requested_tokens)
        }
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .any(|value| provider_pool_identifiers_match(&requested, value, &requested_tokens)),
        _ => false,
    }) || ["models", "model_ids"]
        .iter()
        .filter_map(|key| window.get(*key).and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .any(|value| provider_pool_identifiers_match(&requested, value, &requested_tokens))
}

fn provider_pool_window_family_matches_model(
    window: &Map<String, Value>,
    requested_tokens: &std::collections::BTreeSet<String>,
) -> bool {
    let explicit_scope = window
        .get("scope")
        .and_then(Value::as_str)
        .map(|scope| scope.trim().to_ascii_lowercase());
    // A model-scoped window without an explicit model must not accidentally
    // match a token from its opaque code.
    if explicit_scope.as_deref() == Some("model") || provider_pool_window_has_explicit_model(window)
    {
        return false;
    }

    let mut families = Vec::new();
    for key in ["quota_group", "quota_family", "family", "bucket"] {
        if let Some(value) = window.get(key).and_then(Value::as_str) {
            families.push(value.to_string());
        }
    }
    if let Some(code) = window.get("code").and_then(Value::as_str) {
        let code = code.trim();
        if let Some((prefix, _)) = code.split_once(['_', ':', '/']) {
            families.push(prefix.to_string());
        }
    }
    families.into_iter().any(|family| {
        let normalized = provider_pool_normalize_identifier(&family);
        if normalized.is_empty()
            || [
                "account",
                "quota",
                "window",
                "primary",
                "secondary",
                "rate",
                "reset",
            ]
            .iter()
            .any(|generic| normalized == *generic)
        {
            return false;
        }
        requested_tokens.iter().any(|token| {
            token.len() >= 3
                && (token == &normalized
                    || token.contains(&normalized)
                    || normalized.contains(token))
        })
    })
}

fn provider_pool_identifiers_match(
    requested: &str,
    candidate: &str,
    requested_tokens: &std::collections::BTreeSet<String>,
) -> bool {
    let candidate_tokens = provider_pool_identifier_tokens(candidate);
    let candidate = provider_pool_normalize_identifier(candidate);
    if candidate.is_empty() {
        return false;
    }
    requested == candidate
        || candidate_tokens
            .iter()
            .any(|token| {
                requested_tokens.contains(token) && provider_pool_is_specific_model_token(token)
            })
        // Handle compact upstream identifiers such as `spark` embedded in a
        // provider model name (`vendor-codex-spark`) while avoiding accidental
        // one/two-character matches.
        || (candidate.len() >= 4
            && requested.len() >= 6
            && (requested.contains(&candidate) || candidate.contains(requested)))
}

fn provider_pool_is_specific_model_token(token: &str) -> bool {
    token.len() >= 4
        && !token.chars().all(|character| character.is_ascii_digit())
        && ![
            "auto",
            "base",
            "claude",
            "codex",
            "default",
            "fast",
            "flash",
            "free",
            "gemini",
            "gpt",
            "latest",
            "mini",
            "model",
            "plus",
            "pro",
            "reasoning",
            "team",
            "think",
            "thinking",
            "tiered",
            "vendor",
        ]
        .contains(&token)
}

fn provider_pool_window_has_explicit_model(window: &Map<String, Value>) -> bool {
    [
        "model",
        "model_name",
        "model_id",
        "quota_model",
        "quota_model_name",
        "target_model",
        "models",
        "model_ids",
    ]
    .iter()
    .any(|key| match window.get(*key) {
        Some(Value::String(value)) => !value.trim().is_empty(),
        Some(Value::Array(values)) => values
            .iter()
            .any(|value| value.as_str().is_some_and(|value| !value.trim().is_empty())),
        _ => false,
    })
}

fn provider_pool_normalize_identifier(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect()
}

fn provider_pool_identifier_tokens(value: &str) -> std::collections::BTreeSet<String> {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| token.len() >= 3)
        .collect()
}

/// Materialize quota windows from the small set of shapes emitted by current
/// and legacy adapters.  Model maps (`quota_by_model`/`models`) are converted
/// to the same window representation used by status snapshots, with the map
/// key retained as the model identity.  Keeping this normalization here means
/// provider adapters do not need to grow model-specific quota code whenever an
/// upstream introduces another independent bucket.
fn provider_pool_collect_quota_windows(source: &Map<String, Value>) -> Vec<Map<String, Value>> {
    let mut windows = Vec::new();
    for key in ["windows", "additional_quota_windows"] {
        if let Some(values) = source.get(key).and_then(Value::as_array) {
            windows.extend(values.iter().filter_map(Value::as_object).cloned());
        }
    }
    for key in ["quota_by_model", "models", "model_quotas"] {
        let Some(models) = source.get(key).and_then(Value::as_object) else {
            continue;
        };
        for (model_name, item) in models {
            let Some(item) = item.as_object() else {
                continue;
            };
            let mut window = item.clone();
            window
                .entry("model".to_string())
                .or_insert_with(|| json!(model_name));
            window
                .entry("scope".to_string())
                .or_insert_with(|| json!("model"));
            window
                .entry("code".to_string())
                .or_insert_with(|| json!(format!("model:{model_name}")));
            windows.push(window);
        }
    }
    windows
}

pub fn provider_pool_quota_snapshot_updated_at(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> Option<u64> {
    let quota_snapshot = provider_pool_member_quota_snapshot(key, provider_type)?;
    provider_pool_timestamp_unix_secs(quota_snapshot.get("updated_at"))
}

pub fn provider_pool_quota_metadata_updated_at(
    upstream_metadata: Option<&Value>,
    provider_type: &str,
) -> Option<u64> {
    let bucket = provider_pool_metadata_bucket(upstream_metadata, provider_type)?;
    provider_pool_timestamp_unix_secs(bucket.get("updated_at"))
}

pub fn provider_pool_quota_metadata_provider_type(metadata_update: &Value) -> Option<String> {
    let object = metadata_update.as_object()?;
    let service = ProviderPoolService::with_builtin_adapters();
    let known_provider_type = service
        .provider_types()
        .find(|provider_type| object.contains_key(*provider_type))
        .map(ToOwned::to_owned);
    known_provider_type.or_else(|| {
        object
            .iter()
            .find(|(_, value)| value.is_object())
            .map(|(provider_type, _)| provider_type.clone())
    })
}

pub fn provider_pool_key_scheduling_label(
    is_active: bool,
    cooldown_reason: Option<&str>,
    cooldown_ttl_seconds: Option<u64>,
) -> (String, String, String, Vec<Value>) {
    if !is_active {
        return (
            "blocked".to_string(),
            "inactive".to_string(),
            "已禁用".to_string(),
            vec![json!({
                "code": "inactive",
                "label": "已禁用",
                "blocking": true,
                "source": "manual",
                "ttl_seconds": Value::Null,
                "detail": Value::Null,
            })],
        );
    }
    if let Some(reason) = cooldown_reason {
        return (
            "degraded".to_string(),
            "cooldown".to_string(),
            "冷却中".to_string(),
            vec![json!({
                "code": "cooldown",
                "label": "冷却中",
                "blocking": true,
                "source": "pool",
                "ttl_seconds": cooldown_ttl_seconds,
                "detail": reason,
            })],
        );
    }
    (
        "available".to_string(),
        "available".to_string(),
        "可用".to_string(),
        Vec::new(),
    )
}

pub(crate) fn provider_pool_metadata_bucket<'a>(
    upstream_metadata: Option<&'a Value>,
    provider_type: &str,
) -> Option<&'a Map<String, Value>> {
    upstream_metadata
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get(&provider_type.trim().to_ascii_lowercase()))
        .and_then(Value::as_object)
}

pub(crate) fn provider_pool_json_bool(value: Option<&Value>) -> Option<bool> {
    match value {
        Some(Value::Bool(value)) => Some(*value),
        Some(Value::String(value)) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn provider_pool_json_f64(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(value)) => value.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|value| value.is_finite())
}

pub(crate) fn provider_pool_timestamp_unix_secs(value: Option<&Value>) -> Option<u64> {
    let mut timestamp = provider_pool_json_f64(value)?;
    if timestamp <= 0.0 {
        return None;
    }
    if timestamp > 1_000_000_000_000.0 {
        timestamp /= 1000.0;
    }
    Some(timestamp as u64)
}

pub(crate) fn provider_pool_current_unix_secs() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

fn provider_pool_reset_deadline_unix_secs(
    item: &Map<String, Value>,
    fallback_observed_at: Option<u64>,
) -> Option<u64> {
    provider_pool_timestamp_unix_secs(item.get("reset_at"))
        .or_else(|| provider_pool_timestamp_unix_secs(item.get("next_reset_at")))
        .or_else(|| {
            let reset_seconds = provider_pool_json_f64(item.get("reset_seconds"))
                .or_else(|| provider_pool_json_f64(item.get("reset_after_seconds")))?;
            if reset_seconds < 0.0 {
                return None;
            }
            let base = provider_pool_timestamp_unix_secs(item.get("observed_at"))
                .or_else(|| provider_pool_timestamp_unix_secs(item.get("updated_at")))
                .or(fallback_observed_at)?;
            Some(base.saturating_add(reset_seconds.ceil() as u64))
        })
}

pub(crate) fn provider_pool_reset_deadline_elapsed(
    item: &Map<String, Value>,
    fallback_observed_at: Option<u64>,
    now_unix_secs: u64,
) -> bool {
    provider_pool_reset_deadline_unix_secs(item, fallback_observed_at)
        .is_some_and(|reset_at| reset_at <= now_unix_secs)
}

fn provider_pool_quota_window_is_exhausted(window: &Map<String, Value>) -> bool {
    provider_pool_json_bool(window.get("is_exhausted"))
        .or_else(|| provider_pool_json_bool(window.get("exhausted")))
        .or_else(|| {
            provider_pool_json_f64(
                window
                    .get("used_ratio")
                    .or_else(|| window.get("usage_ratio")),
            )
            .map(|value| value >= 1.0 - 1e-6)
        })
        .or_else(|| {
            provider_pool_json_f64(window.get("used_percent")).map(|value| value >= 100.0 - 1e-6)
        })
        .or_else(|| {
            provider_pool_json_f64(
                window
                    .get("remaining_ratio")
                    .or_else(|| window.get("remaining_fraction")),
            )
            .map(|value| value <= 1e-6)
        })
        .or_else(|| {
            provider_pool_json_f64(window.get("remaining_percent")).map(|value| value <= 1e-6)
        })
        .or_else(|| {
            let remaining = provider_pool_json_f64(
                window
                    .get("remaining")
                    .or_else(|| window.get("remaining_value")),
            )?;
            let limit = provider_pool_json_f64(
                window
                    .get("limit")
                    .or_else(|| window.get("limit_value"))
                    .or_else(|| window.get("total")),
            )?;
            (limit > 0.0).then_some(remaining <= 0.0)
        })
        .unwrap_or(false)
}

fn provider_pool_window_is_model_scoped(window: &Map<String, Value>) -> bool {
    window
        .get("scope")
        .and_then(Value::as_str)
        .is_some_and(|scope| scope.trim().eq_ignore_ascii_case("model"))
        || provider_pool_window_has_explicit_model(window)
}

fn provider_pool_quota_snapshot_matches_provider(
    quota_snapshot: &Map<String, Value>,
    provider_type: &str,
) -> bool {
    let normalized_provider_type = provider_type.trim().to_ascii_lowercase();
    match quota_snapshot
        .get("provider_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(provider_type) => provider_type.eq_ignore_ascii_case(&normalized_provider_type),
        None => {
            provider_pool_json_bool(quota_snapshot.get("exhausted")) == Some(true)
                || quota_snapshot
                    .get("code")
                    .and_then(Value::as_str)
                    .is_some_and(|code| !code.trim().eq_ignore_ascii_case("unknown"))
                || quota_snapshot
                    .get("updated_at")
                    .is_some_and(|value| !value.is_null())
                || quota_snapshot
                    .get("observed_at")
                    .is_some_and(|value| !value.is_null())
                || quota_snapshot
                    .get("usage_ratio")
                    .is_some_and(|value| !value.is_null())
                || quota_snapshot
                    .get("reset_seconds")
                    .is_some_and(|value| !value.is_null())
                || quota_snapshot
                    .get("windows")
                    .and_then(Value::as_array)
                    .is_some_and(|windows| !windows.is_empty())
                || quota_snapshot
                    .get("additional_quota_windows")
                    .and_then(Value::as_array)
                    .is_some_and(|windows| !windows.is_empty())
                || ["quota_by_model", "models", "model_quotas"]
                    .iter()
                    .any(|key| {
                        quota_snapshot
                            .get(*key)
                            .and_then(Value::as_object)
                            .is_some_and(|models| !models.is_empty())
                    })
                || quota_snapshot
                    .get("credits")
                    .and_then(Value::as_object)
                    .is_some_and(|credits| !credits.is_empty())
        }
    }
}

pub(crate) fn provider_pool_quota_snapshot_exhausted_decision(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> Option<bool> {
    let quota_snapshot = key
        .status_snapshot
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("quota"))
        .and_then(Value::as_object)?;
    if !provider_pool_quota_snapshot_matches_provider(quota_snapshot, provider_type) {
        return None;
    }
    let exhausted = provider_pool_json_bool(quota_snapshot.get("exhausted"))?;
    if exhausted {
        let now_unix_secs = provider_pool_current_unix_secs();
        let snapshot_observed_at =
            provider_pool_timestamp_unix_secs(quota_snapshot.get("observed_at"))
                .or_else(|| provider_pool_timestamp_unix_secs(quota_snapshot.get("updated_at")));

        let materialized_windows = provider_pool_collect_quota_windows(quota_snapshot);
        if !materialized_windows.is_empty() {
            // Model-scoped windows are evaluated only when the request model
            // is known.  They must not turn the account-level fallback into
            // an exhausted state for unrelated models.
            let account_scoped_windows = materialized_windows
                .iter()
                .filter(|window| !provider_pool_window_is_model_scoped(window))
                .collect::<Vec<_>>();
            // A snapshot containing only model-scoped buckets has no
            // account-wide signal to apply when the caller did not provide a
            // model name (for example, an admin status listing). Do not let a
            // single exhausted model poison every sibling bucket.
            if account_scoped_windows.is_empty() {
                return Some(false);
            }
            let windows = account_scoped_windows;
            let mut saw_exhausted_window = false;
            let mut saw_active_exhausted_window = false;
            let mut windows_max_ratio = None::<f64>;

            for window in windows.iter() {
                if let Some(ratio) = provider_pool_json_f64(window.get("used_ratio")) {
                    windows_max_ratio =
                        Some(windows_max_ratio.map_or(ratio, |current| current.max(ratio)));
                }
                if provider_pool_quota_window_is_exhausted(window) {
                    saw_exhausted_window = true;
                    let reset_elapsed = now_unix_secs.is_some_and(|now| {
                        provider_pool_reset_deadline_elapsed(window, snapshot_observed_at, now)
                    });
                    if !reset_elapsed {
                        saw_active_exhausted_window = true;
                    }
                }
            }

            if saw_exhausted_window {
                return Some(saw_active_exhausted_window);
            }
            if windows_max_ratio.is_some_and(|ratio| ratio < 1.0 - 1e-6) {
                return Some(false);
            }
        } else if now_unix_secs.is_some_and(|now| {
            provider_pool_reset_deadline_elapsed(quota_snapshot, snapshot_observed_at, now)
        }) {
            return Some(false);
        }
    }
    Some(exhausted)
}

pub(crate) fn provider_pool_quota_usage_ratio(key: &StoredProviderCatalogKey) -> Option<f64> {
    key.status_snapshot
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("quota"))
        .and_then(Value::as_object)
        .and_then(|quota| provider_pool_json_f64(quota.get("usage_ratio")))
}

pub(crate) fn provider_pool_quota_reset_seconds(key: &StoredProviderCatalogKey) -> Option<f64> {
    key.status_snapshot
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("quota"))
        .and_then(Value::as_object)
        .and_then(|quota| provider_pool_json_f64(quota.get("reset_seconds")))
}

pub(crate) fn provider_pool_account_blocked(key: &StoredProviderCatalogKey) -> bool {
    key.oauth_invalid_reason.as_deref().is_some_and(|reason| {
        let normalized = reason.trim().to_ascii_lowercase();
        !normalized.is_empty()
            && [
                "banned",
                "forbidden",
                "blocked",
                "suspend",
                "deactivated",
                "disabled",
                "verification",
                "workspace",
                "受限",
                "封",
                "禁",
            ]
            .iter()
            .any(|hint| normalized.contains(hint))
    })
}
