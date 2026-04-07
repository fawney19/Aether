mod claude_chat;
mod gemini_chat;
mod openai_cli;
mod shared;

pub use claude_chat::convert_openai_chat_response_to_claude_chat;
pub use gemini_chat::convert_openai_chat_response_to_gemini_chat;
pub use openai_cli::convert_openai_chat_response_to_openai_cli;
pub use shared::{
    build_openai_cli_response, build_openai_cli_response_with_content,
    build_openai_cli_response_with_reasoning, OpenAiCliResponseUsage,
};
