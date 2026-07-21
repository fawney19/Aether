use serde_json::{json, Value};

pub const MODEL_DIRECTIVE_API_FORMATS: [&str; 6] = [
    "openai:chat",
    "openai:responses",
    "openai:responses:compact",
    "openai:search",
    "claude:messages",
    "gemini:generate_content",
];
pub const OPENAI_MODEL_DIRECTIVE_SUFFIXES: [&str; 9] = [
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra", "fast",
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
    CodexReasoningPreset(CodexReasoningPreset),
    ServiceTier(ServiceTier),
}

impl ModelOverride {
    pub fn suffix(&self) -> &'static str {
        match self {
            Self::ReasoningEffort(effort) => effort.as_str(),
            Self::CodexReasoningPreset(preset) => preset.as_str(),
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
pub enum CodexReasoningPreset {
    Ultra,
}

impl CodexReasoningPreset {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ultra => "ultra",
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

fn parse_model_override_for_model(suffix: &str, model: &str) -> Option<ModelOverride> {
    if suffix.eq_ignore_ascii_case("ultra") && codex_ultra_preset_supported_for_model(model) {
        return Some(ModelOverride::CodexReasoningPreset(
            CodexReasoningPreset::Ultra,
        ));
    }
    parse_model_override(suffix)
}

pub fn model_directive_suffix_has_builtin_mapping(suffix: &str) -> bool {
    parse_model_override(suffix).is_some() || suffix.eq_ignore_ascii_case("ultra")
}

pub fn model_directive_builtin_suffix_supported_for_source_model(
    suffix: &str,
    source_model: &str,
) -> bool {
    parse_model_override_for_model(suffix, source_model).is_some()
}

fn model_directive_suffix_is_reasoning(suffix: &str) -> bool {
    ReasoningEffort::parse(suffix).is_some() || suffix.eq_ignore_ascii_case("ultra")
}

fn codex_ultra_preset_supported_for_model(model: &str) -> bool {
    crate::formats::openai::responses::codex::resolve_codex_responses_model_capabilities(
        model, model, None,
    )
    .supports_reasoning_effort("ultra")
}

pub fn model_directive_base_model(model: &str) -> Option<String> {
    parse_model_directive(model).map(|directive| directive.base_model)
}

pub(crate) fn model_directive_display_model(model: &str) -> Option<String> {
    let model = model.trim();
    parse_model_directive(model)?;
    Some(model.to_string())
}

pub(crate) fn model_directive_display_model_from_report_context(
    report_context: &Value,
) -> Option<String> {
    report_context
        .get("model")
        .and_then(Value::as_str)
        .and_then(model_directive_display_model)
}

pub fn normalize_model_directive_model(model: &str) -> String {
    parse_model_directive(model)
        .map(|directive| directive.base_model)
        .unwrap_or_else(|| model.trim().to_string())
}

pub fn apply_model_directive_overrides_from_request(
    provider_request_body: &mut Value,
    provider_api_format: &str,
    provider_model: &str,
    request_body: &Value,
    request_path: Option<&str>,
) -> Option<ModelDirective> {
    let source_model = request_body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| request_path.and_then(extract_gemini_model_from_path))?;

    apply_model_directive_overrides_from_model(
        provider_request_body,
        provider_api_format,
        provider_model,
        &source_model,
    )
}

pub fn apply_model_directive_overrides_from_model(
    provider_request_body: &mut Value,
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
) -> Option<ModelDirective> {
    let directive = parse_model_directive(source_model)?;
    let mut patched_body = provider_request_body.clone();
    for override_item in &directive.overrides {
        match override_item {
            ModelOverride::ReasoningEffort(effort) => {
                apply_reasoning_effort_override(
                    &mut patched_body,
                    provider_api_format,
                    provider_model,
                    &directive.base_model,
                    *effort,
                )?;
            }
            ModelOverride::CodexReasoningPreset(preset) => {
                apply_codex_reasoning_preset_override(
                    &mut patched_body,
                    provider_api_format,
                    *preset,
                )?;
            }
            ModelOverride::ServiceTier(tier) => {
                apply_service_tier_override(&mut patched_body, provider_api_format, *tier)?;
            }
        }
    }
    *provider_request_body = patched_body;
    Some(directive)
}

pub fn apply_model_directive_mapping_patch(
    provider_request_body: &mut Value,
    patch: &Value,
) -> Option<()> {
    deep_merge_json(provider_request_body, patch);
    Some(())
}

pub fn default_model_directive_mapping_patch(
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    suffix: &str,
) -> Option<Value> {
    let override_item = parse_model_override_for_model(suffix, source_model)?;
    let mut patch = json!({});
    match override_item {
        ModelOverride::ReasoningEffort(effort) => apply_reasoning_effort_override(
            &mut patch,
            provider_api_format,
            provider_model,
            source_model,
            effort,
        )?,
        ModelOverride::CodexReasoningPreset(preset) => {
            apply_codex_reasoning_preset_override(&mut patch, provider_api_format, preset)?
        }
        ModelOverride::ServiceTier(tier) => {
            apply_service_tier_override(&mut patch, provider_api_format, tier)?
        }
    }
    Some(patch)
}

pub fn default_model_directive_suffixes(provider_api_format: &str) -> &'static [&'static str] {
    match crate::normalize_api_format_alias(provider_api_format).as_str() {
        "openai:chat" | "openai:responses" | "openai:responses:compact" | "openai:search" => {
            &OPENAI_MODEL_DIRECTIVE_SUFFIXES
        }
        "claude:messages" | "gemini:generate_content" => &CROSS_PROVIDER_MODEL_DIRECTIVE_SUFFIXES,
        _ => &[],
    }
}

pub fn default_model_directives_config() -> Value {
    let api_formats = MODEL_DIRECTIVE_API_FORMATS
        .into_iter()
        .map(|api_format| {
            (
                api_format.to_string(),
                json!({
                    "enabled": true,
                    "suffixes": default_model_directive_suffixes(api_format),
                    "mappings": {},
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();

    json!({
        "reasoning_effort": {
            "enabled": true,
            "api_formats": api_formats,
        },
    })
}

fn deep_merge_json(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target_object), Value::Object(patch_object)) => {
            for (key, patch_value) in patch_object {
                match target_object.get_mut(key) {
                    Some(target_value) => deep_merge_json(target_value, patch_value),
                    None => {
                        target_object.insert(key.clone(), patch_value.clone());
                    }
                }
            }
        }
        (target, patch) => {
            *target = patch.clone();
        }
    }
}

fn apply_reasoning_effort_override(
    provider_request_body: &mut Value,
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    effort: ReasoningEffort,
) -> Option<()> {
    if !reasoning_effort_supported_for_model(
        provider_api_format,
        provider_model,
        source_model,
        effort,
    ) {
        return None;
    }
    match crate::normalize_api_format_alias(provider_api_format).as_str() {
        "openai:chat" => set_object_string(
            provider_request_body,
            "reasoning_effort",
            effort.as_openai_model_directive_value(),
        ),
        "openai:responses" | "openai:responses:compact" | "openai:search" => {
            set_openai_responses_reasoning_effort(provider_request_body, effort)
        }
        "claude:messages" => {
            set_claude_reasoning_effort(provider_request_body, effort, provider_model)
        }
        "gemini:generate_content" => {
            set_gemini_reasoning_effort(provider_request_body, effort, provider_model)
        }
        _ => None,
    }
}

fn apply_codex_reasoning_preset_override(
    provider_request_body: &mut Value,
    provider_api_format: &str,
    preset: CodexReasoningPreset,
) -> Option<()> {
    match crate::normalize_api_format_alias(provider_api_format).as_str() {
        "openai:chat" => {
            set_object_string(provider_request_body, "reasoning_effort", preset.as_str())
        }
        "openai:responses" | "openai:responses:compact" | "openai:search" => {
            let object = provider_request_body.as_object_mut()?;
            let reasoning = object
                .entry("reasoning".to_string())
                .or_insert_with(|| json!({}));
            reasoning
                .as_object_mut()?
                .insert("effort".to_string(), json!(preset.as_str()));
            Some(())
        }
        _ => None,
    }
}

fn apply_service_tier_override(
    provider_request_body: &mut Value,
    provider_api_format: &str,
    tier: ServiceTier,
) -> Option<()> {
    match crate::normalize_api_format_alias(provider_api_format).as_str() {
        "openai:chat" | "openai:responses" | "openai:responses:compact" => set_object_string(
            provider_request_body,
            "service_tier",
            tier.as_openai_value(),
        ),
        "openai:search" => Some(()),
        _ => None,
    }
}

fn set_object_string(body: &mut Value, key: &str, value: &str) -> Option<()> {
    body.as_object_mut()?
        .insert(key.to_string(), Value::String(value.to_string()));
    Some(())
}

fn set_openai_responses_reasoning_effort(body: &mut Value, effort: ReasoningEffort) -> Option<()> {
    let body_object = body.as_object_mut()?;
    let reasoning = body_object
        .entry("reasoning".to_string())
        .or_insert_with(|| json!({}));
    if !reasoning.is_object() {
        *reasoning = json!({});
    }
    reasoning.as_object_mut()?.insert(
        "effort".to_string(),
        Value::String(effort.as_openai_model_directive_value().to_string()),
    );
    Some(())
}

fn set_claude_reasoning_effort(
    body: &mut Value,
    effort: ReasoningEffort,
    provider_model: &str,
) -> Option<()> {
    let body_object = body.as_object_mut()?;
    let output_config = body_object
        .entry("output_config".to_string())
        .or_insert_with(|| json!({}));
    if !output_config.is_object() {
        *output_config = json!({});
    }
    output_config.as_object_mut()?.insert(
        "effort".to_string(),
        Value::String(effort.as_claude_output_value().to_string()),
    );

    let thinking = body_object
        .entry("thinking".to_string())
        .or_insert_with(|| json!({}));
    if !thinking.is_object() {
        *thinking = json!({});
    }
    let thinking = thinking.as_object_mut()?;
    if claude_model_uses_adaptive_effort(provider_model) {
        thinking.insert("type".to_string(), Value::String("adaptive".to_string()));
        thinking.remove("budget_tokens");
    } else {
        thinking.insert("type".to_string(), Value::String("enabled".to_string()));
        thinking.insert(
            "budget_tokens".to_string(),
            Value::from(effort.thinking_budget_tokens()),
        );
    }
    Some(())
}

fn set_gemini_reasoning_effort(
    body: &mut Value,
    effort: ReasoningEffort,
    provider_model: &str,
) -> Option<()> {
    let body_object = body.as_object_mut()?;
    let generation_key = if body_object.contains_key("generation_config")
        && !body_object.contains_key("generationConfig")
    {
        "generation_config"
    } else {
        "generationConfig"
    };
    let generation_config = body_object
        .entry(generation_key.to_string())
        .or_insert_with(|| json!({}));
    if !generation_config.is_object() {
        *generation_config = json!({});
    }
    let generation_config = generation_config.as_object_mut()?;
    let thinking_key = if generation_config.contains_key("thinking_config")
        && !generation_config.contains_key("thinkingConfig")
    {
        "thinking_config"
    } else {
        "thinkingConfig"
    };
    generation_config.insert(
        thinking_key.to_string(),
        gemini_reasoning_effort_config(effort, provider_model, thinking_key),
    );
    Some(())
}

fn gemini_reasoning_effort_config(
    effort: ReasoningEffort,
    provider_model: &str,
    thinking_key: &str,
) -> Value {
    if gemini_model_uses_thinking_level(provider_model) {
        if thinking_key == "thinking_config" {
            return json!({
                "include_thoughts": true,
                "thinking_level": effort.as_gemini_level_value(),
            });
        }
        return json!({
            "includeThoughts": true,
            "thinkingLevel": effort.as_gemini_level_value(),
        });
    }

    if thinking_key == "thinking_config" {
        return json!({
            "include_thoughts": true,
            "thinking_budget": effort.thinking_budget_tokens(),
        });
    }
    json!({
        "includeThoughts": true,
        "thinkingBudget": effort.thinking_budget_tokens(),
    })
}

pub fn claude_model_uses_adaptive_effort(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase().replace(['.', '_'], "-");
    model.contains("mythos")
        || model.contains("opus-4-7")
        || model.contains("opus-4-6")
        || model.contains("sonnet-4-6")
}

pub fn gemini_model_uses_thinking_level(model: &str) -> bool {
    model
        .trim()
        .to_ascii_lowercase()
        .split('/')
        .any(|part| part.starts_with("gemini-3"))
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

pub(crate) fn openai_model_resolves_to_gpt_5_6(provider_model: &str, source_model: &str) -> bool {
    matches!(
        resolved_openai_model_identity(provider_model, source_model).0,
        OpenAiModelIdentity::Gpt56 | OpenAiModelIdentity::Opaque
    )
}

pub(crate) fn openai_model_capability_identity(provider_model: &str, source_model: &str) -> String {
    resolved_openai_model_identity(provider_model, source_model).1
}

pub(crate) fn openai_model_capability_is_opaque(provider_model: &str, source_model: &str) -> bool {
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

pub fn extract_gemini_model_from_path(path: &str) -> Option<String> {
    let marker = "/models/";
    let start = path.find(marker)? + marker.len();
    let tail = &path[start..];
    let end = tail.find(':').unwrap_or(tail.len());
    let model = tail[..end].trim();
    (!model.is_empty()).then(|| model.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        apply_model_directive_overrides_from_model, default_model_directive_suffixes,
        default_model_directives_config, parse_model_directive,
        parse_model_directive_with_suffixes, CodexReasoningPreset, ModelDirective,
        ModelDirectiveSuffixResolution, ModelOverride, ReasoningEffort, ServiceTier,
        MODEL_DIRECTIVE_API_FORMATS,
    };

    #[test]
    fn policy_suffix_parser_prefers_the_longest_configured_suffix() {
        assert_eq!(
            parse_model_directive_with_suffixes(
                "deployment-alias-VendorFuture",
                ["Future", "VendorFuture"],
            ),
            Some(ModelDirectiveSuffixResolution {
                base_model: "deployment-alias".to_string(),
                suffixes: vec!["VendorFuture".to_string()],
            })
        );
        assert_eq!(
            parse_model_directive_with_suffixes(
                "deployment-alias-high-VendorFuture",
                ["high", "VendorFuture"],
            ),
            Some(ModelDirectiveSuffixResolution {
                base_model: "deployment-alias".to_string(),
                suffixes: vec!["high".to_string(), "VendorFuture".to_string()],
            })
        );
    }

    #[test]
    fn policy_suffix_parser_rejects_duplicate_or_ambiguous_suffixes() {
        assert_eq!(
            parse_model_directive_with_suffixes("deployment-low-high", ["low", "high"]),
            None
        );
        assert_eq!(
            parse_model_directive_with_suffixes(
                "deployment-VendorFuture",
                ["VendorFuture", "vendorfuture"],
            ),
            None
        );
        assert_eq!(
            parse_model_directive_with_suffixes(
                "deployment-VendorFuture-VendorFuture",
                ["VendorFuture"],
            ),
            None
        );
    }

    #[test]
    fn policy_suffix_parser_does_not_strip_unconfigured_suffixes() {
        assert_eq!(
            parse_model_directive_with_suffixes("deployment-alias-VendorFuture", ["high"]),
            None
        );
    }

    #[test]
    fn parses_supported_reasoning_effort_suffixes() {
        let expected = [
            ("none", ReasoningEffort::None),
            ("minimal", ReasoningEffort::Minimal),
            ("low", ReasoningEffort::Low),
            ("medium", ReasoningEffort::Medium),
            ("high", ReasoningEffort::High),
            ("xhigh", ReasoningEffort::XHigh),
            ("max", ReasoningEffort::Max),
        ];
        for (suffix, effort) in expected {
            assert_eq!(
                parse_model_directive(&format!("gpt-5.6-sol-{suffix}")),
                Some(ModelDirective {
                    base_model: "gpt-5.6-sol".to_string(),
                    overrides: vec![ModelOverride::ReasoningEffort(effort)],
                })
            );
        }
        assert_eq!(
            parse_model_directive("gpt-5.4-xhigh"),
            Some(ModelDirective {
                base_model: "gpt-5.4".to_string(),
                overrides: vec![ModelOverride::ReasoningEffort(ReasoningEffort::XHigh)],
            })
        );
        assert_eq!(
            parse_model_directive("gpt-5.4-MAX"),
            Some(ModelDirective {
                base_model: "gpt-5.4".to_string(),
                overrides: vec![ModelOverride::ReasoningEffort(ReasoningEffort::Max)],
            })
        );
    }

    #[test]
    fn default_config_is_generated_from_the_shared_directive_contract() {
        let config = default_model_directives_config();
        for api_format in MODEL_DIRECTIVE_API_FORMATS {
            assert_eq!(
                config["reasoning_effort"]["api_formats"][api_format]["mappings"],
                json!({})
            );
            assert_eq!(
                config["reasoning_effort"]["api_formats"][api_format]["suffixes"],
                json!(default_model_directive_suffixes(api_format))
            );
        }
    }

    #[test]
    fn parses_supported_service_tier_suffixes() {
        assert_eq!(
            parse_model_directive("gpt-5.4-fast"),
            Some(ModelDirective {
                base_model: "gpt-5.4".to_string(),
                overrides: vec![ModelOverride::ServiceTier(ServiceTier::Priority)],
            })
        );
    }

    #[test]
    fn parses_combined_suffixes_in_canonical_order() {
        let expected = Some(ModelDirective {
            base_model: "gpt-5.4".to_string(),
            overrides: vec![
                ModelOverride::ReasoningEffort(ReasoningEffort::XHigh),
                ModelOverride::ServiceTier(ServiceTier::Priority),
            ],
        });
        assert_eq!(parse_model_directive("gpt-5.4-fast-xhigh"), expected);
        assert_eq!(parse_model_directive("gpt-5.4-xhigh-fast"), expected);
    }

    #[test]
    fn ignores_unknown_or_incomplete_suffixes() {
        assert_eq!(parse_model_directive("gpt-5.4-turbo"), None);
        assert_eq!(parse_model_directive("gpt-5.4"), None);
        assert_eq!(parse_model_directive("-high"), None);
        assert_eq!(parse_model_directive("gpt-5.4-high-json"), None);
        assert_eq!(parse_model_directive("gpt-5.4-low-high"), None);
    }

    #[test]
    fn parses_gpt_5_6_ultra_as_an_internal_reasoning_preset() {
        assert_eq!(
            parse_model_directive("gpt-5.6-sol-ultra"),
            Some(ModelDirective {
                base_model: "gpt-5.6-sol".to_string(),
                overrides: vec![ModelOverride::CodexReasoningPreset(
                    CodexReasoningPreset::Ultra,
                )],
            })
        );

        for model in [
            "gpt-5.6-ultra",
            "gpt-5.6-luna-ultra",
            "gpt-5.4-ultra",
            "gemini-ultra",
        ] {
            assert_eq!(parse_model_directive(model), None);
        }

        let mut unsupported = json!({"model": "gpt-5.4"});
        assert!(apply_model_directive_overrides_from_model(
            &mut unsupported,
            "openai:responses",
            "gpt-5.4",
            "gpt-5.4-ultra",
        )
        .is_none());
    }

    #[test]
    fn applies_reasoning_effort_to_provider_body_shapes() {
        let mut openai_chat = json!({"model": "gpt-5-upstream", "reasoning_effort": "low"});
        apply_model_directive_overrides_from_model(
            &mut openai_chat,
            "openai:chat",
            "gpt-5-upstream",
            "gpt-5.4-xhigh",
        )
        .expect("directive should apply");
        assert_eq!(openai_chat["reasoning_effort"], "xhigh");

        let mut responses = json!({
            "model": "gpt-5-upstream",
            "reasoning": {"effort": "low", "summary": "auto"}
        });
        apply_model_directive_overrides_from_model(
            &mut responses,
            "openai:responses",
            "gpt-5-upstream",
            "gpt-5.6-max",
        )
        .expect("directive should apply");
        assert_eq!(responses["reasoning"]["effort"], "max");
        assert_eq!(responses["reasoning"]["summary"], "auto");

        let mut compact = json!({
            "model": "gpt-5-upstream",
            "reasoning": {"effort": "low"}
        });
        apply_model_directive_overrides_from_model(
            &mut compact,
            "openai:responses:compact",
            "gpt-5-upstream",
            "gpt-5.6-max",
        )
        .expect("directive should apply");
        assert_eq!(compact["reasoning"]["effort"], "max");

        let mut openai_chat_max = json!({"model": "gpt-5-upstream", "reasoning_effort": "low"});
        apply_model_directive_overrides_from_model(
            &mut openai_chat_max,
            "openai:chat",
            "gpt-5-upstream",
            "gpt-5.6-max",
        )
        .expect("directive should apply");
        assert_eq!(openai_chat_max["reasoning_effort"], "max");

        let mut claude = json!({"model": "claude-sonnet-4-5"});
        apply_model_directive_overrides_from_model(
            &mut claude,
            "claude:messages",
            "claude-sonnet-4-5",
            "gpt-5.4-high",
        )
        .expect("directive should apply");
        assert_eq!(claude["thinking"]["budget_tokens"], 4096);

        let mut gemini = json!({});
        apply_model_directive_overrides_from_model(
            &mut gemini,
            "gemini:generate_content",
            "gemini-2.5-pro",
            "gpt-5.4-medium",
        )
        .expect("directive should apply");
        assert_eq!(
            gemini["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            2048
        );
    }

    #[test]
    fn max_suffix_is_capability_aware_for_openai_models() {
        for model in ["gpt-5.6", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            let mut body = json!({"model": model});
            apply_model_directive_overrides_from_model(
                &mut body,
                "openai:responses",
                model,
                &format!("{model}-max"),
            )
            .expect("max directive should apply");
            assert_eq!(body["reasoning"]["effort"], "max", "model: {model}");
        }

        let mut unsupported_model = json!({"model": "gpt-5.4"});
        let original = unsupported_model.clone();
        assert!(apply_model_directive_overrides_from_model(
            &mut unsupported_model,
            "openai:responses",
            "gpt-5.4",
            "gpt-5.4-max",
        )
        .is_none());
        assert_eq!(unsupported_model, original);

        let mut mapped_deployment = json!({"model": "azure-production"});
        apply_model_directive_overrides_from_model(
            &mut mapped_deployment,
            "openai:responses",
            "azure-production",
            "gpt-5.6-sol-max",
        )
        .expect("source model capability should survive provider model mapping");
        assert_eq!(mapped_deployment["reasoning"]["effort"], "max");

        let mut explicit_unsupported_target = json!({"model": "gpt-5.4"});
        let original = explicit_unsupported_target.clone();
        assert!(apply_model_directive_overrides_from_model(
            &mut explicit_unsupported_target,
            "openai:responses",
            "gpt-5.4",
            "gpt-5.6-sol-max",
        )
        .is_none());
        assert_eq!(explicit_unsupported_target, original);

        let mut unknown_future = json!({"model": "gpt-6.0"});
        let original = unknown_future.clone();
        assert!(apply_model_directive_overrides_from_model(
            &mut unknown_future,
            "openai:responses",
            "gpt-6.0",
            "gpt-6.0-max",
        )
        .is_none());
        assert_eq!(unknown_future, original);
    }

    #[test]
    fn openai_only_efforts_do_not_leak_into_cross_provider_mappings() {
        for effort in ["none", "minimal"] {
            for (api_format, provider_model) in [
                ("claude:messages", "claude-sonnet-4-6"),
                ("gemini:generate_content", "gemini-3-pro"),
            ] {
                let mut body = json!({"model": provider_model});
                let original = body.clone();
                assert!(apply_model_directive_overrides_from_model(
                    &mut body,
                    api_format,
                    provider_model,
                    &format!("gpt-5.6-sol-{effort}"),
                )
                .is_none());
                assert_eq!(body, original);
            }
        }
    }

    #[test]
    fn gpt_5_6_rejects_minimal_but_accepts_published_efforts() {
        for effort in ["none", "low", "medium", "high", "xhigh", "max"] {
            let mut body = json!({"model": "azure-production"});
            apply_model_directive_overrides_from_model(
                &mut body,
                "openai:responses",
                "azure-production",
                &format!("gpt-5.6-sol-{effort}"),
            )
            .expect("published GPT-5.6 effort should apply");
            assert_eq!(body["reasoning"]["effort"], effort);
        }

        let mut body = json!({"model": "azure-production"});
        let original = body.clone();
        assert!(apply_model_directive_overrides_from_model(
            &mut body,
            "openai:responses",
            "azure-production",
            "gpt-5.6-sol-minimal",
        )
        .is_none());
        assert_eq!(body, original);

        for family_variant in ["gpt-5.6-preview", "gpt-5.6-sol-2026-07-01"] {
            assert!(super::openai_model_supports_max_reasoning_effort(
                family_variant
            ));
            let mut body = json!({"model": family_variant});
            apply_model_directive_overrides_from_model(
                &mut body,
                "openai:responses",
                family_variant,
                &format!("{family_variant}-max"),
            )
            .expect("GPT-5.6 family variants should accept max");
            assert_eq!(body["reasoning"]["effort"], "max");
        }
    }

    #[test]
    fn prompt_cache_options_capability_requires_gpt_5_6_or_later() {
        for model in [
            "gpt-5.6",
            "gpt-5.6-sol",
            "gpt-5.7",
            "gpt-6",
            "openai/gpt-6.1-pro",
        ] {
            assert!(super::openai_model_supports_prompt_cache_options(model));
        }
        for model in ["gpt-5.5", "gpt-4o", "o3", "azure-production", "gpt-5.6.1"] {
            assert!(!super::openai_model_supports_prompt_cache_options(model));
        }
    }

    #[test]
    fn applies_fast_suffix_to_openai_service_tier() {
        let mut openai_chat = json!({"model": "gpt-5-upstream"});
        apply_model_directive_overrides_from_model(
            &mut openai_chat,
            "openai:chat",
            "gpt-5-upstream",
            "gpt-5.4-fast",
        )
        .expect("directive should apply");
        assert_eq!(openai_chat["service_tier"], "priority");

        let mut responses = json!({"model": "gpt-5-upstream"});
        apply_model_directive_overrides_from_model(
            &mut responses,
            "openai:responses",
            "gpt-5-upstream",
            "gpt-5.4-fast",
        )
        .expect("directive should apply");
        assert_eq!(responses["service_tier"], "priority");
    }

    #[test]
    fn applies_combined_suffixes_to_openai_body() {
        let mut openai_chat = json!({"model": "gpt-5-upstream", "reasoning_effort": "low"});
        apply_model_directive_overrides_from_model(
            &mut openai_chat,
            "openai:chat",
            "gpt-5-upstream",
            "gpt-5.4-fast-xhigh",
        )
        .expect("directive should apply");
        assert_eq!(openai_chat["reasoning_effort"], "xhigh");
        assert_eq!(openai_chat["service_tier"], "priority");

        let mut reversed = json!({"model": "gpt-5-upstream", "reasoning_effort": "low"});
        apply_model_directive_overrides_from_model(
            &mut reversed,
            "openai:chat",
            "gpt-5-upstream",
            "gpt-5.4-xhigh-fast",
        )
        .expect("directive should apply");
        assert_eq!(reversed, openai_chat);
    }

    #[test]
    fn unsupported_combined_suffix_leaves_body_unchanged() {
        let mut claude = json!({"model": "claude-sonnet-4-5"});
        let original = claude.clone();
        assert!(apply_model_directive_overrides_from_model(
            &mut claude,
            "claude:messages",
            "claude-sonnet-4-5",
            "gpt-5.4-fast-xhigh",
        )
        .is_none());
        assert_eq!(claude, original);
    }
}
