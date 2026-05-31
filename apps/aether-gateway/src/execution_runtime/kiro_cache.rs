use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};

const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(300);
const ONE_HOUR_CACHE_TTL: Duration = Duration::from_secs(3600);
const MAX_ENTRIES: usize = 2048;
const PREFIX_LOOKBACK_LIMIT: usize = 10;
const TOKENS_PER_TOOL: u64 = 150;
const TOKENS_PER_MESSAGE: u64 = 4;
const INLINE_IMAGE_DATA_TOKEN_PLACEHOLDER: &str = "[inline-image-data]";
pub(crate) const KIRO_SIMULATED_CACHE_ENABLED_CONTEXT_FIELD: &str = "kiro_simulated_cache_enabled";

static KIRO_PROMPT_CACHE_TRACKER: OnceLock<KiroPromptCacheTracker> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KiroPromptCacheProfile {
    total_input_tokens: u64,
    min_cacheable_tokens: u64,
    breakpoints: Vec<KiroPromptCacheBreakpoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KiroPromptCacheBreakpoint {
    fingerprint: [u8; 32],
    cumulative_tokens: u64,
    ttl: Duration,
}

#[derive(Debug, Clone)]
struct KiroPromptCacheEntry {
    token_count: u64,
    ttl: Duration,
    expires_at: Instant,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct KiroPromptCacheUsage {
    pub(crate) cache_creation_input_tokens: u64,
    pub(crate) cache_read_input_tokens: u64,
}

#[derive(Debug, Default)]
pub(crate) struct KiroPromptCacheTracker {
    entries: Mutex<HashMap<(String, [u8; 32]), KiroPromptCacheEntry>>,
}

#[derive(Debug)]
struct PendingBlock {
    value: Value,
    tokens: u64,
    breakpoint_ttl: Option<Duration>,
    is_message_end: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PrefixCandidate {
    fingerprint: [u8; 32],
    cumulative_tokens: u64,
}

pub(crate) fn kiro_prompt_cache_tracker() -> &'static KiroPromptCacheTracker {
    KIRO_PROMPT_CACHE_TRACKER.get_or_init(KiroPromptCacheTracker::default)
}

pub(crate) fn build_kiro_prompt_cache_profile(
    request_body: &Value,
    total_input_tokens: u64,
) -> Option<KiroPromptCacheProfile> {
    let model = request_body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let flattened = flatten_cacheable_blocks(request_body);
    if flattened.iter().all(|block| block.breakpoint_ttl.is_none()) {
        return None;
    }

    let prelude = canonicalize_json(serde_json::json!({
        "model": request_body.get("model").cloned().unwrap_or(Value::Null),
        "tool_choice": request_body.get("tool_choice").cloned().unwrap_or(Value::Null),
    }));
    let mut prefix_hasher = Sha256::new();
    let prelude_bytes = serde_json::to_vec(&prelude).unwrap_or_default();
    prefix_hasher.update((prelude_bytes.len() as u64).to_be_bytes());
    prefix_hasher.update(prelude_bytes);

    let mut cumulative_tokens = 0u64;
    let mut active_ttl: Option<Duration> = None;
    let mut breakpoints = Vec::new();
    let mut seen_fingerprints = std::collections::BTreeSet::<[u8; 32]>::new();
    let mut lookback_candidates = Vec::new();

    for block in flattened {
        cumulative_tokens = cumulative_tokens.saturating_add(block.tokens);
        let block_bytes = serde_json::to_vec(&block.value).unwrap_or_default();
        let block_hash: [u8; 32] = Sha256::digest(block_bytes).into();
        let mut next_prefix_hasher = prefix_hasher.clone();
        next_prefix_hasher.update(block_hash);
        let fingerprint: [u8; 32] = next_prefix_hasher.finalize().into();
        prefix_hasher = Sha256::new();
        prefix_hasher.update(fingerprint);

        if let Some(ttl) = block.breakpoint_ttl {
            push_lookback_breakpoints(
                &mut breakpoints,
                &mut seen_fingerprints,
                &lookback_candidates,
                ttl,
            );
            active_ttl = Some(ttl);
            push_breakpoint(
                &mut breakpoints,
                &mut seen_fingerprints,
                fingerprint,
                cumulative_tokens,
                ttl,
            );
        }
        if block.is_message_end {
            if let Some(ttl) = active_ttl {
                push_breakpoint(
                    &mut breakpoints,
                    &mut seen_fingerprints,
                    fingerprint,
                    cumulative_tokens,
                    ttl,
                );
            }
        }
        push_prefix_candidate(&mut lookback_candidates, fingerprint, cumulative_tokens);
    }

    let min_cacheable_tokens = minimum_cacheable_tokens_for_model(model);
    let cacheable_breakpoints = breakpoints
        .into_iter()
        .filter(|breakpoint| breakpoint.cumulative_tokens >= min_cacheable_tokens)
        .collect::<Vec<_>>();
    (!cacheable_breakpoints.is_empty()).then_some(KiroPromptCacheProfile {
        total_input_tokens,
        min_cacheable_tokens,
        breakpoints: cacheable_breakpoints,
    })
}

pub(crate) fn kiro_simulated_cache_enabled_from_provider_config(config: Option<&Value>) -> bool {
    config
        .and_then(Value::as_object)
        .and_then(|config| config.get("kiro"))
        .and_then(Value::as_object)
        .and_then(|kiro| kiro.get("simulated_cache_enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub(crate) fn kiro_simulated_cache_enabled_from_report_context(
    report_context: Option<&Value>,
) -> bool {
    report_context
        .and_then(Value::as_object)
        .and_then(|context| context.get(KIRO_SIMULATED_CACHE_ENABLED_CONTEXT_FIELD))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub(crate) fn billed_input_tokens(input_tokens: u64, usage: KiroPromptCacheUsage) -> u64 {
    input_tokens
        .saturating_sub(usage.cache_creation_input_tokens)
        .saturating_sub(usage.cache_read_input_tokens)
}

pub(crate) fn estimate_kiro_prompt_input_tokens(request_body: &Value) -> u64 {
    let system_tokens = request_body
        .get("system")
        .map(count_system_tokens)
        .unwrap_or(0);
    let message_tokens = request_body
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| count_messages_tokens(messages))
        .unwrap_or(0);
    let tool_tokens = request_body
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| tools.len() as u64 * TOKENS_PER_TOOL)
        .unwrap_or(0);

    (system_tokens + message_tokens + tool_tokens).max(1)
}

fn count_messages_tokens(messages: &[Value]) -> u64 {
    if messages.is_empty() {
        return 0;
    }
    let token_estimation_messages = messages
        .iter()
        .map(redact_inline_image_data_for_token_estimation)
        .collect::<Vec<_>>();
    serde_json::to_string(&token_estimation_messages)
        .map(|value| count_text_tokens(&value))
        .unwrap_or_else(|_| messages.iter().map(count_message_tokens).sum::<u64>())
        .saturating_add(messages.len() as u64 * TOKENS_PER_MESSAGE)
}

fn redact_inline_image_data_for_token_estimation(value: &Value) -> Value {
    redact_inline_image_data_value(value, false)
}

fn redact_inline_image_data_value(value: &Value, inside_image_source: bool) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| redact_inline_image_data_value(item, inside_image_source))
                .collect(),
        ),
        Value::Object(object) => {
            let image_block = object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.eq_ignore_ascii_case("image"));
            let image_source = inside_image_source || object_has_image_media_type(object);
            let mut out = serde_json::Map::new();
            for (key, inner) in object {
                let redacted =
                    if image_source && is_inline_image_data_key(key) && inner.as_str().is_some() {
                        Value::String(INLINE_IMAGE_DATA_TOKEN_PLACEHOLDER.to_string())
                    } else {
                        let child_inside_image_source =
                            image_block && key.eq_ignore_ascii_case("source");
                        redact_inline_image_data_value(inner, child_inside_image_source)
                    };
                out.insert(key.clone(), redacted);
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn object_has_image_media_type(object: &serde_json::Map<String, Value>) -> bool {
    object
        .get("media_type")
        .or_else(|| object.get("mediaType"))
        .and_then(Value::as_str)
        .is_some_and(|media_type| media_type.trim().to_ascii_lowercase().starts_with("image/"))
}

fn is_inline_image_data_key(key: &str) -> bool {
    key.eq_ignore_ascii_case("data") || key.eq_ignore_ascii_case("bytes")
}

fn push_breakpoint(
    breakpoints: &mut Vec<KiroPromptCacheBreakpoint>,
    seen_fingerprints: &mut std::collections::BTreeSet<[u8; 32]>,
    fingerprint: [u8; 32],
    cumulative_tokens: u64,
    ttl: Duration,
) {
    if seen_fingerprints.insert(fingerprint) {
        breakpoints.push(KiroPromptCacheBreakpoint {
            fingerprint,
            cumulative_tokens,
            ttl,
        });
    }
}

fn push_lookback_breakpoints(
    breakpoints: &mut Vec<KiroPromptCacheBreakpoint>,
    seen_fingerprints: &mut std::collections::BTreeSet<[u8; 32]>,
    candidates: &[PrefixCandidate],
    ttl: Duration,
) {
    for candidate in candidates {
        push_breakpoint(
            breakpoints,
            seen_fingerprints,
            candidate.fingerprint,
            candidate.cumulative_tokens,
            ttl,
        );
    }
}

fn push_prefix_candidate(
    candidates: &mut Vec<PrefixCandidate>,
    fingerprint: [u8; 32],
    cumulative_tokens: u64,
) {
    candidates.push(PrefixCandidate {
        fingerprint,
        cumulative_tokens,
    });
    if candidates.len() > PREFIX_LOOKBACK_LIMIT {
        candidates.remove(0);
    }
}

fn flatten_cacheable_blocks(request_body: &Value) -> Vec<PendingBlock> {
    let mut blocks = Vec::new();
    if let Some(tools) = request_body.get("tools").and_then(Value::as_array) {
        for (tool_index, tool) in tools.iter().enumerate() {
            let breakpoint_ttl = extract_cache_ttl(tool);
            let mut normalized = tool.clone();
            strip_cache_control(&mut normalized);
            let value = canonicalize_json(serde_json::json!({
                "kind": "tool",
                "tool_index": tool_index,
                "tool": normalized,
            }));
            blocks.push(PendingBlock {
                tokens: TOKENS_PER_TOOL,
                value,
                breakpoint_ttl,
                is_message_end: false,
            });
        }
    }

    if let Some(system) = request_body.get("system") {
        match system {
            Value::Array(items) => {
                for (system_index, item) in items.iter().enumerate() {
                    let breakpoint_ttl = extract_cache_ttl(item);
                    let mut normalized = item.clone();
                    strip_cache_control(&mut normalized);
                    let value = canonicalize_json(serde_json::json!({
                        "kind": "system",
                        "system_index": system_index,
                        "block": normalized,
                    }));
                    blocks.push(PendingBlock {
                        tokens: count_system_block_tokens(item),
                        value,
                        breakpoint_ttl,
                        is_message_end: false,
                    });
                }
            }
            Value::String(text) => {
                let value = canonicalize_json(serde_json::json!({
                    "kind": "system",
                    "system_index": 0,
                    "block": {"type": "text", "text": text},
                }));
                blocks.push(PendingBlock {
                    tokens: count_text_tokens(text),
                    value,
                    breakpoint_ttl: None,
                    is_message_end: false,
                });
            }
            other => {
                let value = canonicalize_json(serde_json::json!({
                    "kind": "system",
                    "system_index": 0,
                    "block": other,
                }));
                blocks.push(PendingBlock {
                    tokens: count_system_block_tokens(other),
                    value,
                    breakpoint_ttl: None,
                    is_message_end: false,
                });
            }
        }
    }

    if let Some(messages) = request_body.get("messages").and_then(Value::as_array) {
        for (message_index, message) in messages.iter().enumerate() {
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match message.get("content") {
                Some(Value::Array(items)) => {
                    let last_block_index = items.len().saturating_sub(1);
                    for (block_index, item) in items.iter().enumerate() {
                        let breakpoint_ttl = extract_cache_ttl(item);
                        let mut normalized = item.clone();
                        strip_cache_control(&mut normalized);
                        let value = canonicalize_json(serde_json::json!({
                            "kind": "message",
                            "message_index": message_index,
                            "role": role,
                            "block_index": block_index,
                            "block": normalized,
                        }));
                        blocks.push(PendingBlock {
                            tokens: count_message_content_tokens(item),
                            value,
                            breakpoint_ttl,
                            is_message_end: block_index == last_block_index,
                        });
                    }
                }
                Some(Value::String(text)) => {
                    let value = canonicalize_json(serde_json::json!({
                        "kind": "message",
                        "message_index": message_index,
                        "role": role,
                        "block_index": 0,
                        "block": {"type": "text", "text": text},
                    }));
                    blocks.push(PendingBlock {
                        tokens: count_text_tokens(text),
                        value,
                        breakpoint_ttl: None,
                        is_message_end: true,
                    });
                }
                Some(other) => {
                    let value = canonicalize_json(serde_json::json!({
                        "kind": "message",
                        "message_index": message_index,
                        "role": role,
                        "block_index": 0,
                        "block": other,
                    }));
                    blocks.push(PendingBlock {
                        tokens: count_message_content_tokens(other),
                        value,
                        breakpoint_ttl: None,
                        is_message_end: true,
                    });
                }
                None => {}
            }
        }
    }
    blocks
}

fn extract_cache_ttl(value: &Value) -> Option<Duration> {
    let cache_control = value.get("cache_control")?.as_object()?;
    if !cache_control
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("ephemeral"))
    {
        return None;
    }
    Some(
        if cache_control
            .get("ttl")
            .and_then(Value::as_str)
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("1h"))
        {
            ONE_HOUR_CACHE_TTL
        } else {
            DEFAULT_CACHE_TTL
        },
    )
}

