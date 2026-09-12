use crate::handlers::shared::{
    decrypt_or_migrate_system_config_secret, system_config_bool, system_config_string,
};
use crate::{AppState, GatewayError};
use serde_json::Value;
use std::net::SocketAddr;
use std::time::Duration;

pub(crate) const SERVER_CHAN_PUSH_ENABLED_KEY: &str = "module.server_chan_push.enabled";
pub(crate) const SERVER_CHAN_PUSH_SEND_KEY_KEY: &str = "module.server_chan_push.send_key";
pub(crate) const SERVER_CHAN_PUSH_TEMPLATE_KEY: &str = "module.server_chan_push.template";
pub(crate) const LEGACY_SERVER_CHAN_ENABLED_KEY: &str =
    "module.important_notification.server_chan_enabled";
pub(crate) const LEGACY_SERVER_CHAN_SEND_KEY_KEY: &str =
    "module.important_notification.server_chan_send_key";
pub(crate) const LEGACY_SERVER_CHAN_TEMPLATE_KEY: &str =
    "module.important_notification.server_chan_template";

const SERVER_CHAN_API_BASE: &str = "https://sctapi.ftqq.com";
const SERVER_CHAN_API_HOST: &str = "sctapi.ftqq.com";
const SERVER_CHAN_API_PORT: u16 = 443;
const SERVER_CHAN_DNS_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PUSH_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_SERVER_CHAN_SEND_KEY_BYTES: usize = 512;
const MAX_SERVER_CHAN_TEMPLATE_BYTES: usize = 256 * 1024;
const MAX_SERVER_CHAN_TITLE_BYTES: usize = 512;
const MAX_SERVER_CHAN_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_SERVER_CHAN_RENDERED_BODY_BYTES: usize = 2 * 1024 * 1024;

#[cfg(test)]
fn build_server_chan_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(300))
        .build()
}

fn validate_server_chan_resolved_addresses(
    addresses: &[SocketAddr],
    allow_benchmarking_ip: bool,
) -> Result<(), &'static str> {
    if addresses.is_empty() {
        return Err("Server Chan API DNS resolution returned no addresses");
    }
    if addresses.iter().any(|address| {
        aether_http::is_private_or_reserved_ip(address.ip())
            && !(allow_benchmarking_ip && aether_http::is_ipv4_benchmarking_fake_ip(address.ip()))
    }) {
        return Err("Server Chan API resolved to a private or reserved address");
    }
    Ok(())
}

/// Build the production client with the API hostname pinned to one validated
/// DNS answer.  The SendKey is carried in the request path; allowing reqwest
/// to resolve the host again at connect time would let a DNS rebinding or a
/// poisoned resolver redirect that credential to an unintended destination.
async fn build_pinned_server_chan_client() -> Result<reqwest::Client, &'static str> {
    let addresses = aether_http::lookup_host_with_limits(
        SERVER_CHAN_API_HOST,
        SERVER_CHAN_API_PORT,
        SERVER_CHAN_DNS_TIMEOUT,
    )
    .await
    .map_err(|error| match error.kind() {
        std::io::ErrorKind::TimedOut => "Server Chan API DNS resolution timed out",
        _ => "Server Chan API DNS resolution failed",
    })?;
    validate_server_chan_resolved_addresses(&addresses, true)?;

    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(SERVER_CHAN_DNS_TIMEOUT)
        .timeout(Duration::from_secs(300))
        .resolve_to_addrs(SERVER_CHAN_API_HOST, &addresses)
        .build()
        .map_err(|_| "Server Chan HTTP client initialization failed")
}

#[derive(Clone)]
pub(crate) struct ServerChanPushConfig {
    pub(crate) enabled: bool,
    pub(crate) send_key: Option<String>,
    pub(crate) template: Option<String>,
}

