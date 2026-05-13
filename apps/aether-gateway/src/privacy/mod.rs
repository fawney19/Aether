use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use aether_data_contracts::DataLayerError;
use aether_runtime_state::RuntimeState;
use chrono::NaiveDate;
use hmac::Mac;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest as _;

use crate::GatewayError;

type HmacSha256 = hmac::Hmac<sha2::Sha256>;

const DEFAULT_REDACTION_TTL_SECONDS: u64 = 300;
const DEFAULT_MAX_SCANNED_CHAT_TEXT_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_MAX_REDACTION_DETECTIONS: usize = 1024;
const HMAC96_BYTES: usize = 12;
const SENTINEL_PREFIX: &str = "<AETHER:";
const DIRECT_RESTORE_SENTINEL_LIMIT: usize = 32;
const MAX_CACHE_SENTINEL_BYTES: usize = 128;
const MAX_CACHE_RECORD_BYTES: usize = 512;

static EMAIL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[A-Z0-9._%+-]{1,64}@[A-Z0-9.-]{1,253}\.[A-Z]{2,63}")
        .expect("email regex should compile")
});
static CN_MOBILE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\+?86[- ]?)?1[3-9]\d[- ]?\d{4}[- ]?\d{4}")
        .expect("cn mobile regex should compile")
});
static CN_LANDLINE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\+?86[- ]?)?0\d{2,3}[- ]\d{7,8}(?:-\d{1,6})?")
        .expect("cn landline regex should compile")
});
static E164_PHONE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\+[1-9]\d(?:[ -]?\d){6,13}\d").expect("e164 phone regex should compile")
});
static CN_ID_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b\d{17}[\dX]\b").expect("cn id regex should compile"));
static PAYMENT_CARD_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:\d[ -]?){12,18}\d\b").expect("payment card regex should compile")
});
static IPV4_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b").expect("ipv4 regex should compile")
});
static OPENAI_KEY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}\b").expect("openai key regex should compile")
});
static ANTHROPIC_KEY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bsk-ant-[A-Za-z0-9_-]{20,}\b").expect("anthropic key regex should compile")
});
static GITHUB_TOKEN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:gh[pousr]_[A-Za-z0-9_]{30,}|github_pat_[A-Za-z0-9_]{30,})\b")
        .expect("github token regex should compile")
});
static SLACK_TOKEN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b").expect("slack token regex should compile")
});
static AWS_KEY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b").expect("aws key regex should compile")
});
static BEARER_TOKEN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]{20,}")
        .expect("bearer token regex should compile")
});
static JWT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b")
        .expect("jwt regex should compile")
});
static ACCESS_TOKEN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\baccess[_-]?token\s*[:=]\s*["']?[A-Za-z0-9._~+/=-]{20,}"#)
        .expect("access token regex should compile")
});
static SECRET_KEY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\bsecret[_-]?key\s*[:=]\s*["']?[A-Za-z0-9._~+/=-]{20,}"#)
        .expect("secret key regex should compile")
});
static HIGH_ENTROPY_TOKEN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Za-z0-9_-]{32,}\b").expect("api key regex should compile"));
static SENTINEL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<AETHER:[A-Z0-9_]+:[A-Z2-7]{20}>").expect("sentinel regex should compile")
});

#[derive(Clone, Copy, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum RedactionKind {
    Email,
    ChinaMobile,
    ChinaLandline,
    Phone,
    ChinaResidentId,
    PaymentCard,
    Ipv4,
    Ipv6,
    OpenAiKey,
    AnthropicKey,
    GitHubToken,
    SlackToken,
    AwsKey,
    BearerToken,
    Jwt,
    AccessToken,
    SecretKey,
    ApiKey,
}

impl RedactionKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Email => "EMAIL",
            Self::ChinaMobile | Self::ChinaLandline => "CN_PHONE",
            Self::Phone => "PHONE",
            Self::ChinaResidentId => "CN_ID",
            Self::PaymentCard => "PAYMENT_CARD",
            Self::Ipv4 => "IPV4",
            Self::Ipv6 => "IPV6",
            Self::OpenAiKey => "OPENAI_KEY",
            Self::AnthropicKey => "ANTHROPIC_KEY",
            Self::GitHubToken => "GITHUB_TOKEN",
            Self::SlackToken => "SLACK_TOKEN",
            Self::AwsKey => "AWS_KEY",
            Self::BearerToken => "BEARER_TOKEN",
            Self::Jwt => "JWT",
            Self::AccessToken => "ACCESS_TOKEN",
            Self::SecretKey => "SECRET_KEY",
            Self::ApiKey => "API_KEY",
        }
    }
}

impl fmt::Debug for RedactionKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

#[derive(Clone)]
pub(crate) struct RedactionSessionConfig {
    hmac_key: Vec<u8>,
    ttl_seconds: u64,
    now_unix_secs: u64,
    enabled_kinds: Option<HashSet<RedactionKind>>,
}

impl RedactionSessionConfig {
    pub(crate) fn new(hmac_key: impl Into<Vec<u8>>, ttl_seconds: u64, now_unix_secs: u64) -> Self {
        Self {
            hmac_key: hmac_key.into(),
            ttl_seconds: ttl_seconds.max(1),
            now_unix_secs,
            enabled_kinds: None,
        }
    }

    pub(crate) fn default_ttl(hmac_key: impl Into<Vec<u8>>, now_unix_secs: u64) -> Self {
        Self::new(hmac_key, DEFAULT_REDACTION_TTL_SECONDS, now_unix_secs)
    }

    fn bucket(&self) -> u64 {
        self.now_unix_secs / self.ttl_seconds
    }

    fn expires_at_unix_secs(&self) -> u64 {
        self.bucket()
            .saturating_add(1)
            .saturating_mul(self.ttl_seconds)
    }

    pub(crate) fn with_enabled_kinds(mut self, enabled_kinds: HashSet<RedactionKind>) -> Self {
        self.enabled_kinds = Some(enabled_kinds);
        self
    }

    fn kind_enabled(&self, kind: RedactionKind) -> bool {
        self.enabled_kinds
            .as_ref()
            .is_none_or(|enabled_kinds| enabled_kinds.contains(&kind))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RedactionScanLimits {
    pub(crate) max_scanned_text_bytes: usize,
    pub(crate) max_detections: usize,
}

impl Default for RedactionScanLimits {
    fn default() -> Self {
        Self {
            max_scanned_text_bytes: DEFAULT_MAX_SCANNED_CHAT_TEXT_BYTES,
            max_detections: DEFAULT_MAX_REDACTION_DETECTIONS,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RedactionLimitError {
    ScannedTextTooLarge { limit: usize },
    TooManyDetections { limit: usize },
}

impl RedactionLimitError {
    pub(crate) const fn client_status(&self) -> http::StatusCode {
        match self {
            Self::ScannedTextTooLarge { .. } => http::StatusCode::PAYLOAD_TOO_LARGE,
            Self::TooManyDetections { .. } => http::StatusCode::UNPROCESSABLE_ENTITY,
        }
    }

    pub(crate) const fn safe_message(&self) -> &'static str {
        match self {
            Self::ScannedTextTooLarge { .. } => "chat pii redaction scanned text limit exceeded",
            Self::TooManyDetections { .. } => "chat pii redaction detection limit exceeded",
        }
    }
}

#[derive(Debug)]
pub(crate) enum RedactionMaskError {
    Limit(RedactionLimitError),
}

impl From<RedactionLimitError> for RedactionMaskError {
    fn from(error: RedactionLimitError) -> Self {
        Self::Limit(error)
    }
}

#[derive(Clone, Copy)]
struct RedactionScanState {
    limits: RedactionScanLimits,
    scanned_text_bytes: usize,
    detections: usize,
}

impl RedactionScanState {
    fn new(limits: RedactionScanLimits) -> Self {
        Self {
            limits,
            scanned_text_bytes: 0,
            detections: 0,
        }
    }

    fn record_scan(&mut self, text: &str) -> Result<(), RedactionLimitError> {
        self.scanned_text_bytes = self.scanned_text_bytes.saturating_add(text.len());
        if self.scanned_text_bytes > self.limits.max_scanned_text_bytes {
            return Err(RedactionLimitError::ScannedTextTooLarge {
                limit: self.limits.max_scanned_text_bytes,
            });
        }
        Ok(())
    }

    fn record_detections(&mut self, count: usize) -> Result<(), RedactionLimitError> {
        self.detections = self.detections.saturating_add(count);
        if self.detections > self.limits.max_detections {
            return Err(RedactionLimitError::TooManyDetections {
                limit: self.limits.max_detections,
            });
        }
        Ok(())
    }
}

#[derive(Default)]
struct DetectorProbe {
    validator_calls: HashMap<RedactionKind, usize>,
}

impl DetectorProbe {
    fn record_validator_call(&mut self, kind: RedactionKind) {
        *self.validator_calls.entry(kind).or_default() += 1;
    }

    #[cfg(test)]
    fn validator_calls(&self, kind: RedactionKind) -> usize {
        self.validator_calls.get(&kind).copied().unwrap_or_default()
    }
}

impl fmt::Debug for RedactionSessionConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedactionSessionConfig")
            .field("hmac_key", &"<redacted>")
            .field("ttl_seconds", &self.ttl_seconds)
            .field("now_unix_secs", &self.now_unix_secs)
            .field(
                "enabled_kind_count",
                &self.enabled_kinds.as_ref().map(HashSet::len),
            )
            .finish()
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct MappingKey {
    kind: RedactionKind,
    original: String,
}

pub(crate) struct RedactionSession {
    config: RedactionSessionConfig,
    mappings: HashMap<MappingKey, RedactionMapping>,
    sentinel_index: HashMap<String, MappingKey>,
    collision_corpus: Vec<String>,
}

impl RedactionSession {
    pub(crate) fn new(config: RedactionSessionConfig) -> Self {
        Self {
            config,
            mappings: HashMap::new(),
            sentinel_index: HashMap::new(),
            collision_corpus: Vec::new(),
        }
    }

    fn set_collision_corpus(&mut self, collision_corpus: Vec<String>) {
        self.collision_corpus = collision_corpus;
    }

    pub(crate) fn redact_text(&mut self, input: &str) -> RedactedText {
        self.redact_text_internal(input, None)
            .expect("unlimited redaction scan should not fail")
    }

    fn redact_text_checked(
        &mut self,
        input: &str,
        scan_state: &mut RedactionScanState,
    ) -> Result<RedactedText, RedactionLimitError> {
        scan_state.record_scan(input)?;
        self.redact_text_internal(input, Some(scan_state))
    }

    async fn redact_text_with_cache(
        &mut self,
        input: &str,
        scan_state: &mut RedactionScanState,
        cache: Option<&RedisRedactionMappingCache<'_>>,
    ) -> Result<RedactedText, RedactionMaskError> {
        scan_state.record_scan(input)?;
        let candidates = select_non_overlapping(
            detect_candidates(input)
                .into_iter()
                .filter(|candidate| self.config.kind_enabled(candidate.kind))
                .collect(),
        );
        scan_state.record_detections(candidates.len())?;
        if candidates.is_empty() {
            return Ok(RedactedText {
                text: input.to_string(),
                matches: Vec::new(),
            });
        }

        let mut redacted = String::with_capacity(input.len());
        let mut cursor = 0;
        let mut matches = Vec::with_capacity(candidates.len());

        for candidate in candidates {
            redacted.push_str(&input[cursor..candidate.start]);
            let sentinel = self
                .sentinel_for_candidate_with_cache(input, candidate.kind, &candidate.value, cache)
                .await;
            redacted.push_str(&sentinel);
            matches.push(RedactionMatch {
                kind: candidate.kind,
                start: candidate.start,
                end: candidate.end,
                original: candidate.value,
                sentinel,
            });
            cursor = candidate.end;
        }
        redacted.push_str(&input[cursor..]);

        Ok(RedactedText {
            text: redacted,
            matches,
        })
    }

    fn redact_text_internal(
        &mut self,
        input: &str,
        mut scan_state: Option<&mut RedactionScanState>,
    ) -> Result<RedactedText, RedactionLimitError> {
        let candidates = select_non_overlapping(
            detect_candidates(input)
                .into_iter()
                .filter(|candidate| self.config.kind_enabled(candidate.kind))
                .collect(),
        );
        if let Some(scan_state) = scan_state.as_mut() {
            scan_state.record_detections(candidates.len())?;
        }
        if candidates.is_empty() {
            return Ok(RedactedText {
                text: input.to_string(),
                matches: Vec::new(),
            });
        }

        let mut redacted = String::with_capacity(input.len());
        let mut cursor = 0;
        let mut matches = Vec::with_capacity(candidates.len());

        for candidate in candidates {
            redacted.push_str(&input[cursor..candidate.start]);
            let sentinel = self.sentinel_for_candidate(input, candidate.kind, &candidate.value);
            redacted.push_str(&sentinel);
            matches.push(RedactionMatch {
                kind: candidate.kind,
                start: candidate.start,
                end: candidate.end,
                original: candidate.value,
                sentinel,
            });
            cursor = candidate.end;
        }
        redacted.push_str(&input[cursor..]);

        Ok(RedactedText {
            text: redacted,
            matches,
        })
    }

    pub(crate) fn mapping_count(&self) -> usize {
        self.mappings.len()
    }

    pub(crate) fn sentinel_for_original(&self, original: &str) -> Option<&str> {
        self.mappings
            .values()
            .find(|mapping| mapping.original == original)
            .map(|mapping| mapping.sentinel.as_str())
    }

    pub(crate) fn mappings(&self) -> impl Iterator<Item = &RedactionMapping> {
        self.mappings.values()
    }

    fn restore_text(&self, input: &str) -> RestoredText {
        if self.mapping_count() <= DIRECT_RESTORE_SENTINEL_LIMIT {
            return restore_text_direct_longest_first(input, self);
        }
        restore_text_with_matcher(input, &SentinelMatcher::new(self))
    }

    fn sentinel_for_candidate(
        &mut self,
        source_text: &str,
        kind: RedactionKind,
        original: &str,
    ) -> String {
        let key = MappingKey {
            kind,
            original: normalize_redaction_value(kind, original),
        };
        if let Some(mapping) = self.mappings.get(&key) {
            return mapping.sentinel.clone();
        }

        let bucket = self.config.bucket();
        let mut collision_counter = 0u32;
        let sentinel = loop {
            let candidate = self.build_sentinel(kind, original, bucket, collision_counter);
            let collides_with_input =
                self.sentinel_collides_with_original_text(source_text, &candidate);
            let collides_with_mapping = self
                .sentinel_index
                .get(&candidate)
                .is_some_and(|existing_key| existing_key != &key);
            if !collides_with_input && !collides_with_mapping {
                break candidate;
            }
            collision_counter = collision_counter.saturating_add(1);
        };

        let mapping = RedactionMapping {
            kind,
            original: original.to_string(),
            normalized_value: key.original.clone(),
            sentinel: sentinel.clone(),
            bucket,
            created_at_unix_secs: self.config.now_unix_secs,
            expires_at_unix_secs: self.config.expires_at_unix_secs(),
        };
        self.sentinel_index.insert(sentinel.clone(), key.clone());
        self.mappings.insert(key, mapping);
        sentinel
    }

    async fn sentinel_for_candidate_with_cache(
        &mut self,
        source_text: &str,
        kind: RedactionKind,
        original: &str,
        cache: Option<&RedisRedactionMappingCache<'_>>,
    ) -> String {
        let key = MappingKey {
            kind,
            original: normalize_redaction_value(kind, original),
        };
        if let Some(mapping) = self.mappings.get(&key) {
            return mapping.sentinel.clone();
        }

        let bucket = self.config.bucket();
        if let Some(cache) = cache {
            if let Ok(Some(sentinel)) = cache.lookup_sentinel(kind, &key.original, bucket).await {
                if sentinel.len() <= MAX_CACHE_SENTINEL_BYTES
                    && self.cached_sentinel_usable(source_text, &key, &sentinel)
                {
                    self.insert_mapping_for_sentinel(key, kind, original, sentinel.clone(), bucket);
                    return sentinel;
                }
            }
        }

        let sentinel = self.sentinel_for_candidate(source_text, kind, original);
        if let Some(cache) = cache {
            if let Some(mapping) = self.mappings.get(&key) {
                let ttl_seconds = self
                    .config
                    .expires_at_unix_secs()
                    .saturating_sub(self.config.now_unix_secs)
                    .max(1);
                let _ = cache
                    .store(
                        &RedactionCacheRecord::from_mapping(mapping),
                        &mapping.normalized_value,
                        ttl_seconds,
                    )
                    .await;
            }
        }
        sentinel
    }

    fn cached_sentinel_usable(&self, source_text: &str, key: &MappingKey, sentinel: &str) -> bool {
        if self.sentinel_collides_with_original_text(source_text, sentinel) {
            return false;
        }
        !self
            .sentinel_index
            .get(sentinel)
            .is_some_and(|existing_key| existing_key != key)
    }

    fn sentinel_collides_with_original_text(&self, source_text: &str, sentinel: &str) -> bool {
        source_text.contains(sentinel)
            || self
                .collision_corpus
                .iter()
                .any(|original_text| original_text.contains(sentinel))
    }

    fn insert_mapping_for_sentinel(
        &mut self,
        key: MappingKey,
        kind: RedactionKind,
        original: &str,
        sentinel: String,
        bucket: u64,
    ) {
        let mapping = RedactionMapping {
            kind,
            original: original.to_string(),
            normalized_value: key.original.clone(),
            sentinel: sentinel.clone(),
            bucket,
            created_at_unix_secs: self.config.now_unix_secs,
            expires_at_unix_secs: self.config.expires_at_unix_secs(),
        };
        self.sentinel_index.insert(sentinel, key.clone());
        self.mappings.insert(key, mapping);
    }

    fn build_sentinel(
        &self,
        kind: RedactionKind,
        original: &str,
        bucket: u64,
        collision_counter: u32,
    ) -> String {
        let mut mac = HmacSha256::new_from_slice(&self.config.hmac_key)
            .expect("HMAC accepts keys of any size");
        mac.update(b"aether-redaction-v1\0");
        mac.update(kind.label().as_bytes());
        mac.update(b"\0");
        mac.update(bucket.to_string().as_bytes());
        mac.update(b"\0");
        mac.update(collision_counter.to_string().as_bytes());
        mac.update(b"\0");
        mac.update(normalize_redaction_value(kind, original).as_bytes());
        let digest = mac.finalize().into_bytes();
        format!(
            "{}{}:{}>",
            SENTINEL_PREFIX,
            kind.label(),
            base32_no_pad(&digest[..HMAC96_BYTES])
        )
    }
}

impl fmt::Debug for RedactionSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut counts: HashMap<&'static str, usize> = HashMap::new();
        for mapping in self.mappings.values() {
            *counts.entry(mapping.kind.label()).or_default() += 1;
        }
        formatter
            .debug_struct("RedactionSession")
            .field("ttl_seconds", &self.config.ttl_seconds)
            .field("bucket", &self.config.bucket())
            .field("mapping_count", &self.mappings.len())
            .field("type_counts", &counts)
            .finish()
    }
}

pub(crate) struct MaskedChatRequest {
    pub(crate) body: Vec<u8>,
    pub(crate) session: RedactionSession,
    pub(crate) redacted: bool,
}

impl fmt::Debug for MaskedChatRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MaskedChatRequest")
            .field("body_len", &self.body.len())
            .field("session", &self.session)
            .field("redacted", &self.redacted)
            .finish()
    }
}