fn strip_cache_control(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                strip_cache_control(item);
            }
        }
        Value::Object(map) => {
            map.remove("cache_control");
            for item in map.values_mut() {
                strip_cache_control(item);
            }
        }
        _ => {}
    }
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize_json).collect()),
        Value::Object(map) => {
            let ordered: BTreeMap<_, _> = map
                .into_iter()
                .map(|(key, value)| (key, canonicalize_json(value)))
                .collect();
            let mut out = serde_json::Map::new();
            for (key, value) in ordered {
                out.insert(key, value);
            }
            Value::Object(out)
        }
        other => other,
    }
}

fn count_system_tokens(system: &Value) -> u64 {
    match system {
        Value::Null => 0,
        Value::String(text) => count_text_tokens(text),
        Value::Array(blocks) => blocks.iter().map(count_system_block_tokens).sum(),
        Value::Object(_) => count_system_block_tokens(system),
        _ => 0,
    }
}

fn count_system_block_tokens(block: &Value) -> u64 {
    block
        .get("text")
        .and_then(Value::as_str)
        .map(count_text_tokens)
        .unwrap_or_else(|| {
            block
                .get("thinking")
                .and_then(Value::as_str)
                .map(count_text_tokens)
                .unwrap_or_else(|| {
                    block
                        .get("content")
                        .map(count_message_content_tokens)
                        .unwrap_or(0)
                })
        })
}

