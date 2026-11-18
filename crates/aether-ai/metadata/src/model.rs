pub const OPENAI_MODEL_DIRECTIVE_SUFFIXES: [&str; 8] = [
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "fast",
];

pub const CROSS_PROVIDER_MODEL_DIRECTIVE_SUFFIXES: [&str; 5] =
    ["low", "medium", "high", "xhigh", "max"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDirective {
    pub base_model: String,
    pub overrides: Vec<ModelOverride>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDirectiveSuffixResolution {
    pub base_model: String,
    pub suffixes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelOverride {
    ReasoningEffort(ReasoningEffort),
    ServiceTier(ServiceTier),
}

impl ModelOverride {
    pub fn suffix(&self) -> &'static str {
        match self {
            Self::ReasoningEffort(effort) => effort.as_str(),
            Self::ServiceTier(tier) => tier.as_directive_suffix(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl ReasoningEffort {
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "minimal" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::XHigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn as_openai_chat_value(self) -> &'static str {
        self.as_str()
    }

    pub fn as_openai_responses_value(self) -> &'static str {
        self.as_str()
    }

    pub fn as_openai_model_directive_value(self) -> &'static str {
        self.as_str()
    }

    pub fn as_claude_output_value(self) -> &'static str {
        match self {
            Self::None | Self::Minimal => "low",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn as_gemini_level_value(self) -> &'static str {
        match self {
            Self::None | Self::Minimal | Self::Low => "low",
            Self::Medium => "medium",
            Self::High | Self::XHigh | Self::Max => "high",
        }
    }

    pub fn thinking_budget_tokens(self) -> u64 {
        match self {
            Self::None => 0,
            Self::Minimal => 512,
            Self::Low => 1280,
            Self::Medium => 2048,
            Self::High => 4096,
            Self::XHigh | Self::Max => 8192,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceTier {
    Priority,
}

impl ServiceTier {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "fast" => Some(Self::Priority),
            _ => None,
        }
    }

    pub fn as_directive_suffix(self) -> &'static str {
        match self {
            Self::Priority => "fast",
        }
    }

    pub fn as_openai_value(self) -> &'static str {
        match self {
            Self::Priority => "priority",
        }
    }
}

pub fn parse_model_directive(model: &str) -> Option<ModelDirective> {
    let resolution = parse_model_directive_with_suffixes(
        model,
        OPENAI_MODEL_DIRECTIVE_SUFFIXES.iter().copied(),
    )?;
    let overrides = resolution
        .suffixes
        .iter()
        .map(|suffix| parse_model_override_for_model(suffix, &resolution.base_model))
        .collect::<Option<Vec<_>>>()?;
    Some(ModelDirective {
        base_model: resolution.base_model,
        overrides,
    })
}

pub fn parse_model_directive_with_suffixes<'a>(
    model: &str,
    suffixes: impl IntoIterator<Item = &'a str>,
) -> Option<ModelDirectiveSuffixResolution> {
    let mut configured_suffixes = Vec::<String>::new();
    for suffix in suffixes {
        let suffix = suffix.trim();
        if suffix.is_empty() || suffix.starts_with('-') || suffix.ends_with('-') {
            continue;
        }
        if let Some(existing) = configured_suffixes
            .iter()
            .find(|existing| existing.eq_ignore_ascii_case(suffix))
        {
            if existing != suffix {
                return None;
            }
            continue;
        }
        configured_suffixes.push(suffix.to_string());
    }
    configured_suffixes
        .sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));

    let mut base_model = model.trim();
    let mut matched_suffixes = Vec::<String>::new();
    let mut matched_reasoning_effort = false;
    let mut matched_service_tier = false;
    while let Some((candidate_base, suffix)) = configured_suffixes.iter().find_map(|suffix| {
        strip_model_directive_suffix(base_model, suffix)
            .map(|candidate_base| (candidate_base, suffix))
    }) {
        if matched_suffixes
            .iter()
            .any(|matched| matched.eq_ignore_ascii_case(suffix))
        {
            return None;
        }
        if model_directive_suffix_is_reasoning(suffix) {
            if matched_reasoning_effort {
                return None;
            }
            matched_reasoning_effort = true;
        } else if ServiceTier::parse(suffix).is_some() {
            if matched_service_tier {
                return None;
            }
            matched_service_tier = true;
        }
        matched_suffixes.push(suffix.clone());
        base_model = candidate_base.trim();
    }

    if base_model.is_empty() || matched_suffixes.is_empty() {
        return None;
    }
    matched_suffixes.sort_by(|left, right| {
        model_directive_suffix_rank(left)
            .cmp(&model_directive_suffix_rank(right))
            .then_with(|| left.cmp(right))
    });
    Some(ModelDirectiveSuffixResolution {
        base_model: base_model.to_string(),
        suffixes: matched_suffixes,
    })
}

