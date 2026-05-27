use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{AppState, GatewayError};

pub(crate) const RISK_CONTROL_ENABLED_CONFIG_KEY: &str = "module.risk_control.enabled";
pub(crate) const RISK_CONTROL_CONFIG_KEY: &str = "module.risk_control.config";

const MAX_KEYWORD_ITEMS: usize = 1000;
const MAX_KEYWORD_CHARS: usize = 512;
const MAX_THRESHOLD_ITEMS: usize = 100;
const MAX_THRESHOLD_NAME_CHARS: usize = 100;
const MAX_BLOCK_MESSAGE_CHARS: usize = 500;
const MAX_AUTO_ACTION_VIOLATION_THRESHOLD: u64 = 1000;
const MAX_AUTO_ACTION_WINDOW_SECONDS: u64 = 31_536_000;
const MAX_RETENTION_DAYS: u64 = 3650;
const MAX_RETENTION_AUTO_RUN_INTERVAL_MINUTES: u64 = 60 * 24 * 7;
const MIN_OBSERVE_QUEUE_CAPACITY: usize = 16;
const MAX_OBSERVE_QUEUE_CAPACITY: usize = 65_536;
const MAX_SCOPE_ITEMS: usize = 1000;
const MAX_SCOPE_VALUE_CHARS: usize = 200;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RiskControlMode {
    Off,
    #[default]
    Observe,
    PreBlock,
}