fn count_message_tokens(message: &Value) -> u64 {
    let Some(object) = message.as_object() else {
        return 0;
    };
    let content = object.get("content");
    TOKENS_PER_MESSAGE
        + content
            .map(count_message_content_tokens)
            .unwrap_or_else(|| estimate_serialized_value_tokens(message))
}

fn count_message_content_tokens(value: &Value) -> u64 {
    match value {
        Value::Null => 0,
        Value::String(text) => count_text_tokens(text),
        Value::Array(items) => items.iter().map(count_message_content_tokens).sum(),
        Value::Object(object) => {
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                return count_text_tokens(text);
            }
            if let Some(thinking) = object.get("thinking").and_then(Value::as_str) {
                return count_text_tokens(thinking);
            }
            if let Some(input) = object.get("input") {
                return estimate_serialized_value_tokens(input);
            }
            if let Some(content) = object.get("content") {
                return count_message_content_tokens(content);
            }
            0
        }
        _ => 0,
    }
}

fn estimate_serialized_value_tokens(value: &Value) -> u64 {
    serde_json::to_string(value)
        .map(|value| count_text_tokens(&value))
        .unwrap_or(1)
}

fn count_text_tokens(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }

    let mut cjk_count = 0usize;
    let mut other_count = 0usize;
    for c in text.chars() {
        if c.is_whitespace() {
            continue;
        }
        if is_cjk(c) {
            cjk_count += 1;
        } else {
            other_count += 1;
        }
    }

    let tokens = (cjk_count as f64 / 1.5) + (other_count as f64 / 3.5);
    tokens.round() as u64
}

