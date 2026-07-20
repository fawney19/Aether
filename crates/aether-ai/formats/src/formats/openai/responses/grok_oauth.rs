use serde_json::{json, Value};

const GROK_OAUTH_REASONING_MODEL: &str = "grok-4.5";
const GROK_OAUTH_DEFAULT_REASONING_EFFORT: &str = "high";

pub(crate) fn apply_grok_oauth_responses_reasoning_default(
    body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
    provider_model: &str,
) {
    if !provider_type.trim().eq_ignore_ascii_case("grok_oauth")
        || crate::normalize_api_format_alias(provider_api_format) != "openai:responses"
    {
        return;
    }

    let model = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(provider_model.trim());
    if !model.eq_ignore_ascii_case(GROK_OAUTH_REASONING_MODEL) {
        return;
    }

    let Some(body) = body.as_object_mut() else {
        return;
    };
    let reasoning = body
        .entry("reasoning".to_string())
        .or_insert_with(|| json!({}));
    if reasoning.is_null() {
        *reasoning = json!({});
    }
    let Some(reasoning) = reasoning.as_object_mut() else {
        return;
    };
    if reasoning.get("effort").is_none_or(Value::is_null) {
        reasoning.insert(
            "effort".to_string(),
            Value::String(GROK_OAUTH_DEFAULT_REASONING_EFFORT.to_string()),
        );
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::apply_grok_oauth_responses_reasoning_default;

    #[test]
    fn defaults_grok_4_5_reasoning_effort_to_high() {
        let mut body = json!({"model": "grok-4.5", "input": "hello"});

        apply_grok_oauth_responses_reasoning_default(
            &mut body,
            "grok_oauth",
            "openai:responses",
            "grok-4.5",
        );

        assert_eq!(body["reasoning"]["effort"], "high");
    }

    #[test]
    fn preserves_explicit_grok_4_5_reasoning_effort() {
        let mut body = json!({
            "model": "grok-4.5",
            "input": "hello",
            "reasoning": {"effort": "medium"}
        });

        apply_grok_oauth_responses_reasoning_default(
            &mut body,
            "grok_oauth",
            "openai:responses",
            "grok-4.5",
        );

        assert_eq!(body["reasoning"]["effort"], "medium");
    }

    #[test]
    fn leaves_other_models_and_providers_unchanged() {
        for (provider_type, model) in [("grok_oauth", "grok-4"), ("custom", "grok-4.5")] {
            let mut body = json!({"model": model, "input": "hello"});

            apply_grok_oauth_responses_reasoning_default(
                &mut body,
                provider_type,
                "openai:responses",
                model,
            );

            assert!(body.get("reasoning").is_none());
        }
    }
}
