#[path = "normalize/chat.rs"]
mod chat;
#[path = "normalize/responses.rs"]
mod responses;
#[cfg(test)]
#[path = "normalize/tests.rs"]
mod tests;

pub(crate) use self::chat::{
    build_cross_format_openai_chat_request_body, build_cross_format_openai_chat_upstream_url,
    build_local_openai_chat_request_body, build_local_openai_chat_upstream_url,
};
pub(crate) use self::responses::{
    build_cross_format_openai_responses_request_body,
    build_cross_format_openai_responses_upstream_url, build_local_openai_responses_request_body,
    build_local_openai_responses_upstream_url,
};