impl RiskControlMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Observe => "observe",
            Self::PreBlock => "pre_block",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RiskControlKeywordMode {
    KeywordOnly,
    #[default]
    KeywordAndApi,
    ApiOnly,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RiskControlKeywordMatchMode {
    #[default]
    Contains,
    Exact,
    Regex,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RiskControlModelFilterMode {
    #[default]
    All,
    Include,
    Exclude,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct RiskControlModelFilterConfig {
    #[serde(default)]
    pub(crate) mode: RiskControlModelFilterMode,
    #[serde(default)]
    pub(crate) models: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RiskControlScopeMode {
    #[default]
    All,
    Include,
    Exclude,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct RiskControlScopeListConfig {
    #[serde(default)]
    pub(crate) mode: RiskControlScopeMode,
    #[serde(default)]
    pub(crate) values: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct RiskControlScopeConfig {
    #[serde(default)]
    pub(crate) users: RiskControlScopeListConfig,
    #[serde(default)]
    pub(crate) user_groups: RiskControlScopeListConfig,
    #[serde(default)]
    pub(crate) api_keys: RiskControlScopeListConfig,
    #[serde(default)]
    pub(crate) route_families: RiskControlScopeListConfig,
    #[serde(default)]
    pub(crate) route_kinds: RiskControlScopeListConfig,
    #[serde(default)]
    pub(crate) endpoints: RiskControlScopeListConfig,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RiskControlScopeMatchContext<'a> {
    pub(crate) user_id: Option<&'a str>,
    pub(crate) user_group_ids: &'a [String],
    pub(crate) api_key_id: Option<&'a str>,
    pub(crate) route_family: Option<&'a str>,
    pub(crate) route_kind: Option<&'a str>,
    pub(crate) endpoint: Option<&'a str>,
    pub(crate) model: Option<&'a str>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RiskControlProviderConfig {
    #[serde(default = "default_provider_base_url")]
    pub(crate) base_url: String,
    #[serde(default = "default_provider_model")]
    pub(crate) model: String,
    #[serde(default)]
    pub(crate) api_keys: Vec<String>,
    #[serde(default = "default_provider_timeout_ms")]
    pub(crate) timeout_ms: u64,
    #[serde(default = "default_provider_max_retries")]
    pub(crate) max_retries: usize,
    #[serde(default = "default_key_freeze_seconds")]
    pub(crate) key_freeze_seconds: u64,
    #[serde(default)]
    pub(crate) fail_closed: bool,
}

impl Default for RiskControlProviderConfig {
    fn default() -> Self {
        Self {
            base_url: default_provider_base_url(),
            model: default_provider_model(),
            api_keys: Vec::new(),
            timeout_ms: default_provider_timeout_ms(),
            max_retries: default_provider_max_retries(),
            key_freeze_seconds: default_key_freeze_seconds(),
            fail_closed: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RiskControlHashBlockConfig {
    #[serde(default = "default_true")]
    pub(crate) enabled: bool,
    #[serde(default = "default_true")]
    pub(crate) learn_from_flagged: bool,
}

impl Default for RiskControlHashBlockConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            learn_from_flagged: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RiskControlAutoActionConfig {
    #[serde(default)]
    pub(crate) enabled: bool,
    #[serde(default = "default_violation_threshold")]
    pub(crate) violation_threshold: u64,
    #[serde(default = "default_violation_window_seconds")]
    pub(crate) window_seconds: u64,
    #[serde(default = "default_true")]
    pub(crate) disable_user: bool,
    #[serde(default)]
    pub(crate) lock_api_key: bool,
}

impl Default for RiskControlAutoActionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            violation_threshold: default_violation_threshold(),
            window_seconds: default_violation_window_seconds(),
            disable_user: true,
            lock_api_key: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RiskControlRetentionConfig {
    #[serde(default = "default_hit_retention_days")]
    pub(crate) hit_days: u64,
    #[serde(default = "default_non_hit_retention_days")]
    pub(crate) non_hit_days: u64,
    #[serde(default = "default_retention_auto_run_interval_minutes")]
    pub(crate) auto_run_interval_minutes: u64,
}

impl Default for RiskControlRetentionConfig {
    fn default() -> Self {
        Self {
            hit_days: default_hit_retention_days(),
            non_hit_days: default_non_hit_retention_days(),
            auto_run_interval_minutes: default_retention_auto_run_interval_minutes(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RiskControlNotificationConfig {
    #[serde(default)]
    pub(crate) enabled: bool,
    #[serde(default = "default_true")]
    pub(crate) notify_on_flagged: bool,
    #[serde(default = "default_true")]
    pub(crate) notify_on_auto_action: bool,
    #[serde(default)]
    pub(crate) notify_on_user_action_notice: bool,
    #[serde(default)]
    pub(crate) include_excerpt: bool,
}

impl Default for RiskControlNotificationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            notify_on_flagged: true,
            notify_on_auto_action: true,
            notify_on_user_action_notice: false,
            include_excerpt: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RiskControlObserveConfig {
    #[serde(default = "default_observe_queue_capacity")]
    pub(crate) queue_capacity: usize,
}

impl Default for RiskControlObserveConfig {
    fn default() -> Self {
        Self {
            queue_capacity: default_observe_queue_capacity(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct RiskControlRuntimeConfig {
    #[serde(default)]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) mode: RiskControlMode,
    #[serde(default)]
    pub(crate) keyword_mode: RiskControlKeywordMode,
    #[serde(default)]
    pub(crate) keyword_match_mode: RiskControlKeywordMatchMode,
    #[serde(default)]
    pub(crate) keywords: Vec<String>,
    #[serde(default)]
    pub(crate) keyword_exemptions: Vec<String>,
    #[serde(default)]
    pub(crate) thresholds: BTreeMap<String, f64>,
    #[serde(default)]
    pub(crate) model_filter: RiskControlModelFilterConfig,
    #[serde(default)]
    pub(crate) scope: RiskControlScopeConfig,
    #[serde(default)]
    pub(crate) provider: RiskControlProviderConfig,
    #[serde(default)]
    pub(crate) hash_block: RiskControlHashBlockConfig,
    #[serde(default)]
    pub(crate) auto_action: RiskControlAutoActionConfig,
    #[serde(default)]
    pub(crate) retention: RiskControlRetentionConfig,
    #[serde(default)]
    pub(crate) notification: RiskControlNotificationConfig,
    #[serde(default)]
    pub(crate) observe: RiskControlObserveConfig,
    #[serde(default = "default_sample_rate")]
    pub(crate) sample_rate: f64,
    #[serde(default = "default_max_text_chars")]
    pub(crate) max_text_chars: usize,
    #[serde(default = "default_excerpt_chars")]
    pub(crate) excerpt_chars: usize,
    #[serde(default)]
    pub(crate) log_all: bool,
    #[serde(default = "default_block_status")]
    pub(crate) block_status: u16,
    #[serde(default = "default_block_message")]
    pub(crate) block_message: String,
}

impl Default for RiskControlRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: RiskControlMode::default(),
            keyword_mode: RiskControlKeywordMode::default(),
            keyword_match_mode: RiskControlKeywordMatchMode::default(),
            keywords: Vec::new(),
            keyword_exemptions: Vec::new(),
            thresholds: BTreeMap::new(),
            model_filter: RiskControlModelFilterConfig::default(),
            scope: RiskControlScopeConfig::default(),
            provider: RiskControlProviderConfig::default(),
            hash_block: RiskControlHashBlockConfig::default(),
            auto_action: RiskControlAutoActionConfig::default(),
            retention: RiskControlRetentionConfig::default(),
            notification: RiskControlNotificationConfig::default(),
            observe: RiskControlObserveConfig::default(),
            sample_rate: default_sample_rate(),
            max_text_chars: default_max_text_chars(),
            excerpt_chars: default_excerpt_chars(),
            log_all: false,
            block_status: default_block_status(),
            block_message: default_block_message(),
        }
    }
}

impl RiskControlRuntimeConfig {
    pub(crate) fn sanitized(mut self) -> Self {
        self.provider.api_keys = self
            .provider
            .api_keys
            .into_iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect();
        self.keywords = normalize_terms(self.keywords);
        self.keyword_exemptions = normalize_terms(self.keyword_exemptions);
        self.thresholds = normalize_thresholds(self.thresholds);
        self.model_filter.models = normalize_model_filter_models(self.model_filter.models);
        if self.model_filter.mode == RiskControlModelFilterMode::All {
            self.model_filter.models.clear();
        }
        self.scope.users = normalize_scope_list(self.scope.users);
        self.scope.user_groups = normalize_scope_list(self.scope.user_groups);
        self.scope.api_keys = normalize_scope_list(self.scope.api_keys);
        self.scope.route_families = normalize_scope_list(self.scope.route_families);
        self.scope.route_kinds = normalize_scope_list(self.scope.route_kinds);
        self.scope.endpoints = normalize_scope_list(self.scope.endpoints);
        self.provider.base_url = self
            .provider
            .base_url
            .trim()
            .trim_end_matches('/')
            .to_string();
        if self.provider.base_url.is_empty() {
            self.provider.base_url = default_provider_base_url();
        }
        self.provider.model = self.provider.model.trim().to_string();
        if self.provider.model.is_empty() {
            self.provider.model = default_provider_model();
        }
        self.provider.timeout_ms = self.provider.timeout_ms.clamp(500, 60_000);
        self.provider.max_retries = self.provider.max_retries.min(8);
        self.provider.key_freeze_seconds = self.provider.key_freeze_seconds.min(86_400);
        self.sample_rate = if self.sample_rate.is_finite() {
            self.sample_rate.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.max_text_chars = self.max_text_chars.clamp(256, 2 * 1024 * 1024);
        self.excerpt_chars = self.excerpt_chars.clamp(64, 4096);
        self.block_status = if (400..=499).contains(&self.block_status) {
            self.block_status
        } else {
            default_block_status()
        };
        self.block_message = normalize_block_message(self.block_message);
        self.auto_action.violation_threshold = self
            .auto_action
            .violation_threshold
            .clamp(1, MAX_AUTO_ACTION_VIOLATION_THRESHOLD);
        self.auto_action.window_seconds = self
            .auto_action
            .window_seconds
            .clamp(60, MAX_AUTO_ACTION_WINDOW_SECONDS);
        self.retention.hit_days = self.retention.hit_days.min(MAX_RETENTION_DAYS);
        self.retention.non_hit_days = self.retention.non_hit_days.min(MAX_RETENTION_DAYS);
        self.retention.auto_run_interval_minutes = self
            .retention
            .auto_run_interval_minutes
            .min(MAX_RETENTION_AUTO_RUN_INTERVAL_MINUTES);
        self.observe.queue_capacity = self
            .observe
            .queue_capacity
            .clamp(MIN_OBSERVE_QUEUE_CAPACITY, MAX_OBSERVE_QUEUE_CAPACITY);
        self
    }

    pub(crate) fn redacted_json(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).unwrap_or_else(|_| json!({}));
        if let Some(keys) = value
            .get_mut("provider")
            .and_then(|provider| provider.get_mut("api_keys"))
            .and_then(serde_json::Value::as_array_mut)
        {
            for key in keys {
                *key = json!(mask_secret(key.as_str().unwrap_or_default()));
            }
        }
        value
    }

    pub(crate) fn validate_provider_base_url(&self) -> Result<(), String> {
        validate_provider_base_url(&self.provider.base_url)
    }

    pub(crate) fn includes_model(&self, model: Option<&str>) -> bool {
        match self.model_filter.mode {
            RiskControlModelFilterMode::All => true,
            RiskControlModelFilterMode::Include => model
                .map(|model| model_filter_contains(&self.model_filter.models, model))
                .unwrap_or(false),
            RiskControlModelFilterMode::Exclude => model
                .map(|model| !model_filter_contains(&self.model_filter.models, model))
                .unwrap_or(true),
        }
    }

    pub(crate) fn includes_scope(&self, context: &RiskControlScopeMatchContext<'_>) -> bool {
        self.includes_model(context.model)
            && scope_list_includes(&self.scope.users, context.user_id)
            && scope_list_includes_any(&self.scope.user_groups, context.user_group_ids)
            && scope_list_includes(&self.scope.api_keys, context.api_key_id)
            && scope_list_includes(&self.scope.route_families, context.route_family)
            && scope_list_includes(&self.scope.route_kinds, context.route_kind)
            && scope_list_includes(&self.scope.endpoints, context.endpoint)
    }
}

fn normalize_terms(values: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let value = value.chars().take(MAX_KEYWORD_CHARS).collect::<String>();
        if !out
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&value))
        {
            out.push(value);
        }
        if out.len() >= MAX_KEYWORD_ITEMS {
            break;
        }
    }
    out
}

fn normalize_thresholds(thresholds: BTreeMap<String, f64>) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for (name, value) in thresholds {
        let name = name.trim();
        if name.is_empty() || !value.is_finite() {
            continue;
        }
        let name = name
            .chars()
            .take(MAX_THRESHOLD_NAME_CHARS)
            .collect::<String>();
        out.insert(name, value.clamp(0.0, 1.0));
        if out.len() >= MAX_THRESHOLD_ITEMS {
            break;
        }
    }
    out
}

fn normalize_model_filter_models(models: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for model in models {
        let model = model.trim();
        if model.is_empty() {
            continue;
        }
        let model = model.chars().take(200).collect::<String>();
        if !out
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&model))
        {
            out.push(model);
        }
        if out.len() >= 1000 {
            break;
        }
    }
    out
}

fn normalize_scope_list(mut list: RiskControlScopeListConfig) -> RiskControlScopeListConfig {
    list.values = normalize_scope_values(list.values);
    if list.mode == RiskControlScopeMode::All {
        list.values.clear();
    }
    list
}

fn normalize_scope_values(values: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let value = value
            .chars()
            .take(MAX_SCOPE_VALUE_CHARS)
            .collect::<String>();
        if !out
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&value))
        {
            out.push(value);
        }
        if out.len() >= MAX_SCOPE_ITEMS {
            break;
        }
    }
    out
}

fn model_filter_contains(models: &[String], model: &str) -> bool {
    let model = model.trim();
    !model.is_empty()
        && models
            .iter()
            .any(|candidate| candidate.trim().eq_ignore_ascii_case(model))
}

fn scope_list_includes(list: &RiskControlScopeListConfig, value: Option<&str>) -> bool {
    match list.mode {
        RiskControlScopeMode::All => true,
        RiskControlScopeMode::Include => value
            .map(|value| scope_value_contains(&list.values, value))
            .unwrap_or(false),
        RiskControlScopeMode::Exclude => value
            .map(|value| !scope_value_contains(&list.values, value))
            .unwrap_or(true),
    }
}

fn scope_list_includes_any(list: &RiskControlScopeListConfig, values: &[String]) -> bool {
    match list.mode {
        RiskControlScopeMode::All => true,
        RiskControlScopeMode::Include => values
            .iter()
            .any(|value| scope_value_contains(&list.values, value)),
        RiskControlScopeMode::Exclude => !values
            .iter()
            .any(|value| scope_value_contains(&list.values, value)),
    }
}

fn scope_value_contains(values: &[String], value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && values
            .iter()
            .any(|candidate| candidate.trim().eq_ignore_ascii_case(value))
}

fn normalize_block_message(value: String) -> String {
    let value = value.trim();
    if value.is_empty() {
        return default_block_message();
    }
    value.chars().take(MAX_BLOCK_MESSAGE_CHARS).collect()
}

pub(crate) fn validate_provider_base_url(value: &str) -> Result<(), String> {
    let value = value.trim();
    let parsed =
        url::Url::parse(value).map_err(|err| format!("Provider Base URL 不是合法 URL: {err}"))?;
    if parsed.scheme() != "https" {
        return Err("Provider Base URL 必须使用 HTTPS".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Provider Base URL 不能包含用户名或密码".to_string());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("Provider Base URL 不能包含 query 或 fragment".to_string());
    }
    let host = parsed
        .host_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Provider Base URL 必须包含 host".to_string())?;
    validate_public_host(host)
}

fn validate_public_host(host: &str) -> Result<(), String> {
    let host_lower = host.trim_end_matches('.').to_ascii_lowercase();
    let host_for_ip_parse = host_lower
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host_lower.as_str());
    if matches!(
        host_for_ip_parse,
        "localhost" | "localhost.localdomain" | "0.0.0.0"
    ) || host_for_ip_parse.ends_with(".localhost")
    {
        return Err("Provider Base URL host 不能是 localhost".to_string());
    }
    if let Ok(ip) = host_for_ip_parse.parse::<IpAddr>() {
        if is_blocked_provider_ip(ip) {
            return Err("Provider Base URL host 不能是内网、回环、链路本地或保留地址".to_string());
        }
    }
    Ok(())
}

pub(crate) fn is_blocked_provider_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ipv4_is_this_network(ip)
                || ipv4_is_shared(ip)
                || ipv4_is_benchmark(ip)
                || ipv4_is_ietf_protocol_assignment(ip)
                || ip.is_multicast()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified()
                || ipv4_is_reserved(ip)
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_blocked_provider_ip(IpAddr::V4(mapped));
            }
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
                || ipv6_is_ipv4_compatible(ip)
                || ipv6_is_site_local(ip)
                || ipv6_is_documentation(ip)
                || ipv6_is_special_purpose(ip)
        }
    }
}

fn ipv4_is_this_network(ip: Ipv4Addr) -> bool {
    ip.octets()[0] == 0
}

fn ipv4_is_shared(ip: Ipv4Addr) -> bool {
    let [first, second, ..] = ip.octets();
    first == 100 && (64..=127).contains(&second)
}

fn ipv4_is_benchmark(ip: Ipv4Addr) -> bool {
    let [first, second, ..] = ip.octets();
    first == 198 && matches!(second, 18 | 19)
}

fn ipv4_is_ietf_protocol_assignment(ip: Ipv4Addr) -> bool {
    let [first, second, third, ..] = ip.octets();
    first == 192 && second == 0 && third == 0
}

fn ipv4_is_reserved(ip: Ipv4Addr) -> bool {
    ip.octets()[0] >= 240
}

fn ipv6_is_documentation(ip: Ipv6Addr) -> bool {
    ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8
}

fn ipv6_is_ipv4_compatible(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    segments[..6].iter().all(|segment| *segment == 0)
}

fn ipv6_is_site_local(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    (0xfec0..=0xfeff).contains(&first)
}

fn ipv6_is_special_purpose(ip: Ipv6Addr) -> bool {
    ipv6_in_prefix(ip, Ipv6Addr::new(0x0064, 0xff9b, 0, 0, 0, 0, 0, 0), 96)
        || ipv6_in_prefix(ip, Ipv6Addr::new(0x0064, 0xff9b, 0x0001, 0, 0, 0, 0, 0), 48)
        || ipv6_in_prefix(ip, Ipv6Addr::new(0x0100, 0, 0, 0, 0, 0, 0, 0), 64)
        || ipv6_in_prefix(ip, Ipv6Addr::new(0x0100, 0, 0, 0x0001, 0, 0, 0, 0), 64)
        || ipv6_in_prefix(ip, Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23)
        || ipv6_in_prefix(ip, Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16)
        || ipv6_in_prefix(ip, Ipv6Addr::new(0x2620, 0x004f, 0x8000, 0, 0, 0, 0, 0), 48)
        || ipv6_in_prefix(ip, Ipv6Addr::new(0x3ffe, 0, 0, 0, 0, 0, 0, 0), 16)
        || ipv6_in_prefix(ip, Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20)
        || ipv6_in_prefix(ip, Ipv6Addr::new(0x5f00, 0, 0, 0, 0, 0, 0, 0), 16)
}

fn ipv6_in_prefix(ip: Ipv6Addr, prefix: Ipv6Addr, prefix_len: u32) -> bool {
    let mask = if prefix_len == 0 {
        0
    } else {
        u128::MAX << (128 - prefix_len)
    };
    let ip = u128::from_be_bytes(ip.octets());
    let prefix = u128::from_be_bytes(prefix.octets());
    (ip & mask) == (prefix & mask)
}

pub(crate) async fn read_risk_control_runtime_config(
    state: &AppState,
) -> Result<RiskControlRuntimeConfig, GatewayError> {
    let enabled = state
        .read_system_config_json_value(RISK_CONTROL_ENABLED_CONFIG_KEY)
        .await?
        .and_then(|value| value.as_bool())
        .unwrap_or(false);

    let mut config = state
        .read_system_config_json_value(RISK_CONTROL_CONFIG_KEY)
        .await?
        .map(serde_json::from_value::<RiskControlRuntimeConfig>)
        .transpose()
        .map_err(|err| GatewayError::Internal(format!("invalid risk control config: {err}")))?
        .unwrap_or_default();
    config.enabled = enabled;
    Ok(config.sanitized())
}

fn mask_secret(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return String::new();
    }
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= 8 {
        return "****".to_string();
    }
    let prefix = chars.iter().take(4).collect::<String>();
    let suffix = chars
        .iter()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{prefix}****{suffix}")
}

