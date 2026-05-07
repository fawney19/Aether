use tracing::warn;

use crate::handlers::shared::system_config_bool;
use crate::state::AppState;

pub(crate) const ENABLE_MODEL_DIRECTIVES_CONFIG_KEY: &str = "enable_model_directives";
pub(crate) const MODEL_DIRECTIVES_CONFIG_KEY: &str = "model_directives";
const REASONING_EFFORT_DIRECTIVE_KEY: &str = "reasoning_effort";

pub(crate) async fn model_directives_enabled(state: &AppState) -> bool {
    match state
        .read_system_config_json_value(ENABLE_MODEL_DIRECTIVES_CONFIG_KEY)
        .await
    {
        Ok(value) => system_config_bool(value.as_ref(), false),
        Err(error) => {
            warn!(
                error = ?error,
                "gateway model directives config lookup failed"
            );
            false
        }
    }
}

pub(crate) async fn reasoning_model_directive_enabled(state: &AppState) -> bool {
    model_directives_enabled(state).await
        && read_reasoning_model_directive_settings(state)
            .await
            .map(|settings| settings.enabled())
            .unwrap_or(true)
}

pub(crate) async fn reasoning_model_directive_enabled_for_api_format(
    state: &AppState,
    api_format: &str,
) -> bool {
    if !model_directives_enabled(state).await {
        return false;
    }
    let settings = read_reasoning_model_directive_settings(state).await;
    let enabled = settings
        .as_ref()
        .map(|settings| settings.enabled())
        .unwrap_or(true);
    if !enabled {
        return false;
    }

    let api_format = crate::ai_serving::normalize_api_format_alias(api_format);
    if api_format.is_empty() {
        return false;
    }

    settings
        .as_ref()
        .and_then(|settings| settings.api_format_enabled(&api_format))
        .unwrap_or(true)
}

pub(crate) async fn reasoning_model_directive_enabled_for_api_format_and_model(
    state: &AppState,
    api_format: &str,
    requested_model: Option<&str>,
) -> bool {
    if !model_directives_enabled(state).await {
        return false;
    }
    let settings = read_reasoning_model_directive_settings(state).await;
    let enabled = settings
        .as_ref()
        .map(|settings| settings.enabled())
        .unwrap_or(true);
    if !enabled {
        return false;
    }

    let api_format = crate::ai_serving::normalize_api_format_alias(api_format);
    if api_format.is_empty() {
        return false;
    }

    let api_format_enabled = settings
        .as_ref()
        .and_then(|settings| settings.api_format_enabled(&api_format))
        .unwrap_or(true);
    if !api_format_enabled {
        return false;
    }

    let Some(suffix) = requested_model.and_then(reasoning_suffix_from_model) else {
        return false;
    };

    settings
        .as_ref()
        .and_then(|settings| settings.api_format_mapping(&api_format, &suffix))
        .or_else(|| default_reasoning_mapping(&api_format, &suffix))
        .is_some()
}

pub(crate) async fn reasoning_model_directive_mapping_for_api_format_and_model(
    state: &AppState,
    api_format: &str,
    requested_model: Option<&str>,
) -> Option<serde_json::Value> {
    if !reasoning_model_directive_enabled_for_api_format_and_model(
        state,
        api_format,
        requested_model,
    )
    .await
    {
        return None;
    }
    let suffix = requested_model.and_then(reasoning_suffix_from_model)?;
    let api_format = crate::ai_serving::normalize_api_format_alias(api_format);
    let settings = read_reasoning_model_directive_settings(state).await;
    settings
        .as_ref()
        .and_then(|settings| settings.api_format_mapping(&api_format, &suffix))
        .or_else(|| default_reasoning_mapping(&api_format, &suffix))
}

const DEFAULT_REASONING_SUFFIXES: &[&str] = &["low", "medium", "high", "xhigh", "max"];

#[derive(Debug, Clone, Default)]
struct ReasoningModelDirectiveSettings {
    enabled: Option<bool>,
    api_formats: Option<serde_json::Map<String, serde_json::Value>>,
}

impl ReasoningModelDirectiveSettings {
    fn enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    fn api_format_enabled(&self, api_format: &str) -> Option<bool> {
        let api_formats = self.api_formats.as_ref()?;
        api_formats.iter().find_map(|(key, value)| {
            if crate::ai_serving::normalize_api_format_alias(key) != api_format {
                return None;
            }
            Some(match value {
                serde_json::Value::Object(object) => object
                    .get("enabled")
                    .map(|value| system_config_bool(Some(value), true))
                    .unwrap_or(true),
                _ => system_config_bool(Some(value), true),
            })
        })
    }

