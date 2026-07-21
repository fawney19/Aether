use serde_json::Value;

pub mod codex;
pub mod request;
pub mod response;
pub mod spec;
pub mod stream;

const TOOL_ERROR_PREFIX: &str = "[tool error]";

/// Semantic operation carried by an OpenAI Responses request that asks the
/// service to compact a thread. The request still uses the Responses wire
/// contract and transport endpoint.
pub const OPENAI_RESPONSES_OPERATION_COMPACT: &str = "compact";

/// Resolves the operation expressed by an OpenAI Responses wire request.
///
/// `responses_compaction_v2` is represented by a `compaction_trigger` input
/// item on the normal Responses request. The legacy Compact API format is
/// retained as the same operation for observability and scoped model mapping.
pub fn openai_responses_request_operation(api_format: &str, body: &Value) -> Option<&'static str> {
    if aether_ai_formats::is_openai_responses_compact_format(api_format) {
        return Some(OPENAI_RESPONSES_OPERATION_COMPACT);
    }
    if !aether_ai_formats::is_openai_responses_format(api_format) {
        return None;
    }

    body.get("input")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("type").and_then(Value::as_str) == Some("compaction_trigger"))
        })
        .then_some(OPENAI_RESPONSES_OPERATION_COMPACT)
}

fn encode_tool_result_error(output: Value, is_error: bool) -> Value {
    if !is_error {
        return output;
    }
    let detail = match output {
        Value::String(text) => text,
        Value::Null => String::new(),
        value => serde_json::to_string(&value).unwrap_or_else(|_| value.to_string()),
    };
    if detail.is_empty() {
        Value::String(TOOL_ERROR_PREFIX.to_string())
    } else {
        Value::String(format!("{TOOL_ERROR_PREFIX}\n{detail}"))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{openai_responses_request_operation, OPENAI_RESPONSES_OPERATION_COMPACT};

    #[test]
    fn resolves_compaction_trigger_as_compact_operation_on_responses_transport() {
        assert_eq!(
            openai_responses_request_operation(
                "openai:responses",
                &json!({
                    "input": [
                        {"role": "user", "content": "keep working"},
                        {"type": "compaction_trigger"}
                    ]
                }),
            ),
            Some(OPENAI_RESPONSES_OPERATION_COMPACT)
        );
        assert_eq!(
            openai_responses_request_operation(
                "openai:responses",
                &json!({"input": [{"role": "user", "content": "keep working"}]}),
            ),
            None
        );
    }

    #[test]
    fn resolves_legacy_compact_contract_without_a_body_marker() {
        assert_eq!(
            openai_responses_request_operation("openai:responses:compact", &json!({})),
            Some(OPENAI_RESPONSES_OPERATION_COMPACT)
        );
    }
}
