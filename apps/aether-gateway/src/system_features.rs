use std::collections::BTreeSet;
use std::sync::Arc;

use sha2::Digest as _;
use tracing::warn;

use crate::ai_serving::{
    default_model_directive_mapping_patch, default_model_directive_suffixes,
    model_directive_builtin_suffix_supported_for_source_model,
    model_directive_suffix_has_builtin_mapping, parse_model_directive,
    parse_model_directive_with_suffixes, ReasoningEffort, ServiceTier, MODEL_DIRECTIVE_API_FORMATS,
};

use crate::handlers::shared::system_config_bool;
use crate::state::AppState;

pub(crate) const ENABLE_MODEL_DIRECTIVES_CONFIG_KEY: &str = "enable_model_directives";
pub(crate) const MODEL_DIRECTIVES_CONFIG_KEY: &str = "model_directives";
const REASONING_EFFORT_DIRECTIVE_KEY: &str = "reasoning_effort";

#[derive(Debug, Clone)]
pub(crate) struct ModelDirectivePolicySnapshot {
    policy: Arc<ModelDirectivePolicy>,
}

#[derive(Debug)]
struct ModelDirectivePolicy {
    directives_enabled: bool,
    reasoning_settings: Option<ReasoningModelDirectiveSettings>,
    cache_key: String,
}

impl Default for ModelDirectivePolicySnapshot {
    fn default() -> Self {
        Self::from_config_values(None, None)
    }
}

impl ModelDirectivePolicySnapshot {
    pub(crate) async fn load(state: &AppState) -> Self {
        let (enabled, settings) = tokio::join!(
            state.read_system_config_json_value(ENABLE_MODEL_DIRECTIVES_CONFIG_KEY),
            state.read_system_config_json_value(MODEL_DIRECTIVES_CONFIG_KEY),
        );
        if let Err(error) = &enabled {
            warn!(
                error = ?error,
                "gateway model directives config lookup failed"
            );
        }
        if let Err(error) = &settings {
            warn!(
                error = ?error,
                "gateway model directives detail config lookup failed"
            );
        }
        model_directive_policy_snapshot_from_config_reads(enabled, settings)
    }

    pub(crate) fn from_config_values(
        enabled: Option<&serde_json::Value>,
        settings: Option<&serde_json::Value>,
    ) -> Self {
        let directives_enabled = system_config_bool(enabled, false);
        let reasoning_settings = parse_reasoning_model_directive_settings(settings);
        let cache_key =
            model_directive_policy_cache_key(directives_enabled, reasoning_settings.as_ref());
        Self {
            policy: Arc::new(ModelDirectivePolicy {
                directives_enabled,
                reasoning_settings,
                cache_key,
            }),
        }
    }

    pub(crate) fn cache_key(&self) -> &str {
        self.policy.cache_key.as_str()
    }

    pub(crate) fn reasoning_enabled(&self) -> bool {
        self.policy.directives_enabled
            && self
                .policy
                .reasoning_settings
                .as_ref()
                .map(ReasoningModelDirectiveSettings::enabled)
                .unwrap_or(true)
    }

