use serde_json::Value;

use super::prompt_cache::OpenAiPromptCacheContractViolation;
use super::reasoning::OpenAiReasoningContractViolation;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenAiProviderRequestContractViolation {
    CodexCompact(super::responses::codex::CodexOpenAiCompactRequestContractViolation),
    Responses(super::responses::request::OpenAiResponsesRequestContractViolation),
    PromptCache(OpenAiPromptCacheContractViolation),
    Reasoning(OpenAiReasoningContractViolation),
}

#[derive(Clone, Copy, Debug)]
pub struct OpenAiProviderRequestFinalization<'a> {
    pub source_api_format: &'a str,
    pub provider_api_format: &'a str,
    pub provider_type: &'a str,
    pub provider_model: &'a str,
    pub source_model: &'a str,
    pub body_rules: Option<&'a Value>,
    pub upstream_is_stream: bool,
    pub require_body_stream_field: bool,
}

pub fn finalize_openai_provider_request(
    body: &mut Value,
    finalization: OpenAiProviderRequestFinalization<'_>,
) -> Result<(), OpenAiProviderRequestContractViolation> {
    finalize_openai_provider_request_with_codex_model_capabilities(body, finalization, None)
}

pub fn finalize_openai_provider_request_with_codex_model_capabilities(
    body: &mut Value,
    finalization: OpenAiProviderRequestFinalization<'_>,
    model_capabilities: Option<&super::responses::codex::CodexResponsesModelCapabilities>,
) -> Result<(), OpenAiProviderRequestContractViolation> {
    let is_codex_reasoning_endpoint = finalization
        .provider_type
        .trim()
        .eq_ignore_ascii_case("codex")
        && (crate::is_openai_responses_family_format(finalization.provider_api_format)
            || crate::api_format_alias_matches(finalization.provider_api_format, "openai:search"));
    let resolved_model_capabilities = (is_codex_reasoning_endpoint && model_capabilities.is_none())
        .then(|| {
            super::responses::codex::resolve_codex_responses_model_capabilities(
                finalization.provider_model,
                finalization.source_model,
                None,
            )
        });
    let model_capabilities = is_codex_reasoning_endpoint
        .then(|| model_capabilities.or(resolved_model_capabilities.as_ref()))
        .flatten();
    match crate::normalize_api_format_alias(finalization.source_api_format).as_str() {
        "openai:responses" | "openai:responses:compact" => {
            super::responses::codex::apply_codex_openai_responses_special_body_edits_with_source_model_and_capabilities(
                body,
                finalization.provider_type,
                finalization.provider_api_format,
                finalization.provider_model,
                finalization.source_model,
                model_capabilities,
                finalization.body_rules,
            );
        }
        _ => {
            super::responses::codex::apply_codex_openai_responses_chat_body_edits_with_source_model_and_capabilities(
                body,
                finalization.provider_type,
                finalization.provider_api_format,
                finalization.provider_model,
                finalization.source_model,
                model_capabilities,
                finalization.body_rules,
            )
        }
    }
    super::responses::codex::normalize_codex_openai_reasoning_wire_effort(
        body,
        finalization.provider_type,
        finalization.provider_api_format,
    );
    super::responses::codex::apply_openai_responses_compact_special_body_edits(
        body,
        finalization.provider_api_format,
    );
    crate::enforce_request_body_stream_field(
        body,
        finalization.provider_api_format,
        finalization.upstream_is_stream,
        finalization.require_body_stream_field,
    );
    super::search::apply_openai_search_request_projection(body, finalization.provider_api_format);
    let provider_model = body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(finalization.provider_model);
    super::responses::codex::validate_codex_openai_responses_compact_request_contract(
        body,
        finalization.provider_type,
        finalization.provider_api_format,
    )
    .map_err(OpenAiProviderRequestContractViolation::CodexCompact)?;
    validate_openai_provider_request_contract_with_codex_model_capabilities(
        finalization.provider_api_format,
        provider_model,
        finalization.source_model,
        body,
        model_capabilities,
    )
}

pub fn validate_openai_provider_request_contract(
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    body: &Value,
) -> Result<(), OpenAiProviderRequestContractViolation> {
    validate_openai_provider_request_contract_with_codex_model_capabilities(
        provider_api_format,
        provider_model,
        source_model,
        body,
        None,
    )
}

