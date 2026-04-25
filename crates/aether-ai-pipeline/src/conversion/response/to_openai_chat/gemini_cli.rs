use serde_json::Value;

use super::super::from_openai_chat::convert_openai_chat_response_to_openai_cli;
use super::gemini_chat::convert_gemini_chat_response_to_openai_chat;

pub fn convert_gemini_cli_response_to_openai_cli(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    let canonical = convert_gemini_chat_response_to_openai_chat(body_json, report_context)?;
    convert_openai_chat_response_to_openai_cli(&canonical, report_context, false)
}