fn strip_model_directive_suffix<'a>(model: &'a str, suffix: &str) -> Option<&'a str> {
    let suffix_start = model.len().checked_sub(suffix.len())?;
    let separator = suffix_start.checked_sub(1)?;
    if !model.is_char_boundary(suffix_start)
        || !model.is_char_boundary(separator)
        || model.as_bytes().get(separator) != Some(&b'-')
        || !model[suffix_start..].eq_ignore_ascii_case(suffix)
    {
        return None;
    }
    Some(&model[..separator])
}

fn model_directive_suffix_rank(suffix: &str) -> u8 {
    if model_directive_suffix_is_reasoning(suffix) {
        0
    } else if ServiceTier::parse(suffix).is_some() {
        1
    } else {
        2
    }
}

fn parse_model_override(suffix: &str) -> Option<ModelOverride> {
    ReasoningEffort::parse(suffix)
        .map(ModelOverride::ReasoningEffort)
        .or_else(|| ServiceTier::parse(suffix).map(ModelOverride::ServiceTier))
}

pub fn parse_model_override_for_model(suffix: &str, model: &str) -> Option<ModelOverride> {
    let _ = model;
    parse_model_override(suffix)
}

pub fn model_directive_suffix_has_builtin_mapping(suffix: &str) -> bool {
    parse_model_override(suffix).is_some()
}

pub fn model_directive_builtin_suffix_supported_for_source_model(
    suffix: &str,
    source_model: &str,
) -> bool {
    parse_model_override_for_model(suffix, source_model).is_some()
}

fn model_directive_suffix_is_reasoning(suffix: &str) -> bool {
    ReasoningEffort::parse(suffix).is_some()
}

pub fn model_directive_base_model(model: &str) -> Option<String> {
    parse_model_directive(model).map(|directive| directive.base_model)
}

pub fn normalize_model_directive_model(model: &str) -> String {
    parse_model_directive(model)
        .map(|directive| directive.base_model)
        .unwrap_or_else(|| model.trim().to_string())
}

pub fn openai_model_supports_max_reasoning_effort(model: &str) -> bool {
    is_openai_gpt_5_6_family(model)
}

pub fn openai_model_supports_prompt_cache_options(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase().replace('_', "-");
    let model = normalized.rsplit('/').next().unwrap_or_default();
    openai_gpt_model_version(model)
        .is_some_and(|(major, minor)| major > 5 || (major == 5 && minor >= 6))
}

pub fn reasoning_effort_supported_for_model(
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    effort: ReasoningEffort,
) -> bool {
    match crate::normalize_api_format_alias(provider_api_format).as_str() {
        "openai:chat" | "openai:responses" | "openai:responses:compact" | "openai:search" => {
            match resolved_openai_model_identity(provider_model, source_model).0 {
                OpenAiModelIdentity::Gpt56 => effort != ReasoningEffort::Minimal,
                OpenAiModelIdentity::ConcreteOther => effort != ReasoningEffort::Max,
                OpenAiModelIdentity::Opaque => true,
            }
        }
        "claude:messages" | "gemini:generate_content" => matches!(
            effort,
            ReasoningEffort::Low
                | ReasoningEffort::Medium
                | ReasoningEffort::High
                | ReasoningEffort::XHigh
                | ReasoningEffort::Max
        ),
        _ => false,
    }
}

