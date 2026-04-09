#[path = "normalize/chat.rs"]
mod chat;
#[path = "normalize/cli.rs"]
mod cli;
#[cfg(test)]
#[path = "normalize/tests.rs"]
mod tests;

pub(crate) use self::chat::{
    build_cross_format_openai_chat_request_body, build_cross_format_openai_chat_upstream_url,
    build_local_openai_chat_request_body, build_local_openai_chat_upstream_url,
};
pub(crate) use self::cli::{
    build_cross_format_openai_cli_request_body, build_cross_format_openai_cli_upstream_url,
    build_local_openai_cli_request_body, build_local_openai_cli_upstream_url,
};