fn default_true() -> bool {
    true
}

fn default_provider_base_url() -> String {
    "https://api.openai.com".to_string()
}

fn default_provider_model() -> String {
    "omni-moderation-latest".to_string()
}

fn default_provider_timeout_ms() -> u64 {
    8_000
}

fn default_provider_max_retries() -> usize {
    2
}

fn default_key_freeze_seconds() -> u64 {
    300
}

fn default_sample_rate() -> f64 {
    1.0
}

fn default_max_text_chars() -> usize {
    64 * 1024
}

fn default_excerpt_chars() -> usize {
    512
}

fn default_block_status() -> u16 {
    400
}

fn default_block_message() -> String {
    "请求触发风控策略，已拒绝转发。".to_string()
}

fn default_violation_threshold() -> u64 {
    3
}

fn default_violation_window_seconds() -> u64 {
    86_400
}

fn default_hit_retention_days() -> u64 {
    90
}

fn default_non_hit_retention_days() -> u64 {
    14
}

fn default_retention_auto_run_interval_minutes() -> u64 {
    60
}

fn default_observe_queue_capacity() -> usize {
    1024
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_base_url_requires_public_https_url() {
        assert!(validate_provider_base_url("https://api.openai.com").is_ok());
        assert!(validate_provider_base_url("http://api.openai.com").is_err());
        assert!(validate_provider_base_url("https://localhost").is_err());
        assert!(validate_provider_base_url("https://127.0.0.1").is_err());
        assert!(validate_provider_base_url("https://10.0.0.2").is_err());
        assert!(validate_provider_base_url("https://169.254.169.254").is_err());
        assert!(validate_provider_base_url("https://100.64.1.1").is_err());
        assert!(validate_provider_base_url("https://198.18.0.1").is_err());
        assert!(validate_provider_base_url("https://224.0.0.1").is_err());
        assert!(validate_provider_base_url("https://240.0.0.1").is_err());
        assert!(validate_provider_base_url("https://[::1]").is_err());
        assert!(validate_provider_base_url("https://[::8.8.8.8]").is_err());
        assert!(validate_provider_base_url("https://[::ffff:127.0.0.1]").is_err());
        assert!(validate_provider_base_url("https://[64:ff9b::808:808]").is_err());
        assert!(validate_provider_base_url("https://[100::1]").is_err());
        assert!(validate_provider_base_url("https://[2001::1]").is_err());
        assert!(validate_provider_base_url("https://[2001:db8::1]").is_err());
        assert!(validate_provider_base_url("https://[2002:0808:0808::1]").is_err());
        assert!(validate_provider_base_url("https://[5f00::1]").is_err());
        assert!(validate_provider_base_url("https://user:pass@example.com").is_err());
        assert!(validate_provider_base_url("https://api.openai.com?x=1").is_err());
    }

    #[test]
    fn model_filter_include_and_exclude_are_case_insensitive() {
        let include = RiskControlRuntimeConfig {
            model_filter: RiskControlModelFilterConfig {
                mode: RiskControlModelFilterMode::Include,
                models: vec!["GPT-5".to_string()],
            },
            ..RiskControlRuntimeConfig::default()
        }
        .sanitized();
        assert!(include.includes_model(Some("gpt-5")));
        assert!(!include.includes_model(Some("gpt-4.1")));
        assert!(!include.includes_model(None));

        let exclude = RiskControlRuntimeConfig {
            model_filter: RiskControlModelFilterConfig {
                mode: RiskControlModelFilterMode::Exclude,
                models: vec!["gpt-5".to_string()],
            },
            ..RiskControlRuntimeConfig::default()
        }
        .sanitized();
        assert!(!exclude.includes_model(Some("GPT-5")));
        assert!(exclude.includes_model(Some("gpt-4.1")));
        assert!(exclude.includes_model(None));
    }

    #[test]
    fn runtime_scope_filters_user_key_route_endpoint_and_model() {
        let config = RiskControlRuntimeConfig {
            model_filter: RiskControlModelFilterConfig {
                mode: RiskControlModelFilterMode::Include,
                models: vec!["gpt-5".to_string()],
            },
            scope: RiskControlScopeConfig {
                users: RiskControlScopeListConfig {
                    mode: RiskControlScopeMode::Include,
                    values: vec!["user-1".to_string()],
                },
                user_groups: RiskControlScopeListConfig {
                    mode: RiskControlScopeMode::Include,
                    values: vec!["group-risk".to_string()],
                },
                api_keys: RiskControlScopeListConfig {
                    mode: RiskControlScopeMode::Exclude,
                    values: vec!["key-blocked".to_string()],
                },
                route_families: RiskControlScopeListConfig {
                    mode: RiskControlScopeMode::Include,
                    values: vec!["openai".to_string()],
                },
                route_kinds: RiskControlScopeListConfig {
                    mode: RiskControlScopeMode::Include,
                    values: vec!["chat".to_string()],
                },
                endpoints: RiskControlScopeListConfig {
                    mode: RiskControlScopeMode::Include,
                    values: vec!["openai:chat".to_string()],
                },
            },
            ..RiskControlRuntimeConfig::default()
        }
        .sanitized();

        let allowed_group_ids = vec!["group-risk".to_string()];
        let blocked_group_ids = vec!["group-safe".to_string()];
        let allowed = RiskControlScopeMatchContext {
            user_id: Some("USER-1"),
            user_group_ids: &allowed_group_ids,
            api_key_id: Some("key-ok"),
            route_family: Some("openai"),
            route_kind: Some("chat"),
            endpoint: Some("openai:chat"),
            model: Some("GPT-5"),
        };
        assert!(config.includes_scope(&allowed));

        assert!(!config.includes_scope(&RiskControlScopeMatchContext {
            user_id: Some("user-2"),
            ..allowed
        }));
        assert!(!config.includes_scope(&RiskControlScopeMatchContext {
            user_group_ids: &blocked_group_ids,
            ..allowed
        }));
        assert!(!config.includes_scope(&RiskControlScopeMatchContext {
            api_key_id: Some("KEY-BLOCKED"),
            ..allowed
        }));
        assert!(!config.includes_scope(&RiskControlScopeMatchContext {
            route_kind: Some("responses"),
            ..allowed
        }));
        assert!(!config.includes_scope(&RiskControlScopeMatchContext {
            endpoint: Some("claude:messages"),
            ..allowed
        }));
        assert!(!config.includes_scope(&RiskControlScopeMatchContext {
            model: Some("gpt-4.1"),
            ..allowed
        }));
    }

    #[test]
    fn runtime_config_sanitizes_keyword_and_threshold_bounds() {
        let long_keyword = "x".repeat(MAX_KEYWORD_CHARS + 20);
        let long_threshold_name = "category".repeat(30);
        let mut thresholds = BTreeMap::new();
        thresholds.insert(" violence ".to_string(), 2.0);
        thresholds.insert("bad".to_string(), -1.0);
        thresholds.insert(" ".to_string(), 0.5);
        thresholds.insert(long_threshold_name, 0.4);
        for index in 0..(MAX_THRESHOLD_ITEMS + 10) {
            thresholds.insert(format!("cat-{index:03}"), 0.5);
        }
        let config = RiskControlRuntimeConfig {
            keywords: vec![
                " blocked ".to_string(),
                "BLOCKED".to_string(),
                String::new(),
                long_keyword,
            ]
            .into_iter()
            .chain((0..(MAX_KEYWORD_ITEMS + 10)).map(|index| format!("term-{index:04}")))
            .collect(),
            keyword_exemptions: vec![" demo ".to_string(), "DEMO".to_string()],
            thresholds,
            ..RiskControlRuntimeConfig::default()
        }
        .sanitized();

        assert_eq!(config.keywords[0], "blocked");
        assert_eq!(config.keyword_exemptions, vec!["demo"]);
        assert!(config.keywords.len() <= MAX_KEYWORD_ITEMS);
        assert!(config
            .keywords
            .iter()
            .all(|keyword| keyword.chars().count() <= MAX_KEYWORD_CHARS));
        assert!(config.thresholds.len() <= MAX_THRESHOLD_ITEMS);
        assert_eq!(config.thresholds.get("violence"), Some(&1.0));
        assert_eq!(config.thresholds.get("bad"), Some(&0.0));
        assert!(!config.thresholds.contains_key(""));
        assert!(config
            .thresholds
            .keys()
            .all(|name| name.chars().count() <= MAX_THRESHOLD_NAME_CHARS));
    }

    #[test]
    fn runtime_config_sanitizes_operational_bounds() {
        let config = RiskControlRuntimeConfig {
            block_message: format!("  {}  ", "拦".repeat(MAX_BLOCK_MESSAGE_CHARS + 20)),
            auto_action: RiskControlAutoActionConfig {
                violation_threshold: MAX_AUTO_ACTION_VIOLATION_THRESHOLD + 1,
                window_seconds: MAX_AUTO_ACTION_WINDOW_SECONDS + 1,
                ..RiskControlAutoActionConfig::default()
            },
            retention: RiskControlRetentionConfig {
                hit_days: MAX_RETENTION_DAYS + 1,
                non_hit_days: MAX_RETENTION_DAYS + 2,
                auto_run_interval_minutes: MAX_RETENTION_AUTO_RUN_INTERVAL_MINUTES + 1,
            },
            observe: RiskControlObserveConfig {
                queue_capacity: MAX_OBSERVE_QUEUE_CAPACITY + 10,
            },
            ..RiskControlRuntimeConfig::default()
        }
        .sanitized();

        assert_eq!(
            config.block_message.chars().count(),
            MAX_BLOCK_MESSAGE_CHARS
        );
        assert_eq!(
            config.auto_action.violation_threshold,
            MAX_AUTO_ACTION_VIOLATION_THRESHOLD
        );
        assert_eq!(
            config.auto_action.window_seconds,
            MAX_AUTO_ACTION_WINDOW_SECONDS
        );
        assert_eq!(config.retention.hit_days, MAX_RETENTION_DAYS);
        assert_eq!(config.retention.non_hit_days, MAX_RETENTION_DAYS);
        assert_eq!(
            config.retention.auto_run_interval_minutes,
            MAX_RETENTION_AUTO_RUN_INTERVAL_MINUTES
        );

        let empty_message = RiskControlRuntimeConfig {
            block_message: "   ".to_string(),
            ..RiskControlRuntimeConfig::default()
        }
        .sanitized();
        assert_eq!(empty_message.block_message, default_block_message());
    }
}