pub fn openai_model_resolves_to_gpt_5_6(provider_model: &str, source_model: &str) -> bool {
    matches!(
        resolved_openai_model_identity(provider_model, source_model).0,
        OpenAiModelIdentity::Gpt56 | OpenAiModelIdentity::Opaque
    )
}

pub fn openai_model_capability_identity(provider_model: &str, source_model: &str) -> String {
    resolved_openai_model_identity(provider_model, source_model).1
}

pub fn openai_model_capability_is_opaque(provider_model: &str, source_model: &str) -> bool {
    resolved_openai_model_identity(provider_model, source_model).0 == OpenAiModelIdentity::Opaque
}

fn resolved_openai_model_identity(
    provider_model: &str,
    source_model: &str,
) -> (OpenAiModelIdentity, String) {
    let provider_model = normalize_model_directive_model(provider_model);
    let provider_identity = classify_openai_model_identity(&provider_model);
    if provider_identity != OpenAiModelIdentity::Opaque {
        return (provider_identity, provider_model);
    }
    let source_model = normalize_model_directive_model(source_model);
    (classify_openai_model_identity(&source_model), source_model)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenAiModelIdentity {
    Gpt56,
    ConcreteOther,
    Opaque,
}

fn classify_openai_model_identity(model: &str) -> OpenAiModelIdentity {
    if is_openai_gpt_5_6_family(model) {
        return OpenAiModelIdentity::Gpt56;
    }
    let normalized = model.trim().to_ascii_lowercase().replace('_', "-");
    let model = normalized.rsplit('/').next().unwrap_or_default();
    if openai_gpt_model_identity_is_concrete(model)
        || openai_o_series_model_identity_is_concrete(model)
        || model.starts_with("chatgpt-")
        || model.starts_with("codex-")
    {
        OpenAiModelIdentity::ConcreteOther
    } else {
        OpenAiModelIdentity::Opaque
    }
}

fn openai_gpt_model_identity_is_concrete(model: &str) -> bool {
    let Some(rest) = model.strip_prefix("gpt-") else {
        return false;
    };
    let mut segments = rest.split('-');
    let Some(version) = segments.next() else {
        return false;
    };
    if openai_gpt_model_version(model).is_none() {
        return false;
    }
    let version_is_omni = version.ends_with('o');
    if version.contains('.') || version_is_omni {
        return true;
    }
    match segments.next() {
        None => true,
        Some(variant) => {
            variant.chars().all(|character| character.is_ascii_digit())
                || matches!(
                    variant,
                    "audio"
                        | "chat"
                        | "codex"
                        | "mini"
                        | "nano"
                        | "pro"
                        | "realtime"
                        | "search"
                        | "turbo"
                        | "vision"
                )
        }
    }
}

fn openai_gpt_model_version(model: &str) -> Option<(u64, u64)> {
    let version = model.strip_prefix("gpt-")?.split('-').next()?;
    let version = version.strip_suffix('o').unwrap_or(version);
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map(str::parse).transpose().ok()?.unwrap_or(0);
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor))
}

fn openai_o_series_model_identity_is_concrete(model: &str) -> bool {
    let Some(rest) = model.strip_prefix('o') else {
        return false;
    };
    rest.split('-').next().is_some_and(|version| {
        !version.is_empty() && version.chars().all(|character| character.is_ascii_digit())
    })
}

fn is_openai_gpt_5_6_family(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase().replace('_', "-");
    let model = normalized.rsplit('/').next().unwrap_or_default();
    openai_gpt_model_version(model) == Some((5, 6))
}

pub fn map_thinking_budget_to_openai_reasoning_effort(value: u64) -> &'static str {
    match value {
        0..=1664 => "low",
        1665..=3072 => "medium",
        3073..=6144 => "high",
        _ => "xhigh",
    }
}