impl std::fmt::Debug for ServerChanPushConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerChanPushConfig")
            .field("enabled", &self.enabled)
            .field("send_key", &self.send_key.as_ref().map(|_| "[REDACTED]"))
            .field("template", &self.template.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

pub(crate) async fn server_chan_push_module_enabled(
    state: &AppState,
) -> Result<bool, GatewayError> {
    let canonical = state
        .read_system_config_json_value(SERVER_CHAN_PUSH_ENABLED_KEY)
        .await?;
    if canonical.is_some() {
        return Ok(system_config_bool(canonical.as_ref(), false));
    }
    let legacy = state
        .read_system_config_json_value(LEGACY_SERVER_CHAN_ENABLED_KEY)
        .await?;
    Ok(system_config_bool(legacy.as_ref(), false))
}

pub(crate) async fn server_chan_push_configured(state: &AppState) -> Result<bool, GatewayError> {
    Ok(read_server_chan_push_config(state)
        .await?
        .send_key
        .is_some())
}

pub(crate) async fn read_server_chan_push_config(
    state: &AppState,
) -> Result<ServerChanPushConfig, GatewayError> {
    let enabled = server_chan_push_module_enabled(state).await?;
    let send_key = read_server_chan_secret(state).await?;
    let template = read_server_chan_value(
        state,
        SERVER_CHAN_PUSH_TEMPLATE_KEY,
        LEGACY_SERVER_CHAN_TEMPLATE_KEY,
    )
    .await?
    .and_then(|value| system_config_string(Some(&value)));
    if let Some(template) = template.as_deref() {
        validate_server_chan_field("template", template, MAX_SERVER_CHAN_TEMPLATE_BYTES)?;
    }

    Ok(ServerChanPushConfig {
        enabled,
        send_key,
        template,
    })
}

async fn read_server_chan_secret(state: &AppState) -> Result<Option<String>, GatewayError> {
    for key in [
        SERVER_CHAN_PUSH_SEND_KEY_KEY,
        LEGACY_SERVER_CHAN_SEND_KEY_KEY,
    ] {
        let Some(value) = state.read_system_config_json_value(key).await? else {
            continue;
        };
        let Some(value) = system_config_string(Some(&value)) else {
            return Ok(None);
        };
        let value = decrypt_or_migrate_system_config_secret(state, key, value).await?;
        validate_server_chan_field("send_key", &value, MAX_SERVER_CHAN_SEND_KEY_BYTES)?;
        return Ok(Some(value));
    }
    Ok(None)
}

async fn read_server_chan_value(
    state: &AppState,
    canonical_key: &str,
    legacy_key: &str,
) -> Result<Option<Value>, GatewayError> {
    let canonical = state.read_system_config_json_value(canonical_key).await?;
    if canonical.is_some() {
        return Ok(canonical);
    }
    state.read_system_config_json_value(legacy_key).await
}

pub(crate) async fn send_server_chan_push(
    _state: &AppState,
    config: &ServerChanPushConfig,
    title: &str,
    markdown_body: &str,
) -> Result<(), GatewayError> {
    let Some(send_key) = config.send_key.as_deref() else {
        return Err(GatewayError::Internal(
            "未配置 Server 酱 SendKey".to_string(),
        ));
    };
    let send_key = send_key.trim();
    if send_key.is_empty() {
        return Err(GatewayError::Internal(
            "Server 酱 SendKey 不能为空".to_string(),
        ));
    }
    if !is_safe_server_chan_send_key(send_key) {
        return Err(GatewayError::Internal(
            "Server 酱 SendKey 格式无效".to_string(),
        ));
    }
    validate_server_chan_field("send_key", send_key, MAX_SERVER_CHAN_SEND_KEY_BYTES)?;
    validate_server_chan_field("title", title, MAX_SERVER_CHAN_TITLE_BYTES)?;
    validate_server_chan_field("body", markdown_body, MAX_SERVER_CHAN_BODY_BYTES)?;
    let desp = render_server_chan_desp(config.template.as_deref(), title, markdown_body)?;
    let url = format!("{SERVER_CHAN_API_BASE}/{send_key}.send");
    let client = build_pinned_server_chan_client()
        .await
        .map_err(|message| GatewayError::Internal(message.to_string()))?;
    let response = client
        .post(url)
        .form(&[("title", title), ("desp", desp.as_str())])
        .send()
        .await
        .map_err(|err| GatewayError::Internal(server_chan_request_error_message(&err)))?;
    let status = response.status();
    let body = aether_http::read_response_bytes_with_limit(response, MAX_PUSH_RESPONSE_BYTES)
        .await
        .map_err(|err| GatewayError::Internal(server_chan_response_body_error_message(&err)))?;
    let text = String::from_utf8_lossy(&body);
    if let Some(message) = server_chan_response_failure_message(status, &text) {
        return Err(GatewayError::Internal(message));
    }
    Ok(())
}

fn is_safe_server_chan_send_key(send_key: &str) -> bool {
    !send_key.is_empty()
        && send_key.len() <= MAX_SERVER_CHAN_SEND_KEY_BYTES
        && send_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn server_chan_response_failure_message(status: http::StatusCode, text: &str) -> Option<String> {
    if !status.is_success() {
        return Some(format!("Server Chan returned HTTP {status}"));
    }
    let payload = serde_json::from_str::<Value>(text).ok()?;
    let code_is_ok = payload
        .get("code")
        .and_then(|value| {
            value
                .as_i64()
                .map(|code| code == 0)
                .or_else(|| value.as_str().map(|code| code.trim() == "0"))
        })
        .unwrap_or(true);
    (!code_is_ok).then(|| "Server Chan returned failure".to_string())
}

fn server_chan_request_error_message(error: &reqwest::Error) -> String {
    // Reqwest errors may include the request URL. ServerChan authenticates with
    // the SendKey in that URL's path, so forwarding the source error would leak
    // the credential into logs and notification test responses.
    format!("Server Chan request failed ({})", reqwest_error_kind(error))
}

fn server_chan_response_body_error_message(error: &aether_http::ResponseBodyReadError) -> String {
    match error {
        aether_http::ResponseBodyReadError::TooLarge { max_bytes } => {
            format!("Server Chan response exceeds {max_bytes} bytes")
        }
        aether_http::ResponseBodyReadError::Read(error) => format!(
            "Server Chan response read failed ({})",
            reqwest_error_kind(error)
        ),
    }
}

fn reqwest_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_request() {
        "request"
    } else {
        "transport"
    }
}

fn validate_server_chan_field(
    field: &str,
    value: &str,
    max_bytes: usize,
) -> Result<(), GatewayError> {
    if value.len() > max_bytes || value.bytes().any(|byte| byte == 0) {
        return Err(GatewayError::Internal(format!(
            "Server Chan {field} exceeds the allowed size or contains a NUL byte"
        )));
    }
    Ok(())
}

fn render_server_chan_desp(
    template: Option<&str>,
    title: &str,
    markdown_body: &str,
) -> Result<String, GatewayError> {
    validate_server_chan_field("title", title, MAX_SERVER_CHAN_TITLE_BYTES)?;
    validate_server_chan_field("body", markdown_body, MAX_SERVER_CHAN_BODY_BYTES)?;
    let template = template
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("{body}");
    validate_server_chan_field("template", template, MAX_SERVER_CHAN_TEMPLATE_BYTES)?;

    let mut rendered = String::with_capacity(
        template
            .len()
            .saturating_add(title.len())
            .saturating_add(markdown_body.len())
            .min(MAX_SERVER_CHAN_RENDERED_BODY_BYTES),
    );
    let mut cursor = 0usize;
    while cursor < template.len() {
        let remaining = &template[cursor..];
        let title_match = remaining.find("{title}");
        let body_match = remaining.find("{body}");
        let next = match (title_match, body_match) {
            (None, None) => {
                append_server_chan_rendered_part(&mut rendered, remaining)?;
                cursor = template.len();
                continue;
            }
            (Some(index), None) => (index, "{title}", title),
            (None, Some(index)) => (index, "{body}", markdown_body),
            (Some(title_index), Some(body_index)) if title_index <= body_index => {
                (title_index, "{title}", title)
            }
            (Some(_), Some(body_index)) => (body_index, "{body}", markdown_body),
        };
        append_server_chan_rendered_part(&mut rendered, &remaining[..next.0])?;
        append_server_chan_rendered_part(&mut rendered, next.2)?;
        cursor += next.0 + next.1.len();
    }
    Ok(rendered)
}

fn append_server_chan_rendered_part(output: &mut String, part: &str) -> Result<(), GatewayError> {
    let next_len = output.len().checked_add(part.len()).ok_or_else(|| {
        GatewayError::Internal("Server Chan rendered body is too large".to_string())
    })?;
    if next_len > MAX_SERVER_CHAN_RENDERED_BODY_BYTES {
        return Err(GatewayError::Internal(
            "Server Chan rendered body exceeds the allowed size".to_string(),
        ));
    }
    output.push_str(part);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        build_server_chan_client, is_safe_server_chan_send_key, read_server_chan_push_config,
        render_server_chan_desp, server_chan_request_error_message,
        server_chan_response_body_error_message, server_chan_response_failure_message,
        validate_server_chan_resolved_addresses, LEGACY_SERVER_CHAN_SEND_KEY_KEY,
    };
    use crate::data::GatewayDataState;
    use crate::handlers::shared::decrypt_system_config_secret;
    use crate::AppState;
    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
    use axum::{
        http::{header, StatusCode},
        response::IntoResponse,
        routing::post,
        Router,
    };
    use std::net::SocketAddr;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn legacy_send_key_is_migrated_at_its_original_config_key() {
        let plaintext = "SCT-legacy-plaintext-send-key";
        let data = GatewayDataState::disabled()
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY)
            .with_system_config_values_for_tests([(
                LEGACY_SERVER_CHAN_SEND_KEY_KEY.to_string(),
                serde_json::json!(plaintext),
            )]);
        let mut state = AppState::new().expect("gateway state should build");
        state.replace_data_state(Arc::new(data));

        let config = read_server_chan_push_config(&state)
            .await
            .expect("legacy config should read");
        assert_eq!(config.send_key.as_deref(), Some(plaintext));

        let stored = state
            .read_system_config_json_value_strong(LEGACY_SERVER_CHAN_SEND_KEY_KEY)
            .await
            .expect("migrated legacy key should read")
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .expect("migrated legacy key should remain a string");
        assert_ne!(stored, plaintext);
        assert_eq!(
            decrypt_system_config_secret(&state, LEGACY_SERVER_CHAN_SEND_KEY_KEY, &stored)
                .expect("migrated legacy key should decrypt"),
            plaintext
        );
    }

    #[tokio::test]
    async fn server_chan_client_never_forwards_send_key_across_redirects() {
        let redirected_hits = Arc::new(AtomicUsize::new(0));
        let redirected_hits_for_route = Arc::clone(&redirected_hits);
        let redirected_listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("redirect target listener");
        let redirected_addr = redirected_listener
            .local_addr()
            .expect("redirect target addr");
        let redirected_app = Router::new().route(
            "/capture",
            post(move || {
                let hits = Arc::clone(&redirected_hits_for_route);
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        );
        let redirected_server = tokio::spawn(async move {
            axum::serve(redirected_listener, redirected_app)
                .await
                .expect("redirect target server");
        });

        let source_listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("redirect source listener");
        let source_addr = source_listener.local_addr().expect("redirect source addr");
        let location = format!("http://{redirected_addr}/capture");
        let source_app = Router::new().route(
            "/SCT-secret-send-key.send",
            post(move || {
                let location = location.clone();
                async move { (StatusCode::FOUND, [(header::LOCATION, location)]).into_response() }
            }),
        );
        let source_server = tokio::spawn(async move {
            axum::serve(source_listener, source_app)
                .await
                .expect("redirect source server");
        });

        let response = build_server_chan_client()
            .expect("client")
            .post(format!("http://{source_addr}/SCT-secret-send-key.send"))
            .send()
            .await
            .expect("redirect response");
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(redirected_hits.load(Ordering::SeqCst), 0);

        source_server.abort();
        redirected_server.abort();
    }

    #[tokio::test]
    async fn server_chan_transport_error_does_not_expose_send_key_url() {
        let send_key = "SCT-secret-send-key";
        let error = reqwest::Client::new()
            .post(format!("ftp://sctapi.ftqq.com/{send_key}.send"))
            .send()
            .await
            .expect_err("unsupported URL scheme should fail before network I/O");

        let message = server_chan_request_error_message(&error);
        assert!(message.starts_with("Server Chan request failed ("));
        assert!(!message.contains(send_key));
        assert!(!message.contains("sctapi.ftqq.com"));
        assert!(!message.contains(".send"));

        let body_error = aether_http::ResponseBodyReadError::Read(error);
        let message = server_chan_response_body_error_message(&body_error);
        assert!(message.starts_with("Server Chan response read failed ("));
        assert!(!message.contains(send_key));
        assert!(!message.contains("sctapi.ftqq.com"));
        assert!(!message.contains(".send"));
    }

    #[test]
    fn server_chan_dns_answers_must_be_public() {
        assert!(validate_server_chan_resolved_addresses(
            &[SocketAddr::from(([1, 1, 1, 1], 443))],
            false,
        )
        .is_ok());
        for address in [
            SocketAddr::from(([127, 0, 0, 1], 443)),
            SocketAddr::from(([10, 0, 0, 1], 443)),
            SocketAddr::from(([169, 254, 169, 254], 443)),
        ] {
            assert!(
                validate_server_chan_resolved_addresses(&[address], false).is_err(),
                "private Server Chan DNS answer should be rejected: {address}"
            );
        }
        assert!(validate_server_chan_resolved_addresses(&[], false).is_err());
    }

    #[test]
    fn server_chan_dns_allows_benchmarking_ip_for_builtin_host() {
        let fake = SocketAddr::from(([198, 18, 75, 234], 443));
        assert!(validate_server_chan_resolved_addresses(&[fake], true).is_ok());
        assert!(validate_server_chan_resolved_addresses(
            &[fake, SocketAddr::from(([127, 0, 0, 1], 443))],
            true,
        )
        .is_err());
        assert!(validate_server_chan_resolved_addresses(&[fake], false).is_err());
    }

    #[test]
    fn server_chan_failure_response_does_not_expose_arbitrary_body() {
        let secret_body = "upstream echoed SCT-secret-send-key and internal details";
        let message =
            server_chan_response_failure_message(http::StatusCode::BAD_GATEWAY, secret_body)
                .expect("non-success response should fail");
        assert_eq!(message, "Server Chan returned HTTP 502 Bad Gateway");
        assert!(!message.contains(secret_body));
        assert!(!message.contains("SCT-secret-send-key"));

        let message = server_chan_response_failure_message(
            http::StatusCode::OK,
            r#"{"code":"SCT-secret-send-key","message":"internal details"}"#,
        )
        .expect("non-zero business response should fail");
        assert_eq!(message, "Server Chan returned failure");
    }

    #[test]
    fn server_chan_send_key_cannot_escape_the_request_path() {
        assert!(is_safe_server_chan_send_key("SCT123abc-._"));
        for unsafe_key in [
            "SCT/other",
            "SCT?token=secret",
            "SCT#fragment",
            "SCT\\other",
            "SCT key",
            "SCT\r\nX-Injected: yes",
        ] {
            assert!(
                !is_safe_server_chan_send_key(unsafe_key),
                "unsafe key: {unsafe_key:?}"
            );
        }
    }

    #[test]
    fn server_chan_desp_uses_template_when_provided() {
        let rendered =
            render_server_chan_desp(Some("**{title}**\n\n{body}\n\n--end--"), "告警", "原始正文")
                .expect("template should render");
        assert_eq!(rendered, "**告警**\n\n原始正文\n\n--end--");
    }

    #[test]
    fn server_chan_desp_falls_back_to_markdown_body_for_empty_template() {
        assert_eq!(
            render_server_chan_desp(None, "告警", "原始正文").expect("fallback should render"),
            "原始正文"
        );
        assert_eq!(
            render_server_chan_desp(Some("   "), "告警", "原始正文")
                .expect("fallback should render"),
            "原始正文"
        );
    }

    #[test]
    fn server_chan_desp_rejects_expansion_bombs_and_oversized_content() {
        let template = "{body}".repeat(super::MAX_SERVER_CHAN_TEMPLATE_BYTES / 6 + 1);
        assert!(render_server_chan_desp(Some(&template), "告警", "正文").is_err());
        let body = "x".repeat(super::MAX_SERVER_CHAN_BODY_BYTES + 1);
        assert!(render_server_chan_desp(None, "告警", &body).is_err());
    }
}
