use serde_json::{Map, Value};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalCoreSyncErrorKind {
    InvalidRequest,
    Authentication,
    PermissionDenied,
    NotFound,
    RateLimit,
    ContextLengthExceeded,
    Overloaded,
    ServerError,
}

pub fn is_core_error_finalize_kind(report_kind: &str) -> bool {
    core_error_default_client_api_format(report_kind).is_some()
}

pub fn core_error_default_client_api_format(report_kind: &str) -> Option<&'static str> {
    crate::contracts::core_error_default_client_api_format(report_kind)
}

pub fn core_error_background_report_kind(report_kind: &str) -> Option<&'static str> {
    crate::contracts::core_error_background_report_kind(report_kind)
}

pub fn core_success_background_report_kind(report_kind: &str) -> Option<&'static str> {
    crate::contracts::core_success_background_report_kind(report_kind)
}

pub fn build_core_error_body_for_client_format(
    client_api_format: &str,
    message: &str,
    code: Option<&str>,
    kind: LocalCoreSyncErrorKind,
) -> Option<Value> {
    let mut error_object = Map::new();
    error_object.insert("message".to_string(), Value::String(message.to_string()));

    match aether_ai_formats::normalize_api_format_alias(client_api_format).as_str() {
        "openai:chat" | "openai:responses" | "openai:responses:compact" => {
            error_object.insert(
                "type".to_string(),
                Value::String(map_local_sync_error_kind_to_openai_type(kind).to_string()),
            );
            if let Some(code) = code.filter(|value| !value.is_empty()) {
                error_object.insert("code".to_string(), Value::String(code.to_string()));
            }
            Some(Value::Object(Map::from_iter([(
                "error".to_string(),
                Value::Object(error_object),
            )])))
        }
        "claude:messages" => {
            error_object.insert(
                "type".to_string(),
                Value::String(map_local_sync_error_kind_to_claude_type(kind).to_string()),
            );
            if let Some(code) = code.filter(|value| !value.is_empty()) {
                error_object.insert("code".to_string(), Value::String(code.to_string()));
            }
            Some(Value::Object(Map::from_iter([
                ("type".to_string(), Value::String("error".to_string())),
                ("error".to_string(), Value::Object(error_object)),
            ])))
        }
        "gemini:generate_content" => Some(Value::Object(Map::from_iter([(
            "error".to_string(),
            Value::Object(Map::from_iter([
                (
                    "code".to_string(),
                    Value::from(map_local_sync_error_kind_to_gemini_code(kind)),
                ),
                ("message".to_string(), Value::String(message.to_string())),
                (
                    "status".to_string(),
                    Value::String(map_local_sync_error_kind_to_gemini_status(kind).to_string()),
                ),
            ])),
        )]))),
        _ => None,
    }
}

fn map_local_sync_error_kind_to_openai_type(kind: LocalCoreSyncErrorKind) -> &'static str {
    match kind {
        LocalCoreSyncErrorKind::InvalidRequest => "invalid_request_error",
        LocalCoreSyncErrorKind::Authentication => "authentication_error",
        LocalCoreSyncErrorKind::PermissionDenied => "permission_error",
        LocalCoreSyncErrorKind::NotFound => "not_found_error",
        LocalCoreSyncErrorKind::RateLimit => "rate_limit_error",
        LocalCoreSyncErrorKind::ContextLengthExceeded => "context_length_exceeded",
        LocalCoreSyncErrorKind::Overloaded | LocalCoreSyncErrorKind::ServerError => "server_error",
    }
}

fn map_local_sync_error_kind_to_claude_type(kind: LocalCoreSyncErrorKind) -> &'static str {
    match kind {
        LocalCoreSyncErrorKind::InvalidRequest | LocalCoreSyncErrorKind::ContextLengthExceeded => {
            "invalid_request_error"
        }
        LocalCoreSyncErrorKind::Authentication => "authentication_error",
        LocalCoreSyncErrorKind::PermissionDenied => "permission_error",
        LocalCoreSyncErrorKind::NotFound => "not_found_error",
        LocalCoreSyncErrorKind::RateLimit => "rate_limit_error",
        LocalCoreSyncErrorKind::Overloaded | LocalCoreSyncErrorKind::ServerError => "api_error",
    }
}

fn map_local_sync_error_kind_to_gemini_code(kind: LocalCoreSyncErrorKind) -> u16 {
    match kind {
        LocalCoreSyncErrorKind::InvalidRequest | LocalCoreSyncErrorKind::ContextLengthExceeded => {
            400
        }
        LocalCoreSyncErrorKind::Authentication => 401,
        LocalCoreSyncErrorKind::PermissionDenied => 403,
        LocalCoreSyncErrorKind::NotFound => 404,
        LocalCoreSyncErrorKind::RateLimit => 429,
        LocalCoreSyncErrorKind::Overloaded => 503,
        LocalCoreSyncErrorKind::ServerError => 500,
    }
}

fn map_local_sync_error_kind_to_gemini_status(kind: LocalCoreSyncErrorKind) -> &'static str {
    match kind {
        LocalCoreSyncErrorKind::InvalidRequest | LocalCoreSyncErrorKind::ContextLengthExceeded => {
            "INVALID_ARGUMENT"
        }
        LocalCoreSyncErrorKind::Authentication => "UNAUTHENTICATED",
        LocalCoreSyncErrorKind::PermissionDenied => "PERMISSION_DENIED",
        LocalCoreSyncErrorKind::NotFound => "NOT_FOUND",
        LocalCoreSyncErrorKind::RateLimit => "RESOURCE_EXHAUSTED",
        LocalCoreSyncErrorKind::Overloaded => "UNAVAILABLE",
        LocalCoreSyncErrorKind::ServerError => "INTERNAL",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_core_error_body_for_client_format, core_success_background_report_kind,
        is_core_error_finalize_kind, LocalCoreSyncErrorKind,
    };

    #[test]
    fn builds_openai_core_error_body() {
        let body = build_core_error_body_for_client_format(
            "openai:chat",
            "bad request",
            Some("invalid_request"),
            LocalCoreSyncErrorKind::InvalidRequest,
        )
        .expect("body should build");

        assert_eq!(body["error"]["message"], "bad request");
        assert_eq!(body["error"]["type"], "invalid_request_error");
        assert_eq!(body["error"]["code"], "invalid_request");
    }

    #[test]
    fn recognizes_finalize_kind_and_success_mapping() {
        assert!(is_core_error_finalize_kind("openai_chat_sync_finalize"));
        assert_eq!(
            core_success_background_report_kind("openai_chat_sync_finalize"),
            Some("openai_chat_sync_success")
        );
    }
}