fn is_cjk(c: char) -> bool {
    matches!(
        c,
        '\u{4E00}'..='\u{9FFF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{3040}'..='\u{309F}'
            | '\u{30A0}'..='\u{30FF}'
            | '\u{AC00}'..='\u{D7AF}'
            | '\u{1100}'..='\u{11FF}'
            | '\u{3130}'..='\u{318F}'
    )
}

fn minimum_cacheable_tokens_for_model(model: &str) -> u64 {
    let model = model.to_ascii_lowercase();
    if model.contains("opus") {
        4096
    } else if model.contains("haiku-3") || model.contains("haiku_3") {
        2048
    } else {
        1024
    }
}

impl KiroPromptCacheTracker {
    pub(crate) fn compute_and_update(
        &self,
        credential_id: String,
        profile: &KiroPromptCacheProfile,
    ) -> KiroPromptCacheUsage {
        self.compute_and_update_at(credential_id, profile, Instant::now())
    }

    fn compute_and_update_at(
        &self,
        credential_id: String,
        profile: &KiroPromptCacheProfile,
        now: Instant,
    ) -> KiroPromptCacheUsage {
        let Ok(mut entries) = self.entries.lock() else {
            return KiroPromptCacheUsage::default();
        };

        entries.retain(|_, entry| entry.expires_at > now);
        let last_breakpoint = profile.breakpoints.last().copied();
        let Some(last_breakpoint) = last_breakpoint else {
            return KiroPromptCacheUsage::default();
        };

        let mut matched_tokens = 0;
        for breakpoint in profile
            .breakpoints
            .iter()
            .rev()
            .take(PREFIX_LOOKBACK_LIMIT.saturating_add(1))
        {
            let key = (credential_id.clone(), breakpoint.fingerprint);
            let Some(entry) = entries.get(&key) else {
                continue;
            };
            if entry.expires_at > now {
                matched_tokens = entry
                    .token_count
                    .min(breakpoint.cumulative_tokens)
                    .min(profile.total_input_tokens);
                break;
            }
        }

        let creation_tokens = last_breakpoint
            .cumulative_tokens
            .min(profile.total_input_tokens)
            .saturating_sub(matched_tokens);

        for breakpoint in &profile.breakpoints {
            let key = (credential_id.clone(), breakpoint.fingerprint);
            match entries.get_mut(&key) {
                Some(existing) => {
                    existing.token_count = existing.token_count.max(breakpoint.cumulative_tokens);
                    existing.ttl = existing.ttl.max(breakpoint.ttl);
                }
                None => {
                    self.evict_to_capacity(&mut entries);
                    entries.insert(
                        key,
                        KiroPromptCacheEntry {
                            token_count: breakpoint.cumulative_tokens,
                            ttl: breakpoint.ttl,
                            expires_at: now + breakpoint.ttl,
                        },
                    );
                }
            }
        }

        KiroPromptCacheUsage {
            cache_creation_input_tokens: creation_tokens,
            cache_read_input_tokens: matched_tokens,
        }
    }