#[derive(Default, Clone)]
pub(crate) struct RedactionSessionSlot {
    session: Arc<Mutex<Option<RedactionSession>>>,
    sessions_by_candidate: Arc<Mutex<HashMap<String, RedactionSession>>>,
}

impl RedactionSessionSlot {
    pub(crate) fn put(&self, session: RedactionSession) {
        *self
            .session
            .lock()
            .expect("redaction session slot should lock") = Some(session);
    }

    pub(crate) fn put_for_candidate(&self, candidate_id: &str, session: RedactionSession) {
        self.sessions_by_candidate
            .lock()
            .expect("redaction session candidate slot should lock")
            .insert(candidate_id.to_string(), session);
    }

    pub(crate) fn clear(&self) {
        *self
            .session
            .lock()
            .expect("redaction session slot should lock") = None;
        self.sessions_by_candidate
            .lock()
            .expect("redaction session candidate slot should lock")
            .clear();
    }

    pub(crate) fn take(&self) -> Option<RedactionSession> {
        self.session
            .lock()
            .expect("redaction session slot should lock")
            .take()
    }

    pub(crate) fn take_for_candidate(
        &self,
        candidate_id: Option<&str>,
    ) -> Option<RedactionSession> {
        let Some(candidate_id) = candidate_id else {
            return self.take_without_candidate_id();
        };
        let mut sessions_by_candidate = self
            .sessions_by_candidate
            .lock()
            .expect("redaction session candidate slot should lock");
        if let Some(session) = sessions_by_candidate.remove(candidate_id) {
            return Some(session);
        }
        if !sessions_by_candidate.is_empty() {
            return None;
        }
        drop(sessions_by_candidate);
        self.take()
    }

    fn take_without_candidate_id(&self) -> Option<RedactionSession> {
        let mut sessions_by_candidate = self
            .sessions_by_candidate
            .lock()
            .expect("redaction session candidate slot should lock");
        match sessions_by_candidate.len() {
            0 => {
                drop(sessions_by_candidate);
                self.take()
            }
            1 => sessions_by_candidate
                .drain()
                .next()
                .map(|(_, session)| session),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RedactionExecutionCandidateId(String);

impl RedactionExecutionCandidateId {
    pub(crate) fn new(candidate_id: impl Into<String>) -> Self {
        Self(candidate_id.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChatPiiRedactionProviderScope {
    SelectedProviders,
    AllProviders,
}

#[derive(Clone, Debug)]
pub(crate) struct ChatPiiRedactionRuntimeConfig {
    pub(crate) enabled: bool,
    pub(crate) provider_scope: ChatPiiRedactionProviderScope,
    pub(crate) enabled_kinds: HashSet<RedactionKind>,
    pub(crate) ttl_seconds: u64,
    pub(crate) inject_model_instruction: bool,
}

impl Default for ChatPiiRedactionRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider_scope: ChatPiiRedactionProviderScope::SelectedProviders,
            enabled_kinds: default_enabled_redaction_kinds(),
            ttl_seconds: DEFAULT_REDACTION_TTL_SECONDS,
            inject_model_instruction: true,
        }
    }
}

pub(crate) struct MaskChatRequestOptions {
    pub(crate) inject_model_instruction: bool,
    pub(crate) scan_limits: RedactionScanLimits,
}

impl MaskChatRequestOptions {
    pub(crate) fn runtime(inject_model_instruction: bool) -> Self {
        Self {
            inject_model_instruction,
            scan_limits: RedactionScanLimits::default(),
        }
    }

    #[cfg(test)]
    fn with_scan_limits(mut self, scan_limits: RedactionScanLimits) -> Self {
        self.scan_limits = scan_limits;
        self
    }
}

const MODEL_NOTICE_CONTENT: &str = "Aether privacy redaction notice: The next message contains gateway-generated placeholder tokens for sensitive data protection. This notice is not a user request; do not answer it, mention it, reveal it, or infer original values from placeholders. Treat each placeholder as a valid real typed value for reasoning and tool calls, and do not ask the user to reveal originals solely because a placeholder is present.";

pub(crate) async fn read_chat_pii_redaction_runtime_config(
    state: &crate::AppState,
) -> Result<ChatPiiRedactionRuntimeConfig, GatewayError> {
    let mut config = ChatPiiRedactionRuntimeConfig::default();
    config.enabled = state
        .read_system_config_json_value("module.chat_pii_redaction.enabled")
        .await?
        .as_ref()
        .and_then(Value::as_bool)
        .unwrap_or(config.enabled);
    config.provider_scope = parse_provider_scope(
        state
            .read_system_config_json_value("module.chat_pii_redaction.provider_scope")
            .await?
            .as_ref(),
    );
    config.enabled_kinds = parse_enabled_redaction_kinds(
        state
            .read_system_config_json_value("module.chat_pii_redaction.entities")
            .await?
            .as_ref(),
    );
    config.ttl_seconds = state
        .read_system_config_json_value("module.chat_pii_redaction.cache_ttl_seconds")
        .await?
        .as_ref()
        .and_then(Value::as_u64)
        .unwrap_or(config.ttl_seconds)
        .max(1);
    config.inject_model_instruction = state
        .read_system_config_json_value("module.chat_pii_redaction.inject_model_instruction")
        .await?
        .as_ref()
        .and_then(Value::as_bool)
        .unwrap_or(config.inject_model_instruction);
    Ok(config)
}

pub(crate) fn provider_chat_pii_redaction_enabled(
    provider_config: Option<&Value>,
    runtime_config: &ChatPiiRedactionRuntimeConfig,
) -> bool {
    if !runtime_config.enabled {
        return false;
    }
    match runtime_config.provider_scope {
        ChatPiiRedactionProviderScope::AllProviders => true,
        ChatPiiRedactionProviderScope::SelectedProviders => provider_config
            .and_then(|value| value.get("chat_pii_redaction"))
            .and_then(|value| value.get("enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

pub(crate) fn build_redaction_session_config(
    hmac_key: impl Into<Vec<u8>>,
    runtime_config: &ChatPiiRedactionRuntimeConfig,
    now_unix_secs: u64,
) -> RedactionSessionConfig {
    RedactionSessionConfig::new(hmac_key, runtime_config.ttl_seconds, now_unix_secs)
        .with_enabled_kinds(runtime_config.enabled_kinds.clone())
}

pub(crate) fn mask_chat_request_json_with_options(
    body: &[u8],
    config: RedactionSessionConfig,
    options: MaskChatRequestOptions,
) -> MaskedChatRequest {
    try_mask_chat_request_json_with_options(body, config, options)
        .expect("default chat redaction limits should not be exceeded")
}

pub(crate) fn mask_chat_request_json(
    body: &[u8],
    config: RedactionSessionConfig,
) -> MaskedChatRequest {
    mask_chat_request_json_with_options(body, config, MaskChatRequestOptions::runtime(false))
}

pub(crate) fn try_mask_chat_request_json_with_options(
    body: &[u8],
    config: RedactionSessionConfig,
    options: MaskChatRequestOptions,
) -> Result<MaskedChatRequest, RedactionLimitError> {
    mask_chat_request_json_internal(body, config, options, None)
}

pub(crate) async fn try_mask_chat_request_json_with_cache_options(
    body: &[u8],
    config: RedactionSessionConfig,
    options: MaskChatRequestOptions,
    cache: Option<&RedisRedactionMappingCache<'_>>,
) -> Result<MaskedChatRequest, RedactionMaskError> {
    mask_chat_request_json_internal_async(body, config, options, cache).await
}

fn mask_chat_request_json_internal(
    body: &[u8],
    config: RedactionSessionConfig,
    options: MaskChatRequestOptions,
    _cache: Option<&RedisRedactionMappingCache<'_>>,
) -> Result<MaskedChatRequest, RedactionLimitError> {
    let mut session = RedactionSession::new(config);
    let Ok(mut value) = serde_json::from_slice::<Value>(body) else {
        return Ok(MaskedChatRequest {
            body: body.to_vec(),
            session,
            redacted: false,
        });
    };

    let Some(collision_corpus) = value
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| chat_message_collision_corpus(messages))
    else {
        return Ok(MaskedChatRequest {
            body: body.to_vec(),
            session,
            redacted: false,
        });
    };
    session.set_collision_corpus(collision_corpus);

    let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) else {
        return Ok(MaskedChatRequest {
            body: body.to_vec(),
            session,
            redacted: false,
        });
    };

    let mut scan_state = RedactionScanState::new(options.scan_limits);
    let mut redacted = false;
    let mut notice_inserted = false;
    let mut index = 0;
    while index < messages.len() {
        let message_redacted =
            mask_chat_message_value(&mut messages[index], &mut session, &mut scan_state)?;
        redacted |= message_redacted;
        if options.inject_model_instruction && message_redacted && !notice_inserted {
            messages.insert(index, model_notice_message());
            notice_inserted = true;
            index += 1;
        }
        index += 1;
    }

    if !redacted {
        return Ok(MaskedChatRequest {
            body: body.to_vec(),
            session,
            redacted,
        });
    }

    let body = serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec());
    Ok(MaskedChatRequest {
        body,
        session,
        redacted,
    })
}

async fn mask_chat_request_json_internal_async(
    body: &[u8],
    config: RedactionSessionConfig,
    options: MaskChatRequestOptions,
    cache: Option<&RedisRedactionMappingCache<'_>>,
) -> Result<MaskedChatRequest, RedactionMaskError> {
    let mut session = RedactionSession::new(config);
    let Ok(mut value) = serde_json::from_slice::<Value>(body) else {
        return Ok(MaskedChatRequest {
            body: body.to_vec(),
            session,
            redacted: false,
        });
    };

    let Some(collision_corpus) = value
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| chat_message_collision_corpus(messages))
    else {
        return Ok(MaskedChatRequest {
            body: body.to_vec(),
            session,
            redacted: false,
        });
    };
    session.set_collision_corpus(collision_corpus);

    let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) else {
        return Ok(MaskedChatRequest {
            body: body.to_vec(),
            session,
            redacted: false,
        });
    };

    let mut scan_state = RedactionScanState::new(options.scan_limits);
    let mut redacted = false;
    let mut notice_inserted = false;
    let mut index = 0;
    while index < messages.len() {
        let message_redacted = mask_chat_message_value_async(
            &mut messages[index],
            &mut session,
            &mut scan_state,
            cache,
        )
        .await?;
        redacted |= message_redacted;
        if options.inject_model_instruction && message_redacted && !notice_inserted {
            messages.insert(index, model_notice_message());
            notice_inserted = true;
            index += 1;
        }
        index += 1;
    }

    if !redacted {
        return Ok(MaskedChatRequest {
            body: body.to_vec(),
            session,
            redacted,
        });
    }

    let body = serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec());
    Ok(MaskedChatRequest {
        body,
        session,
        redacted,
    })
}

fn model_notice_message() -> Value {
    serde_json::json!({
        "role": "assistant",
        "content": MODEL_NOTICE_CONTENT,
    })
}

fn chat_message_collision_corpus(messages: &[Value]) -> Vec<String> {
    let mut corpus = Vec::new();
    for message in messages {
        collect_chat_message_collision_text(message, &mut corpus);
    }
    corpus
}

fn collect_chat_message_collision_text(message: &Value, corpus: &mut Vec<String>) {
    let Some(message) = message.as_object() else {
        return;
    };
    if let Some(content) = message.get("content") {
        collect_chat_content_collision_text(content, corpus);
    }
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_calls {
            collect_tool_call_argument_collision_text(tool_call, corpus);
        }
    }
}

fn collect_chat_content_collision_text(content: &Value, corpus: &mut Vec<String>) {
    match content {
        Value::String(text) => corpus.push(text.clone()),
        Value::Array(parts) => {
            for part in parts {
                collect_chat_content_part_collision_text(part, corpus);
            }
        }
        _ => {}
    }
}

fn collect_chat_content_part_collision_text(part: &Value, corpus: &mut Vec<String>) {
    let Some(part) = part.as_object() else {
        return;
    };
    if part.get("type").and_then(Value::as_str) != Some("text") {
        return;
    }
    if let Some(text) = part.get("text").and_then(Value::as_str) {
        corpus.push(text.to_string());
    }
}

fn collect_tool_call_argument_collision_text(tool_call: &Value, corpus: &mut Vec<String>) {
    if let Some(arguments) = tool_call
        .get("function")
        .and_then(Value::as_object)
        .and_then(|function| function.get("arguments"))
        .and_then(Value::as_str)
    {
        corpus.push(arguments.to_string());
    }
}

fn mask_chat_message_value(
    message: &mut Value,
    session: &mut RedactionSession,
    scan_state: &mut RedactionScanState,
) -> Result<bool, RedactionLimitError> {
    let Some(message) = message.as_object_mut() else {
        return Ok(false);
    };

    let mut redacted = false;
    if let Some(content) = message.get_mut("content") {
        redacted |= mask_chat_content_value(content, session, scan_state)?;
    }
    if let Some(tool_calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) {
        for tool_call in tool_calls {
            redacted |= mask_tool_call_arguments(tool_call, session, scan_state)?;
        }
    }
    Ok(redacted)
}

fn mask_chat_content_value(
    content: &mut Value,
    session: &mut RedactionSession,
    scan_state: &mut RedactionScanState,
) -> Result<bool, RedactionLimitError> {
    match content {
        Value::String(text) => mask_json_string(text, session, scan_state),
        Value::Array(parts) => {
            let mut redacted = false;
            for part in parts {
                redacted |= mask_chat_content_part(part, session, scan_state)?;
            }
            Ok(redacted)
        }
        _ => Ok(false),
    }
}

fn mask_chat_content_part(
    part: &mut Value,
    session: &mut RedactionSession,
    scan_state: &mut RedactionScanState,
) -> Result<bool, RedactionLimitError> {
    let Some(part) = part.as_object_mut() else {
        return Ok(false);
    };
    if part.get("type").and_then(Value::as_str) != Some("text") {
        return Ok(false);
    }
    let Some(Value::String(text)) = part.get_mut("text") else {
        return Ok(false);
    };
    mask_json_string(text, session, scan_state)
}