    pub(crate) fn resolve_reasoning(
        &self,
        api_format: &str,
        requested_model: Option<&str>,
    ) -> ReasoningModelDirectiveResolution {
        if !self.reasoning_enabled() {
            return ReasoningModelDirectiveResolution::default();
        }

        let api_format = crate::ai_serving::normalize_api_format_alias(api_format);
        if api_format.is_empty()
            || !self
                .policy
                .reasoning_settings
                .as_ref()
                .and_then(|settings| settings.api_format_enabled(&api_format))
                .unwrap_or(true)
        {
            return ReasoningModelDirectiveResolution::default();
        }

        let enabled_suffixes = self
            .policy
            .reasoning_settings
            .as_ref()
            .and_then(|settings| settings.api_format_suffixes(&api_format));
        let mappings = self
            .policy
            .reasoning_settings
            .as_ref()
            .and_then(|settings| settings.api_format_mappings(&api_format));
        let default_suffixes;
        let suffixes = match enabled_suffixes {
            Some(suffixes) => suffixes
                .iter()
                .filter(|suffix| {
                    model_directive_suffix_has_builtin_mapping(suffix)
                        || mappings.is_some_and(|mappings| mappings.contains_key(*suffix))
                })
                .map(String::as_str)
                .collect::<Vec<_>>(),
            None => {
                default_suffixes = default_model_directive_suffixes(&api_format);
                default_suffixes.to_vec()
            }
        };
        let Some(directive) = requested_model
            .and_then(|model| parse_model_directive_with_suffixes(model, suffixes.iter().copied()))
        else {
            return ReasoningModelDirectiveResolution::default();
        };
        if directive.suffixes.iter().any(|suffix| {
            model_directive_suffix_has_builtin_mapping(suffix)
                && !model_directive_builtin_suffix_supported_for_source_model(
                    suffix,
                    &directive.base_model,
                )
        }) {
            return ReasoningModelDirectiveResolution::default();
        }

        ReasoningModelDirectiveResolution {
            enabled: true,
            api_format,
            base_model: directive.base_model,
            suffixes: directive.suffixes.clone(),
            custom_mapping: model_directive_mapping_for_suffixes(&directive.suffixes, mappings),
        }
    }
}

fn model_directive_policy_snapshot_from_config_reads<E>(
    enabled: Result<Option<serde_json::Value>, E>,
    settings: Result<Option<serde_json::Value>, E>,
) -> ModelDirectivePolicySnapshot {
    match (enabled, settings) {
        (Ok(enabled), Ok(settings)) => {
            ModelDirectivePolicySnapshot::from_config_values(enabled.as_ref(), settings.as_ref())
        }
        _ => ModelDirectivePolicySnapshot::default(),
    }
}

fn model_directive_policy_cache_key(
    directives_enabled: bool,
    settings: Option<&ReasoningModelDirectiveSettings>,
) -> String {
    let mut hasher = sha2::Sha256::new();
    let policy_value = serde_json::to_value((directives_enabled, settings))
        .expect("parsed model directive policy should always serialize");
    let serialized = serde_json::to_vec(&canonicalize_policy_json(policy_value))
        .expect("canonical model directive policy should always serialize");
    hasher.update(serialized);
    format!("{:x}", hasher.finalize())
}

fn canonicalize_policy_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => {
            let mut entries = object.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_policy_json(value)))
                    .collect(),
            )
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonicalize_policy_json).collect())
        }
        value => value,
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ReasoningModelDirectiveResolution {
    enabled: bool,
    api_format: String,
    base_model: String,
    suffixes: Vec<String>,
    custom_mapping: Option<serde_json::Value>,
}

impl ReasoningModelDirectiveResolution {
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn base_model(&self) -> Option<&str> {
        self.enabled.then_some(self.base_model.as_str())
    }

    pub(crate) fn mapping_patch_for_mapped_model(
        &self,
        mapped_model: &str,
    ) -> Result<Option<serde_json::Value>, &'static str> {
        if !self.enabled {
            return Ok(None);
        }

        let mut patch = serde_json::json!({});
        let mut has_patch = false;
        for suffix in &self.suffixes {
            if model_directive_suffix_has_builtin_mapping(suffix) {
                let Some(builtin_patch) = default_model_directive_mapping_patch(
                    &self.api_format,
                    mapped_model,
                    &self.base_model,
                    suffix,
                ) else {
                    return Err("model_directive_target_unsupported");
                };
                deep_merge_json(&mut patch, &builtin_patch);
                has_patch = true;
            }
        }
        if let Some(custom_mapping) = &self.custom_mapping {
            deep_merge_json(&mut patch, custom_mapping);
            has_patch = true;
        }
        if !has_patch {
            return Err("model_directive_mapping_missing");
        }
        Ok(Some(patch))
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
struct ReasoningModelDirectiveSettings {
    enabled: Option<bool>,
    api_formats: Vec<ReasoningApiFormatSettings>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct ReasoningApiFormatSettings {
    api_format: String,
    enabled: bool,
    suffixes: Option<BTreeSet<String>>,
    mappings: Option<serde_json::Map<String, serde_json::Value>>,
}

impl ReasoningModelDirectiveSettings {
    fn enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    fn api_format_enabled(&self, api_format: &str) -> Option<bool> {
        self.api_format_settings(api_format)
            .map(|settings| settings.enabled)
    }

