use serde_json::Value;

use crate::formats::shared::model_directives::ReasoningEffort;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiReasoningViolationKind {
    InvalidType,
    InvalidEnum,
    UnsupportedForModel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiReasoningContractViolation {
    pub kind: OpenAiReasoningViolationKind,
    pub field: String,
    pub value: Option<String>,
    pub reason: String,
}

pub fn validate_openai_reasoning_request(
    source_api_format: &str,
    provider_api_format: &str,
    provider_model: &str,
    body: &Value,
) -> Result<(), OpenAiReasoningContractViolation> {
    let source_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    validate_openai_reasoning_request_with_source_model(
        source_api_format,
        provider_api_format,
        provider_model,
        source_model,
        body,
    )
}

pub(crate) fn validate_openai_reasoning_request_with_source_model(
    source_api_format: &str,
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    body: &Value,
) -> Result<(), OpenAiReasoningContractViolation> {
    validate_openai_reasoning_request_with_model_profile(
        source_api_format,
        provider_api_format,
        provider_model,
        source_model,
        body,
        None,
        None,
    )
}

pub(crate) fn validate_openai_reasoning_request_with_model_profile(
    source_api_format: &str,
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    body: &Value,
    model_card_reasoning_efforts: Option<&[String]>,
    supports_reasoning_mode: Option<bool>,
) -> Result<(), OpenAiReasoningContractViolation> {
    let Some(object) = body.as_object() else {
        return Ok(());
    };
    let source_api_format = crate::normalize_api_format_alias(source_api_format);
    let reasoning = match source_api_format.as_str() {
        "openai:responses" | "openai:responses:compact" | "openai:search" => {
            match object.get("reasoning") {
                Some(Value::Object(reasoning)) => Some(reasoning),
                Some(Value::Null) => None,
                Some(value) => {
                    return Err(OpenAiReasoningContractViolation {
                        kind: OpenAiReasoningViolationKind::InvalidType,
                        field: "reasoning".to_string(),
                        value: Some(value.to_string()),
                        reason: "reasoning must be an object".to_string(),
                    });
                }
                None => None,
            }
        }
        "openai:chat" => None,
        _ => return Ok(()),
    };

    let provider_model = provider_model.trim();
    let provider_model = if provider_model.is_empty() {
        source_model
    } else {
        provider_model
    };

    let effort = match source_api_format.as_str() {
        "openai:chat" => object.get("reasoning_effort"),
        "openai:responses" | "openai:responses:compact" | "openai:search" => {
            reasoning.and_then(|reasoning| reasoning.get("effort"))
        }
        _ => None,
    };
    if let Some(value) = effort.filter(|value| !value.is_null()) {
        validate_reasoning_effort(
            value,
            source_api_format.as_str(),
            provider_api_format,
            provider_model,
            source_model,
            model_card_reasoning_efforts,
        )?;
    }

    if source_api_format != "openai:search" {
        if let Some(mode) = reasoning
            .and_then(|reasoning| reasoning.get("mode"))
            .filter(|value| !value.is_null())
        {
            validate_reasoning_mode(mode, provider_model, source_model, supports_reasoning_mode)?;
        }
    }
    if let Some(context) = reasoning
        .and_then(|reasoning| reasoning.get("context"))
        .filter(|value| !value.is_null())
    {
        validate_reasoning_context(context)?;
    }
    if let Some(summary) = reasoning
        .and_then(|reasoning| reasoning.get("summary"))
        .filter(|value| !value.is_null())
    {
        validate_reasoning_summary(summary)?;
    }

    Ok(())
}

fn validate_reasoning_effort(
    value: &Value,
    source_api_format: &str,
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    model_card_reasoning_efforts: Option<&[String]>,
) -> Result<(), OpenAiReasoningContractViolation> {
    let field = if source_api_format == "openai:chat" {
        "reasoning_effort"
    } else {
        "reasoning.effort"
    };
    let Some(raw) = value.as_str() else {
        return Err(OpenAiReasoningContractViolation {
            kind: OpenAiReasoningViolationKind::InvalidType,
            field: field.to_string(),
            value: Some(value.to_string()),
            reason: "reasoning effort must be a string".to_string(),
        });
    };
    if raw.trim().is_empty() {
        return Err(OpenAiReasoningContractViolation {
            kind: OpenAiReasoningViolationKind::InvalidEnum,
            field: field.to_string(),
            value: Some(raw.to_string()),
            reason: "reasoning effort must not be empty".to_string(),
        });
    }
    if raw.trim() == "ultra" {
        return Err(OpenAiReasoningContractViolation {
            kind: OpenAiReasoningViolationKind::InvalidEnum,
            field: field.to_string(),
            value: Some(raw.to_string()),
            reason: "ultra is a Codex client preset, not an OpenAI wire effort".to_string(),
        });
    }
    if let Some(supported_efforts) =
        model_card_reasoning_efforts.filter(|values| !values.is_empty())
    {
        if supported_efforts
            .iter()
            .any(|effort| effort == raw.trim() || (raw.trim() == "max" && effort == "ultra"))
        {
            return Ok(());
        }
        return Err(OpenAiReasoningContractViolation {
            kind: OpenAiReasoningViolationKind::UnsupportedForModel,
            field: field.to_string(),
            value: Some(raw.to_string()),
            reason: "provider model card does not support the requested reasoning effort"
                .to_string(),
        });
    }
    let Some(effort) = ReasoningEffort::parse(raw) else {
        return Ok(());
    };
    if crate::reasoning_effort_supported_for_model(
        provider_api_format,
        provider_model,
        source_model,
        effort,
    ) {
        return Ok(());
    }
    Err(OpenAiReasoningContractViolation {
        kind: OpenAiReasoningViolationKind::UnsupportedForModel,
        field: field.to_string(),
        value: Some(raw.to_string()),
        reason: "provider model does not support the requested reasoning effort".to_string(),
    })
}

fn validate_reasoning_mode(
    value: &Value,
    provider_model: &str,
    source_model: &str,
    supports_reasoning_mode: Option<bool>,
) -> Result<(), OpenAiReasoningContractViolation> {
    let Some(mode) = value.as_str() else {
        return Err(OpenAiReasoningContractViolation {
            kind: OpenAiReasoningViolationKind::InvalidType,
            field: "reasoning.mode".to_string(),
            value: Some(value.to_string()),
            reason: "reasoning mode must be a string".to_string(),
        });
    };
    if mode.trim().is_empty() {
        return Err(OpenAiReasoningContractViolation {
            kind: OpenAiReasoningViolationKind::InvalidEnum,
            field: "reasoning.mode".to_string(),
            value: Some(mode.to_string()),
            reason: "reasoning mode must not be empty".to_string(),
        });
    }
    if !matches!(mode, "standard" | "pro") {
        return Err(OpenAiReasoningContractViolation {
            kind: OpenAiReasoningViolationKind::InvalidEnum,
            field: "reasoning.mode".to_string(),
            value: Some(mode.to_string()),
            reason: "reasoning mode supports standard or pro".to_string(),
        });
    }
    let supported = supports_reasoning_mode.unwrap_or_else(|| {
        crate::formats::shared::model_directives::openai_model_resolves_to_gpt_5_6(
            provider_model,
            source_model,
        )
    });
    if supported {
        return Ok(());
    }
    Err(OpenAiReasoningContractViolation {
        kind: OpenAiReasoningViolationKind::UnsupportedForModel,
        field: "reasoning.mode".to_string(),
        value: Some(mode.to_string()),
        reason: "provider model does not support reasoning mode".to_string(),
    })
}

fn validate_reasoning_context(value: &Value) -> Result<(), OpenAiReasoningContractViolation> {
    let Some(context) = value.as_str() else {
        return Err(OpenAiReasoningContractViolation {
            kind: OpenAiReasoningViolationKind::InvalidType,
            field: "reasoning.context".to_string(),
            value: Some(value.to_string()),
            reason: "reasoning context must be a string".to_string(),
        });
    };
    if matches!(context, "auto" | "current_turn" | "all_turns") {
        return Ok(());
    }
    Err(OpenAiReasoningContractViolation {
        kind: OpenAiReasoningViolationKind::InvalidEnum,
        field: "reasoning.context".to_string(),
        value: Some(context.to_string()),
        reason: "reasoning context is not a supported wire value".to_string(),
    })
}

fn validate_reasoning_summary(value: &Value) -> Result<(), OpenAiReasoningContractViolation> {
    let Some(summary) = value.as_str() else {
        return Err(OpenAiReasoningContractViolation {
            kind: OpenAiReasoningViolationKind::InvalidType,
            field: "reasoning.summary".to_string(),
            value: Some(value.to_string()),
            reason: "reasoning summary must be a string".to_string(),
        });
    };
    if matches!(summary, "auto" | "concise" | "detailed") {
        return Ok(());
    }
    Err(OpenAiReasoningContractViolation {
        kind: OpenAiReasoningViolationKind::InvalidEnum,
        field: "reasoning.summary".to_string(),
        value: Some(summary.to_string()),
        reason: "reasoning summary is not a supported wire value".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{validate_openai_reasoning_request, OpenAiReasoningViolationKind};

    #[test]
    fn mapped_model_is_authoritative_for_openai_reasoning_effort() {
        let alias = json!({
            "model": "deployment-alias",
            "reasoning": {"effort": "max"}
        });
        validate_openai_reasoning_request(
            "openai:responses",
            "openai:responses",
            "gpt-5.6-sol",
            &alias,
        )
        .expect("GPT-5.6 should accept max");

        let error = validate_openai_reasoning_request(
            "openai:responses",
            "openai:responses",
            "gpt-5.4",
            &alias,
        )
        .expect_err("GPT-5.4 should reject max");
        assert_eq!(
            error.kind,
            OpenAiReasoningViolationKind::UnsupportedForModel
        );
    }

    #[test]
    fn gpt_5_6_rejects_known_unsupported_effort_and_preserves_custom_effort() {
        let unsupported = json!({
            "model": "gpt-5.6-terra",
            "reasoning_effort": "minimal"
        });
        let error = validate_openai_reasoning_request(
            "openai:chat",
            "openai:chat",
            "gpt-5.6-terra",
            &unsupported,
        )
        .expect_err("known unsupported effort should be rejected");
        assert_eq!(
            error.kind,
            OpenAiReasoningViolationKind::UnsupportedForModel
        );

        validate_openai_reasoning_request(
            "openai:responses",
            "openai:responses",
            "gpt-5.6-terra",
            &json!({
                "model": "gpt-5.6-terra",
                "reasoning": {"effort": "future"}
            }),
        )
        .expect("model-advertised custom effort should pass through");

        let ultra = validate_openai_reasoning_request(
            "openai:responses",
            "openai:responses",
            "gpt-5.6-terra",
            &json!({"reasoning": {"effort": "ultra"}}),
        )
        .expect_err("Codex local ultra preset should not enter the OpenAI wire contract");
        assert_eq!(ultra.kind, OpenAiReasoningViolationKind::InvalidEnum);

        let empty = validate_openai_reasoning_request(
            "openai:responses",
            "openai:responses",
            "gpt-5.6-terra",
            &json!({"reasoning": {"effort": " "}}),
        )
        .expect_err("empty reasoning effort should be rejected");
        assert_eq!(empty.kind, OpenAiReasoningViolationKind::InvalidEnum);
    }

    #[test]
    fn reasoning_mode_is_responses_only_and_requires_gpt_5_6() {
        for mode in ["standard", "pro"] {
            validate_openai_reasoning_request(
                "openai:responses",
                "openai:responses",
                "gpt-5.6-sol",
                &json!({"model": "deployment-alias", "reasoning": {"mode": mode}}),
            )
            .expect("GPT-5.6 should accept reasoning mode");
        }

        let unsupported = validate_openai_reasoning_request(
            "openai:responses",
            "openai:responses",
            "gpt-5.4",
            &json!({"reasoning": {"mode": "pro"}}),
        )
        .expect_err("earlier GPT models should reject reasoning mode");
        assert_eq!(
            unsupported.kind,
            OpenAiReasoningViolationKind::UnsupportedForModel
        );

        let invalid = validate_openai_reasoning_request(
            "openai:responses",
            "openai:responses",
            "gpt-5.6-sol",
            &json!({"reasoning": {"mode": "fast"}}),
        )
        .expect_err("unknown reasoning mode should be rejected");
        assert_eq!(invalid.kind, OpenAiReasoningViolationKind::InvalidEnum);

        let empty = validate_openai_reasoning_request(
            "openai:responses",
            "openai:responses",
            "gpt-5.6-sol",
            &json!({"reasoning": {"mode": ""}}),
        )
        .expect_err("empty reasoning mode should be rejected");
        assert_eq!(empty.kind, OpenAiReasoningViolationKind::InvalidEnum);

        validate_openai_reasoning_request(
            "openai:chat",
            "openai:chat",
            "gpt-5.4",
            &json!({"reasoning": {"mode": "pro"}}),
        )
        .expect("Chat Completions does not define reasoning.mode");
    }

    #[test]
    fn reasoning_context_validates_wire_values_without_model_gating() {
        for context in ["auto", "current_turn", "all_turns"] {
            validate_openai_reasoning_request(
                "openai:responses",
                "openai:responses",
                "gpt-5.4",
                &json!({"reasoning": {"context": context}}),
            )
            .expect("reasoning context should remain available to Codex Responses models");
        }

        let invalid = validate_openai_reasoning_request(
            "openai:responses",
            "openai:responses",
            "gpt-5.6-sol",
            &json!({"reasoning": {"context": "session"}}),
        )
        .expect_err("unknown reasoning context should be rejected");
        assert_eq!(invalid.kind, OpenAiReasoningViolationKind::InvalidEnum);
    }

    #[test]
    fn reasoning_summary_accepts_only_openai_wire_values() {
        for summary in ["auto", "concise", "detailed"] {
            validate_openai_reasoning_request(
                "openai:responses",
                "openai:responses",
                "gpt-5.6-sol",
                &json!({"reasoning": {"summary": summary}}),
            )
            .expect("documented reasoning summary should be accepted");
        }

        for (summary, expected_kind) in [
            (json!("none"), OpenAiReasoningViolationKind::InvalidEnum),
            (json!(true), OpenAiReasoningViolationKind::InvalidType),
        ] {
            let error = validate_openai_reasoning_request(
                "openai:responses",
                "openai:responses",
                "gpt-5.6-sol",
                &json!({"reasoning": {"summary": summary}}),
            )
            .expect_err("invalid reasoning summary should be rejected");
            assert_eq!(error.kind, expected_kind);
        }
    }

    #[test]
    fn nullable_reasoning_fields_are_treated_as_unconfigured() {
        for body in [
            json!({"model": "gpt-5.6-sol", "reasoning": null}),
            json!({
                "model": "gpt-5.6-sol",
                "reasoning": {"effort": null, "mode": null, "context": null}
            }),
        ] {
            validate_openai_reasoning_request(
                "openai:responses",
                "openai:responses",
                "gpt-5.6-sol",
                &body,
            )
            .expect("nullable Responses reasoning fields should be omitted semantically");
        }
        validate_openai_reasoning_request(
            "openai:chat",
            "openai:chat",
            "gpt-5.6-sol",
            &json!({"model": "gpt-5.6-sol", "reasoning_effort": null}),
        )
        .expect("nullable Chat reasoning effort should be omitted semantically");
    }
}