fn mask_tool_call_arguments(
    tool_call: &mut Value,
    session: &mut RedactionSession,
    scan_state: &mut RedactionScanState,
) -> Result<bool, RedactionLimitError> {
    let Some(function) = tool_call.get_mut("function").and_then(Value::as_object_mut) else {
        return Ok(false);
    };
    let Some(Value::String(arguments)) = function.get_mut("arguments") else {
        return Ok(false);
    };
    mask_json_string(arguments, session, scan_state)
}

fn mask_json_string(
    text: &mut String,
    session: &mut RedactionSession,
    scan_state: &mut RedactionScanState,
) -> Result<bool, RedactionLimitError> {
    let redacted = session.redact_text_checked(text, scan_state)?;
    if redacted.matches.is_empty() {
        return Ok(false);
    }
    *text = redacted.text;
    Ok(true)
}

async fn mask_chat_message_value_async(
    message: &mut Value,
    session: &mut RedactionSession,
    scan_state: &mut RedactionScanState,
    cache: Option<&RedisRedactionMappingCache<'_>>,
) -> Result<bool, RedactionMaskError> {
    let Some(message) = message.as_object_mut() else {
        return Ok(false);
    };

    let mut redacted = false;
    if let Some(content) = message.get_mut("content") {
        redacted |= mask_chat_content_value_async(content, session, scan_state, cache).await?;
    }
    if let Some(tool_calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) {
        for tool_call in tool_calls {
            redacted |=
                mask_tool_call_arguments_async(tool_call, session, scan_state, cache).await?;
        }
    }
    Ok(redacted)
}

async fn mask_chat_content_value_async(
    content: &mut Value,
    session: &mut RedactionSession,
    scan_state: &mut RedactionScanState,
    cache: Option<&RedisRedactionMappingCache<'_>>,
) -> Result<bool, RedactionMaskError> {
    match content {
        Value::String(text) => mask_json_string_async(text, session, scan_state, cache).await,
        Value::Array(parts) => {
            let mut redacted = false;
            for part in parts {
                redacted |= mask_chat_content_part_async(part, session, scan_state, cache).await?;
            }
            Ok(redacted)
        }
        _ => Ok(false),
    }
}

async fn mask_chat_content_part_async(
    part: &mut Value,
    session: &mut RedactionSession,
    scan_state: &mut RedactionScanState,
    cache: Option<&RedisRedactionMappingCache<'_>>,
) -> Result<bool, RedactionMaskError> {
    let Some(part) = part.as_object_mut() else {
        return Ok(false);
    };
    if part.get("type").and_then(Value::as_str) != Some("text") {
        return Ok(false);
    }
    let Some(Value::String(text)) = part.get_mut("text") else {
        return Ok(false);
    };
    mask_json_string_async(text, session, scan_state, cache).await
}

async fn mask_tool_call_arguments_async(
    tool_call: &mut Value,
    session: &mut RedactionSession,
    scan_state: &mut RedactionScanState,
    cache: Option<&RedisRedactionMappingCache<'_>>,
) -> Result<bool, RedactionMaskError> {
    let Some(function) = tool_call.get_mut("function").and_then(Value::as_object_mut) else {
        return Ok(false);
    };
    let Some(Value::String(arguments)) = function.get_mut("arguments") else {
        return Ok(false);
    };
    mask_json_string_async(arguments, session, scan_state, cache).await
}

async fn mask_json_string_async(
    text: &mut String,
    session: &mut RedactionSession,
    scan_state: &mut RedactionScanState,
    cache: Option<&RedisRedactionMappingCache<'_>>,
) -> Result<bool, RedactionMaskError> {
    let redacted = session
        .redact_text_with_cache(text, scan_state, cache)
        .await?;
    if redacted.matches.is_empty() {
        return Ok(false);
    }
    *text = redacted.text;
    Ok(true)
}

pub(crate) struct RestoredSyncResponseBody {
    pub(crate) body: Vec<u8>,
    pub(crate) restored: bool,
}

impl fmt::Debug for RestoredSyncResponseBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RestoredSyncResponseBody")
            .field("body_len", &self.body.len())
            .field("restored", &self.restored)
            .finish()
    }
}

struct RestoredText {
    text: String,
    restored: bool,
}

pub(crate) fn restore_sync_response_body(
    headers: &mut BTreeMap<String, String>,
    body: &[u8],
    session: &RedactionSession,
) -> Result<RestoredSyncResponseBody, GatewayError> {
    if session.mapping_count() == 0 {
        return Ok(RestoredSyncResponseBody {
            body: body.to_vec(),
            restored: false,
        });
    }

    ensure_identity_response_encoding(headers)?;

    let restored = if response_body_is_json(headers, body) {
        restore_json_response_body(body, session)?
    } else {
        restore_text_response_body(body, session)
    };
    if restored.restored {
        set_content_length(headers, restored.body.len());
    }
    Ok(restored)
}

fn ensure_identity_response_encoding(
    headers: &BTreeMap<String, String>,
) -> Result<(), GatewayError> {
    for encoding in headers
        .iter()
        .filter(|(header_name, _)| header_name.eq_ignore_ascii_case("content-encoding"))
        .map(|(_, value)| value)
    {
        if encoding.trim().is_empty() || encoding.trim().eq_ignore_ascii_case("identity") {
            continue;
        }
        return Err(GatewayError::Internal(
            "redaction response restoration does not support encoded response bodies".to_string(),
        ));
    }
    Ok(())
}

fn response_body_is_json(headers: &BTreeMap<String, String>, body: &[u8]) -> bool {
    if header_value(headers, "content-type").is_some_and(content_type_is_json) {
        return true;
    }
    body.iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| matches!(byte, b'{' | b'['))
}

fn content_type_is_json(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .is_some_and(|mime| mime == "application/json" || mime.ends_with("+json"))
}

fn restore_json_response_body(
    body: &[u8],
    session: &RedactionSession,
) -> Result<RestoredSyncResponseBody, GatewayError> {
    let Ok(mut value) = serde_json::from_slice::<Value>(body) else {
        return Ok(restore_text_response_body(body, session));
    };
    if !restore_json_strings(&mut value, session) {
        return Ok(RestoredSyncResponseBody {
            body: body.to_vec(),
            restored: false,
        });
    }
    let body = serde_json::to_vec(&value).map_err(|err| GatewayError::Internal(err.to_string()))?;
    Ok(RestoredSyncResponseBody {
        body,
        restored: true,
    })
}

fn restore_json_strings(value: &mut Value, session: &RedactionSession) -> bool {
    match value {
        Value::String(text) => {
            let restored = session.restore_text(text);
            if !restored.restored {
                return false;
            }
            *text = restored.text;
            true
        }
        Value::Array(values) => {
            let mut restored = false;
            for value in values {
                restored = restore_json_strings(value, session) || restored;
            }
            restored
        }
        Value::Object(values) => {
            let mut restored = false;
            for value in values.values_mut() {
                restored = restore_json_strings(value, session) || restored;
            }
            restored
        }
        _ => false,
    }
}

fn restore_text_response_body(body: &[u8], session: &RedactionSession) -> RestoredSyncResponseBody {
    let Ok(text) = std::str::from_utf8(body) else {
        return RestoredSyncResponseBody {
            body: body.to_vec(),
            restored: false,
        };
    };
    let restored = session.restore_text(text);
    RestoredSyncResponseBody {
        body: restored.text.into_bytes(),
        restored: restored.restored,
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum StreamRestoreMode {
    Sse,
    Text,
}

pub(crate) struct StreamingResponseRestorer<'a> {
    session: &'a RedactionSession,
    matcher: SentinelMatcher<'a>,
    mode: StreamRestoreMode,
    max_sentinel_len: usize,
    text_carry: Vec<u8>,
    sse_line_buffer: Vec<u8>,
    sse_json_event_lines: Vec<Vec<u8>>,
}

impl<'a> StreamingResponseRestorer<'a> {
    pub(crate) fn new(
        headers: &BTreeMap<String, String>,
        session: &'a RedactionSession,
    ) -> Result<Self, GatewayError> {
        if session.mapping_count() > 0 {
            ensure_identity_response_encoding(headers)?;
        }
        let mode = if header_value(headers, "content-type").is_some_and(content_type_is_sse) {
            StreamRestoreMode::Sse
        } else {
            StreamRestoreMode::Text
        };
        Ok(Self::with_mode(session, mode))
    }

    #[cfg(test)]
    fn for_sse(session: &'a RedactionSession) -> Self {
        Self::with_mode(session, StreamRestoreMode::Sse)
    }

    #[cfg(test)]
    fn for_text(session: &'a RedactionSession) -> Self {
        Self::with_mode(session, StreamRestoreMode::Text)
    }

    fn with_mode(session: &'a RedactionSession, mode: StreamRestoreMode) -> Self {
        let max_sentinel_len = session
            .mappings()
            .map(|mapping| mapping.sentinel.len())
            .max()
            .unwrap_or_default();
        Self {
            session,
            matcher: SentinelMatcher::new(session),
            mode,
            max_sentinel_len,
            text_carry: Vec::new(),
            sse_line_buffer: Vec::new(),
            sse_json_event_lines: Vec::new(),
        }
    }

    pub(crate) fn push_chunk(&mut self, chunk: &[u8]) -> Result<Vec<u8>, GatewayError> {
        if self.max_sentinel_len == 0 {
            return Ok(chunk.to_vec());
        }
        match self.mode {
            StreamRestoreMode::Sse => self.push_sse_chunk(chunk),
            StreamRestoreMode::Text => Ok(self.push_text_chunk(chunk)),
        }
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<u8>, GatewayError> {
        if self.max_sentinel_len == 0 {
            return Ok(Vec::new());
        }
        match self.mode {
            StreamRestoreMode::Sse => self.finish_sse(),
            StreamRestoreMode::Text => Ok(self.flush_text_carry()),
        }
    }

    #[cfg(test)]
    fn max_text_carry_len(&self) -> usize {
        self.max_sentinel_len.saturating_sub(1)
    }

    #[cfg(test)]
    fn pending_text_carry_len(&self) -> usize {
        self.text_carry.len()
    }

    fn push_text_chunk(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.text_carry.extend_from_slice(chunk);
        self.restore_available_text(false)
    }

    fn flush_text_carry(&mut self) -> Vec<u8> {
        self.restore_available_text(true)
    }

    fn restore_available_text(&mut self, flush: bool) -> Vec<u8> {
        let scan_limit = if flush {
            self.text_carry.len()
        } else {
            self.text_carry
                .len()
                .saturating_sub(self.max_sentinel_len.saturating_sub(1))
        };
        let mut output = Vec::with_capacity(scan_limit);
        let mut index = 0;
        while index < scan_limit {
            if let Some(mapping) = self.matcher.matching_mapping_at(&self.text_carry, index) {
                output.extend_from_slice(mapping.original.as_bytes());
                index += mapping.sentinel.len();
            } else {
                output.push(self.text_carry[index]);
                index += 1;
            }
        }
        self.text_carry.drain(..index);
        output
    }

    fn push_sse_chunk(&mut self, chunk: &[u8]) -> Result<Vec<u8>, GatewayError> {
        self.sse_line_buffer.extend_from_slice(chunk);
        let mut output = Vec::new();
        while let Some(line_end) = self.sse_line_buffer.iter().position(|byte| *byte == b'\n') {
            let line = self.sse_line_buffer.drain(..=line_end).collect::<Vec<_>>();
            self.push_sse_line(line, &mut output)?;
        }
        Ok(output)
    }

    fn finish_sse(&mut self) -> Result<Vec<u8>, GatewayError> {
        let mut output = Vec::new();
        if !self.sse_line_buffer.is_empty() {
            let line = std::mem::take(&mut self.sse_line_buffer);
            self.push_sse_line(line, &mut output)?;
        }
        self.flush_sse_json_event(&mut output)?;
        Ok(output)
    }

    fn push_sse_line(&mut self, line: Vec<u8>, output: &mut Vec<u8>) -> Result<(), GatewayError> {
        if !self.sse_json_event_lines.is_empty() {
            if sse_line_is_blank(&line) {
                self.flush_sse_json_event(output)?;
                output.extend_from_slice(&line);
            } else {
                self.sse_json_event_lines.push(line);
            }
            return Ok(());
        }

        if sse_line_is_blank(&line) {
            output.extend_from_slice(&line);
            return Ok(());
        }

        let Some(value) = sse_data_line_value(&line) else {
            output.extend_from_slice(&line);
            return Ok(());
        };
        if sse_data_value_is_done(value) {
            output.extend_from_slice(&line);
            return Ok(());
        }
        if sse_data_value_may_be_json(value) {
            self.sse_json_event_lines.push(line);
            return Ok(());
        }
        output.extend(self.restore_sse_text_data_line(&line));
        Ok(())
    }

    fn flush_sse_json_event(&mut self, output: &mut Vec<u8>) -> Result<(), GatewayError> {
        if self.sse_json_event_lines.is_empty() {
            return Ok(());
        }

        let event_lines = std::mem::take(&mut self.sse_json_event_lines);
        let Some(payload) = sse_event_data_payload(&event_lines) else {
            output.extend(event_lines.into_iter().flatten());
            return Ok(());
        };
        if payload.trim() == "[DONE]" {
            output.extend(event_lines.into_iter().flatten());
            return Ok(());
        }

        let Ok(mut value) = serde_json::from_str::<Value>(&payload) else {
            for line in event_lines {
                if sse_data_line_value(&line).is_some() {
                    output.extend(self.restore_sse_text_data_line(&line));
                } else {
                    output.extend_from_slice(&line);
                }
            }
            return Ok(());
        };

        if !restore_json_strings(&mut value, self.session) {
            output.extend(event_lines.into_iter().flatten());
            return Ok(());
        }

        let restored_payload =
            serde_json::to_string(&value).map_err(|err| GatewayError::Internal(err.to_string()))?;
        let mut wrote_data_line = false;
        for line in event_lines {
            if sse_data_line_value(&line).is_some() {
                if !wrote_data_line {
                    output.extend(replace_sse_data_line_value(
                        &line,
                        restored_payload.as_bytes(),
                    ));
                    wrote_data_line = true;
                }
            } else {
                output.extend_from_slice(&line);
            }
        }
        Ok(())
    }

    fn restore_sse_text_data_line(&self, line: &[u8]) -> Vec<u8> {
        let Some(range) = sse_data_line_value_range(line) else {
            return line.to_vec();
        };
        let (body, ending) = split_sse_line_ending(line);
        let restored = restore_known_sentinels_in_bytes(&body[range.clone()], self.session);
        let mut output = Vec::with_capacity(line.len());
        output.extend_from_slice(&body[..range.start]);
        output.extend(restored);
        output.extend_from_slice(ending);
        output
    }
}

enum SentinelMatcher<'a> {
    Direct(Vec<&'a RedactionMapping>),
    Trie(SentinelTrie<'a>),
}

impl<'a> SentinelMatcher<'a> {
    fn new(session: &'a RedactionSession) -> Self {
        let mut mappings = session.mappings().collect::<Vec<_>>();
        mappings.sort_by_key(|mapping| std::cmp::Reverse(mapping.sentinel.len()));
        if mappings.len() <= DIRECT_RESTORE_SENTINEL_LIMIT {
            Self::Direct(mappings)
        } else {
            Self::Trie(SentinelTrie::new(mappings))
        }
    }

    fn matching_mapping_at(&self, input: &[u8], index: usize) -> Option<&'a RedactionMapping> {
        match self {
            Self::Direct(mappings) => mappings.iter().copied().find(|mapping| {
                input
                    .get(index..)
                    .is_some_and(|remaining| remaining.starts_with(mapping.sentinel.as_bytes()))
            }),
            Self::Trie(trie) => trie.matching_mapping_at(input, index),
        }
    }
}

struct SentinelTrie<'a> {
    nodes: Vec<SentinelTrieNode<'a>>,
}

struct SentinelTrieNode<'a> {
    children: HashMap<u8, usize>,
    mapping: Option<&'a RedactionMapping>,
}

impl<'a> SentinelTrie<'a> {
    fn new(mappings: Vec<&'a RedactionMapping>) -> Self {
        let mut trie = Self {
            nodes: vec![SentinelTrieNode {
                children: HashMap::new(),
                mapping: None,
            }],
        };
        for mapping in mappings {
            trie.insert(mapping);
        }
        trie
    }

    fn insert(&mut self, mapping: &'a RedactionMapping) {
        let mut node_index = 0;
        for byte in mapping.sentinel.bytes() {
            let next_index = if let Some(next_index) = self.nodes[node_index].children.get(&byte) {
                *next_index
            } else {
                self.nodes.push(SentinelTrieNode {
                    children: HashMap::new(),
                    mapping: None,
                });
                let next_index = self.nodes.len() - 1;
                self.nodes[node_index].children.insert(byte, next_index);
                next_index
            };
            node_index = next_index;
        }
        self.nodes[node_index].mapping = Some(mapping);
    }

