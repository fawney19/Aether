use std::sync::LazyLock;

use axum::body::Body;
use axum::http::{Response, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use regex::Regex;
use serde_json::json;
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::ai_serving::AiSurfaceFinalizeError;
use crate::constants::*;
use crate::insert_header_if_missing;

/// 开启后记录不截断但仍脱敏的内部错误详情，默认关闭。
static GATEWAY_ERROR_DETAIL_LOGGING: LazyLock<bool> = LazyLock::new(|| {
    parse_gateway_error_detail_logging(
        std::env::var("AETHER_GATEWAY_ERROR_DETAIL_LOGGING")
            .ok()
            .as_deref(),
    )
});

fn parse_gateway_error_detail_logging(value: Option<&str>) -> bool {
    // 仅接受精确的小写 true/false；未设置或无效值默认关闭详情日志。
    value
        .and_then(|value| value.parse::<bool>().ok())
        .unwrap_or(false)
}

// 按 URL authority 的边界匹配 userinfo，避免跨过路径、查询串和片段中的 @。
static ERROR_URL_USERINFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)([a-z][a-z0-9+.-]*://)[^\s/?\#"<>]*@"#)
        .expect("error URL userinfo regex should compile")
});

static ERROR_CREDENTIAL_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?ix)
        \b
            (?:password|passwd|pwd|(?:access|refresh|id|session|auth)[_-]?token|token|
               (?:client[_-]?)?secret|(?:api|access|secret|private)[_-]?key|
               (?:proxy[_-])?authorization)
            (?:\\*["'])?(?:\s|\\+[nrt])*[:=](?:\s|\\+[nrt])*
            (?:Some\((?:\s|\\+[nrt])*)?
            (?:(?:Bearer|Basic)(?:\s|\\+[nrt])+)?
        |(?:\b|\\+[nrt])Bearer(?:\s|\\+[nrt])+"#,
    )
    .expect("error credential prefix regex should compile")
});

/// 检查是否启用了内部错误详情日志。
pub(crate) fn gateway_error_detail_logging_enabled() -> bool {
    *GATEWAY_ERROR_DETAIL_LOGGING
}

/// 日志摘要：移除 URL userinfo、常见凭据键值和 Bearer 内容，再限制为 256 字节。
/// 这是自由文本的有限规则，不能保证识别任意敏感内容或编码后的字段名。
pub(crate) fn redact_error_detail(error: &impl std::fmt::Display) -> String {
    redact_error_str(&error.to_string())
}

/// 脱敏 Debug 格式的错误详情（用于未实现 Display 的错误类型）。
pub(crate) fn redact_error_debug(error: &impl std::fmt::Debug) -> String {
    redact_error_str(&format!("{error:?}"))
}

pub(crate) fn redact_error_str(message: &str) -> String {
    const MAX_LEN: usize = 256;
    // 必须先处理完整凭据，再截断；否则截断位置可能落在密码和 @host 之间。
    let mut redacted = redact_error_str_unbounded(message);
    if redacted.len() > MAX_LEN {
        let mut end = MAX_LEN;
        while !redacted.is_char_boundary(end) {
            end -= 1;
        }
        redacted.truncate(end);
        redacted.push_str("...");
    }
    redacted
}

/// 详情日志仅取消长度限制，继续使用与摘要相同的凭据脱敏规则。
fn redact_error_str_unbounded(message: &str) -> String {
    let urls_redacted = ERROR_URL_USERINFO.replace_all(message, "$1");
    let mut result = String::with_capacity(urls_redacted.len());
    let mut cursor = 0;
    let mut prefixes = ERROR_CREDENTIAL_PREFIX.find_iter(&urls_redacted).peekable();
    while let Some(prefix) = prefixes.next() {
        // 带引号的值可能包含 password= 等文本，已遮盖的内容不再重复处理。
        if prefix.start() < cursor {
            continue;
        }
        let value_start = prefix.end();
        // 未引用的值最多读到下一个凭据字段，避免吞掉它的开头却留下带空格的值。
        let unquoted_limit = prefixes
            .peek()
            .map_or(urls_redacted.len() - value_start, |next| {
                urls_redacted[value_start..next.start()]
                    .trim_end_matches([',', ';', '&'])
                    .len()
            });
        let value_end =
            value_start + credential_value_len(&urls_redacted[value_start..], unquoted_limit);
        result.push_str(&urls_redacted[cursor..value_start]);
        result.push_str("[REDACTED]");
        cursor = value_end;
    }
    result.push_str(&urls_redacted[cursor..]);
    result
}