fn validate_openai_provider_request_contract_with_codex_model_capabilities(
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    body: &Value,
    model_capabilities: Option<&super::responses::codex::CodexResponsesModelCapabilities>,
) -> Result<(), OpenAiProviderRequestContractViolation> {
    super::responses::request::validate_openai_responses_request_contract(
        body,
        provider_api_format,
    )
    .map_err(OpenAiProviderRequestContractViolation::Responses)?;
    super::prompt_cache::validate_openai_prompt_cache_request_with_source_model(
        provider_api_format,
        provider_model,
        source_model,
        body,
    )
    .map_err(OpenAiProviderRequestContractViolation::PromptCache)?;
    super::reasoning::validate_openai_reasoning_request_with_model_profile(
        provider_api_format,
        provider_api_format,
        provider_model,
        source_model,
        body,
        model_capabilities.map(|capabilities| capabilities.supported_reasoning_efforts.as_slice()),
        None,
    )
    .map_err(OpenAiProviderRequestContractViolation::Reasoning)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        finalize_openai_provider_request,
        finalize_openai_provider_request_with_codex_model_capabilities,
        validate_openai_provider_request_contract, OpenAiProviderRequestFinalization,
    };
    use crate::CodexResponsesModelCapabilities;

    #[test]
    fn validates_reasoning_and_prompt_cache_against_the_final_provider_model() {
        let body = json!({
            "model": "gpt-5.6-sol",
            "input": [],
            "reasoning": {"effort": "max"},
            "prompt_cache_options": {"mode": "explicit", "ttl": "30m"}
        });
        validate_openai_provider_request_contract(
            "openai:responses",
            "gpt-5.6-sol",
            "gpt-5.6-sol",
            &body,
        )
        .expect("GPT-5.6 request should satisfy the final provider contract");

        assert!(validate_openai_provider_request_contract(
            "openai:responses",
            "gpt-5.4",
            "gpt-5.6-sol",
            &body,
        )
        .is_err());
    }

    #[test]
    fn opaque_provider_models_inherit_source_capabilities_but_concrete_models_do_not() {
        let body = json!({
            "model": "azure-production",
            "input": [],
            "reasoning": {"effort": "max", "mode": "pro"},
            "prompt_cache_options": {"mode": "explicit", "ttl": "30m"}
        });
        validate_openai_provider_request_contract(
            "openai:responses",
            "azure-production",
            "gpt-5.6-sol-max",
            &body,
        )
        .expect("opaque deployments should inherit the concrete source model capability");
        assert!(validate_openai_provider_request_contract(
            "openai:responses",
            "gpt-5.4",
            "gpt-5.6-sol",
            &body,
        )
        .is_err());
    }

    #[test]
    fn codex_finalization_enforces_model_card_reasoning_efforts() {
        let mut body = json!({
            "model": "gpt-5.6-sol",
            "input": [],
            "reasoning": {"effort": "minimal"}
        });

        let error = finalize_openai_provider_request(
            &mut body,
            OpenAiProviderRequestFinalization {
                source_api_format: "openai:responses",
                provider_api_format: "openai:responses",
                provider_type: "codex",
                provider_model: "gpt-5.6-sol",
                source_model: "gpt-5.6-sol",
                body_rules: None,
                upstream_is_stream: false,
                require_body_stream_field: true,
            },
        )
        .expect_err("GPT-5.6 Codex model card should reject minimal");

        assert!(matches!(
            error,
            super::OpenAiProviderRequestContractViolation::Reasoning(
                super::OpenAiReasoningContractViolation {
                    kind: crate::formats::openai::reasoning::OpenAiReasoningViolationKind::UnsupportedForModel,
                    ..
                }
            )
        ));
    }

    #[test]
    fn finalization_reapplies_codex_and_compact_projection_after_mutations() {
        let mut body = json!({
            "model": "gpt-5.6-sol",
            "input": [],
            "store": true,
            "include": ["reasoning.encrypted_content"],
            "client_metadata": {"source": "mapping"},
            "stream": true,
            "stream_options": {"include_usage": true},
            "tool_choice": "auto",
            "temperature": 0.5,
            "previous_response_id": "resp_123"
        });
        finalize_openai_provider_request(
            &mut body,
            OpenAiProviderRequestFinalization {
                source_api_format: "openai:responses",
                provider_api_format: "openai:responses:compact",
                provider_type: "codex",
                provider_model: "gpt-5.6-sol",
                source_model: "gpt-5.6-sol",
                body_rules: None,
                upstream_is_stream: false,
                require_body_stream_field: true,
            },
        )
        .expect("final Compact request should satisfy its provider contract");

        for field in [
            "store",
            "include",
            "client_metadata",
            "stream",
            "stream_options",
            "tool_choice",
            "temperature",
            "previous_response_id",
        ] {
            assert!(body.get(field).is_none(), "{field} must not reach Compact");
        }
    }

    #[test]
    fn non_responses_sources_receive_codex_responses_reasoning_defaults() {
        for source_api_format in ["openai:chat", "claude:messages", "gemini:generate_content"] {
            let mut body = json!({
                "model": "gpt-5.6-sol",
                "input": []
            });
            finalize_openai_provider_request(
                &mut body,
                OpenAiProviderRequestFinalization {
                    source_api_format,
                    provider_api_format: "openai:responses",
                    provider_type: "codex",
                    provider_model: "gpt-5.6-sol",
                    source_model: "gpt-5.6-sol",
                    body_rules: None,
                    upstream_is_stream: true,
                    require_body_stream_field: true,
                },
            )
            .expect("Codex Responses request should satisfy the final provider contract");

            assert_eq!(body["reasoning"]["effort"], "low");
            assert!(body["reasoning"].get("summary").is_none());
        }
    }

    #[test]
    fn codex_ultra_preset_uses_max_for_every_codex_model_on_the_wire() {
        let mut sol = json!({
            "model": "gpt-5.6-sol",
            "input": [],
            "reasoning": {"effort": "ultra"}
        });
        let mut luna = json!({
            "model": "gpt-5.6-luna",
            "input": [],
            "reasoning": {"effort": "ultra"}
        });
        let finalization_for = |model| OpenAiProviderRequestFinalization {
            source_api_format: "openai:responses",
            provider_api_format: "openai:responses",
            provider_type: "codex",
            provider_model: model,
            source_model: model,
            body_rules: None,
            upstream_is_stream: true,
            require_body_stream_field: true,
        };

        finalize_openai_provider_request(&mut sol, finalization_for("gpt-5.6-sol"))
            .expect("Sol ultra preset should map to the OpenAI wire contract");
        assert_eq!(sol["reasoning"]["effort"], "max");

        finalize_openai_provider_request(&mut luna, finalization_for("gpt-5.6-luna"))
            .expect("Luna ultra preset should map to the OpenAI wire contract");
        assert_eq!(luna["reasoning"]["effort"], "max");
    }

    #[test]
    fn dynamic_codex_card_controls_default_effort_and_keeps_mode_model_specific() {
        let finalization = OpenAiProviderRequestFinalization {
            source_api_format: "openai:responses",
            provider_api_format: "openai:responses",
            provider_type: "codex",
            provider_model: "gpt-5.7-sol",
            source_model: "gpt-5.7-sol",
            body_rules: None,
            upstream_is_stream: true,
            require_body_stream_field: true,
        };
        let capabilities = CodexResponsesModelCapabilities {
            use_responses_lite: true,
            supports_reasoning_summary_parameter: true,
            default_reasoning_effort: Some("ultra".to_string()),
            default_reasoning_summary: None,
            supported_reasoning_efforts: vec!["max".to_string(), "ultra".to_string()],
            supports_parallel_tool_calls: true,
            support_verbosity: true,
            default_verbosity: Some("low".to_string()),
            supported_service_tiers: vec!["priority".to_string()],
        };

        let mut default_body = json!({"model": "gpt-5.7-sol", "input": []});
        finalize_openai_provider_request_with_codex_model_capabilities(
            &mut default_body,
            finalization,
            Some(&capabilities),
        )
        .expect("card default ultra should use the max wire effort");
        assert_eq!(default_body["reasoning"]["effort"], "max");

        let mut mode_body = json!({
            "model": "gpt-5.7-sol",
            "input": [],
            "reasoning": {"effort": "max", "mode": "pro"}
        });
        let mode_error = finalize_openai_provider_request_with_codex_model_capabilities(
            &mut mode_body,
            finalization,
            Some(&capabilities),
        )
        .expect_err("Responses Lite alone must not enable GPT-5.6 reasoning modes");
        assert!(matches!(
            mode_error,
            super::OpenAiProviderRequestContractViolation::Reasoning(_)
        ));

        let ultra_only = CodexResponsesModelCapabilities {
            supported_reasoning_efforts: vec!["ultra".to_string()],
            ..capabilities.clone()
        };
        let mut ultra_only_body = json!({
            "model": "gpt-5.7-sol",
            "input": [],
            "reasoning": {"effort": "ultra"}
        });
        finalize_openai_provider_request_with_codex_model_capabilities(
            &mut ultra_only_body,
            finalization,
            Some(&ultra_only),
        )
        .expect("Codex maps the Ultra preset to max without card-list wire validation");
        assert_eq!(ultra_only_body["reasoning"]["effort"], "max");
    }

    #[test]
    fn codex_search_normalizes_reasoning_and_projects_the_typed_request() {
        let finalization = OpenAiProviderRequestFinalization {
            source_api_format: "openai:search",
            provider_api_format: "openai:search",
            provider_type: "codex",
            provider_model: "gpt-5.6-sol",
            source_model: "gpt-5.6-sol",
            body_rules: None,
            upstream_is_stream: false,
            require_body_stream_field: false,
        };
        let mut body = json!({
            "id": "session-1",
            "model": "gpt-5.6-sol",
            "reasoning": {
                "effort": "ultra",
                "summary": "auto",
                "context": "current_turn",
                "future_reasoning_field": true
            },
            "commands": {"search_query": [{"q": "Aether"}]},
            "store": false,
            "future_request_field": {"enabled": true},
            "stream": true
        });

        finalize_openai_provider_request(&mut body, finalization)
            .expect("Codex Search request should finalize");

        assert_eq!(body["reasoning"]["effort"], "max");
        assert_eq!(body["reasoning"]["summary"], "auto");
        assert_eq!(body["reasoning"]["context"], "current_turn");
        assert!(body["reasoning"].get("future_reasoning_field").is_none());
        assert_eq!(body["commands"]["search_query"][0]["q"], "Aether");
        assert!(body.get("store").is_none());
        assert!(body.get("future_request_field").is_none());
        assert!(body.get("stream").is_none());
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("include").is_none());
    }

    #[test]
    fn codex_search_validates_reasoning_effort_against_model_card() {
        let finalization = OpenAiProviderRequestFinalization {
            source_api_format: "openai:search",
            provider_api_format: "openai:search",
            provider_type: "codex",
            provider_model: "gpt-5.6-sol",
            source_model: "gpt-5.6-sol",
            body_rules: None,
            upstream_is_stream: false,
            require_body_stream_field: false,
        };
        let mut supported = json!({
            "id": "session-1",
            "model": "gpt-5.6-sol",
            "reasoning": {"effort": "high"}
        });
        finalize_openai_provider_request(&mut supported, finalization)
            .expect("published Search effort should pass");

        let mut unsupported = json!({
            "id": "session-1",
            "model": "gpt-5.6-sol",
            "reasoning": {"effort": "none"}
        });
        let error = finalize_openai_provider_request(&mut unsupported, finalization)
            .expect_err("unpublished Search effort should be rejected");
        assert!(matches!(
            error,
            super::OpenAiProviderRequestContractViolation::Reasoning(_)
        ));
    }

    #[test]
    fn dynamic_codex_card_preserves_custom_reasoning_effort_case() {
        let finalization = OpenAiProviderRequestFinalization {
            source_api_format: "openai:responses",
            provider_api_format: "openai:responses",
            provider_type: "codex",
            provider_model: "codex-custom",
            source_model: "codex-custom",
            body_rules: None,
            upstream_is_stream: true,
            require_body_stream_field: true,
        };
        let capabilities = CodexResponsesModelCapabilities {
            use_responses_lite: false,
            supports_reasoning_summary_parameter: true,
            default_reasoning_effort: Some("VendorEffortX".to_string()),
            default_reasoning_summary: None,
            supported_reasoning_efforts: vec!["VendorEffortX".to_string()],
            supports_parallel_tool_calls: true,
            support_verbosity: true,
            default_verbosity: Some("low".to_string()),
            supported_service_tiers: vec![],
        };

        let mut body = json!({"model": "codex-custom", "input": []});
        finalize_openai_provider_request_with_codex_model_capabilities(
            &mut body,
            finalization,
            Some(&capabilities),
        )
        .expect("custom card effort should remain exact");
        assert_eq!(body["reasoning"]["effort"], "VendorEffortX");

        let mut custom = json!({
            "model": "codex-custom",
            "input": [],
            "reasoning": {"effort": "vendoreffortx"}
        });
        let error = finalize_openai_provider_request_with_codex_model_capabilities(
            &mut custom,
            finalization,
            Some(&capabilities),
        )
        .expect_err("custom reasoning efforts should match the model card exactly");
        assert!(matches!(
            error,
            super::OpenAiProviderRequestContractViolation::Reasoning(_)
        ));

        let mut ultra = json!({
            "model": "codex-custom",
            "input": [],
            "reasoning": {"effort": "ultra"}
        });
        let error = finalize_openai_provider_request_with_codex_model_capabilities(
            &mut ultra,
            finalization,
            Some(&capabilities),
        )
        .expect_err("ultra should require model-card support before mapping to max");
        assert!(matches!(
            error,
            super::OpenAiProviderRequestContractViolation::Reasoning(_)
        ));
    }

    #[test]
    fn gpt_5_6_sol_uses_the_responses_lite_request_contract() {
        let mut body = json!({
            "model": "gpt-5.6-sol",
            "instructions": "Follow the project instructions.",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{
                        "type": "input_image",
                        "image_url": "data:image/png;base64,aGVsbG8=",
                        "detail": "original"
                    }]
                },
                {
                    "type": "function_call_output",
                    "call_id": "call-1",
                    "output": [{
                        "type": "input_image",
                        "image_url": "data:image/png;base64,ZnVuY3Rpb24=",
                        "detail": "high"
                    }]
                },
                {
                    "type": "custom_tool_call_output",
                    "call_id": "call-2",
                    "output": [{
                        "type": "input_image",
                        "image_url": "data:image/png;base64,Y3VzdG9t",
                        "detail": "auto"
                    }]
                }
            ],
            "tools": [{
                "type": "function",
                "name": "lookup",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "detail": {"type": "string"}
                    }
                }
            }],
            "parallel_tool_calls": true
        });

        let finalization = OpenAiProviderRequestFinalization {
            source_api_format: "openai:responses",
            provider_api_format: "openai:responses",
            provider_type: "codex",
            provider_model: "gpt-5.6-sol",
            source_model: "gpt-5.6-sol",
            body_rules: None,
            upstream_is_stream: true,
            require_body_stream_field: true,
        };
        finalize_openai_provider_request(&mut body, finalization)
            .expect("GPT-5.6 Sol should satisfy the Responses Lite contract");
        let first = body.clone();
        finalize_openai_provider_request(&mut body, finalization)
            .expect("Responses Lite finalization should be idempotent");

        assert_eq!(body, first);
        assert!(body.get("instructions").is_none());
        assert!(body.get("tools").is_none());
        assert_eq!(body["input"][0]["type"], "additional_tools");
        assert_eq!(body["input"][0]["role"], "developer");
        assert_eq!(body["input"][0]["tools"][0]["name"], "lookup");
        assert_eq!(body["input"][1]["type"], "message");
        assert_eq!(body["input"][1]["role"], "developer");
        assert_eq!(
            body["input"][1]["content"][0]["text"],
            "Follow the project instructions."
        );
        assert!(body["input"][2]["content"][0].get("detail").is_none());
        assert!(body["input"][3]["output"][0].get("detail").is_none());
        assert!(body["input"][4]["output"][0].get("detail").is_none());
        assert_eq!(
            body["input"][0]["tools"][0]["parameters"]["properties"]["detail"]["type"],
            "string"
        );
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(body["reasoning"]["effort"], "low");
        assert_eq!(body["reasoning"]["context"], "all_turns");
        assert!(body["reasoning"].get("summary").is_none());
    }

    #[test]
    fn gpt_5_6_sol_uses_standard_responses_contract_for_server_side_compaction() {
        let mut body = json!({
            "model": "gpt-5.6-sol",
            "instructions": "Preserve the standard Responses request shape.",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "compact"}]
            }],
            "tools": [{"type": "function", "name": "lookup"}],
            "context_management": [{
                "type": "compaction",
                "compact_threshold": 128000
            }],
            "parallel_tool_calls": true
        });
        let finalization = OpenAiProviderRequestFinalization {
            source_api_format: "openai:responses",
            provider_api_format: "openai:responses",
            provider_type: "codex",
            provider_model: "gpt-5.6-sol",
            source_model: "gpt-5.6-sol",
            body_rules: None,
            upstream_is_stream: true,
            require_body_stream_field: true,
        };

        finalize_openai_provider_request(&mut body, finalization)
            .expect("server-side compaction should use the standard Responses contract");
        let first = body.clone();
        finalize_openai_provider_request(&mut body, finalization)
            .expect("standard Responses finalization should be idempotent");

        assert_eq!(body, first);
        assert_eq!(body["context_management"][0]["compact_threshold"], 128000);
        assert_eq!(
            body["instructions"],
            "Preserve the standard Responses request shape."
        );
        assert_eq!(body["tools"][0]["name"], "lookup");
        assert_eq!(body["parallel_tool_calls"], true);
        assert!(body["reasoning"].get("context").is_none());
    }

    #[test]
    fn opaque_codex_deployments_use_the_exact_source_model_card() {
        let mut body = json!({
            "model": "azure-production",
            "instructions": "Use the configured tools.",
            "input": [],
            "tools": [],
            "parallel_tool_calls": true
        });

        finalize_openai_provider_request(
            &mut body,
            OpenAiProviderRequestFinalization {
                source_api_format: "openai:responses",
                provider_api_format: "openai:responses:compact",
                provider_type: "codex",
                provider_model: "azure-production",
                source_model: "gpt-5.6-terra",
                body_rules: None,
                upstream_is_stream: false,
                require_body_stream_field: true,
            },
        )
        .expect("opaque Codex deployment should use the exact source model card");

        assert!(body.get("instructions").is_none());
        assert!(body.get("tools").is_none());
        assert_eq!(body["input"][0]["type"], "additional_tools");
        assert_eq!(body["input"][1]["role"], "developer");
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(body["reasoning"]["effort"], "medium");
        assert_eq!(body["reasoning"]["context"], "all_turns");
        for field in body.as_object().expect("object").keys() {
            assert!(
                [
                    "model",
                    "input",
                    "instructions",
                    "tools",
                    "parallel_tool_calls",
                    "reasoning",
                    "service_tier",
                    "prompt_cache_key",
                    "text",
                ]
                .contains(&field.as_str()),
                "unexpected Compact field: {field}"
            );
        }
    }

    #[test]
    fn gpt_5_4_keeps_the_standard_codex_responses_shape() {
        let mut body = json!({
            "model": "gpt-5.4",
            "instructions": "Keep this top-level instruction.",
            "input": [],
            "tools": [{"type": "function", "name": "lookup", "parameters": {}}],
            "parallel_tool_calls": true
        });

        finalize_openai_provider_request(
            &mut body,
            OpenAiProviderRequestFinalization {
                source_api_format: "openai:responses",
                provider_api_format: "openai:responses",
                provider_type: "codex",
                provider_model: "gpt-5.4",
                source_model: "gpt-5.4",
                body_rules: None,
                upstream_is_stream: true,
                require_body_stream_field: true,
            },
        )
        .expect("GPT-5.4 should satisfy the standard Codex Responses contract");

        assert_eq!(body["instructions"], "Keep this top-level instruction.");
        assert_eq!(body["tools"][0]["name"], "lookup");
        assert_eq!(body["parallel_tool_calls"], true);
        assert_eq!(body["reasoning"]["effort"], "medium");
        assert!(body["reasoning"].get("context").is_none());
        assert!(body["reasoning"].get("summary").is_none());
    }

    #[test]
    fn cross_format_compact_finalization_removes_post_conversion_fields() {
        for source_api_format in ["claude:messages", "gemini:generate_content"] {
            let mut body = json!({
                "model": "gpt-5.6-sol",
                "input": [],
                "client_metadata": {"source": "mapping"},
                "include": ["reasoning.encrypted_content"],
                "store": true,
                "stream": true,
                "stream_options": {"include_usage": true},
                "tool_choice": "auto",
                "parallel_tool_calls": true,
                "reasoning": {"effort": "max"},
                "text": {"verbosity": "medium"},
                "tools": [{"type": "function", "name": "lookup", "parameters": {}}]
            });
            finalize_openai_provider_request(
                &mut body,
                OpenAiProviderRequestFinalization {
                    source_api_format,
                    provider_api_format: "openai:responses:compact",
                    provider_type: "codex",
                    provider_model: "gpt-5.6-sol",
                    source_model: "gpt-5.6-sol",
                    body_rules: None,
                    upstream_is_stream: false,
                    require_body_stream_field: true,
                },
            )
            .expect("cross-format Compact request should satisfy its final contract");

            for field in [
                "client_metadata",
                "include",
                "store",
                "stream",
                "stream_options",
                "tool_choice",
            ] {
                assert!(body.get(field).is_none(), "{field} must not reach Compact");
            }
            assert_eq!(body["parallel_tool_calls"], false);
            assert_eq!(body["reasoning"]["effort"], "max");
            assert_eq!(body["reasoning"]["context"], "all_turns");
            assert_eq!(body["text"]["verbosity"], "medium");
            assert!(body.get("tools").is_none());
            assert_eq!(body["input"][0]["tools"][0]["name"], "lookup");
        }
    }
}