    fn matching_mapping_at(&self, input: &[u8], index: usize) -> Option<&'a RedactionMapping> {
        let mut node_index = 0;
        let mut best = None;
        for byte in input.get(index..).unwrap_or_default() {
            let Some(next_index) = self.nodes[node_index].children.get(byte).copied() else {
                break;
            };
            node_index = next_index;
            if let Some(mapping) = self.nodes[node_index].mapping {
                best = Some(mapping);
            }
        }
        best
    }
}

fn restore_text_direct_longest_first(input: &str, session: &RedactionSession) -> RestoredText {
    let mut mappings = session.mappings().collect::<Vec<_>>();
    mappings.sort_by_key(|mapping| std::cmp::Reverse(mapping.sentinel.len()));
    let mut text = input.to_string();
    let mut restored = false;
    for mapping in mappings {
        if text.contains(&mapping.sentinel) {
            text = text.replace(&mapping.sentinel, &mapping.original);
            restored = true;
        }
    }
    RestoredText { text, restored }
}

fn restore_text_with_matcher(input: &str, matcher: &SentinelMatcher<'_>) -> RestoredText {
    let input_bytes = input.as_bytes();
    let mut output = Vec::with_capacity(input_bytes.len());
    let mut restored = false;
    let mut index = 0;
    while index < input_bytes.len() {
        if let Some(mapping) = matcher.matching_mapping_at(input_bytes, index) {
            output.extend_from_slice(mapping.original.as_bytes());
            index += mapping.sentinel.len();
            restored = true;
        } else {
            output.push(input_bytes[index]);
            index += 1;
        }
    }
    let text = String::from_utf8(output).unwrap_or_else(|_| input.to_string());
    RestoredText { text, restored }
}

fn restore_known_sentinels_in_bytes(input: &[u8], session: &RedactionSession) -> Vec<u8> {
    let matcher = SentinelMatcher::new(session);
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if let Some(mapping) = matcher.matching_mapping_at(input, index) {
            output.extend_from_slice(mapping.original.as_bytes());
            index += mapping.sentinel.len();
        } else {
            output.push(input[index]);
            index += 1;
        }
    }
    output
}

fn content_type_is_sse(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .map(str::trim)
        .is_some_and(|mime| mime.eq_ignore_ascii_case("text/event-stream"))
}

fn split_sse_line_ending(line: &[u8]) -> (&[u8], &[u8]) {
    if line.ends_with(b"\r\n") {
        (&line[..line.len() - 2], &line[line.len() - 2..])
    } else if line.ends_with(b"\n") {
        (&line[..line.len() - 1], &line[line.len() - 1..])
    } else {
        (line, &[])
    }
}

fn sse_line_is_blank(line: &[u8]) -> bool {
    split_sse_line_ending(line).0.is_empty()
}

fn sse_data_line_value_range(line: &[u8]) -> Option<std::ops::Range<usize>> {
    let (body, _) = split_sse_line_ending(line);
    if !body.starts_with(b"data:") {
        return None;
    }
    let start = if body.get(5) == Some(&b' ') { 6 } else { 5 };
    Some(start..body.len())
}

fn sse_data_line_value(line: &[u8]) -> Option<&[u8]> {
    let (body, _) = split_sse_line_ending(line);
    sse_data_line_value_range(line).map(|range| &body[range])
}

fn sse_data_value_is_done(value: &[u8]) -> bool {
    std::str::from_utf8(value)
        .map(str::trim)
        .is_ok_and(|value| value == "[DONE]")
}

fn sse_data_value_may_be_json(value: &[u8]) -> bool {
    value
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| matches!(byte, b'{' | b'['))
}

fn sse_event_data_payload(lines: &[Vec<u8>]) -> Option<String> {
    let mut payload = String::new();
    let mut saw_data = false;
    for line in lines {
        let Some(value) = sse_data_line_value(line) else {
            continue;
        };
        if saw_data {
            payload.push('\n');
        }
        payload.push_str(std::str::from_utf8(value).ok()?);
        saw_data = true;
    }
    saw_data.then_some(payload)
}

fn replace_sse_data_line_value(line: &[u8], value: &[u8]) -> Vec<u8> {
    let Some(range) = sse_data_line_value_range(line) else {
        return line.to_vec();
    };
    let (body, ending) = split_sse_line_ending(line);
    let mut output = Vec::with_capacity(body.len() - range.len() + value.len() + ending.len());
    output.extend_from_slice(&body[..range.start]);
    output.extend_from_slice(value);
    output.extend_from_slice(ending);
    output
}

fn header_value<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn set_content_length(headers: &mut BTreeMap<String, String>, body_len: usize) {
    headers.retain(|name, _| !name.eq_ignore_ascii_case("content-length"));
    headers.insert("content-length".to_string(), body_len.to_string());
}

pub(crate) struct RedactedText {
    pub(crate) text: String,
    pub(crate) matches: Vec<RedactionMatch>,
}

impl fmt::Debug for RedactedText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedactedText")
            .field("text_len", &self.text.len())
            .field("match_count", &self.matches.len())
            .finish()
    }
}

pub(crate) struct RedactionMatch {
    pub(crate) kind: RedactionKind,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) original: String,
    pub(crate) sentinel: String,
}

impl fmt::Debug for RedactionMatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedactionMatch")
            .field("kind", &self.kind)
            .field("start", &self.start)
            .field("end", &self.end)
            .field("original_len", &self.original.len())
            .field("sentinel", &redacted_sentinel_debug(&self.sentinel))
            .finish()
    }
}

pub(crate) struct RedactionMapping {
    pub(crate) kind: RedactionKind,
    pub(crate) original: String,
    pub(crate) normalized_value: String,
    pub(crate) sentinel: String,
    pub(crate) bucket: u64,
    pub(crate) created_at_unix_secs: u64,
    pub(crate) expires_at_unix_secs: u64,
}

impl fmt::Debug for RedactionMapping {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedactionMapping")
            .field("kind", &self.kind)
            .field("original_len", &self.original.len())
            .field("normalized_value_len", &self.normalized_value.len())
            .field("sentinel", &redacted_sentinel_debug(&self.sentinel))
            .field("bucket", &self.bucket)
            .field("created_at_unix_secs", &self.created_at_unix_secs)
            .field("expires_at_unix_secs", &self.expires_at_unix_secs)
            .finish()
    }
}

#[derive(Deserialize, Serialize)]
pub(crate) struct RedactionCacheRecord {
    pub(crate) kind: RedactionKind,
    pub(crate) sentinel: String,
    pub(crate) bucket: u64,
    pub(crate) expires_at_unix_secs: u64,
}

impl RedactionCacheRecord {
    pub(crate) fn from_mapping(mapping: &RedactionMapping) -> Self {
        Self {
            kind: mapping.kind,
            sentinel: mapping.sentinel.clone(),
            bucket: mapping.bucket,
            expires_at_unix_secs: mapping.expires_at_unix_secs,
        }
    }
}

impl fmt::Debug for RedactionCacheRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedactionCacheRecord")
            .field("kind", &self.kind)
            .field("sentinel", &redacted_sentinel_debug(&self.sentinel))
            .field("bucket", &self.bucket)
            .field("expires_at_unix_secs", &self.expires_at_unix_secs)
            .finish()
    }
}

pub(crate) struct RedisRedactionMappingCache<'a> {
    runtime_state: &'a RuntimeState,
    key_prefix: String,
}

impl<'a> RedisRedactionMappingCache<'a> {
    pub(crate) fn new(runtime_state: &'a RuntimeState) -> Self {
        Self {
            runtime_state,
            key_prefix: "privacy:redaction:mapping".to_string(),
        }
    }

    pub(crate) fn with_key_prefix(
        runtime_state: &'a RuntimeState,
        key_prefix: impl Into<String>,
    ) -> Self {
        Self {
            runtime_state,
            key_prefix: key_prefix.into(),
        }
    }

    pub(crate) async fn store(
        &self,
        record: &RedactionCacheRecord,
        normalized_value: &str,
        ttl_seconds: u64,
    ) -> Result<(), DataLayerError> {
        if record.sentinel.len() > MAX_CACHE_SENTINEL_BYTES {
            return Err(DataLayerError::InvalidInput(
                "redaction cache sentinel is too large".to_string(),
            ));
        }
        let value = serde_json::to_string(record)
            .map_err(|err| DataLayerError::InvalidInput(err.to_string()))?;
        if value.len() > MAX_CACHE_RECORD_BYTES {
            return Err(DataLayerError::InvalidInput(
                "redaction cache record is too large".to_string(),
            ));
        }
        let ttl_seconds = ttl_seconds.max(1);
        self.runtime_state
            .kv_set(
                &self.forward_cache_key(record.kind, normalized_value, record.bucket),
                record.sentinel.clone(),
                Some(Duration::from_secs(ttl_seconds)),
            )
            .await?;
        self.runtime_state
            .kv_set(
                &self.reverse_cache_key(&record.sentinel),
                value,
                Some(Duration::from_secs(ttl_seconds)),
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn lookup_sentinel(
        &self,
        kind: RedactionKind,
        normalized_value: &str,
        bucket: u64,
    ) -> Result<Option<String>, DataLayerError> {
        let sentinel = self
            .get_string(&self.forward_cache_key(kind, normalized_value, bucket))
            .await?;
        Ok(sentinel.filter(|value| {
            value.len() <= MAX_CACHE_SENTINEL_BYTES && sentinel_matches_kind(kind, value)
        }))
    }

    pub(crate) async fn load(
        &self,
        sentinel: &str,
    ) -> Result<Option<RedactionCacheRecord>, DataLayerError> {
        let raw = self.get_string(&self.reverse_cache_key(sentinel)).await?;
        raw.map(|value| {
            serde_json::from_str::<RedactionCacheRecord>(&value)
                .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))
        })
        .transpose()
    }

    async fn get_string(&self, key: &str) -> Result<Option<String>, DataLayerError> {
        self.runtime_state.kv_get(key).await
    }

    fn forward_cache_key(
        &self,
        kind: RedactionKind,
        normalized_value: &str,
        bucket: u64,
    ) -> String {
        format!(
            "{}:forward:{}:{}:{}",
            self.key_prefix,
            kind.label(),
            bucket,
            normalized_value_cache_digest(normalized_value)
        )
    }

    fn reverse_cache_key(&self, sentinel: &str) -> String {
        format!("{}:reverse:{}", self.key_prefix, sentinel)
    }
}

#[derive(Clone)]
struct Candidate {
    kind: RedactionKind,
    start: usize,
    end: usize,
    value: String,
    priority: u8,
}

impl Candidate {
    fn new(kind: RedactionKind, start: usize, end: usize, value: &str, priority: u8) -> Self {
        Self {
            kind,
            start,
            end,
            value: value.to_string(),
            priority,
        }
    }

    fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
}

fn detect_candidates(input: &str) -> Vec<Candidate> {
    detect_candidates_with_probe(input, None)
}

fn detect_candidates_with_probe(
    input: &str,
    mut probe: Option<&mut DetectorProbe>,
) -> Vec<Candidate> {
    // Segment-level scan-bypass cache is intentionally deferred until profiling shows regex scanning is the bottleneck.
    let mut candidates = Vec::new();

    if input.contains('@') {
        push_regex_candidates(
            input,
            &EMAIL_REGEX,
            RedactionKind::Email,
            10,
            &mut candidates,
            probe.as_deref_mut(),
            |value| value.contains('@'),
        );
    }

    let digit_count = input.chars().filter(|ch| ch.is_ascii_digit()).count();
    if digit_count >= 8 {
        push_regex_candidates(
            input,
            &CN_MOBILE_REGEX,
            RedactionKind::ChinaMobile,
            20,
            &mut candidates,
            probe.as_deref_mut(),
            is_valid_cn_mobile,
        );
        push_regex_candidates(
            input,
            &CN_LANDLINE_REGEX,
            RedactionKind::ChinaLandline,
            21,
            &mut candidates,
            probe.as_deref_mut(),
            is_valid_cn_landline,
        );
        push_regex_candidates(
            input,
            &E164_PHONE_REGEX,
            RedactionKind::Phone,
            30,
            &mut candidates,
            probe.as_deref_mut(),
            is_valid_e164_phone,
        );
        push_regex_candidates(
            input,
            &CN_ID_REGEX,
            RedactionKind::ChinaResidentId,
            40,
            &mut candidates,
            probe.as_deref_mut(),
            is_valid_cn_resident_id,
        );
        push_regex_candidates(
            input,
            &PAYMENT_CARD_REGEX,
            RedactionKind::PaymentCard,
            50,
            &mut candidates,
            probe.as_deref_mut(),
            is_valid_payment_card,
        );
    }

    if input.contains('.') {
        push_regex_candidates(
            input,
            &IPV4_REGEX,
            RedactionKind::Ipv4,
            60,
            &mut candidates,
            probe.as_deref_mut(),
            |value| value.parse::<Ipv4Addr>().is_ok(),
        );
        push_regex_candidates(
            input,
            &JWT_REGEX,
            RedactionKind::Jwt,
            120,
            &mut candidates,
            probe.as_deref_mut(),
            is_valid_jwt_like,
        );
    }

    if input.contains(':') {
        candidates.extend(detect_ipv6_candidates(input));
    }

    if input.contains("sk-") {
        push_regex_candidates(
            input,
            &OPENAI_KEY_REGEX,
            RedactionKind::OpenAiKey,
            80,
            &mut candidates,
            probe.as_deref_mut(),
            |_| true,
        );
    }
    if input.contains("sk-ant-") {
        push_regex_candidates(
            input,
            &ANTHROPIC_KEY_REGEX,
            RedactionKind::AnthropicKey,
            70,
            &mut candidates,
            probe.as_deref_mut(),
            |_| true,
        );
    }
    if input.contains("ghp_")
        || input.contains("gho_")
        || input.contains("ghu_")
        || input.contains("ghs_")
        || input.contains("ghr_")
        || input.contains("github_pat_")
    {
        push_regex_candidates(
            input,
            &GITHUB_TOKEN_REGEX,
            RedactionKind::GitHubToken,
            90,
            &mut candidates,
            probe.as_deref_mut(),
            |_| true,
        );
    }
    if input.contains("xox") {
        push_regex_candidates(
            input,
            &SLACK_TOKEN_REGEX,
            RedactionKind::SlackToken,
            100,
            &mut candidates,
            probe.as_deref_mut(),
            |_| true,
        );
    }
    if input.contains("AKIA") || input.contains("ASIA") {
        push_regex_candidates(
            input,
            &AWS_KEY_REGEX,
            RedactionKind::AwsKey,
            110,
            &mut candidates,
            probe.as_deref_mut(),
            |_| true,
        );
    }
    if input.contains("Bearer ") || input.contains("bearer ") {
        push_regex_candidates(
            input,
            &BEARER_TOKEN_REGEX,
            RedactionKind::BearerToken,
            115,
            &mut candidates,
            probe.as_deref_mut(),
            is_valid_bearer_token,
        );
    }
    if input.contains("access_token")
        || input.contains("access-token")
        || input.contains("AccessToken")
    {
        push_regex_candidates(
            input,
            &ACCESS_TOKEN_REGEX,
            RedactionKind::AccessToken,
            116,
            &mut candidates,
            probe.as_deref_mut(),
            is_valid_named_token,
        );
    }
    if input.contains("secret_key") || input.contains("secret-key") || input.contains("SecretKey") {
        push_regex_candidates(
            input,
            &SECRET_KEY_REGEX,
            RedactionKind::SecretKey,
            117,
            &mut candidates,
            probe.as_deref_mut(),
            is_valid_named_token,
        );
    }

    if input_may_contain_high_entropy_token(input) {
        push_regex_candidates(
            input,
            &HIGH_ENTROPY_TOKEN_REGEX,
            RedactionKind::ApiKey,
            200,
            &mut candidates,
            probe,
            is_strict_high_entropy_token,
        );
    }

    candidates.retain(|candidate| !looks_like_existing_sentinel(&candidate.value));
    candidates
}

fn push_regex_candidates(
    input: &str,
    regex: &Regex,
    kind: RedactionKind,
    priority: u8,
    candidates: &mut Vec<Candidate>,
    mut probe: Option<&mut DetectorProbe>,
    validator: impl Fn(&str) -> bool,
) {
    for matched in regex.find_iter(input) {
        let value = matched.as_str();
        if !has_token_boundary(input, matched.start(), matched.end()) {
            continue;
        }
        if let Some(probe) = probe.as_deref_mut() {
            probe.record_validator_call(kind);
        }
        if validator(value) {
            candidates.push(Candidate::new(
                kind,
                matched.start(),
                matched.end(),
                value,
                priority,
            ));
        }
    }
}