fn credential_value_len(value: &str, unquoted_limit: usize) -> usize {
    let bytes = value.as_bytes();
    let opening_slashes = bytes.iter().take_while(|byte| **byte == b'\\').count();
    if let Some(quote @ (b'"' | b'\'')) = bytes.get(opening_slashes) {
        // 仅同一转义层的引号可闭合；无法确认边界时多遮盖，避免泄露密码尾部。
        let mut slashes = 0;
        for (index, byte) in bytes.iter().enumerate().skip(opening_slashes + 1) {
            if byte == quote && slashes == opening_slashes {
                return index + 1;
            }
            slashes = if *byte == b'\\' { slashes + 1 } else { 0 };
        }
        // 不完整的引号内容整体遮盖，避免保留密码片段。
        return value.len();
    }

    let mut escaped = false;
    // DSN 未引用密码中的标点也可能是凭据，只以未转义的空白为结束边界。
    for (index, ch) in value[..unquoted_limit].char_indices() {
        if ch == '\\' {
            escaped = true;
        } else if escaped {
            escaped = false;
        } else if ch.is_whitespace() {
            return index;
        }
    }
    unquoted_limit
}

#[derive(Debug, Clone)]
pub(crate) enum GatewayError {
    UpstreamUnavailable {
        trace_id: String,
        message: String,
    },
    ControlUnavailable {
        trace_id: String,
        message: String,
    },
    LocalExecutionPlanningTimeout {
        trace_id: String,
        phase: &'static str,
        timeout_ms: u64,
    },
    AdmissionTimeout {
        trace_id: String,
        gate: &'static str,
        queue_budget_ms: u64,
    },
    Client {
        status: StatusCode,
        message: String,
    },
    PlanUsageLimited(crate::plan_usage_policy::PlanUsagePolicyRejection),
    LastActiveAdminUpdateDenied,
    LastActiveAdminDeleteDenied,
    Internal(String),
}

