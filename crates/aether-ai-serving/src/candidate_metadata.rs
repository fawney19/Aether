use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use serde_json::{Map, Value};

use crate::{ConversionMode, ExecutionStrategy};

pub struct AiCandidateMetadataParts<'a> {
    pub provider_api_format: &'a str,
    pub client_api_format: &'a str,
    pub global_model_id: &'a str,
    pub global_model_name: &'a str,
    pub model_id: &'a str,
    pub selected_provider_model_name: &'a str,
    pub mapping_matched_model: Option<&'a str>,
    pub provider_name: &'a str,
    pub key_name: &'a str,
    pub extra_fields: Map<String, Value>,
}

pub fn build_ai_candidate_metadata(parts: AiCandidateMetadataParts<'_>) -> Value {
    let mut object = Map::new();
    object.insert(
        "provider_api_format".to_string(),
        Value::String(parts.provider_api_format.to_string()),
    );
    object.insert(
        "client_api_format".to_string(),
        Value::String(parts.client_api_format.to_string()),
    );
    object.insert(
        "global_model_id".to_string(),
        Value::String(parts.global_model_id.to_string()),
    );
    object.insert(
        "global_model_name".to_string(),
        Value::String(parts.global_model_name.to_string()),
    );
    object.insert(
        "model_id".to_string(),
        Value::String(parts.model_id.to_string()),
    );
    object.insert(
        "selected_provider_model_name".to_string(),
        Value::String(parts.selected_provider_model_name.to_string()),
    );
    object.insert(
        "mapping_matched_model".to_string(),
        parts
            .mapping_matched_model
            .map(|value| Value::String(value.to_string()))
            .unwrap_or(Value::Null),
    );
    object.insert(
        "provider_name".to_string(),
        Value::String(parts.provider_name.to_string()),
    );
    object.insert(
        "key_name".to_string(),
        Value::String(parts.key_name.to_string()),
    );
    object.extend(parts.extra_fields);
    Value::Object(object)
}

pub fn build_ai_candidate_metadata_from_candidate(
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    provider_api_format: &str,
    client_api_format: &str,
    extra_fields: Map<String, Value>,
) -> Value {
    build_ai_candidate_metadata(AiCandidateMetadataParts {
        provider_api_format,
        client_api_format,
        global_model_id: candidate.global_model_id.as_str(),
        global_model_name: candidate.global_model_name.as_str(),
        model_id: candidate.model_id.as_str(),
        selected_provider_model_name: candidate.selected_provider_model_name.as_str(),
        mapping_matched_model: candidate.mapping_matched_model.as_deref(),
        provider_name: candidate.provider_name.as_str(),
        key_name: candidate.key_name.as_str(),
        extra_fields,
    })
}

pub fn append_ai_execution_contract_fields_to_value(
    value: Value,
    execution_strategy: &str,
    conversion_mode: &str,
    client_contract: &str,
    provider_contract: &str,
) -> Value {
    match value {
        Value::Object(mut object) => {
            object.insert(
                "execution_strategy".to_string(),
                Value::String(execution_strategy.to_string()),
            );
            object.insert(
                "conversion_mode".to_string(),
                Value::String(conversion_mode.to_string()),
            );
            object.insert(
                "client_contract".to_string(),
                Value::String(client_contract.to_string()),
            );
            object.insert(
                "provider_contract".to_string(),
                Value::String(provider_contract.to_string()),
            );
            Value::Object(object)
        }
        other => other,
    }
}

pub fn ai_local_execution_contract_for_formats(
    client_api_format: &str,
    provider_api_format: &str,
) -> (ExecutionStrategy, ConversionMode) {
    if aether_ai_formats::api_format_alias_matches(client_api_format, provider_api_format) {
        return (ExecutionStrategy::LocalSameFormat, ConversionMode::None);
    }

    let conversion_mode =
        if aether_ai_formats::request_conversion_kind(client_api_format, provider_api_format)
            .is_some()
        {
            ConversionMode::Bidirectional
        } else {
            ConversionMode::None
        };
    (ExecutionStrategy::LocalCrossFormat, conversion_mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn candidate_metadata_builds_base_candidate_fields_and_extra_data() {
        let mut extra_fields = Map::new();
        extra_fields.insert("source".to_string(), json!("test"));

        let metadata = build_ai_candidate_metadata(AiCandidateMetadataParts {
            provider_api_format: "openai:responses",
            client_api_format: "claude:messages",
            global_model_id: "global-1",
            global_model_name: "gpt-5.4",
            model_id: "model-1",
            selected_provider_model_name: "gpt-5.4",
            mapping_matched_model: Some("gpt-5"),
            provider_name: "RightCode",
            key_name: "key-a",
            extra_fields,
        });

        assert_eq!(metadata["provider_api_format"], "openai:responses");
        assert_eq!(metadata["client_api_format"], "claude:messages");
        assert_eq!(metadata["global_model_id"], "global-1");
        assert_eq!(metadata["mapping_matched_model"], "gpt-5");
        assert_eq!(metadata["source"], "test");
    }

    #[test]
    fn execution_contract_fields_append_to_object_values_only() {
        let value = append_ai_execution_contract_fields_to_value(
            json!({"existing": true}),
            "local_cross_format",
            "bidirectional",
            "openai:chat",
            "claude:messages",
        );

        assert_eq!(value["existing"], true);
        assert_eq!(value["execution_strategy"], "local_cross_format");
        assert_eq!(value["conversion_mode"], "bidirectional");
        assert_eq!(value["client_contract"], "openai:chat");
        assert_eq!(value["provider_contract"], "claude:messages");

        assert_eq!(
            append_ai_execution_contract_fields_to_value(
                Value::Null,
                "local_same_format",
                "none",
                "openai:chat",
                "openai:chat",
            ),
            Value::Null
        );
    }

    #[test]
    fn local_execution_contract_is_derived_from_client_and_provider_formats() {
        assert_eq!(
            ai_local_execution_contract_for_formats(" OPENAI:CHAT ", "openai:chat"),
            (ExecutionStrategy::LocalSameFormat, ConversionMode::None)
        );
        assert_eq!(
            ai_local_execution_contract_for_formats("openai:chat", "claude:messages"),
            (
                ExecutionStrategy::LocalCrossFormat,
                ConversionMode::Bidirectional,
            )
        );
        assert_eq!(
            ai_local_execution_contract_for_formats("openai:chat", "unknown:format"),
            (ExecutionStrategy::LocalCrossFormat, ConversionMode::None)
        );
    }
}