    fn api_format_mappings(
        &self,
        api_format: &str,
    ) -> Option<serde_json::Map<String, serde_json::Value>> {
        let api_formats = self.api_formats.as_ref()?;
        api_formats.iter().find_map(|(key, value)| {
            if crate::ai_serving::normalize_api_format_alias(key) != api_format {
                return None;
            }
            let object = value.as_object()?;
            if let Some(mappings) = object
                .get("mappings")
                .and_then(serde_json::Value::as_object)
            {
                return Some(normalize_reasoning_mappings(mappings));
            }
            let mappings = object
                .get("suffixes")?
                .as_array()?
                .iter()
                .filter_map(|value| value.as_str())
                .filter_map(normalize_reasoning_suffix)
                .filter_map(|suffix| {
                    default_reasoning_mapping(api_format, &suffix).map(|mapping| (suffix, mapping))
                })
                .collect::<serde_json::Map<_, _>>();
            Some(mappings)
        })
    }

    fn api_format_mapping(&self, api_format: &str, suffix: &str) -> Option<serde_json::Value> {
        self.api_format_mappings(api_format)
            .and_then(|mappings| mappings.get(suffix).cloned())
    }
}

fn reasoning_suffix_from_model(model: &str) -> Option<String> {
    let model = model.trim();
    let (base_model, suffix) = model.rsplit_once('-')?;
    if base_model.trim().is_empty() {
        return None;
    }
    normalize_reasoning_suffix(suffix)
}

fn normalize_reasoning_suffix(suffix: &str) -> Option<String> {
    let normalized = suffix.trim().to_ascii_lowercase();
    DEFAULT_REASONING_SUFFIXES
        .contains(&normalized.as_str())
        .then_some(normalized)
}

fn normalize_reasoning_mappings(
    mappings: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    mappings
        .iter()
        .filter_map(|(suffix, mapping)| {
            normalize_reasoning_suffix(suffix).map(|suffix| (suffix, mapping.clone()))
        })
        .collect()
}

fn default_reasoning_mapping(api_format: &str, suffix: &str) -> Option<serde_json::Value> {
    match api_format {
        "openai:chat" => {
            let effort = openai_reasoning_effort_value(suffix)?;
            Some(serde_json::json!({ "reasoning_effort": effort }))
        }
        "openai:responses" | "openai:responses:compact" => {
            let effort = openai_reasoning_effort_value(suffix)?;
            Some(serde_json::json!({ "reasoning": { "effort": effort } }))
        }
        "claude:messages" => Some(serde_json::json!({
            "thinking": {
                "type": "enabled",
                "budget_tokens": match suffix {
                    "low" => 1024,
                    "medium" => 4096,
                    "high" => 8192,
                    "xhigh" => 16384,
                    "max" => 32768,
                    _ => return None,
                }
            }
        })),
        "gemini:generate_content" => Some(serde_json::json!({
            "generationConfig": {
                "thinkingConfig": {
                    "thinkingBudget": match suffix {
                        "low" => 1024,
                        "medium" => 4096,
                        "high" => 8192,
                        "xhigh" => 16384,
                        "max" => -1,
                        _ => return None,
                    }
                }
            }
        })),
        _ => None,
    }
}

fn openai_reasoning_effort_value(suffix: &str) -> Option<&'static str> {
    match suffix {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" | "max" => Some("xhigh"),
        _ => None,
    }
}

async fn read_reasoning_model_directive_settings(
    state: &AppState,
) -> Option<ReasoningModelDirectiveSettings> {
    match state
        .read_system_config_json_value(MODEL_DIRECTIVES_CONFIG_KEY)
        .await
    {
        Ok(value) => parse_reasoning_model_directive_settings(value.as_ref()),
        Err(error) => {
            warn!(
                error = ?error,
                "gateway model directives detail config lookup failed"
            );
            None
        }
    }
}

fn parse_reasoning_model_directive_settings(
    value: Option<&serde_json::Value>,
) -> Option<ReasoningModelDirectiveSettings> {
    let root = value?.as_object()?;
    let reasoning = root.get(REASONING_EFFORT_DIRECTIVE_KEY)?.as_object()?;
    Some(ReasoningModelDirectiveSettings {
        enabled: reasoning
            .get("enabled")
            .map(|value| system_config_bool(Some(value), true)),
        api_formats: reasoning
            .get("api_formats")
            .and_then(serde_json::Value::as_object)
            .cloned(),
    })
}

#[cfg(test)]
mod tests {
    use super::parse_reasoning_model_directive_settings;
    use serde_json::json;

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
                            "max": { "thinking": { "type": "enabled", "budget_tokens": 32768 } }
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
            settings.api_format_mapping("claude:messages", "max"),
            Some(json!({ "thinking": { "type": "enabled", "budget_tokens": 32768 } }))
        );
        assert_eq!(settings.api_format_enabled("gemini:generate_content"), None);
    }
}