    fn evict_to_capacity(&self, entries: &mut HashMap<(String, [u8; 32]), KiroPromptCacheEntry>) {
        while MAX_ENTRIES > 0 && entries.len() >= MAX_ENTRIES {
            let Some(oldest_key) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            entries.remove(&oldest_key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long_text(label: &str) -> String {
        format!("{} {}", label, "cacheable prompt chunk ".repeat(300))
    }

    #[test]
    fn profile_strips_cache_control_from_fingerprint() {
        let default_ttl_body = serde_json::json!({
            "model": "claude-sonnet-4.6",
            "system": [{
                "type": "text",
                "text": long_text("system"),
                "cache_control": {"type": "ephemeral"}
            }],
            "messages": [{"role": "user", "content": "Perform a web search for the query: Shanghai weather"}],
            "tools": [{"type": "web_search_20250305", "name": "web_search"}]
        });
        let one_hour_body = serde_json::json!({
            "model": "claude-sonnet-4.6",
            "system": [{
                "type": "text",
                "text": long_text("system"),
                "cache_control": {"type": "ephemeral", "ttl": "1h"}
            }],
            "messages": [{"role": "user", "content": "Perform a web search for the query: Shanghai weather"}],
            "tools": [{"type": "web_search_20250305", "name": "web_search"}]
        });

        let default_profile = build_kiro_prompt_cache_profile(&default_ttl_body, 1800)
            .expect("default ttl body should build a cache profile");
        let one_hour_profile = build_kiro_prompt_cache_profile(&one_hour_body, 1800)
            .expect("one hour body should build a cache profile");

        assert_eq!(
            default_profile
                .breakpoints
                .last()
                .map(|value| value.fingerprint),
            one_hour_profile
                .breakpoints
                .last()
                .map(|value| value.fingerprint)
        );
        assert_eq!(
            default_profile.breakpoints.last().map(|value| value.ttl),
            Some(DEFAULT_CACHE_TTL)
        );
        assert_eq!(
            one_hour_profile.breakpoints.last().map(|value| value.ttl),
            Some(ONE_HOUR_CACHE_TTL)
        );
    }

    #[test]
    fn tracker_supports_prefix_hits_without_extending_expiry() {
        let base = serde_json::json!({
            "model": "claude-sonnet-4.6",
            "system": [{
                "type": "text",
                "text": long_text("shared system"),
                "cache_control": {"type": "ephemeral"}
            }],
            "messages": [{"role": "user", "content": "Perform a web search for the query: Shanghai weather"}],
            "tools": [{"type": "web_search_20250305", "name": "web_search"}]
        });
        let extended = serde_json::json!({
            "model": "claude-sonnet-4.6",
            "system": [{
                "type": "text",
                "text": long_text("shared system"),
                "cache_control": {"type": "ephemeral"}
            }],
            "messages": [
                {"role": "user", "content": "Perform a web search for the query: Shanghai weather"},
                {"role": "assistant", "content": "Previous answer"},
                {"role": "user", "content": "Perform a web search for the query: Shanghai weather tomorrow"}
            ],
            "tools": [{"type": "web_search_20250305", "name": "web_search"}]
        });
        let base_profile =
            build_kiro_prompt_cache_profile(&base, 1800).expect("base should be cacheable");
        let extended_profile =
            build_kiro_prompt_cache_profile(&extended, 2200).expect("extended should be cacheable");
        let tracker = KiroPromptCacheTracker::default();
        let start = Instant::now();

        let first = tracker.compute_and_update_at("cred".to_string(), &base_profile, start);
        assert!(first.cache_creation_input_tokens > 0);
        assert_eq!(first.cache_read_input_tokens, 0);

        let hit = tracker.compute_and_update_at(
            "cred".to_string(),
            &extended_profile,
            start + Duration::from_secs(299),
        );
        assert!(hit.cache_read_input_tokens > 0);

        let expired = tracker.compute_and_update_at(
            "cred".to_string(),
            &base_profile,
            start + Duration::from_secs(301),
        );
        assert!(expired.cache_creation_input_tokens > 0);
        assert_eq!(expired.cache_read_input_tokens, 0);
    }

    #[test]
    fn tracker_reads_cached_prefix_when_cache_control_moves_to_new_tail() {
        let first = serde_json::json!({
            "model": "claude-sonnet-4.6",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": long_text("shared first turn"),
                    "cache_control": {"type": "ephemeral"}
                }]
            }]
        });
        let second = serde_json::json!({
            "model": "claude-sonnet-4.6",
            "messages": [
                {
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": long_text("shared first turn")
                    }]
                },
                {
                    "role": "assistant",
                    "content": "cached response"
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": long_text("new tail turn"),
                        "cache_control": {"type": "ephemeral"}
                    }]
                }
            ]
        });
        let first_profile =
            build_kiro_prompt_cache_profile(&first, estimate_kiro_prompt_input_tokens(&first))
                .expect("first request should be cacheable");
        let second_profile =
            build_kiro_prompt_cache_profile(&second, estimate_kiro_prompt_input_tokens(&second))
                .expect("second request should be cacheable");
        let tracker = KiroPromptCacheTracker::default();
        let start = Instant::now();

        let created = tracker.compute_and_update_at("cred".to_string(), &first_profile, start);
        assert!(created.cache_creation_input_tokens > 0);
        assert_eq!(created.cache_read_input_tokens, 0);

        let hit = tracker.compute_and_update_at(
            "cred".to_string(),
            &second_profile,
            start + Duration::from_secs(60),
        );
        assert!(hit.cache_read_input_tokens > 0);
        assert!(hit.cache_creation_input_tokens > 0);
    }

    #[test]
    fn billed_input_tokens_subtracts_cache_usage() {
        assert_eq!(
            billed_input_tokens(
                100,
                KiroPromptCacheUsage {
                    cache_creation_input_tokens: 30,
                    cache_read_input_tokens: 40,
                },
            ),
            30
        );
        assert_eq!(
            billed_input_tokens(
                20,
                KiroPromptCacheUsage {
                    cache_creation_input_tokens: 30,
                    cache_read_input_tokens: 40,
                },
            ),
            0
        );
    }

    #[test]
    fn estimated_input_keeps_serialized_message_overhead_outside_cache() {
        let tracker = KiroPromptCacheTracker::default();
        let first = serde_json::json!({
            "model": "claude-sonnet-4-6",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "cacheable prompt chunk ".repeat(500),
                    "cache_control": {"type": "ephemeral"}
                }]
            }]
        });
        let second = serde_json::json!({
            "model": "claude-sonnet-4-6",
            "messages": [
                {
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": "cacheable prompt chunk ".repeat(500),
                        "cache_control": {"type": "ephemeral"}
                    }]
                },
                {
                    "role": "assistant",
                    "content": "cached reply"
                },
                {
                    "role": "user",
                    "content": "new user turn"
                }
            ]
        });

        let first_estimated = estimate_kiro_prompt_input_tokens(&first);
        let first_profile = build_kiro_prompt_cache_profile(&first, first_estimated)
            .expect("first request should be cacheable");
        tracker.compute_and_update("cred".to_string(), &first_profile);

        let second_estimated = estimate_kiro_prompt_input_tokens(&second);
        let second_profile = build_kiro_prompt_cache_profile(&second, second_estimated)
            .expect("second request should be cacheable");
        let usage = tracker.compute_and_update("cred".to_string(), &second_profile);

        assert!(
            second_estimated
                > usage
                    .cache_creation_input_tokens
                    .saturating_add(usage.cache_read_input_tokens)
        );
        assert!(billed_input_tokens(second_estimated, usage) > 0);
    }

    #[test]
    fn estimated_input_tokens_include_message_overhead() {
        let request = serde_json::json!({
            "model": "claude-opus-4-7",
            "system": [{
                "type": "text",
                "text": "cacheable system ".repeat(400),
                "cache_control": {"type": "ephemeral"}
            }],
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "cacheable prompt ".repeat(800),
                    "cache_control": {"type": "ephemeral"}
                }]
            }]
        });

        let estimated = estimate_kiro_prompt_input_tokens(&request);
        let profile = build_kiro_prompt_cache_profile(&request, estimated)
            .expect("request should produce a cache profile");
        let tracker = KiroPromptCacheTracker::default();
        let usage = tracker.compute_and_update("cred".to_string(), &profile);

        let last_breakpoint_tokens = profile
            .breakpoints
            .last()
            .map(|breakpoint| breakpoint.cumulative_tokens)
            .expect("cache profile should have a breakpoint");

        assert!(estimated > last_breakpoint_tokens);
        assert!(billed_input_tokens(estimated, usage) > 0);
    }

    #[test]
    fn estimated_input_tokens_do_not_count_inline_image_base64_as_text() {
        let request = serde_json::json!({
            "model": "claude-sonnet-4-6",
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "text",
                        "text": "Please inspect this screenshot."
                    },
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "a".repeat(200_000)
                        }
                    }
                ]
            }]
        });

        let estimated = estimate_kiro_prompt_input_tokens(&request);

        assert!(
            estimated < 1_000,
            "estimated input tokens should ignore inline image bytes, got {estimated}"
        );
    }

    #[test]
    fn kiro_simulated_cache_enabled_defaults_to_false_when_missing() {
        assert!(!kiro_simulated_cache_enabled_from_provider_config(None));
        assert!(!kiro_simulated_cache_enabled_from_provider_config(Some(
            &serde_json::json!({})
        )));
        assert!(!kiro_simulated_cache_enabled_from_provider_config(Some(
            &serde_json::json!({"kiro": {}})
        )));
        assert!(!kiro_simulated_cache_enabled_from_provider_config(Some(
            &serde_json::json!({"kiro": {"simulated_cache_enabled": false}})
        )));
    }

    #[test]
    fn kiro_simulated_cache_enabled_reads_nested_provider_config() {
        assert!(kiro_simulated_cache_enabled_from_provider_config(Some(
            &serde_json::json!({"kiro": {"simulated_cache_enabled": true}})
        )));
    }

    #[test]
    fn kiro_simulated_cache_enabled_reads_report_context_flag() {
        assert!(!kiro_simulated_cache_enabled_from_report_context(None));
        assert!(!kiro_simulated_cache_enabled_from_report_context(Some(
            &serde_json::json!({})
        )));
        assert!(kiro_simulated_cache_enabled_from_report_context(Some(
            &serde_json::json!({"kiro_simulated_cache_enabled": true})
        )));
    }
}