impl GatewayError {
    pub(crate) fn into_message(self) -> String {
        match self {
            Self::UpstreamUnavailable { message, .. }
            | Self::ControlUnavailable { message, .. }
            | Self::Client { message, .. }
            | Self::Internal(message) => message,
            Self::PlanUsageLimited(rejection) => format!(
                "subscription plan {} limit {} reached for {} window; retry after {} seconds",
                rejection.metric, rejection.limit, rejection.window, rejection.retry_after
            ),
            Self::LocalExecutionPlanningTimeout {
                phase, timeout_ms, ..
            } => {
                format!("local execution planning timed out in {phase} after {timeout_ms}ms")
            }
            Self::AdmissionTimeout {
                gate,
                queue_budget_ms,
                ..
            } => {
                format!("gateway admission gate {gate} timed out after {queue_budget_ms}ms")
            }
            Self::LastActiveAdminUpdateDenied => "不能降级或停用最后一个管理员账户".to_string(),
            Self::LastActiveAdminDeleteDenied => "不能删除最后一个管理员账户".to_string(),
        }
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response<Body> {
        match self {
            Self::UpstreamUnavailable { trace_id, message } => {
                let error_fingerprint = gateway_error_fingerprint(&message);
                warn!(
                    trace_id = %trace_id,
                    error_fingerprint,
                    error_length = message.len(),
                    "gateway proxy unavailable"
                );
                let body = Json(json!({
                    "error": {
                        "message": "gateway proxy unavailable",
                        "trace_id": trace_id,
                    }
                }));
                let mut response = (StatusCode::BAD_GATEWAY, body).into_response();
                let _ =
                    insert_header_if_missing(response.headers_mut(), TRACE_ID_HEADER, &trace_id);
                let _ = insert_header_if_missing(
                    response.headers_mut(),
                    GATEWAY_HEADER,
                    "rust-phase3b",
                );
                response
            }
            Self::ControlUnavailable { trace_id, message } => {
                let error_fingerprint = gateway_error_fingerprint(&message);
                warn!(
                    trace_id = %trace_id,
                    error_fingerprint,
                    error_length = message.len(),
                    "gateway control unavailable"
                );
                let body = Json(json!({
                    "error": {
                        "message": "gateway control unavailable",
                        "trace_id": trace_id,
                    }
                }));
                let mut response = (StatusCode::BAD_GATEWAY, body).into_response();
                let _ =
                    insert_header_if_missing(response.headers_mut(), TRACE_ID_HEADER, &trace_id);
                let _ = insert_header_if_missing(
                    response.headers_mut(),
                    GATEWAY_HEADER,
                    "rust-phase3b",
                );
                response
            }
            Self::LocalExecutionPlanningTimeout {
                trace_id,
                phase,
                timeout_ms,
            } => {
                warn!(
                    trace_id = %trace_id,
                    phase,
                    timeout_ms,
                    "gateway local execution planning timed out"
                );
                let body = Json(json!({
                    "error": {
                        "message": "gateway local execution planning timed out",
                        "trace_id": trace_id,
                    }
                }));
                let mut response = (StatusCode::GATEWAY_TIMEOUT, body).into_response();
                let _ =
                    insert_header_if_missing(response.headers_mut(), TRACE_ID_HEADER, &trace_id);
                let _ = insert_header_if_missing(
                    response.headers_mut(),
                    GATEWAY_HEADER,
                    "rust-phase3b",
                );
                response
            }
            Self::AdmissionTimeout {
                trace_id,
                gate,
                queue_budget_ms,
            } => {
                tracing::debug!(
                    trace_id = %trace_id,
                    gate,
                    queue_budget_ms,
                    "gateway admission gate timed out"
                );
                let body = Json(json!({
                    "error": {
                        "message": "gateway admission queue timed out",
                        "trace_id": trace_id,
                    }
                }));
                let mut response = (StatusCode::TOO_MANY_REQUESTS, body).into_response();
                let _ =
                    insert_header_if_missing(response.headers_mut(), TRACE_ID_HEADER, &trace_id);
                let _ = insert_header_if_missing(
                    response.headers_mut(),
                    GATEWAY_HEADER,
                    "rust-phase3b",
                );
                let _ = insert_header_if_missing(response.headers_mut(), "Retry-After", "1");
                response
            }
            Self::Client { status, message } => (
                status,
                Json(json!({
                    "error": {
                        "message": message,
                    }
                })),
            )
                .into_response(),
            Self::PlanUsageLimited(rejection) => (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "error": {
                        "type": "plan_usage_limit_exceeded",
                        "message": "套餐使用限制已达到上限，请稍后重试",
                        "details": {
                            "metric": rejection.metric,
                            "window": rejection.window,
                            "limit": rejection.limit,
                            "retry_after": rejection.retry_after,
                        }
                    }
                })),
            )
                .into_response(),
            Self::LastActiveAdminUpdateDenied => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "不能降级或停用最后一个管理员账户" })),
            )
                .into_response(),
            Self::LastActiveAdminDeleteDenied => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "不能删除最后一个管理员账户" })),
            )
                .into_response(),
            Self::Internal(message) => {
                log_gateway_internal_error(&message, gateway_error_detail_logging_enabled());
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": {
                            "message": "internal server error",
                        }
                    })),
                )
                    .into_response()
            }
        }
    }
}

fn log_gateway_internal_error(message: &str, detail_logging: bool) {
    let error_fingerprint = gateway_error_fingerprint(message);
    if detail_logging {
        tracing::error!(
            event_name = "gateway_internal_error",
            error_fingerprint,
            error_length = message.len(),
            error_detail = %redact_error_str_unbounded(message),
            "internal gateway error hidden from client"
        );
    } else {
        tracing::error!(
            event_name = "gateway_internal_error",
            error_fingerprint,
            error_length = message.len(),
            "internal gateway error hidden from client"
        );
    }
}

fn gateway_error_fingerprint(message: &str) -> String {
    let digest = Sha256::digest(message.as_bytes());
    format!("{:x}", digest)[..16].to_string()
}