fn detect_ipv6_candidates(input: &str) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    let mut start = None;
    for (index, ch) in input.char_indices() {
        if ch.is_ascii_hexdigit() || ch == ':' || ch == '.' {
            start.get_or_insert(index);
            continue;
        }
        if let Some(token_start) = start.take() {
            push_ipv6_candidate(input, token_start, index, &mut candidates);
        }
    }
    if let Some(token_start) = start {
        push_ipv6_candidate(input, token_start, input.len(), &mut candidates);
    }
    candidates
}

fn push_ipv6_candidate(input: &str, start: usize, end: usize, candidates: &mut Vec<Candidate>) {
    let value = &input[start..end];
    if value.contains(':') && value.parse::<Ipv6Addr>().is_ok() {
        candidates.push(Candidate::new(RedactionKind::Ipv6, start, end, value, 65));
    }
}

fn select_non_overlapping(mut candidates: Vec<Candidate>) -> Vec<Candidate> {
    candidates.sort_by(|left, right| {
        right
            .len()
            .cmp(&left.len())
            .then_with(|| left.priority.cmp(&right.priority))
            .then_with(|| left.start.cmp(&right.start))
    });
    let mut occupied = HashSet::new();
    let mut selected = Vec::new();
    for candidate in candidates {
        if (candidate.start..candidate.end).any(|index| occupied.contains(&index)) {
            continue;
        }
        occupied.extend(candidate.start..candidate.end);
        selected.push(candidate);
    }
    selected.sort_by_key(|candidate| candidate.start);
    selected
}

fn has_token_boundary(input: &str, start: usize, end: usize) -> bool {
    let before = input[..start].chars().next_back();
    let after = input[end..].chars().next();
    !before.is_some_and(is_sensitive_token_char) && !after.is_some_and(is_sensitive_token_char)
}

fn is_sensitive_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '@')
}

fn input_may_contain_high_entropy_token(input: &str) -> bool {
    let mut token_len = 0usize;
    let mut class_mask = 0u8;
    for ch in input.chars() {
        let class = high_entropy_token_char_class(ch);
        if class == 0 {
            if token_len >= 32 && class_mask.count_ones() >= 3 {
                return true;
            }
            token_len = 0;
            class_mask = 0;
            continue;
        }
        token_len += 1;
        class_mask |= class;
    }
    token_len >= 32 && class_mask.count_ones() >= 3
}

fn high_entropy_token_char_class(ch: char) -> u8 {
    match ch {
        'a'..='z' => 0b0001,
        'A'..='Z' => 0b0010,
        '0'..='9' => 0b0100,
        '_' | '-' => 0b1000,
        _ => 0,
    }
}

fn digits_only(value: &str) -> String {
    value.chars().filter(|ch| ch.is_ascii_digit()).collect()
}

fn is_valid_cn_mobile(value: &str) -> bool {
    let mut digits = digits_only(value);
    if digits.len() == 13 && digits.starts_with("86") {
        digits = digits[2..].to_string();
    }
    digits.len() == 11
        && digits.starts_with('1')
        && digits
            .as_bytes()
            .get(1)
            .is_some_and(|digit| matches!(digit, b'3'..=b'9'))
}

fn is_valid_cn_landline(value: &str) -> bool {
    if !value.contains('-') {
        return false;
    }
    let mut digits = digits_only(value);
    if digits.len() >= 12 && digits.starts_with("86") {
        digits = digits[2..].to_string();
    }
    digits.starts_with('0') && (10..=17).contains(&digits.len())
}

fn is_valid_e164_phone(value: &str) -> bool {
    if !value.starts_with('+') {
        return false;
    }
    let digits = digits_only(value);
    (8..=15).contains(&digits.len()) && !digits.starts_with('0')
}

fn is_valid_cn_resident_id(value: &str) -> bool {
    if value.len() != 18 || !value[..17].chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }
    let birth = &value[6..14];
    let year = birth[0..4].parse::<i32>().ok();
    let month = birth[4..6].parse::<u32>().ok();
    let day = birth[6..8].parse::<u32>().ok();
    if year
        .zip(month)
        .zip(day)
        .and_then(|((year, month), day)| NaiveDate::from_ymd_opt(year, month, day))
        .is_none()
    {
        return false;
    }

    const WEIGHTS: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
    const CHECK_CODES: [char; 11] = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];
    let checksum = value[..17]
        .chars()
        .zip(WEIGHTS)
        .map(|(digit, weight)| digit.to_digit(10).unwrap_or_default() * weight)
        .sum::<u32>();
    let expected = CHECK_CODES[(checksum % 11) as usize];
    value
        .chars()
        .nth(17)
        .is_some_and(|actual| actual.to_ascii_uppercase() == expected)
}

fn is_valid_payment_card(value: &str) -> bool {
    let digits = digits_only(value);
    if !(13..=19).contains(&digits.len())
        || digits
            .chars()
            .all(|ch| ch == digits.chars().next().unwrap_or_default())
    {
        return false;
    }
    if !matches!(digits.as_bytes().first(), Some(b'3' | b'4' | b'5' | b'6')) {
        return false;
    }
    luhn_valid(&digits)
}

fn luhn_valid(digits: &str) -> bool {
    let mut sum = 0u32;
    let mut double = false;
    for digit in digits.chars().rev().filter_map(|ch| ch.to_digit(10)) {
        let mut value = digit;
        if double {
            value *= 2;
            if value > 9 {
                value -= 9;
            }
        }
        sum += value;
        double = !double;
    }
    sum.is_multiple_of(10)
}

fn is_valid_bearer_token(value: &str) -> bool {
    value
        .split_once(char::is_whitespace)
        .map(|(_, token)| token.trim().len() >= 20)
        .unwrap_or(false)
}

fn is_valid_named_token(value: &str) -> bool {
    value
        .split_once([':', '='])
        .map(|(_, token)| {
            let token = token.trim().trim_matches(['\'', '"']);
            token.len() >= 20 && token.chars().any(|ch| ch.is_ascii_digit())
        })
        .unwrap_or(false)
}

fn is_valid_jwt_like(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            part.len() >= 10
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        })
}

fn is_strict_high_entropy_token(value: &str) -> bool {
    if value.len() < 32 || looks_like_existing_sentinel(value) {
        return false;
    }
    let mut classes = 0;
    classes += value.chars().any(|ch| ch.is_ascii_lowercase()) as u8;
    classes += value.chars().any(|ch| ch.is_ascii_uppercase()) as u8;
    classes += value.chars().any(|ch| ch.is_ascii_digit()) as u8;
    classes += value.chars().any(|ch| matches!(ch, '_' | '-')) as u8;
    classes >= 3 && shannon_entropy(value) >= 4.0
}

fn shannon_entropy(value: &str) -> f64 {
    let mut counts = HashMap::<u8, usize>::new();
    for byte in value.bytes() {
        *counts.entry(byte).or_default() += 1;
    }
    let len = value.len() as f64;
    counts
        .values()
        .map(|count| {
            let probability = *count as f64 / len;
            -probability * probability.log2()
        })
        .sum()
}

fn looks_like_existing_sentinel(value: &str) -> bool {
    SENTINEL_REGEX.is_match(value)
}

fn sentinel_matches_kind(kind: RedactionKind, sentinel: &str) -> bool {
    if !full_sentinel_match(sentinel) {
        return false;
    }
    sentinel
        .strip_prefix(SENTINEL_PREFIX)
        .and_then(|value| value.strip_suffix('>'))
        .and_then(|value| value.split_once(':'))
        .is_some_and(|(label, _)| label == kind.label())
}

fn full_sentinel_match(value: &str) -> bool {
    SENTINEL_REGEX
        .find(value)
        .is_some_and(|matched| matched.start() == 0 && matched.end() == value.len())
}

fn normalize_redaction_value(kind: RedactionKind, value: &str) -> String {
    match kind {
        RedactionKind::Email => value.trim().to_ascii_lowercase(),
        RedactionKind::ChinaMobile
        | RedactionKind::ChinaLandline
        | RedactionKind::Phone
        | RedactionKind::PaymentCard => digits_only(value),
        RedactionKind::AccessToken | RedactionKind::SecretKey => value
            .split_once([':', '='])
            .map(|(_, token)| token.trim().trim_matches(['\'', '"']).to_string())
            .unwrap_or_else(|| value.trim().to_string()),
        RedactionKind::BearerToken => value
            .split_once(char::is_whitespace)
            .map(|(_, token)| token.trim().to_string())
            .unwrap_or_else(|| value.trim().to_string()),
        _ => value.trim().to_string(),
    }
}

fn normalized_value_cache_digest(normalized_value: &str) -> String {
    let digest = sha2::Sha256::digest(normalized_value.as_bytes());
    base32_no_pad(&digest[..HMAC96_BYTES])
}

fn parse_provider_scope(value: Option<&Value>) -> ChatPiiRedactionProviderScope {
    match value
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("all_providers") => ChatPiiRedactionProviderScope::AllProviders,
        _ => ChatPiiRedactionProviderScope::SelectedProviders,
    }
}

fn parse_enabled_redaction_kinds(value: Option<&Value>) -> HashSet<RedactionKind> {
    let Some(values) = value.and_then(Value::as_array) else {
        return default_enabled_redaction_kinds();
    };
    values
        .iter()
        .filter_map(Value::as_str)
        .flat_map(redaction_kinds_for_entity)
        .collect::<HashSet<_>>()
}

fn default_enabled_redaction_kinds() -> HashSet<RedactionKind> {
    [
        RedactionKind::Email,
        RedactionKind::ChinaMobile,
        RedactionKind::ChinaLandline,
        RedactionKind::Phone,
        RedactionKind::ChinaResidentId,
        RedactionKind::PaymentCard,
        RedactionKind::Ipv4,
        RedactionKind::Ipv6,
        RedactionKind::OpenAiKey,
        RedactionKind::AnthropicKey,
        RedactionKind::GitHubToken,
        RedactionKind::SlackToken,
        RedactionKind::AwsKey,
        RedactionKind::ApiKey,
        RedactionKind::AccessToken,
        RedactionKind::SecretKey,
        RedactionKind::BearerToken,
        RedactionKind::Jwt,
    ]
    .into_iter()
    .collect()
}

fn redaction_kinds_for_entity(entity: &str) -> impl Iterator<Item = RedactionKind> {
    match entity.trim().to_ascii_lowercase().as_str() {
        "email" => vec![RedactionKind::Email],
        "cn_phone" => vec![RedactionKind::ChinaMobile, RedactionKind::ChinaLandline],
        "global_phone" => vec![RedactionKind::Phone],
        "cn_id" => vec![RedactionKind::ChinaResidentId],
        "payment_card" => vec![RedactionKind::PaymentCard],
        "ipv4" => vec![RedactionKind::Ipv4],
        "ipv6" => vec![RedactionKind::Ipv6],
        "api_key" => vec![
            RedactionKind::OpenAiKey,
            RedactionKind::AnthropicKey,
            RedactionKind::GitHubToken,
            RedactionKind::SlackToken,
            RedactionKind::AwsKey,
            RedactionKind::ApiKey,
        ],
        "access_token" => vec![RedactionKind::AccessToken],
        "secret_key" => vec![RedactionKind::SecretKey],
        "bearer_token" => vec![RedactionKind::BearerToken],
        "jwt" => vec![RedactionKind::Jwt],
        _ => Vec::new(),
    }
    .into_iter()
}

fn base32_no_pad(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut output = String::with_capacity((bytes.len() * 8).div_ceil(5));
    let mut bit_buffer = 0u32;
    let mut bit_count = 0u8;

    for byte in bytes {
        bit_buffer = (bit_buffer << 8) | u32::from(*byte);
        bit_count += 8;
        while bit_count >= 5 {
            bit_count -= 5;
            let index = ((bit_buffer >> bit_count) & 0x1f) as usize;
            output.push(ALPHABET[index] as char);
        }
    }

    if bit_count > 0 {
        let index = ((bit_buffer << (5 - bit_count)) & 0x1f) as usize;
        output.push(ALPHABET[index] as char);
    }

    output
}

fn redacted_sentinel_debug(sentinel: &str) -> String {
    let kind = sentinel
        .strip_prefix(SENTINEL_PREFIX)
        .and_then(|rest| rest.split_once(':'))
        .map(|(kind, _)| kind)
        .unwrap_or("UNKNOWN");
    format!("<AETHER:{kind}:redacted>")
}

#[cfg(test)]
mod tests {
    use super::{
        build_redaction_session_config, detect_candidates_with_probe, mask_chat_request_json,
        mask_chat_request_json_with_options, parse_enabled_redaction_kinds,
        provider_chat_pii_redaction_enabled, restore_sync_response_body,
        try_mask_chat_request_json_with_cache_options, try_mask_chat_request_json_with_options,
        ChatPiiRedactionProviderScope, ChatPiiRedactionRuntimeConfig, DetectorProbe, MappingKey,
        MaskChatRequestOptions, RedactionKind, RedactionLimitError, RedactionMapping,
        RedactionScanLimits, RedactionSession, RedactionSessionConfig, RedactionSessionSlot,
        RedisRedactionMappingCache, SentinelMatcher, StreamingResponseRestorer,
    };
    use std::collections::BTreeMap;
    use std::time::Duration;

    use aether_runtime_state::{RedisClientConfig, RuntimeState};
    use aether_testkit::ManagedRedisServer;
    use serde_json::json;

    fn assert_debug_surface_hides_values(debug: &str, originals: &[&str], sentinels: &[String]) {
        for original in originals {
            assert!(
                !debug.contains(original),
                "debug leaked original {original}"
            );
        }
        for sentinel in sentinels {
            assert!(
                !debug.contains(sentinel),
                "debug leaked sentinel {sentinel}"
            );
        }
    }

