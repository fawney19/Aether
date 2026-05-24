use serde_json::Value;

pub(super) fn openai_request_is_image_generation_intent(
    _requested_model: &str,
    body_json: &Value,
) -> bool {
    request_forces_image_generation_tool(body_json)
}

fn request_forces_image_generation_tool(body_json: &Value) -> bool {
    body_json
        .get("tool_choice")
        .is_some_and(value_is_image_generation_tool)
}

fn value_is_image_generation_tool(value: &Value) -> bool {
    value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|tool_type| tool_type.trim().eq_ignore_ascii_case("image_generation"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_declaration_without_tool_choice_does_not_trigger_image_generation() {
        let body_json = serde_json::json!({
            "model": "gpt-image-2",
            "input": "Draw a mountain observatory",
            "tools": [{"type": "image_generation"}]
        });

        assert!(!openai_request_is_image_generation_intent(
            "gpt-image-2",
            &body_json
        ));
    }

    #[test]
    fn explicit_image_generation_tool_choice_triggers_image_generation() {
        let body_json = serde_json::json!({
            "model": "gpt-image-2",
            "input": "Draw a mountain observatory",
            "tools": [{"type": "image_generation"}],
            "tool_choice": {"type": "image_generation"}
        });

        assert!(openai_request_is_image_generation_intent(
            "gpt-image-2",
            &body_json
        ));
    }
}