    fn api_format_mappings(
        &self,
        api_format: &str,
    ) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.api_format_settings(api_format)?.mappings.as_ref()
    }

    fn api_format_suffixes(&self, api_format: &str) -> Option<&BTreeSet<String>> {
        self.api_format_settings(api_format)?.suffixes.as_ref()
    }

    fn api_format_settings(&self, api_format: &str) -> Option<&ReasoningApiFormatSettings> {
        self.api_formats
            .iter()
            .find(|settings| settings.api_format == api_format)
    }
}

fn model_directive_suffixes_from_model(model: &str) -> Option<Vec<String>> {
    Some(
        parse_model_directive(model)?
            .overrides
            .iter()
            .map(|override_item| override_item.suffix().to_string())
            .collect(),
    )
}

fn normalize_model_directive_suffix(suffix: &str) -> Option<String> {
    let suffix = suffix.trim();
    if suffix.is_empty() {
        return None;
    }
    ReasoningEffort::parse(suffix)
        .map(|effort| effort.as_str().to_string())
        .or_else(|| ServiceTier::parse(suffix).map(|tier| tier.as_directive_suffix().to_string()))
        .or_else(|| Some(suffix.to_string()))
}

fn normalize_configured_suffixes<'a>(suffixes: impl Iterator<Item = &'a str>) -> BTreeSet<String> {
    suffixes
        .filter_map(normalize_model_directive_suffix)
        .collect()
}

fn normalize_reasoning_mappings(
    mappings: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut normalized = serde_json::Map::new();
    for (suffix, mapping) in mappings {
        let Some(suffix) = normalize_model_directive_suffix(suffix) else {
            continue;
        };
        normalized.insert(suffix, mapping.clone());
    }
    normalized
}

fn default_reasoning_mapping(api_format: &str, suffix: &str) -> Option<serde_json::Value> {
    default_reasoning_mapping_for_model(api_format, "", suffix)
}

fn default_reasoning_mapping_for_model(
    api_format: &str,
    model: &str,
    suffix: &str,
) -> Option<serde_json::Value> {
    default_model_directive_mapping_patch(api_format, model, model, suffix)
}