    fn source_chunk_before_struct<'a>(source: &'a str, marker: &str) -> &'a str {
        let marker_start = source.find(marker).expect("struct marker should exist");
        let chunk_start = marker_start.saturating_sub(220);
        &source[chunk_start..marker_start]
    }

    fn assert_not_serde_derived(source: &str, marker: &str) {
        let chunk = source_chunk_before_struct(source, marker);
        assert!(
            !chunk.contains("Serialize"),
            "{marker} must not derive Serialize"
        );
        assert!(
            !chunk.contains("Deserialize"),
            "{marker} must not derive Deserialize"
        );
    }

    fn session_at(now_unix_secs: u64) -> RedactionSession {
        RedactionSession::new(RedactionSessionConfig::new(
            b"redaction-test-key".to_vec(),
            300,
            now_unix_secs,
        ))
    }

    fn assert_base32_sentinel(sentinel: &str) {
        let body = sentinel
            .strip_prefix("<AETHER:")
            .and_then(|rest| rest.strip_suffix('>'))
            .expect("sentinel should use Aether wrapper");
        let (_, payload) = body
            .split_once(':')
            .expect("sentinel should include type and payload");
        assert_eq!(payload.len(), 20, "HMAC96 base32 payload is 20 chars");
        assert!(payload
            .chars()
            .all(|ch| matches!(ch, 'A'..='Z' | '2'..='7')));
        assert_ne!(payload.len(), 24, "old hex HMAC96 payload must not be used");
    }

    fn session_with_response_mapping(original: &str, sentinel: &str) -> RedactionSession {
        let mut session = session_at(600);
        let key = MappingKey {
            kind: RedactionKind::Email,
            original: original.to_string(),
        };
        session
            .sentinel_index
            .insert(sentinel.to_string(), key.clone());
        session.mappings.insert(
            key,
            RedactionMapping {
                kind: RedactionKind::Email,
                original: original.to_string(),
                normalized_value: original.to_string(),
                sentinel: sentinel.to_string(),
                bucket: 2,
                created_at_unix_secs: 600,
                expires_at_unix_secs: 900,
            },
        );
        session
    }

    async fn start_managed_redis_or_skip() -> Option<ManagedRedisServer> {
        match ManagedRedisServer::start().await {
            Ok(server) => Some(server),
            Err(err) if err.to_string().contains("No such file or directory") => None,
            Err(err) => panic!("redis server should start: {err}"),
        }
    }

    async fn redis_cache_runtime_state(redis_url: &str, key_prefix: &str) -> RuntimeState {
        RuntimeState::redis(
            RedisClientConfig {
                url: redis_url.to_string(),
                key_prefix: Some(key_prefix.to_string()),
            },
            Some(1_000),
        )
        .await
        .expect("redis runtime state should build")
    }

    #[test]
    fn pii_redaction_session_detects_reuses_rolls_over_and_avoids_collisions() {
        let input = concat!(
            "email alice@example.com again alice@example.com ",
            "mobile 13800138000 landline 010-12345678 phone +14155552671 ",
            "id 11010519491231002X card 4111 1111 1111 1111 ",
            "ips 192.168.0.1 2001:db8::1 ",
            "keys sk-proj-AbCdEfGhIjKlMnOpQrStUvWxYz123456 sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz1234567890 ",
            "ghp_abcdefghijklmnopqrstuvwxyzABCDEFGHIJ ",
            "xo", "xb-123456789012-123456789012-AbCdEfGhIjKlMnOpQrStUvWx ",
            "AKIAIOSFODNN7EXAMPLE Bearer abcdefghijklmnopqrstuvwxyzABCDEF1234567890 ",
            "access_token=accessValueABCDEF1234567890abcdef secret_key=secretValueABCDEF1234567890abcdef ",
            "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJpc3MiOiJ0ZXN0IiwiZXhwIjoyMDAwMDAwMDAwfQ.signature123456789 ",
            "q8HjK2LmN9PqR4StU7VwX0YzA3BcD6Ef"
        );
        let originals = [
            "alice@example.com",
            "13800138000",
            "010-12345678",
            "+14155552671",
            "11010519491231002X",
            "4111 1111 1111 1111",
            "192.168.0.1",
            "2001:db8::1",
            "sk-proj-AbCdEfGhIjKlMnOpQrStUvWxYz123456",
            "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz1234567890",
            "ghp_abcdefghijklmnopqrstuvwxyzABCDEFGHIJ",
            concat!("xo", "xb-123456789012-123456789012-AbCdEfGhIjKlMnOpQrStUvWx"),
            "AKIAIOSFODNN7EXAMPLE",
            "Bearer abcdefghijklmnopqrstuvwxyzABCDEF1234567890",
            "access_token=accessValueABCDEF1234567890abcdef",
            "secret_key=secretValueABCDEF1234567890abcdef",
            "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJpc3MiOiJ0ZXN0IiwiZXhwIjoyMDAwMDAwMDAwfQ.signature123456789",
            "q8HjK2LmN9PqR4StU7VwX0YzA3BcD6Ef",
        ];

        let mut session = session_at(600);
        let redacted = session.redact_text(input);

        for original in originals {
            assert!(!redacted.text.contains(original), "leaked {original}");
        }
        assert!(redacted.text.contains("<AETHER:EMAIL:"));
        assert!(redacted.text.contains("<AETHER:CN_PHONE:"));
        assert!(redacted.text.contains("<AETHER:ACCESS_TOKEN:"));
        assert!(redacted.text.contains("<AETHER:SECRET_KEY:"));
        assert!(redacted
            .matches
            .iter()
            .all(|matched| matched.sentinel.starts_with("<AETHER:")
                && matched.sentinel.ends_with('>')));
        for matched in &redacted.matches {
            assert_base32_sentinel(&matched.sentinel);
        }
        assert!(redacted.matches.iter().any(|matched| {
            matched.kind == RedactionKind::ChinaMobile && matched.sentinel.contains(":CN_PHONE:")
        }));
        assert!(redacted.matches.iter().any(|matched| {
            matched.kind == RedactionKind::ChinaLandline && matched.sentinel.contains(":CN_PHONE:")
        }));
        assert!(redacted
            .matches
            .iter()
            .any(|matched| matched.kind == RedactionKind::AccessToken));
        assert!(redacted
            .matches
            .iter()
            .any(|matched| matched.kind == RedactionKind::SecretKey));
        assert_eq!(session.mapping_count(), originals.len());

        let first_email = session
            .sentinel_for_original("alice@example.com")
            .expect("email sentinel should exist")
            .to_string();
        assert_base32_sentinel(&first_email);
        assert_eq!(redacted.text.matches(&first_email).count(), 2);

        let debug = format!("{session:?}");
        assert!(debug.contains("mapping_count"));
        assert!(!debug.contains("alice@example.com"));
        assert!(!debug.contains(&first_email));
        assert!(!debug.contains("redaction-test-key"));

        let mut same_bucket = session_at(899);
        same_bucket.redact_text("alice@example.com");
        assert_eq!(
            same_bucket.sentinel_for_original("alice@example.com"),
            Some(first_email.as_str())
        );

        let mut next_bucket = session_at(900);
        next_bucket.redact_text("alice@example.com");
        assert_ne!(
            next_bucket.sentinel_for_original("alice@example.com"),
            Some(first_email.as_str())
        );

        let colliding_literal = first_email;
        let mut collision_session = session_at(600);
        collision_session.redact_text(&format!(
            "literal {colliding_literal} value alice@example.com"
        ));
        assert_ne!(
            collision_session.sentinel_for_original("alice@example.com"),
            Some(colliding_literal.as_str())
        );
    }

    #[test]
    fn pii_redaction_no_mapping_leak() {
        let originals = [
            "alice@example.com",
            "sk-proj-AbCdEfGhIjKlMnOpQrStUvWxYz123456",
            "access_token=accessValueABCDEF1234567890abcdef",
        ];
        let request = json!({
            "model": "gpt-5",
            "messages": [{
                "role": "user",
                "content": format!(
                    "Email {} key {} token {}",
                    originals[0], originals[1], originals[2]
                ),
            }]
        });
        let masked = mask_chat_request_json(
            &serde_json::to_vec(&request).expect("request should serialize"),
            test_config(),
        );
        assert!(masked.redacted);
        let sentinels = originals
            .iter()
            .map(|original| {
                masked
                    .session
                    .sentinel_for_original(original)
                    .expect("sentinel should exist")
                    .to_string()
            })
            .collect::<Vec<_>>();

        for debug in [
            format!("{:?}", masked.session),
            format!("{:?}", masked),
            format!(
                "{:?}",
                RedactionSessionConfig::new(originals[1].as_bytes().to_vec(), 300, 600)
            ),
        ] {
            assert_debug_surface_hides_values(&debug, &originals, &sentinels);
        }

        let mut session = session_at(600);
        let redacted = session.redact_text(&format!(
            "Email {} key {} token {}",
            originals[0], originals[1], originals[2]
        ));
        let redacted_debug = format!("{redacted:?}");
        assert_debug_surface_hides_values(&redacted_debug, &originals, &sentinels);
        for matched in &redacted.matches {
            let matched_debug = format!("{matched:?}");
            assert_debug_surface_hides_values(&matched_debug, &originals, &sentinels);
            assert!(matched_debug.contains("<AETHER:"));
            assert!(matched_debug.contains(":redacted>"));
        }
        for mapping in session.mappings() {
            let mapping_debug = format!("{mapping:?}");
            assert_debug_surface_hides_values(&mapping_debug, &originals, &sentinels);
            assert!(mapping_debug.contains("original_len"));
            assert!(mapping_debug.contains("normalized_value_len"));
            assert!(mapping_debug.contains(":redacted>"));
        }

        let privacy_source = include_str!("mod.rs");
        for marker in [
            "pub(crate) struct RedactionSession {",
            "pub(crate) struct RedactionSessionSlot {",
            "pub(crate) struct MaskedChatRequest {",
            "pub(crate) struct RedactionMatch {",
            "pub(crate) struct RedactionMapping {",
        ] {
            assert_not_serde_derived(privacy_source, marker);
        }
        let session_serialize_impl = format!("impl {} for {}", "Serialize", "RedactionSession");
        let mapping_serialize_impl = format!("impl {} for {}", "Serialize", "RedactionMapping");
        assert!(!privacy_source.contains(&session_serialize_impl));
        assert!(!privacy_source.contains(&mapping_serialize_impl));

        let report_context_source = include_str!("../ai_serving/planner/report_context.rs");
        for forbidden in [
            "RedactionSession",
            "RedactionMapping",
            "RedactionSessionSlot",
            "redaction_session",
            "redaction_mapping",
            "sentinel_index",
        ] {
            assert!(
                !report_context_source.contains(forbidden),
                "report context source must not expose {forbidden}"
            );
        }
        assert!(report_context_source.contains("original_request_body_json"));
    }

    #[test]
    fn pii_redaction_false_positives_are_not_detected() {
        let input = concat!(
            "Order AETHER-20240502-123456 date 2026-05-02 decimal 12345.67890 ",
            "short ids 123456 9876543210 invalid card 4111111111111112 ",
            "short sk word sk-test low entropy aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ",
            "name Alice Zhang address 北京市朝阳区建国路88号"
        );
        let mut session = session_at(600);
        let redacted = session.redact_text(input);

        assert_eq!(redacted.text, input);
        assert!(redacted.matches.is_empty());
        assert_eq!(session.mapping_count(), 0);
    }

    #[test]
    fn pii_redaction_request_masks_chat_message_text_and_tool_arguments() {
        let request = json!({
            "model": "gpt-5",
            "metadata": {
                "owner": "metadata@example.com"
            },
            "temperature": 0.2,
            "tools": [{
                "type": "function",
                "function": {
                    "name": "lookup_customer",
                    "description": "schema owner schema@example.com stays visible",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "email": {"type": "string", "description": "example schema@example.com"}
                        }
                    }
                }
            }],
            "messages": [
                {
                    "role": "system",
                    "content": "Contact alice@example.com with quote \"ok\", path C:\\tmp, and newline\nend."
                },
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Phone +14155552671 should be masked."},
                        {"type": "image_url", "text": "image text bob@example.com must stay."},
                        {"type": "input_text", "text": "input text carol@example.com must stay."},
                        {"type": "text", "text": 42}
                    ]
                },
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "type": "function",
                        "function": {
                            "name": "lookup_customer",
                            "arguments": "{\"phone\":\"13800138000\",\"secret\":\"secret_key=secretValueABCDEF1234567890abcdef\"}"
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_1",
                    "content": "Tool result access_token=accessValueABCDEF1234567890abcdef"
                }
            ]
        });
        let raw = serde_json::to_vec(&request).expect("request should serialize");

        let masked = mask_chat_request_json(&raw, test_config());

        assert!(masked.redacted);
        assert_eq!(masked.session.mapping_count(), 5);
        let masked_json: serde_json::Value =
            serde_json::from_slice(&masked.body).expect("masked request should stay valid JSON");

        assert_eq!(masked_json["model"], "gpt-5");
        assert_eq!(masked_json["metadata"]["owner"], "metadata@example.com");
        assert_eq!(masked_json["temperature"], 0.2);
        assert_eq!(
            masked_json["tools"][0]["function"]["description"],
            "schema owner schema@example.com stays visible"
        );
        assert_eq!(
            masked_json["tools"][0]["function"]["parameters"]["properties"]["email"]["description"],
            "example schema@example.com"
        );

        let system_content = masked_json["messages"][0]["content"]
            .as_str()
            .expect("system content should remain a string");
        assert!(!system_content.contains("alice@example.com"));
        assert!(system_content.contains("\"ok\""));
        assert!(system_content.contains("C:\\tmp"));
        assert!(system_content.contains("newline\nend"));
        assert!(system_content.contains(
            masked
                .session
                .sentinel_for_original("alice@example.com")
                .expect("email sentinel should exist")
        ));

        let text_part = masked_json["messages"][1]["content"][0]["text"]
            .as_str()
            .expect("text part should remain a string");
        assert!(!text_part.contains("+14155552671"));
        assert!(text_part.contains(
            masked
                .session
                .sentinel_for_original("+14155552671")
                .expect("phone sentinel should exist")
        ));
        assert_eq!(
            masked_json["messages"][1]["content"][1]["text"],
            "image text bob@example.com must stay."
        );
        assert_eq!(
            masked_json["messages"][1]["content"][2]["text"],
            "input text carol@example.com must stay."
        );

        let arguments = masked_json["messages"][2]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .expect("tool call arguments should remain a string");
        assert!(!arguments.contains("13800138000"));
        assert!(!arguments.contains("secretValueABCDEF1234567890abcdef"));
        assert!(arguments.contains(
            masked
                .session
                .sentinel_for_original("13800138000")
                .expect("mobile sentinel should exist")
        ));
        assert!(arguments.contains(
            masked
                .session
                .sentinel_for_original("secret_key=secretValueABCDEF1234567890abcdef")
                .expect("secret sentinel should exist")
        ));

        let tool_content = masked_json["messages"][3]["content"]
            .as_str()
            .expect("tool content should remain a string");
        assert!(!tool_content.contains("accessValueABCDEF1234567890abcdef"));
        assert!(tool_content.contains(
            masked
                .session
                .sentinel_for_original("access_token=accessValueABCDEF1234567890abcdef")
                .expect("access token sentinel should exist")
        ));
    }

    #[test]
    fn pii_redaction_request_avoids_cross_message_and_tool_argument_sentinel_collisions() {
        let mut probe = session_at(600);
        probe.redact_text("Contact alice@example.com");
        let colliding_literal = probe
            .sentinel_for_original("alice@example.com")
            .expect("probe sentinel should exist")
            .to_string();
        let request = json!({
            "model": "gpt-5",
            "messages": [
                {"role": "user", "content": format!("literal {colliding_literal} stays unknown")},
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "type": "function",
                        "function": {
                            "name": "lookup_contact",
                            "arguments": "{\"email\":\"alice@example.com\"}"
                        }
                    }]
                }
            ]
        });

        let masked = mask_chat_request_json(
            &serde_json::to_vec(&request).expect("request should serialize"),
            test_config(),
        );

        let current_sentinel = masked
            .session
            .sentinel_for_original("alice@example.com")
            .expect("current request sentinel should exist")
            .to_string();
        assert_ne!(current_sentinel, colliding_literal);
        let masked_json: serde_json::Value =
            serde_json::from_slice(&masked.body).expect("masked request should parse");
        assert_eq!(
            masked_json["messages"][0]["content"],
            format!("literal {colliding_literal} stays unknown")
        );
        let arguments = masked_json["messages"][1]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .expect("arguments should be text");
        assert!(!arguments.contains("alice@example.com"));
        assert!(arguments.contains(&current_sentinel));

        let mut headers =
            BTreeMap::from([("content-type".to_string(), "application/json".to_string())]);
        let restored = restore_sync_response_body(
            &mut headers,
            serde_json::to_vec(&json!({
                "message": format!("literal {colliding_literal} current {current_sentinel}")
            }))
            .expect("response should serialize")
            .as_slice(),
            &masked.session,
        )
        .expect("restore should succeed");
        let restored_json: serde_json::Value =
            serde_json::from_slice(&restored.body).expect("restored body should parse");
        assert_eq!(
            restored_json["message"],
            format!("literal {colliding_literal} current alice@example.com")
        );
    }

    #[test]
    fn pii_redaction_request_noops_for_invalid_absent_or_malformed_messages() {
        for raw in [
            b"{not json".to_vec(),
            br#"{"model":"gpt-5","metadata":{"owner":"alice@example.com"}}"#.to_vec(),
            br#"{"model":"gpt-5","messages":{"role":"user","content":"alice@example.com"}}"#
                .to_vec(),
        ] {
            let masked = mask_chat_request_json(&raw, test_config());

            assert!(!masked.redacted);
            assert_eq!(masked.body, raw);
            assert_eq!(masked.session.mapping_count(), 0);
        }
    }

    #[test]
    fn pii_redaction_request_noops_when_chat_messages_are_clean() {
        let raw = br#"{"model":"gpt-5","messages":[{"role":"user","content":"hello"}],"metadata":{"trace":"keep"}}"#;

        let masked = mask_chat_request_json(raw, test_config());

        assert!(!masked.redacted);
        assert_eq!(masked.body, raw);
        assert_eq!(masked.session.mapping_count(), 0);
    }

    #[test]
    fn pii_redaction_sync_restore_json_strings_reserialize_safely_and_updates_content_length() {
        let sentinel = "<AETHER:EMAIL:ABCDEFGHIJKLMNOPQRST>";
        let original = "alice@example.com said \"ok\" from C:\\tmp\\a\nnext";
        let session = session_with_response_mapping(original, sentinel);
        let body = serde_json::to_vec(&json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": format!("Restored: {sentinel}"),
                }
            }]
        }))
        .expect("response should serialize");
        let mut headers = BTreeMap::from([
            ("content-type".to_string(), "application/json".to_string()),
            ("content-length".to_string(), body.len().to_string()),
        ]);

        let restored = restore_sync_response_body(&mut headers, &body, &session)
            .expect("restore should succeed");

        assert!(restored.restored);
        let restored_len = restored.body.len().to_string();
        assert_eq!(
            headers.get("content-length").map(String::as_str),
            Some(restored_len.as_str())
        );
        let restored_json: serde_json::Value =
            serde_json::from_slice(&restored.body).expect("restored body should stay valid JSON");
        assert_eq!(
            restored_json["choices"][0]["message"]["content"],
            format!("Restored: {original}")
        );
    }

    #[test]
    fn pii_redaction_sync_restore_assistant_content_and_tool_call_arguments() {
        let request = json!({
            "model": "gpt-5",
            "messages": [
                {"role": "user", "content": "Email alice@example.com"},
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "type": "function",
                        "function": {
                            "name": "lookup_customer",
                            "arguments": "{\"phone\":\"13800138000\"}"
                        }
                    }]
                }
            ]
        });
        let masked = mask_chat_request_json(
            &serde_json::to_vec(&request).expect("request should serialize"),
            test_config(),
        );
        let email_sentinel = masked
            .session
            .sentinel_for_original("alice@example.com")
            .expect("email sentinel should exist");
        let phone_sentinel = masked
            .session
            .sentinel_for_original("13800138000")
            .expect("phone sentinel should exist");
        let body = serde_json::to_vec(&json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": format!("Customer {email_sentinel}"),
                    "tool_calls": [{
                        "type": "function",
                        "function": {
                            "name": "lookup_customer",
                            "arguments": format!("{{\"phone\":\"{phone_sentinel}\"}}")
                        }
                    }]
                }
            }]
        }))
        .expect("response should serialize");
        let mut headers = BTreeMap::from([(
            "content-type".to_string(),
            "application/json; charset=utf-8".to_string(),
        )]);

        let restored = restore_sync_response_body(&mut headers, &body, &masked.session)
            .expect("restore should succeed");

        let restored_json: serde_json::Value =
            serde_json::from_slice(&restored.body).expect("restored body should stay valid JSON");
        assert_eq!(
            restored_json["choices"][0]["message"]["content"],
            "Customer alice@example.com"
        );
        assert_eq!(
            restored_json["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"phone\":\"13800138000\"}"
        );
    }

    #[test]
    fn pii_redaction_sync_restore_unknown_sentinel_looking_strings_remain_unchanged() {
        let known_sentinel = "<AETHER:EMAIL:ABCDEFGHIJKLMNOPQRST>";
        let unknown_sentinel = "<AETHER:EMAIL:TSRQPONMLKJIHGFEDCBA>";
        let session = session_with_response_mapping("alice@example.com", known_sentinel);
        let body = serde_json::to_vec(&json!({
            "message": format!("known {known_sentinel} unknown {unknown_sentinel}"),
        }))
        .expect("response should serialize");
        let mut headers =
            BTreeMap::from([("content-type".to_string(), "application/json".to_string())]);

        let restored = restore_sync_response_body(&mut headers, &body, &session)
            .expect("restore should succeed");

        let restored_json: serde_json::Value =
            serde_json::from_slice(&restored.body).expect("restored body should stay valid JSON");
        assert_eq!(
            restored_json["message"],
            format!("known alice@example.com unknown {unknown_sentinel}")
        );
    }

    #[test]
    fn pii_redaction_sync_restore_text_body_replaces_current_request_sentinels() {
        let known_sentinel = "<AETHER:EMAIL:ABCDEFGHIJKLMNOPQRST>";
        let unknown_sentinel = "<AETHER:EMAIL:TSRQPONMLKJIHGFEDCBA>";
        let session = session_with_response_mapping("alice@example.com", known_sentinel);
        let body = format!("plain {known_sentinel} and {unknown_sentinel}").into_bytes();
        let mut headers = BTreeMap::from([
            ("content-type".to_string(), "text/plain".to_string()),
            ("Content-Length".to_string(), body.len().to_string()),
        ]);

        let restored = restore_sync_response_body(&mut headers, &body, &session)
            .expect("restore should succeed");

        assert_eq!(
            std::str::from_utf8(&restored.body).expect("restored text should be utf8"),
            format!("plain alice@example.com and {unknown_sentinel}")
        );
        let restored_len = restored.body.len().to_string();
        assert_eq!(headers.get("Content-Length"), None);
        assert_eq!(
            headers.get("content-length").map(String::as_str),
            Some(restored_len.as_str())
        );
    }

    #[test]
    fn pii_redaction_compressed_response_safe_error() {
        let sentinel = "<AETHER:EMAIL:ABCDEFGHIJKLMNOPQRST>";
        let session = session_with_response_mapping("alice@example.com", sentinel);
        let body = format!("compressed bytes with {sentinel}").into_bytes();
        let mut headers = BTreeMap::from([
            ("Content-Encoding".to_string(), "identity".to_string()),
            ("content-encoding".to_string(), "gzip".to_string()),
            ("content-length".to_string(), body.len().to_string()),
        ]);

        let err = restore_sync_response_body(&mut headers, &body, &session)
            .expect_err("compressed response should fail safely");

        let message = format!("{err:?}");
        assert!(message.contains("encoded response bodies"));
        assert!(!message.contains("alice@example.com"));
        assert!(!message.contains(sentinel));
        assert_eq!(
            headers.get("Content-Encoding").map(String::as_str),
            Some("identity")
        );
        assert_eq!(
            headers.get("content-encoding").map(String::as_str),
            Some("gzip")
        );
        let original_len = body.len().to_string();
        assert_eq!(
            headers.get("content-length").map(String::as_str),
            Some(original_len.as_str())
        );

        let stream_err = match StreamingResponseRestorer::new(&headers, &session) {
            Ok(_) => panic!("compressed stream response should fail safely"),
            Err(err) => err,
        };
        let stream_message = format!("{stream_err:?}");
        assert!(stream_message.contains("encoded response bodies"));
        assert!(!stream_message.contains("alice@example.com"));
        assert!(!stream_message.contains(sentinel));
    }

    #[test]
    fn pii_redaction_stream_restore_raw_text_split_sentinel_across_three_chunks() {
        let known_sentinel = "<AETHER:EMAIL:ABCDEFGHIJKLMNOPQRST>";
        let unknown_sentinel = "<AETHER:EMAIL:TSRQPONMLKJIHGFEDCBA>";
        let session = session_with_response_mapping("alice@example.com", known_sentinel);
        let mut restorer = StreamingResponseRestorer::for_text(&session);
        let chunks = [
            b"prefix <AET".as_slice(),
            b"HER:EMAIL:ABC".as_slice(),
            b"DEFGHIJKLMNOPQRST> middle ".as_slice(),
            unknown_sentinel.as_bytes(),
        ];

        let mut output = Vec::new();
        for chunk in chunks {
            output.extend(restorer.push_chunk(chunk).expect("restore should succeed"));
            assert!(restorer.pending_text_carry_len() <= restorer.max_text_carry_len());
        }
        output.extend(restorer.finish().expect("finish should succeed"));

        assert_eq!(
            String::from_utf8(output).expect("restored stream should be utf8"),
            format!("prefix alice@example.com middle {unknown_sentinel}")
        );
        assert_eq!(restorer.pending_text_carry_len(), 0);
    }

    #[test]
    fn pii_redaction_stream_restore_bounded_carry_and_final_flush() {
        let sentinel = "<AETHER:EMAIL:ABCDEFGHIJKLMNOPQRST>";
        let session = session_with_response_mapping("alice@example.com", sentinel);
        let mut restorer = StreamingResponseRestorer::for_text(&session);
        let chunk = vec![b'x'; sentinel.len() * 3];

        let first = restorer.push_chunk(&chunk).expect("restore should succeed");

        assert!(!first.is_empty());
        assert!(restorer.pending_text_carry_len() <= restorer.max_text_carry_len());
        let flushed = restorer.finish().expect("finish should succeed");
        assert_eq!(first.len() + flushed.len(), chunk.len());
        assert_eq!(restorer.pending_text_carry_len(), 0);
    }

    #[test]
    fn pii_redaction_stream_restore_sse_json_split_sentinel_across_three_chunks() {
        let sentinel = "<AETHER:EMAIL:ABCDEFGHIJKLMNOPQRST>";
        let session = session_with_response_mapping("alice@example.com", sentinel);
        let mut restorer = StreamingResponseRestorer::for_sse(&session);
        let chunks = [
            b"data: {\"choices\":[{\"delta\":{\"content\":\"hello <AET".as_slice(),
            b"HER:EMAIL:ABC".as_slice(),
            b"DEFGHIJKLMNOPQRST>\"}}]}\n\n".as_slice(),
        ];

        let mut output = Vec::new();
        for chunk in chunks {
            output.extend(restorer.push_chunk(chunk).expect("restore should succeed"));
        }
        output.extend(restorer.finish().expect("finish should succeed"));
        let output_text = String::from_utf8(output).expect("restored stream should be utf8");

        assert!(output_text.contains("hello alice@example.com"));
        assert!(!output_text.contains(sentinel));
        assert!(output_text.ends_with("\n\n"));
    }

    #[test]
    fn pii_redaction_stream_restore_sse_tool_call_argument_delta_json_strings() {
        let request = json!({
            "model": "gpt-5",
            "messages": [
                {"role": "user", "content": "Email alice@example.com"},
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "type": "function",
                        "function": {
                            "name": "lookup_customer",
                            "arguments": "{\"phone\":\"13800138000\"}"
                        }
                    }]
                }
            ]
        });
        let masked = mask_chat_request_json(
            &serde_json::to_vec(&request).expect("request should serialize"),
            test_config(),
        );
        let email_sentinel = masked
            .session
            .sentinel_for_original("alice@example.com")
            .expect("email sentinel should exist");
        let phone_sentinel = masked
            .session
            .sentinel_for_original("13800138000")
            .expect("phone sentinel should exist");
        let event = format!(
            "data: {}\n\n",
            serde_json::to_string(&json!({
                "choices": [{
                    "delta": {
                        "content": format!("Customer {email_sentinel}"),
                        "tool_calls": [{
                            "index": 0,
                            "function": {
                                "arguments": format!("{{\"phone\":\"{phone_sentinel}\"}}")
                            }
                        }]
                    }
                }]
            }))
            .expect("event should serialize")
        );
        let mut restorer = StreamingResponseRestorer::for_sse(&masked.session);

        let output = restorer
            .push_chunk(event.as_bytes())
            .expect("restore should succeed");

        let output_text = String::from_utf8(output).expect("restored stream should be utf8");
        assert!(output_text.contains("Customer alice@example.com"));
        assert!(output_text.contains("\\\"phone\\\":\\\"13800138000\\\""));
        assert!(!output_text.contains(email_sentinel));
        assert!(!output_text.contains(phone_sentinel));
        assert!(restorer.finish().expect("finish should succeed").is_empty());
    }

    #[test]
    fn pii_redaction_stream_restore_sse_preserves_comments_blank_crlf_multiple_data_and_done() {
        let sentinel = "<AETHER:EMAIL:ABCDEFGHIJKLMNOPQRST>";
        let session = session_with_response_mapping("alice@example.com", sentinel);
        let mut restorer = StreamingResponseRestorer::for_sse(&session);
        let input = format!(
            ": keep-alive\r\n\r\nevent: message\r\ndata: plain {sentinel}\r\ndata: second line\r\n\r\ndata: [DONE]\r\n\r\n"
        );

        let mut output = restorer
            .push_chunk(input.as_bytes())
            .expect("restore should succeed");
        output.extend(restorer.finish().expect("finish should succeed"));

        assert_eq!(
            String::from_utf8(output).expect("restored stream should be utf8"),
            concat!(
                ": keep-alive\r\n",
                "\r\n",
                "event: message\r\n",
                "data: plain alice@example.com\r\n",
                "data: second line\r\n",
                "\r\n",
                "data: [DONE]\r\n",
                "\r\n"
            )
        );
    }

    #[test]
    fn proxy_pii_redaction_provider_scope_gates_master_selected_and_all_provider_modes() {
        let provider_config = json!({"chat_pii_redaction": {"enabled": true}});
        let disabled_provider_config = json!({"chat_pii_redaction": {"enabled": false}});
        let mut config = ChatPiiRedactionRuntimeConfig {
            enabled: false,
            ..ChatPiiRedactionRuntimeConfig::default()
        };

        assert!(!provider_chat_pii_redaction_enabled(
            Some(&provider_config),
            &config
        ));

        config.enabled = true;
        config.provider_scope = ChatPiiRedactionProviderScope::SelectedProviders;
        assert!(provider_chat_pii_redaction_enabled(
            Some(&provider_config),
            &config
        ));
        assert!(!provider_chat_pii_redaction_enabled(None, &config));
        assert!(!provider_chat_pii_redaction_enabled(
            Some(&disabled_provider_config),
            &config
        ));

        config.provider_scope = ChatPiiRedactionProviderScope::AllProviders;
        assert!(provider_chat_pii_redaction_enabled(None, &config));
        assert!(provider_chat_pii_redaction_enabled(
            Some(&disabled_provider_config),
            &config
        ));
    }

    #[test]
    fn proxy_pii_redaction_entity_selection_controls_email_and_phone_detectors() {
        let config = ChatPiiRedactionRuntimeConfig {
            enabled_kinds: [RedactionKind::Email].into_iter().collect(),
            ..ChatPiiRedactionRuntimeConfig::default()
        };
        let request = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "Email alice@example.com phone +14155552671"}]
        });

        let masked = mask_chat_request_json_with_options(
            &serde_json::to_vec(&request).expect("request should serialize"),
            build_redaction_session_config(b"redaction-test-key".to_vec(), &config, 600),
            MaskChatRequestOptions::runtime(false),
        );

        let masked_json: serde_json::Value =
            serde_json::from_slice(&masked.body).expect("masked request should parse");
        let content = masked_json["messages"][0]["content"]
            .as_str()
            .expect("content should stay string");
        assert!(!content.contains("alice@example.com"));
        assert!(content.contains("+14155552671"));
        assert!(content.contains("<AETHER:EMAIL:"));
        assert!(!content.contains("<AETHER:PHONE:"));
    }

    #[test]
    fn proxy_pii_redaction_empty_entity_selection_disables_all_detectors() {
        let config = ChatPiiRedactionRuntimeConfig {
            enabled_kinds: parse_enabled_redaction_kinds(Some(&json!([]))),
            ..ChatPiiRedactionRuntimeConfig::default()
        };
        let request = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "Email alice@example.com phone +14155552671"}]
        });

        let masked = mask_chat_request_json_with_options(
            &serde_json::to_vec(&request).expect("request should serialize"),
            build_redaction_session_config(b"redaction-test-key".to_vec(), &config, 600),
            MaskChatRequestOptions::runtime(true),
        );

        assert!(!masked.redacted);
        let masked_json: serde_json::Value =
            serde_json::from_slice(&masked.body).expect("masked request should parse");
        assert_eq!(masked_json, request);
        assert!(masked_json.to_string().contains("alice@example.com"));
        assert!(!masked_json.to_string().contains("<AETHER:"));
        assert!(!masked_json
            .to_string()
            .contains("Aether privacy redaction notice"));
    }

    #[test]
    fn redaction_session_slot_uses_single_candidate_when_response_header_missing() {
        let slot = RedactionSessionSlot::default();
        let sentinel = "<AETHER:EMAIL:SINGLECANDIDATE001>";
        slot.put_for_candidate(
            "candidate-1",
            session_with_response_mapping("alice@example.com", sentinel),
        );

        let session = slot
            .take_for_candidate(None)
            .expect("single candidate session should be used without header");

        assert_eq!(
            session.sentinel_for_original("alice@example.com"),
            Some(sentinel)
        );
        assert!(slot.take_for_candidate(Some("candidate-1")).is_none());
    }

    #[test]
    fn redaction_session_slot_does_not_guess_when_response_header_missing_for_multiple_candidates()
    {
        let slot = RedactionSessionSlot::default();
        let alice_sentinel = "<AETHER:EMAIL:MULTICANDIDATE001>";
        let bob_sentinel = "<AETHER:EMAIL:MULTICANDIDATE002>";
        slot.put_for_candidate(
            "candidate-1",
            session_with_response_mapping("alice@example.com", alice_sentinel),
        );
        slot.put_for_candidate(
            "candidate-2",
            session_with_response_mapping("bob@example.com", bob_sentinel),
        );

        assert!(slot.take_for_candidate(None).is_none());

        let session = slot
            .take_for_candidate(Some("candidate-2"))
            .expect("specific candidate session should remain available");
        assert_eq!(
            session.sentinel_for_original("bob@example.com"),
            Some(bob_sentinel)
        );
    }

    #[test]
    fn redaction_session_slot_does_not_fallback_to_other_candidate_on_header_miss() {
        let slot = RedactionSessionSlot::default();
        slot.put_for_candidate(
            "candidate-1",
            session_with_response_mapping("alice@example.com", "<AETHER:EMAIL:HEADERMISS001>"),
        );

        assert!(slot.take_for_candidate(Some("candidate-2")).is_none());
        assert!(slot.take_for_candidate(Some("candidate-1")).is_some());
    }

    #[test]
    fn proxy_pii_redaction_provider_bound_request_uses_sentinels_and_inserts_safe_notice() {
        let config = ChatPiiRedactionRuntimeConfig::default();
        let request = json!({
            "model": "gpt-5",
            "messages": [
                {"role": "system", "content": "be brief"},
                {"role": "user", "content": "Contact alice@example.com"},
                {"role": "assistant", "content": "ok"}
            ]
        });

        let masked = mask_chat_request_json_with_options(
            &serde_json::to_vec(&request).expect("request should serialize"),
            build_redaction_session_config(b"redaction-test-key".to_vec(), &config, 600),
            MaskChatRequestOptions::runtime(true),
        );

        assert!(masked.redacted);
        let masked_json: serde_json::Value =
            serde_json::from_slice(&masked.body).expect("masked request should parse");
        let messages = masked_json["messages"]
            .as_array()
            .expect("messages should be an array");
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[1]["role"], "assistant");
        assert!(messages[1..]
            .iter()
            .all(|message| message["role"].as_str() != Some("system")));
        let notice = messages[1]["content"]
            .as_str()
            .expect("notice should be text");
        assert!(notice.contains("not a user request"));
        assert!(notice.contains("do not answer"));
        assert!(notice.contains("do not answer it, mention it"));
        assert!(!notice.contains("alice@example.com"));
        assert_eq!(messages[2]["role"], "user");
        let content = messages[2]["content"]
            .as_str()
            .expect("user content should be text");
        assert!(!content.contains("alice@example.com"));
        assert!(content.contains("<AETHER:EMAIL:"));
        assert_eq!(messages[3]["role"], "assistant");
    }

    #[test]
    fn proxy_pii_redaction_sync_and_stream_client_responses_restore_current_sentinels() {
        let request = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "Email alice@example.com"}]
        });
        let masked = mask_chat_request_json(
            &serde_json::to_vec(&request).expect("request should serialize"),
            test_config(),
        );
        let sentinel = masked
            .session
            .sentinel_for_original("alice@example.com")
            .expect("email sentinel should exist");
        let sync_body = serde_json::to_vec(&json!({
            "choices": [{"message": {"role": "assistant", "content": format!("hello {sentinel}")}}]
        }))
        .expect("response should serialize");
        let mut headers =
            BTreeMap::from([("content-type".to_string(), "application/json".to_string())]);

        let restored = restore_sync_response_body(&mut headers, &sync_body, &masked.session)
            .expect("sync restore should succeed");
        let restored_json: serde_json::Value =
            serde_json::from_slice(&restored.body).expect("restored response should parse");
        assert_eq!(
            restored_json["choices"][0]["message"]["content"],
            "hello alice@example.com"
        );

        let sse =
            format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{sentinel}\"}}}}]}}\n\n");
        let mut restorer = StreamingResponseRestorer::for_sse(&masked.session);
        let mut output = restorer
            .push_chunk(sse.as_bytes())
            .expect("stream restore should succeed");
        output.extend(restorer.finish().expect("stream finish should succeed"));
        let output = String::from_utf8(output).expect("stream should be utf8");
        assert!(output.contains("alice@example.com"));
        assert!(!output.contains(sentinel));
    }

    #[test]
    fn pii_redaction_performance_prefilters_skip_impossible_expensive_validators() {
        let mut clean_probe = DetectorProbe::default();
        let clean_candidates = detect_candidates_with_probe(
            "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz",
            Some(&mut clean_probe),
        );

        assert!(clean_candidates.is_empty());
        for kind in [
            RedactionKind::ChinaResidentId,
            RedactionKind::PaymentCard,
            RedactionKind::Jwt,
            RedactionKind::ApiKey,
        ] {
            assert_eq!(clean_probe.validator_calls(kind), 0);
        }

        let mut token_probe = DetectorProbe::default();
        let token_candidates = detect_candidates_with_probe(
            "token q8HjK2LmN9PqR4StU7VwX0YzA3BcD6Ef",
            Some(&mut token_probe),
        );

        assert_eq!(token_probe.validator_calls(RedactionKind::ApiKey), 1);
        assert!(token_candidates
            .iter()
            .any(|candidate| candidate.kind == RedactionKind::ApiKey));
    }

    #[test]
    fn pii_redaction_performance_limits_reject_scanned_text_and_detection_overflow() {
        assert_eq!(
            RedactionScanLimits::default(),
            RedactionScanLimits {
                max_scanned_text_bytes: 2 * 1024 * 1024,
                max_detections: 1024,
            }
        );

        let large_request = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "x".repeat(2 * 1024 * 1024 + 1)}]
        });
        let large_err = try_mask_chat_request_json_with_options(
            &serde_json::to_vec(&large_request).expect("request should serialize"),
            test_config(),
            MaskChatRequestOptions::runtime(false),
        )
        .expect_err("oversized scan should fail closed");
        assert_eq!(
            large_err,
            RedactionLimitError::ScannedTextTooLarge {
                limit: 2 * 1024 * 1024,
            }
        );
        assert_eq!(
            large_err.client_status(),
            http::StatusCode::PAYLOAD_TOO_LARGE
        );
        assert!(!large_err.safe_message().contains("alice@example.com"));

        let dense_request = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "alice@example.com bob@example.net"}]
        });
        let dense_err = try_mask_chat_request_json_with_options(
            &serde_json::to_vec(&dense_request).expect("request should serialize"),
            test_config(),
            MaskChatRequestOptions::runtime(false).with_scan_limits(RedactionScanLimits {
                max_scanned_text_bytes: 1024,
                max_detections: 1,
            }),
        )
        .expect_err("too many detections should fail closed");
        assert_eq!(
            dense_err,
            RedactionLimitError::TooManyDetections { limit: 1 }
        );
        assert_eq!(
            dense_err.client_status(),
            http::StatusCode::UNPROCESSABLE_ENTITY
        );
        assert!(!dense_err.safe_message().contains("alice@example.com"));
    }

    #[tokio::test]
    async fn pii_redaction_performance_multiturn_cache_reuses_and_rolls_over_sentinels() {
        let Some(redis) = start_managed_redis_or_skip().await else {
            return;
        };
        let runtime_state =
            redis_cache_runtime_state(redis.redis_url(), "pii_redaction_performance_cache").await;
        let cache = RedisRedactionMappingCache::new(&runtime_state);
        let config = ChatPiiRedactionRuntimeConfig {
            enabled_kinds: [RedactionKind::Email].into_iter().collect(),
            ..ChatPiiRedactionRuntimeConfig::default()
        };

        let first_request = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "Contact Alice@Example.com"}],
            "provider_context": "provider-a"
        });
        let first_masked = try_mask_chat_request_json_with_cache_options(
            &serde_json::to_vec(&first_request).expect("request should serialize"),
            build_redaction_session_config(b"redaction-test-key".to_vec(), &config, 600),
            MaskChatRequestOptions::runtime(false),
            Some(&cache),
        )
        .await
        .expect("first masking should succeed");
        let first_sentinel = first_masked
            .session
            .sentinel_for_original("Alice@Example.com")
            .expect("first sentinel should exist")
            .to_string();

        let second_request = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "Contact alice@example.com and bob@example.net"}],
            "provider_context": "provider-b"
        });
        let second_masked = try_mask_chat_request_json_with_cache_options(
            &serde_json::to_vec(&second_request).expect("request should serialize"),
            build_redaction_session_config(b"redaction-test-key".to_vec(), &config, 899),
            MaskChatRequestOptions::runtime(false),
            Some(&cache),
        )
        .await
        .expect("second masking should succeed");
        assert_eq!(
            second_masked
                .session
                .sentinel_for_original("alice@example.com"),
            Some(first_sentinel.as_str())
        );
        assert!(second_masked
            .session
            .sentinel_for_original("bob@example.net")
            .is_some());
        let second_masked_json: serde_json::Value = serde_json::from_slice(&second_masked.body)
            .expect("second masked request should parse");
        assert_eq!(second_masked_json["provider_context"], "provider-b");
        let second_content = second_masked_json["messages"][0]["content"]
            .as_str()
            .expect("second content should be text");
        assert!(second_content.contains(&first_sentinel));
        assert!(!second_content.contains("alice@example.com"));
        assert!(!second_content.contains("bob@example.net"));
        let mut headers =
            BTreeMap::from([("content-type".to_string(), "application/json".to_string())]);
        let restored = restore_sync_response_body(
            &mut headers,
            serde_json::to_vec(&json!({"message": format!("cached {first_sentinel}")}))
                .expect("response should serialize")
                .as_slice(),
            &second_masked.session,
        )
        .expect("restore should succeed");
        let restored_json: serde_json::Value =
            serde_json::from_slice(&restored.body).expect("restored body should parse");
        assert_eq!(restored_json["message"], "cached alice@example.com");

        let rolled_masked = try_mask_chat_request_json_with_cache_options(
            &serde_json::to_vec(&second_request).expect("request should serialize"),
            build_redaction_session_config(b"redaction-test-key".to_vec(), &config, 900),
            MaskChatRequestOptions::runtime(false),
            Some(&cache),
        )
        .await
        .expect("rolled masking should succeed");
        let rolled_sentinel = rolled_masked
            .session
            .sentinel_for_original("alice@example.com")
            .expect("rolled sentinel should exist")
            .to_string();
        assert_ne!(rolled_sentinel, first_sentinel);
        let mut rolled_headers =
            BTreeMap::from([("content-type".to_string(), "application/json".to_string())]);
        let rolled_restored = restore_sync_response_body(
            &mut rolled_headers,
            serde_json::to_vec(&json!({"message": format!("rolled {rolled_sentinel}")}))
                .expect("response should serialize")
                .as_slice(),
            &rolled_masked.session,
        )
        .expect("rolled restore should succeed");
        let rolled_restored_json: serde_json::Value = serde_json::from_slice(&rolled_restored.body)
            .expect("rolled restored body should parse");
        assert_eq!(rolled_restored_json["message"], "rolled alice@example.com");

        let keys = runtime_state
            .scan_keys("privacy:redaction:mapping:*", 100)
            .await
            .expect("keys should read");
        assert!(!keys.is_empty());
        for key in keys {
            assert!(!key.contains("alice@example.com"));
            let raw_key = runtime_state.strip_namespace(&key).to_string();
            let ttl = runtime_state
                .kv_ttl_seconds(&raw_key)
                .await
                .expect("ttl should read");
            assert!(
                ttl.unwrap_or_default() > 0,
                "redaction cache key {key} should have ttl"
            );
        }
    }

    #[tokio::test]
    async fn pii_redaction_performance_cache_hit_avoids_cached_sentinel_literal_collision() {
        let Some(redis) = start_managed_redis_or_skip().await else {
            return;
        };
        let runtime_state = redis_cache_runtime_state(
            redis.redis_url(),
            "pii_redaction_performance_cache_collision",
        )
        .await;
        let cache = RedisRedactionMappingCache::new(&runtime_state);
        let config = ChatPiiRedactionRuntimeConfig {
            enabled_kinds: [RedactionKind::Email].into_iter().collect(),
            ..ChatPiiRedactionRuntimeConfig::default()
        };

        let first_request = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "Contact alice@example.com"}]
        });
        let first_masked = try_mask_chat_request_json_with_cache_options(
            &serde_json::to_vec(&first_request).expect("request should serialize"),
            build_redaction_session_config(b"redaction-test-key".to_vec(), &config, 600),
            MaskChatRequestOptions::runtime(false),
            Some(&cache),
        )
        .await
        .expect("first masking should succeed");
        let cached_sentinel = first_masked
            .session
            .sentinel_for_original("alice@example.com")
            .expect("cached sentinel should exist")
            .to_string();

        let colliding_request = json!({
            "model": "gpt-5",
            "messages": [
                {
                    "role": "user",
                    "content": format!("literal {cached_sentinel} stays unknown")
                },
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "type": "function",
                        "function": {
                            "name": "lookup_contact",
                            "arguments": "{\"email\":\"alice@example.com\"}"
                        }
                    }]
                }
            ]
        });
        let second_masked = try_mask_chat_request_json_with_cache_options(
            &serde_json::to_vec(&colliding_request).expect("request should serialize"),
            build_redaction_session_config(b"redaction-test-key".to_vec(), &config, 899),
            MaskChatRequestOptions::runtime(false),
            Some(&cache),
        )
        .await
        .expect("collision masking should succeed");
        let current_sentinel = second_masked
            .session
            .sentinel_for_original("alice@example.com")
            .expect("current request sentinel should exist")
            .to_string();
        assert_ne!(current_sentinel, cached_sentinel);

        let masked_json: serde_json::Value =
            serde_json::from_slice(&second_masked.body).expect("masked body should parse");
        let content = masked_json["messages"][0]["content"]
            .as_str()
            .expect("content should be text");
        assert_eq!(content.matches(&cached_sentinel).count(), 1);
        assert!(!content.contains("alice@example.com"));
        let arguments = masked_json["messages"][1]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .expect("arguments should be text");
        assert_eq!(arguments.matches(&current_sentinel).count(), 1);
        assert!(!arguments.contains("alice@example.com"));

        let mut headers =
            BTreeMap::from([("content-type".to_string(), "application/json".to_string())]);
        let restored = restore_sync_response_body(
            &mut headers,
            serde_json::to_vec(&json!({
                "message": format!("literal {cached_sentinel} and {current_sentinel}")
            }))
            .expect("response should serialize")
            .as_slice(),
            &second_masked.session,
        )
        .expect("restore should succeed");
        let restored_json: serde_json::Value =
            serde_json::from_slice(&restored.body).expect("restored body should parse");
        assert_eq!(
            restored_json["message"],
            format!("literal {cached_sentinel} and alice@example.com")
        );
    }

    #[tokio::test]
    async fn pii_redaction_performance_ignores_malformed_cached_sentinel() {
        let Some(redis) = start_managed_redis_or_skip().await else {
            return;
        };
        let runtime_state =
            redis_cache_runtime_state(redis.redis_url(), "pii_redaction_performance_bad_cache")
                .await;
        let cache = RedisRedactionMappingCache::new(&runtime_state);
        let config = ChatPiiRedactionRuntimeConfig {
            enabled_kinds: [RedactionKind::Email].into_iter().collect(),
            ..ChatPiiRedactionRuntimeConfig::default()
        };
        runtime_state
            .kv_set(
                &cache.forward_cache_key(RedactionKind::Email, "alice@example.com", 2),
                "<AETHER:PHONE:ABCDEFGHIJKLMNOPQRST>",
                Some(Duration::from_secs(300)),
            )
            .await
            .expect("malformed cache value should write");

        let request = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "Contact alice@example.com"}]
        });
        let masked = try_mask_chat_request_json_with_cache_options(
            &serde_json::to_vec(&request).expect("request should serialize"),
            build_redaction_session_config(b"redaction-test-key".to_vec(), &config, 600),
            MaskChatRequestOptions::runtime(false),
            Some(&cache),
        )
        .await
        .expect("masking should ignore malformed cache value");

        let sentinel = masked
            .session
            .sentinel_for_original("alice@example.com")
            .expect("email sentinel should exist");
        assert!(sentinel.starts_with("<AETHER:EMAIL:"));
        assert_ne!(sentinel, "<AETHER:PHONE:ABCDEFGHIJKLMNOPQRST>");
    }

    #[test]
    fn pii_redaction_performance_restore_uses_large_mapping_matcher() {
        let mut session = session_at(600);
        for index in 0..32 {
            let original = format!("user{index}@example.com");
            let sentinel = format!("<AETHER:EMAIL:{index:0>20}>");
            let key = MappingKey {
                kind: RedactionKind::Email,
                original: original.clone(),
            };
            session.sentinel_index.insert(sentinel.clone(), key.clone());
            session.mappings.insert(
                key,
                RedactionMapping {
                    kind: RedactionKind::Email,
                    original,
                    normalized_value: format!("user{index}@example.com"),
                    sentinel,
                    bucket: 2,
                    created_at_unix_secs: 600,
                    expires_at_unix_secs: 900,
                },
            );
        }
        assert!(matches!(
            SentinelMatcher::new(&session),
            SentinelMatcher::Direct(_)
        ));

        for index in 32..40 {
            let original = format!("user{index}@example.com");
            let sentinel = format!("<AETHER:EMAIL:{index:0>20}>");
            let key = MappingKey {
                kind: RedactionKind::Email,
                original: original.clone(),
            };
            session.sentinel_index.insert(sentinel.clone(), key.clone());
            session.mappings.insert(
                key,
                RedactionMapping {
                    kind: RedactionKind::Email,
                    original,
                    normalized_value: format!("user{index}@example.com"),
                    sentinel,
                    bucket: 2,
                    created_at_unix_secs: 600,
                    expires_at_unix_secs: 900,
                },
            );
        }
        assert!(matches!(
            SentinelMatcher::new(&session),
            SentinelMatcher::Trie(_)
        ));

        let text = "before <AETHER:EMAIL:00000000000000000039> middle <AETHER:EMAIL:00000000000000000001> after";
        let restored = session.restore_text(text);
        assert!(restored.restored);
        assert_eq!(
            restored.text,
            "before user39@example.com middle user1@example.com after"
        );
    }

    #[test]
    fn pii_redaction_performance_documents_segment_scan_bypass_deferral() {
        let source = include_str!("mod.rs");
        assert!(source.contains("Segment-level scan-bypass cache is intentionally deferred"));
    }

    fn test_config() -> RedactionSessionConfig {
        RedactionSessionConfig::new(b"redaction-test-key".to_vec(), 300, 600)
    }
}