impl From<AiSurfaceFinalizeError> for GatewayError {
    fn from(error: AiSurfaceFinalizeError) -> Self {
        GatewayError::Internal(error.0)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::body::to_bytes;
    use axum::http::{header::RETRY_AFTER, StatusCode};
    use axum::response::IntoResponse;

    use crate::constants::TRACE_ID_HEADER;

    use super::{
        gateway_error_fingerprint, log_gateway_internal_error, parse_gateway_error_detail_logging,
        redact_error_debug, redact_error_detail, redact_error_str, GatewayError,
    };

    #[test]
    fn detail_logging_only_accepts_lowercase_true_and_false() {
        assert!(parse_gateway_error_detail_logging(Some("true")));
        assert!(!parse_gateway_error_detail_logging(Some("false")));
        assert!(!parse_gateway_error_detail_logging(None));
        for value in [
            "1", "0", "yes", "no", "on", "off", "TRUE", "FALSE", "True", " true ", "true\n", "",
            "invalid",
        ] {
            assert!(
                !parse_gateway_error_detail_logging(Some(value)),
                "value: {value:?}"
            );
        }
    }

    #[derive(Clone, Default)]
    struct LogBuffer(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("log buffer should lock")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn internal_error_log(message: &str, detail_logging: bool) -> serde_json::Value {
        let buffer = LogBuffer::default();
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_max_level(tracing::Level::ERROR)
            .with_writer(move || writer.clone())
            .finish();
        // 使用线程局部日志捕获和显式开关，不修改进程环境以免干扰并发测试。
        tracing::subscriber::with_default(subscriber, || {
            log_gateway_internal_error(message, detail_logging);
        });
        let bytes = buffer.0.lock().expect("log buffer should lock");
        serde_json::from_slice(&bytes).expect("internal error log should be JSON")
    }

    #[tokio::test]
    async fn internal_errors_do_not_expose_internal_details() {
        let response = GatewayError::Internal(
            "database connection failed: password=internal-secret".to_string(),
        )
        .into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("internal error response body should read");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("internal error response should be JSON");
        assert_eq!(payload["error"]["message"], "internal server error");
        assert!(!String::from_utf8_lossy(&body).contains("internal-secret"));
    }

    #[test]
    fn error_fingerprints_are_stable_without_containing_source_text() {
        let secret_error = "postgresql://admin:database-secret@db.internal/aether";
        let fingerprint = gateway_error_fingerprint(secret_error);

        assert_eq!(fingerprint, gateway_error_fingerprint(secret_error));
        assert_eq!(fingerprint.len(), 16);
        assert!(fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(!fingerprint.contains("database-secret"));
    }

    #[test]
    fn admission_timeout_returns_429_with_retry_after_without_panicking() {
        let trace_id = "trace-admission-timeout".to_string();

        let response = GatewayError::AdmissionTimeout {
            trace_id: trace_id.clone(),
            gate: "gateway_upstream_execution",
            queue_budget_ms: 250,
        }
        .into_response();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("1")
        );
        assert_eq!(
            response
                .headers()
                .get(TRACE_ID_HEADER)
                .and_then(|v| v.to_str().ok()),
            Some(trace_id.as_str())
        );
    }

    #[test]
    fn redact_error_str_strips_url_credentials() {
        let input = "connect failed: postgresql://admin:s3cret@db.internal:5432/aether";
        let redacted = redact_error_str(input);
        assert!(!redacted.contains("s3cret"));
        assert!(redacted.contains("postgresql://"));
        assert!(redacted.contains("db.internal"));
    }

    #[test]
    fn redact_error_str_truncates_long_messages() {
        let long = "x".repeat(500);
        let redacted = redact_error_str(&long);
        assert_eq!(redacted, format!("{}...", "x".repeat(256)));
    }

    #[test]
    fn redact_error_str_preserves_short_messages() {
        let input = "connection refused";
        assert_eq!(redact_error_str(input), input);
    }

    #[test]
    fn redact_error_str_handles_multiple_urls() {
        for (input, expected) in [
            (
                "https://u:first@a/p,https://v:second@b/q",
                "https://a/p,https://b/q",
            ),
            (
                "https://u:first@a,https://v:second@b",
                "https://a,https://b",
            ),
            (
                "failed: https://user:token123@api.example.com/v1 and http://admin:pass@internal.io",
                "failed: https://api.example.com/v1 and http://internal.io",
            ),
        ] {
            assert_eq!(redact_error_str(input), expected);
        }
    }

    #[test]
    fn redact_error_str_handles_url_authority_boundaries_and_punctuation() {
        for (input, expected) in [
            (
                r#"url="postgres://u:p@h", retry=2"#,
                r#"url="postgres://h", retry=2"#,
            ),
            (
                "(https://u:p@h?mode=test#detail)",
                "(https://h?mode=test#detail)",
            ),
            (
                "postgres://u:p@ss@[::1]:5432/db",
                "postgres://[::1]:5432/db",
            ),
            ("https://u:p,a;s's@h/p", "https://h/p"),
            (
                "https://h/path@name?email=a@b#ref@c",
                "https://h/path@name?email=a@b#ref@c",
            ),
        ] {
            assert_eq!(redact_error_str(input), expected);
        }
    }

    #[test]
    fn redact_error_str_redacts_before_truncation() {
        let prefix = format!("{} ", "x".repeat(239));
        let input = format!("{prefix}postgres://u:supersecret@db/app");
        assert_eq!(
            redact_error_str(&input),
            format!("{prefix}postgres://db/ap...")
        );

        let input = format!("password={} host=db", "secret".repeat(100));
        assert_eq!(redact_error_str(&input), "password=[REDACTED] host=db");
    }

    #[test]
    fn redact_error_str_preserves_utf8_and_whitespace() {
        let input = " first  \n\tsecond \r\n";
        assert_eq!(redact_error_str(input), input);
        assert_eq!(
            redact_error_str("  failed:\n\tpostgres://u:p@h\r\n  retry"),
            "  failed:\n\tpostgres://h\r\n  retry"
        );
        for length in [254, 255, 256] {
            let prefix = "x".repeat(length);
            assert_eq!(
                redact_error_str(&format!("{prefix}错误")),
                format!("{prefix}...")
            );
        }
        assert_eq!(redact_error_str(&"x".repeat(256)), "x".repeat(256));
    }

    #[test]
    fn redact_error_str_masks_common_credentials() {
        for key in [
            "password",
            "PASSWORD",
            "passwd",
            "pwd",
            "token",
            "access_token",
            "refresh-token",
            "idToken",
            "session_token",
            "auth-token",
            "secret",
            "client_secret",
            "clientSecret",
            "api_key",
            "api-key",
            "apiKey",
            "access_key",
            "secret_key",
            "private_key",
            "x-api-key",
        ] {
            for separator in ["=", ":", " = ", "\t:\t"] {
                let input = format!("{key}{separator}test-credential retry=2");
                assert_eq!(
                    redact_error_str(&input),
                    format!("{key}{separator}[REDACTED] retry=2")
                );
            }
        }
        assert_eq!(
            redact_error_str("password=one;token=two,secret=three&retry=2"),
            "password=[REDACTED];token=[REDACTED],secret=[REDACTED]"
        );
        assert_eq!(
            redact_error_str("https://u:p@host/path?token=abc&api_key=xyz#details"),
            "https://host/path?token=[REDACTED]&api_key=[REDACTED]"
        );
    }

    #[test]
    fn redact_error_str_does_not_expose_punctuation_in_unquoted_passwords() {
        for secret in [
            "one#two", "one?two", "one&two", "one,two", "one;two", "one)two", "one\"two",
        ] {
            assert_eq!(
                redact_error_str(&format!("password={secret} host=db")),
                "password=[REDACTED] host=db"
            );
        }
        assert_eq!(
            redact_error_str("https://host/db?password=one?two&mode=test"),
            "https://host/db?password=[REDACTED]"
        );
        assert_eq!(
            redact_error_str(r#"password=one,token="two words" retry=2"#),
            "password=[REDACTED],token=[REDACTED] retry=2"
        );
    }

    #[test]
    fn redact_error_str_masks_authorization_and_bearer_values() {
        for (input, expected) in [
            (
                "Authorization: Bearer short",
                "Authorization: Bearer [REDACTED]",
            ),
            (
                "authorization=bEaReR\tabc.def",
                "authorization=bEaReR\t[REDACTED]",
            ),
            (
                "Proxy-Authorization: Basic abc==",
                "Proxy-Authorization: Basic [REDACTED]",
            ),
            (
                r#"{"Authorization": "Bearer secret value"}"#,
                r#"{"Authorization": [REDACTED]}"#,
            ),
            (
                "error: BEARER a+/b==, retry=2",
                "error: BEARER [REDACTED] retry=2",
            ),
        ] {
            assert_eq!(redact_error_str(input), expected);
        }
    }

    #[test]
    fn redact_error_str_masks_quoted_and_escaped_values() {
        for (input, expected) in [
            (
                "password='space secret' host=db",
                "password=[REDACTED] host=db",
            ),
            (
                r#"password="space \"secret" host=db"#,
                "password=[REDACTED] host=db",
            ),
            (
                r"password=space\ secret host=db",
                "password=[REDACTED] host=db",
            ),
            (
                r#"Error { password: "space secret", token: Some("option-secret") }"#,
                "Error { password: [REDACTED], token: Some([REDACTED]) }",
            ),
            ("password='unterminated secret", "password=[REDACTED]"),
            (
                r#"password="another unterminated secret"#,
                "password=[REDACTED]",
            ),
            (
                r#"password="token=inner-secret" retry=2"#,
                "password=[REDACTED] retry=2",
            ),
        ] {
            assert_eq!(redact_error_str(input), expected);
        }
    }

    #[test]
    fn display_and_debug_error_helpers_redact_real_formatted_values() {
        let input =
            r#"{"password": "space \"escaped-secret", "token": "token-secret", "retry": 2}"#;
        for redacted in [redact_error_detail(&input), redact_error_debug(&input)] {
            assert!(!redacted.contains("escaped-secret"));
            assert!(!redacted.contains("token-secret"));
            assert!(redacted.contains("retry"));
            assert_eq!(redacted.matches("[REDACTED]").count(), 2);
        }
    }

    #[test]
    fn debug_error_redaction_handles_escaped_whitespace_and_single_quotes() {
        for input in [
            "Authorization:\nBearer test-secret",
            "Authorization:\tBearer test-secret",
            "\nBearer test-secret",
            "Bearer\ntest-secret",
            r"password=space\ test-secret host=db",
            r"password='space \'test-secret' host=db",
        ] {
            let redacted = redact_error_debug(&input);
            assert!(!redacted.contains("test-secret"), "redacted: {redacted}");
            assert!(redacted.contains("[REDACTED]"));
        }
    }

    #[test]
    fn internal_error_detail_logging_redacts_without_truncating() {
        let padding = "x".repeat(300);
        let message = format!("password=first-secret {padding}\nhttps://u:second-secret@db/path token=third-secret\nretry exhausted");
        let log = internal_error_log(&message, true);
        let fields = &log["fields"];
        assert_eq!(log["level"], "ERROR");
        assert_eq!(fields["event_name"], "gateway_internal_error");
        assert_eq!(fields["error_length"], message.len());
        assert_eq!(
            fields["error_fingerprint"],
            gateway_error_fingerprint(&message)
        );
        assert_eq!(
            fields["error_detail"],
            format!(
                "password=[REDACTED] {padding}\nhttps://db/path token=[REDACTED]\nretry exhausted"
            )
        );
        for secret in ["first-secret", "second-secret", "third-secret"] {
            assert!(!log.to_string().contains(secret));
        }
    }

    #[test]
    fn internal_error_logging_omits_details_when_disabled() {
        let message = "password=internal-secret";
        let log = internal_error_log(message, false);
        let fields = &log["fields"];
        assert_eq!(fields["event_name"], "gateway_internal_error");
        assert_eq!(
            fields["error_fingerprint"],
            gateway_error_fingerprint(message)
        );
        assert_eq!(fields["error_length"], message.len());
        assert!(fields.get("error_detail").is_none());
        assert!(!log.to_string().contains("internal-secret"));
    }
}