fn model_directive_mapping_for_suffixes(
    suffixes: &[String],
    mappings: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<serde_json::Value> {
    let mut combined = serde_json::json!({});
    let mut has_custom_mapping = false;
    for suffix in suffixes {
        if let Some(mapping) = mappings.and_then(|mappings| mappings.get(suffix)) {
            deep_merge_json(&mut combined, mapping);
            has_custom_mapping = true;
        }
    }
    has_custom_mapping.then_some(combined)
}

fn deep_merge_json(target: &mut serde_json::Value, patch: &serde_json::Value) {
    match (target, patch) {
        (serde_json::Value::Object(target_object), serde_json::Value::Object(patch_object)) => {
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

fn parse_reasoning_model_directive_settings(
    value: Option<&serde_json::Value>,
) -> Option<ReasoningModelDirectiveSettings> {
    let root = value?.as_object()?;
    let reasoning = root.get(REASONING_EFFORT_DIRECTIVE_KEY)?.as_object()?;
    let mut api_formats = reasoning
        .get("api_formats")
        .and_then(serde_json::Value::as_object)
        .map(|formats| {
            formats
                .iter()
                .filter_map(|(api_format, value)| {
                    let api_format = crate::ai_serving::normalize_api_format_alias(api_format);
                    if !MODEL_DIRECTIVE_API_FORMATS.contains(&api_format.as_str()) {
                        return None;
                    }
                    let Some(object) = value.as_object() else {
                        return Some(ReasoningApiFormatSettings {
                            api_format,
                            enabled: system_config_bool(Some(value), true),
                            suffixes: None,
                            mappings: None,
                        });
                    };
                    let configured_mappings = object
                        .get("mappings")
                        .and_then(serde_json::Value::as_object);
                    let configured_suffixes =
                        object.get("suffixes").and_then(serde_json::Value::as_array);
                    let suffixes = configured_suffixes
                        .map(|suffixes| {
                            normalize_configured_suffixes(
                                suffixes.iter().filter_map(serde_json::Value::as_str),
                            )
                        })
                        .or_else(|| {
                            configured_mappings.map(|mappings| {
                                normalize_configured_suffixes(mappings.keys().map(String::as_str))
                            })
                        });
                    let mappings = configured_mappings
                        .map(normalize_reasoning_mappings)
                        .or_else(|| configured_suffixes.map(|_| serde_json::Map::new()));
                    Some(ReasoningApiFormatSettings {
                        api_format,
                        enabled: object
                            .get("enabled")
                            .map(|value| system_config_bool(Some(value), true))
                            .unwrap_or(true),
                        suffixes,
                        mappings,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    api_formats.sort_by(|left, right| left.api_format.cmp(&right.api_format));
    Some(ReasoningModelDirectiveSettings {
        enabled: reasoning
            .get("enabled")
            .map(|value| system_config_bool(Some(value), true)),
        api_formats,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        default_reasoning_mapping, default_reasoning_mapping_for_model,
        model_directive_mapping_for_suffixes, model_directive_policy_snapshot_from_config_reads,
        model_directive_suffixes_from_model, parse_reasoning_model_directive_settings,
        ModelDirectivePolicySnapshot,
    };
    use serde_json::json;

    #[test]
    fn model_directive_policy_snapshot_is_immutable_after_source_values_change() {
        let enabled = json!(true);
        let mut settings = json!({
            "reasoning_effort": {
                "enabled": true,
                "api_formats": {
                    "openai:responses": {
                        "suffixes": ["low"]
                    }
                }
            }
        });
        let snapshot =
            ModelDirectivePolicySnapshot::from_config_values(Some(&enabled), Some(&settings));
        let cloned_snapshot = snapshot.clone();
        assert!(std::sync::Arc::ptr_eq(
            &snapshot.policy,
            &cloned_snapshot.policy,
        ));
        let equivalent_snapshot = ModelDirectivePolicySnapshot::from_config_values(
            Some(&json!("true")),
            Some(&json!({
                "reasoning_effort": {
                    "enabled": "true",
                    "api_formats": {
                        "OPENAI:RESPONSES": {
                            "suffixes": ["LOW"]
                        }
                    }
                }
            })),
        );
        assert_eq!(snapshot.cache_key(), equivalent_snapshot.cache_key());

        settings["reasoning_effort"]["api_formats"]["openai:responses"]["suffixes"] =
            json!(["high"]);
        let updated_snapshot =
            ModelDirectivePolicySnapshot::from_config_values(Some(&enabled), Some(&settings));
        assert_ne!(snapshot.cache_key(), updated_snapshot.cache_key());

        assert!(snapshot
            .resolve_reasoning("openai:responses", Some("gpt-5.6-sol-low"))
            .enabled());
        assert!(!snapshot
            .resolve_reasoning("openai:responses", Some("gpt-5.6-sol-high"))
            .enabled());
    }

    #[test]
    fn model_directive_policy_config_read_errors_fail_closed_but_absence_keeps_defaults() {
        let missing_details = model_directive_policy_snapshot_from_config_reads::<&str>(
            Ok(Some(json!(true))),
            Ok(None),
        );
        assert!(missing_details.reasoning_enabled());
        assert!(missing_details
            .resolve_reasoning("openai:responses", Some("gpt-5.6-sol-low"))
            .enabled());

        let detail_error = model_directive_policy_snapshot_from_config_reads(
            Ok(Some(json!(true))),
            Err("detail config unavailable"),
        );
        assert!(!detail_error.reasoning_enabled());
        assert!(!detail_error
            .resolve_reasoning("openai:responses", Some("gpt-5.6-sol-low"))
            .enabled());
        assert_eq!(
            detail_error.cache_key(),
            ModelDirectivePolicySnapshot::default().cache_key()
        );

        let master_error = model_directive_policy_snapshot_from_config_reads(
            Err("master config unavailable"),
            Ok(Some(json!({
                "reasoning_effort": {
                    "enabled": true
                }
            }))),
        );
        assert!(!master_error.reasoning_enabled());
    }

    #[test]
    fn reasoning_model_directive_settings_parse_endpoint_flags() {
        let value = json!({
            "reasoning_effort": {
                "enabled": true,
                "api_formats": {
                    "openai:chat": false,
                    "CLAUDE:MESSAGES": {
                        "enabled": true,
                        "mappings": {
                            "high": { "thinking": { "type": "enabled", "budget_tokens": 8192 } },
                            "max": { "thinking": { "type": "enabled", "budget_tokens": 65536 } }
                        }
                    }
                }
            }
        });

        let settings =
            parse_reasoning_model_directive_settings(Some(&value)).expect("settings should parse");

        assert!(settings.enabled());
        assert_eq!(settings.api_format_enabled("openai:chat"), Some(false));
        assert_eq!(settings.api_format_enabled("claude:messages"), Some(true));
        assert_eq!(
            settings
                .api_format_mappings("claude:messages")
                .and_then(|mappings| mappings.get("max").cloned()),
            Some(json!({ "thinking": { "type": "enabled", "budget_tokens": 65536 } }))
        );
        assert_eq!(settings.api_format_enabled("gemini:generate_content"), None);
    }

    #[test]
    fn configured_mappings_remain_authoritative() {
        let value = json!({
            "reasoning_effort": {
                "api_formats": {
                    "openai:chat": {
                        "suffixes": ["low", "max"]
                    },
                    "openai:responses": {
                        "mappings": {
                            "low": { "reasoning": { "effort": "low" } },
                            "max": { "reasoning": { "effort": "xhigh" } }
                        }
                    },
                    "claude:messages": {
                        "mappings": {
                            "low": { "thinking": { "type": "enabled", "budget_tokens": 1024 } },
                            "high": { "thinking": { "type": "enabled", "budget_tokens": 7777 } }
                        }
                    }
                }
            }
        });

        let settings =
            parse_reasoning_model_directive_settings(Some(&value)).expect("settings should parse");
        assert!(settings
            .api_format_mappings("openai:chat")
            .expect("chat mappings")
            .is_empty());
        assert_eq!(
            settings
                .api_format_suffixes("openai:chat")
                .expect("chat suffixes")
                .iter()
                .collect::<Vec<_>>(),
            vec!["low", "max"]
        );
        let responses = settings
            .api_format_mappings("openai:responses")
            .expect("responses mappings");
        assert_eq!(
            responses["low"],
            json!({ "reasoning": { "effort": "low" } })
        );
        assert_eq!(
            responses["max"],
            json!({ "reasoning": { "effort": "xhigh" } })
        );

        let claude = settings
            .api_format_mappings("claude:messages")
            .expect("Claude mappings");
        assert_eq!(
            claude["low"],
            json!({ "thinking": { "type": "enabled", "budget_tokens": 1024 } })
        );
        assert_eq!(claude["high"]["thinking"]["budget_tokens"], 7777);
    }

    #[test]
    fn default_fast_suffix_maps_to_openai_priority_service_tier() {
        assert_eq!(
            default_reasoning_mapping("openai:chat", "fast"),
            Some(json!({ "service_tier": "priority" }))
        );
        assert_eq!(
            default_reasoning_mapping("openai:responses", "fast"),
            Some(json!({ "service_tier": "priority" }))
        );
        assert_eq!(
            default_reasoning_mapping("openai:responses:compact", "fast"),
            Some(json!({ "service_tier": "priority" }))
        );
        assert_eq!(default_reasoning_mapping("claude:messages", "fast"), None);
        assert_eq!(
            default_reasoning_mapping("gemini:generate_content", "fast"),
            None
        );
    }

    #[test]
    fn openai_reasoning_defaults_cover_the_complete_effort_contract() {
        for effort in ["none", "minimal", "low", "medium", "high", "xhigh"] {
            assert_eq!(
                default_reasoning_mapping("openai:chat", effort),
                Some(json!({ "reasoning_effort": effort }))
            );
            assert_eq!(
                default_reasoning_mapping("openai:responses", effort),
                Some(json!({ "reasoning": { "effort": effort } }))
            );
        }
        assert_eq!(
            default_reasoning_mapping("openai:chat", "max"),
            Some(json!({ "reasoning_effort": "max" }))
        );
        assert_eq!(
            default_reasoning_mapping("openai:responses", "max"),
            Some(json!({ "reasoning": { "effort": "max" } }))
        );
        assert_eq!(
            default_reasoning_mapping_for_model("openai:chat", "gpt-5.6-sol", "max"),
            Some(json!({ "reasoning_effort": "max" }))
        );
        assert_eq!(
            default_reasoning_mapping_for_model("openai:responses", "gpt-5.6-sol", "max"),
            Some(json!({ "reasoning": { "effort": "max" } }))
        );
        assert_eq!(
            default_reasoning_mapping_for_model("openai:responses", "gpt-5.4", "max"),
            None
        );
    }

    #[test]
    fn parses_global_reasoning_suffix_vocabulary() {
        for effort in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
            assert_eq!(
                model_directive_suffixes_from_model(&format!("gpt-5.6-sol-{effort}")),
                Some(vec![effort.to_string()])
            );
        }
    }

    #[test]
    fn combined_suffixes_are_order_insensitive() {
        let expected = Some(vec!["xhigh".to_string(), "fast".to_string()]);
        assert_eq!(
            model_directive_suffixes_from_model("gpt-5.4-fast-xhigh"),
            expected
        );
        assert_eq!(
            model_directive_suffixes_from_model("gpt-5.4-xhigh-fast"),
            expected
        );
        assert_eq!(
            model_directive_mapping_for_suffixes(
                expected.as_ref().expect("suffixes should parse"),
                None,
            ),
            None
        );

        let custom = json!({
            "xhigh": { "reasoning_effort": "high" },
            "fast": { "service_tier": "default" }
        });
        assert_eq!(
            model_directive_mapping_for_suffixes(
                expected.as_ref().expect("suffixes should parse"),
                custom.as_object(),
            ),
            Some(json!({
                "reasoning_effort": "high",
                "service_tier": "default"
            }))
        );
    }

    #[test]
    fn mapped_model_controls_builtin_reasoning_capability() {
        let snapshot = ModelDirectivePolicySnapshot::from_config_values(Some(&json!(true)), None);
        let resolution =
            snapshot.resolve_reasoning("openai:responses", Some("deployment-alias-max"));

        assert_eq!(resolution.base_model(), Some("deployment-alias"));
        assert_eq!(
            resolution.mapping_patch_for_mapped_model("gpt-5.6-sol"),
            Ok(Some(json!({ "reasoning": { "effort": "max" } })))
        );
        assert_eq!(
            resolution.mapping_patch_for_mapped_model("gpt-5.4"),
            Err("model_directive_target_unsupported")
        );

        let known_source = snapshot.resolve_reasoning("openai:responses", Some("gpt-5.6-sol-max"));
        assert_eq!(
            known_source.mapping_patch_for_mapped_model("azure-production"),
            Ok(Some(json!({ "reasoning": { "effort": "max" } })))
        );

        let unsupported_source =
            snapshot.resolve_reasoning("openai:responses", Some("gpt-5.4-max"));
        assert_eq!(
            unsupported_source.mapping_patch_for_mapped_model("azure-production"),
            Err("model_directive_target_unsupported")
        );
    }

    #[test]
    fn codex_ultra_uses_the_builtin_policy_path_for_default_and_explicit_settings() {
        let snapshots = [
            ModelDirectivePolicySnapshot::from_config_values(Some(&json!(true)), None),
            ModelDirectivePolicySnapshot::from_config_values(
                Some(&json!(true)),
                Some(&json!({
                    "reasoning_effort": {
                        "enabled": true,
                        "api_formats": {
                            "openai:responses": {
                                "enabled": true,
                                "suffixes": ["ultra"],
                                "mappings": {}
                            }
                        }
                    }
                })),
            ),
        ];

        for snapshot in snapshots {
            for model in ["gpt-5.6-sol", "gpt-5.6-terra"] {
                let requested_model = format!("{model}-ultra");
                let resolution =
                    snapshot.resolve_reasoning("openai:responses", Some(&requested_model));
                assert_eq!(resolution.base_model(), Some(model));
                assert_eq!(
                    resolution.mapping_patch_for_mapped_model(model),
                    Ok(Some(json!({ "reasoning": { "effort": "ultra" } })))
                );
            }

            for requested_model in [
                "gpt-5.6-ultra",
                "gpt-5.6-luna-ultra",
                "gpt-5.4-ultra",
                "other-model-ultra",
            ] {
                assert!(!snapshot
                    .resolve_reasoning("openai:responses", Some(requested_model))
                    .enabled());
            }
        }
    }

    #[test]
    fn custom_suffix_mapping_defines_an_executable_policy_directive() {
        let snapshot = ModelDirectivePolicySnapshot::from_config_values(
            Some(&json!(true)),
            Some(&json!({
                "reasoning_effort": {
                    "api_formats": {
                        "openai:responses": {
                            "suffixes": ["Future", "VendorFuture"],
                            "mappings": {
                                "VendorFuture": {
                                    "reasoning": { "context": "all_turns" }
                                }
                            }
                        }
                    }
                }
            })),
        );
        let resolution =
            snapshot.resolve_reasoning("openai:responses", Some("deployment-alias-VendorFuture"));

        assert_eq!(resolution.base_model(), Some("deployment-alias"));
        assert_eq!(
            resolution.mapping_patch_for_mapped_model("gpt-5.6-sol"),
            Ok(Some(json!({
                "reasoning": { "context": "all_turns" }
            })))
        );
        assert!(!snapshot
            .resolve_reasoning("openai:responses", Some("deployment-alias-Future"),)
            .enabled());
    }

    #[test]
    fn configured_mapping_keys_define_suffixes_and_overrides() {
        let value = json!({
            "reasoning_effort": {
                "api_formats": {
                    "openai:chat": {
                        "mappings": {
                            "low": { "reasoning_effort": "low" },
                            "medium": { "reasoning_effort": "medium" },
                            "high": { "reasoning_effort": "high" },
                            "xhigh": { "reasoning_effort": "xhigh" },
                            "MAX": { "reasoning_effort": "xhigh" },
                            "fast": { "service_tier": "priority" },
                            "VendorFuture": { "keep": true }
                        }
                    }
                }
            }
        });
        let settings =
            parse_reasoning_model_directive_settings(Some(&value)).expect("settings should parse");
        let suffixes = settings
            .api_format_suffixes("openai:chat")
            .expect("chat suffixes");
        assert_eq!(
            suffixes.iter().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "VendorFuture",
                "fast",
                "high",
                "low",
                "max",
                "medium",
                "xhigh"
            ]
        );
        let mappings = settings
            .api_format_mappings("openai:chat")
            .expect("chat mappings");
        assert_eq!(mappings.len(), 7);
        assert_eq!(mappings["max"], json!({ "reasoning_effort": "xhigh" }));
        assert_eq!(mappings["VendorFuture"], json!({ "keep": true }));
    }

    #[test]
    fn explicit_openai_suffix_allowlist_never_reenables_disabled_efforts() {
        let cases = [
            vec!["minimal", "low", "medium", "high", "xhigh", "max", "fast"],
            vec!["none", "low", "medium", "high", "xhigh", "max", "fast"],
            vec!["low", "medium", "high", "xhigh", "max", "fast"],
        ];

        for expected in cases {
            let expected_suffixes = expected
                .iter()
                .copied()
                .map(str::to_string)
                .collect::<std::collections::BTreeSet<_>>();
            let value = json!({
                "reasoning_effort": {
                    "api_formats": {
                        "openai:responses": {
                            "suffixes": expected
                        }
                    }
                }
            });
            let settings = parse_reasoning_model_directive_settings(Some(&value))
                .expect("settings should parse");
            let suffixes = settings
                .api_format_suffixes("openai:responses")
                .expect("Responses suffixes");

            assert_eq!(suffixes, &expected_suffixes);
        }
    }
}
