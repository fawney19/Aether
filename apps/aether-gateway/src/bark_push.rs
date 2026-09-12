use crate::handlers::shared::{
    bark_device_key_binding, canonical_bark_server_url, decrypt_or_migrate_bark_device_key,
    system_config_bool, system_config_string,
};
use crate::{AppState, GatewayError};
use serde_json::{json, Value};
use std::net::{IpAddr, SocketAddr};

pub(crate) const BARK_PUSH_ENABLED_KEY: &str = "module.bark_push.enabled";
pub(crate) const BARK_PUSH_DEVICE_KEY_KEY: &str = "module.bark_push.device_key";
pub(crate) const BARK_PUSH_SERVER_URL_KEY: &str = "module.bark_push.server_url";
pub(crate) const BARK_PUSH_TEMPLATE_KEY: &str = "module.bark_push.template";

const DEFAULT_BARK_API_BASE: &str = "https://api.day.app";
const BARK_ALLOW_HTTP_ENV: &str = "AETHER_BARK_ALLOW_HTTP";
const BARK_ALLOW_PRIVATE_TARGETS_ENV: &str = "AETHER_BARK_ALLOW_PRIVATE_TARGETS";
const MAX_BARK_RESPONSE_BYTES: usize = 64 * 1024;
const BARK_CONNECT_TIMEOUT_MS: u64 = 10_000;
const BARK_REQUEST_TIMEOUT_MS: u64 = 300_000;
const MAX_BARK_SERVER_URL_BYTES: usize = 2 * 1024;
const MAX_BARK_DEVICE_KEY_BYTES: usize = 512;
const MAX_BARK_TEMPLATE_BYTES: usize = 256 * 1024;
const MAX_BARK_TITLE_BYTES: usize = 512;
const MAX_BARK_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_BARK_RENDERED_BODY_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct BarkPushConfig {
    pub(crate) enabled: bool,
    pub(crate) device_key: Option<String>,
    pub(crate) server_url: String,
    pub(crate) template: Option<String>,
}

impl std::fmt::Debug for BarkPushConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BarkPushConfig")
            .field("enabled", &self.enabled)
            .field(
                "device_key",
                &self.device_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("server_url", &self.server_url)
            .field("template", &self.template.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

pub(crate) async fn bark_push_module_enabled(state: &AppState) -> Result<bool, GatewayError> {
    let value = state
        .read_system_config_json_value(BARK_PUSH_ENABLED_KEY)
        .await?;
    Ok(system_config_bool(value.as_ref(), false))
}

pub(crate) async fn bark_push_configured(state: &AppState) -> Result<bool, GatewayError> {
    let config = read_bark_push_config(state).await?;
    Ok(config.device_key.is_some() && !config.server_url.trim().is_empty())
}

pub(crate) async fn read_bark_push_config(
    state: &AppState,
) -> Result<BarkPushConfig, GatewayError> {
    let enabled = bark_push_module_enabled(state).await?;
    let server_url = state
        .read_system_config_json_value(BARK_PUSH_SERVER_URL_KEY)
        .await?
        .and_then(|value| system_config_string(Some(&value)))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BARK_API_BASE.to_string());
    validate_bark_config_field("server_url", &server_url, MAX_BARK_SERVER_URL_BYTES)?;
    let server_url = normalized_bark_server_url(&server_url)?;
    let binding = bark_device_key_binding(&server_url)
        .ok_or_else(|| GatewayError::Internal("Bark 服务器地址不是合法 URL".to_string()))?;
    let device_key = state
        .read_system_config_json_value(BARK_PUSH_DEVICE_KEY_KEY)
        .await?
        .and_then(|value| system_config_string(Some(&value)));
    let device_key = match device_key {
        Some(value) => Some(decrypt_or_migrate_bark_device_key(state, &binding, value).await?),
        None => None,
    };
    if let Some(device_key) = device_key.as_deref() {
        validate_bark_config_field("device_key", device_key, MAX_BARK_DEVICE_KEY_BYTES)?;
    }
    let template = state
        .read_system_config_json_value(BARK_PUSH_TEMPLATE_KEY)
        .await?
        .and_then(|value| system_config_string(Some(&value)));
    if let Some(template) = template.as_deref() {
        validate_bark_config_field("template", template, MAX_BARK_TEMPLATE_BYTES)?;
    }

    Ok(BarkPushConfig {
        enabled,
        device_key,
        server_url,
        template,
    })
}

pub(crate) async fn send_bark_push(
    _state: &AppState,
    config: &BarkPushConfig,
    title: &str,
    markdown_body: &str,
) -> Result<(), GatewayError> {
    let Some(device_key) = config.device_key.as_deref() else {
        return Err(GatewayError::Internal("未配置 Bark Device Key".to_string()));
    };
    let device_key = device_key.trim();
    if device_key.is_empty() {
        return Err(GatewayError::Internal(
            "Bark Device Key 不能为空".to_string(),
        ));
    }
    validate_bark_config_field("device_key", device_key, MAX_BARK_DEVICE_KEY_BYTES)?;
    validate_bark_content_field("title", title, MAX_BARK_TITLE_BYTES)?;
    validate_bark_content_field("body", markdown_body, MAX_BARK_BODY_BYTES)?;
    let (client, push_url) = build_bark_push_client_and_url(&config.server_url).await?;
    let body = render_bark_body(config.template.as_deref(), title, markdown_body)?;
    let response = client
        .post(push_url)
        .json(&json!({
            "device_key": device_key,
            "title": title,
            "body": body,
        }))
        .send()
        .await
        .map_err(|err| GatewayError::Internal(bark_request_error_message(&err)))?;
    let status = response.status();
    let body = aether_http::read_response_bytes_with_limit(response, MAX_BARK_RESPONSE_BYTES)
        .await
        .map_err(|err| GatewayError::Internal(bark_response_body_error_message(&err)))?;
    let text = String::from_utf8_lossy(&body);
    if !status.is_success() {
        return Err(GatewayError::Internal(format!("Bark 返回 HTTP {status}")));
    }
    if let Ok(payload) = serde_json::from_str::<Value>(&text) {
        let code_is_ok = payload
            .get("code")
            .and_then(|value| {
                value
                    .as_i64()
                    .map(|code| matches!(code, 0 | 200))
                    .or_else(|| {
                        value
                            .as_str()
                            .map(|code| matches!(code.trim(), "0" | "200"))
                    })
            })
            .unwrap_or(true);
        if !code_is_ok {
            return Err(GatewayError::Internal("Bark 返回失败".to_string()));
        }
    }
    Ok(())
}

fn bark_request_error_message(error: &reqwest::Error) -> String {
    format!("Bark 请求失败 ({})", bark_reqwest_error_kind(error))
}

fn bark_response_body_error_message(error: &aether_http::ResponseBodyReadError) -> String {
    match error {
        aether_http::ResponseBodyReadError::TooLarge { max_bytes } => {
            format!("Bark 响应超过 {max_bytes} 字节")
        }
        aether_http::ResponseBodyReadError::Read(error) => {
            format!("Bark 响应读取失败 ({})", bark_reqwest_error_kind(error))
        }
    }
}

fn bark_reqwest_error_kind(error: &reqwest::Error) -> &'static str {
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

fn normalized_bark_server_url(server_url: &str) -> Result<String, GatewayError> {
    canonical_bark_server_url(server_url)
        .ok_or_else(|| GatewayError::Internal("Bark 服务器地址不是合法 URL".to_string()))
}

async fn build_bark_push_client_and_url(
    server_url: &str,
) -> Result<(reqwest::Client, url::Url), GatewayError> {
    validate_bark_config_field("server_url", server_url, MAX_BARK_SERVER_URL_BYTES)?;
    let normalized = normalized_bark_server_url(server_url)?;
    let mut push_url = url::Url::parse(&normalized)
        .map_err(|_| GatewayError::Internal("Bark 服务器地址不是合法 URL".to_string()))?;
    validate_bark_transport_policy(&push_url, env_flag_enabled(BARK_ALLOW_HTTP_ENV))?;

    let host = push_url
        .host_str()
        .ok_or_else(|| GatewayError::Internal("Bark 服务器地址缺少主机名".to_string()))?
        .to_string();
    let port = push_url
        .port_or_known_default()
        .ok_or_else(|| GatewayError::Internal("Bark 服务器地址缺少端口".to_string()))?;
    let addresses = aether_http::lookup_host_with_limits(
        host.as_str(),
        port,
        std::time::Duration::from_millis(BARK_CONNECT_TIMEOUT_MS),
    )
    .await
    .map_err(|error| {
        let message = match error.kind() {
            std::io::ErrorKind::TimedOut => "Bark 服务器 DNS 解析超时",
            std::io::ErrorKind::InvalidData => "Bark 服务器 DNS 解析返回过多地址",
            _ => "Bark 服务器 DNS 解析失败",
        };
        GatewayError::Internal(message.to_string())
    })?;
    let allow_benchmarking_ip = push_url.scheme() == "https"
        && push_url.port_or_known_default() == Some(443)
        && host.eq_ignore_ascii_case("api.day.app");
    validate_bark_resolved_addresses(
        &addresses,
        env_flag_enabled(BARK_ALLOW_PRIVATE_TARGETS_ENV),
        allow_benchmarking_ip,
    )?;

    push_url
        .path_segments_mut()
        .map_err(|_| GatewayError::Internal("Bark 服务器地址不能作为基础 URL".to_string()))?
        .pop_if_empty()
        .push("push");

    let mut builder = aether_http::apply_http_client_config(
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none()),
        &aether_http::HttpClientConfig {
            connect_timeout_ms: Some(BARK_CONNECT_TIMEOUT_MS),
            request_timeout_ms: Some(BARK_REQUEST_TIMEOUT_MS),
            http2_adaptive_window: true,
            ..aether_http::HttpClientConfig::default()
        },
    );
    if host.parse::<IpAddr>().is_err() {
        builder = builder.resolve_to_addrs(&host, &addresses);
    }
    let client = builder
        .build()
        .map_err(|_| GatewayError::Internal("Bark HTTP 客户端初始化失败".to_string()))?;
    Ok((client, push_url))
}

fn validate_bark_transport_policy(url: &url::Url, allow_http: bool) -> Result<(), GatewayError> {
    if url.scheme() == "http" && !allow_http {
        return Err(GatewayError::Internal(format!(
            "Bark 服务器必须使用 HTTPS；如确需明文 HTTP，请显式设置 {BARK_ALLOW_HTTP_ENV}=true"
        )));
    }
    Ok(())
}

fn validate_bark_resolved_addresses(
    addresses: &[SocketAddr],
    allow_private: bool,
    allow_benchmarking_ip: bool,
) -> Result<(), GatewayError> {
    if addresses.is_empty() {
        return Err(GatewayError::Internal(
            "Bark 服务器 DNS 解析未返回地址".to_string(),
        ));
    }
    if !allow_private
        && addresses.iter().any(|address| {
            aether_http::is_private_or_reserved_ip(address.ip())
                && !(allow_benchmarking_ip
                    && aether_http::is_ipv4_benchmarking_fake_ip(address.ip()))
        })
    {
        return Err(GatewayError::Internal(format!(
            "Bark 服务器解析到私有或保留地址；如确需内网自建服务，请显式设置 {BARK_ALLOW_PRIVATE_TARGETS_ENV}=true"
        )));
    }
    Ok(())
}

fn env_flag_enabled(key: &str) -> bool {
    std::env::var(key).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn validate_bark_config_field(
    field: &str,
    value: &str,
    max_bytes: usize,
) -> Result<(), GatewayError> {
    if value.len() > max_bytes || value.bytes().any(|byte| byte == 0) {
        return Err(GatewayError::Internal(format!(
            "Bark {field} exceeds the allowed size or contains a NUL byte"
        )));
    }
    Ok(())
}

fn validate_bark_content_field(
    field: &str,
    value: &str,
    max_bytes: usize,
) -> Result<(), GatewayError> {
    validate_bark_config_field(field, value, max_bytes)
}

fn render_bark_body(
    template: Option<&str>,
    title: &str,
    markdown_body: &str,
) -> Result<String, GatewayError> {
    validate_bark_content_field("title", title, MAX_BARK_TITLE_BYTES)?;
    validate_bark_content_field("body", markdown_body, MAX_BARK_BODY_BYTES)?;
    let template = template
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("{body}");
    validate_bark_config_field("template", template, MAX_BARK_TEMPLATE_BYTES)?;

    let mut rendered = String::with_capacity(template.len().min(MAX_BARK_RENDERED_BODY_BYTES));
    let mut cursor = 0usize;
    while cursor < template.len() {
        let remaining = &template[cursor..];
        let title_match = remaining.find("{title}");
        let body_match = remaining.find("{body}");
        let next = match (title_match, body_match) {
            (None, None) => {
                append_bark_rendered_part(&mut rendered, remaining)?;
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
        append_bark_rendered_part(&mut rendered, &remaining[..next.0])?;
        append_bark_rendered_part(&mut rendered, next.2)?;
        cursor += next.0 + next.1.len();
    }
    if rendered.is_empty() && template.is_empty() {
        return Ok(String::new());
    }
    Ok(rendered)
}

fn append_bark_rendered_part(output: &mut String, part: &str) -> Result<(), GatewayError> {
    let next_len = output
        .len()
        .checked_add(part.len())
        .ok_or_else(|| GatewayError::Internal("Bark rendered body is too large".to_string()))?;
    if next_len > MAX_BARK_RENDERED_BODY_BYTES {
        return Err(GatewayError::Internal(
            "Bark rendered body exceeds the allowed size".to_string(),
        ));
    }
    output.push_str(part);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        bark_request_error_message, bark_response_body_error_message, normalized_bark_server_url,
        render_bark_body, validate_bark_resolved_addresses, validate_bark_transport_policy,
    };
    use std::net::SocketAddr;

    #[test]
    fn bark_body_uses_template_when_provided() {
        let rendered = render_bark_body(Some("{title}\n\n{body}"), "告警", "原始正文")
            .expect("template should render");
        assert_eq!(rendered, "告警\n\n原始正文");
    }

    #[test]
    fn bark_body_falls_back_to_markdown_body_for_empty_template() {
        assert_eq!(
            render_bark_body(None, "告警", "原始正文").expect("fallback should render"),
            "原始正文"
        );
        assert_eq!(
            render_bark_body(Some("   "), "告警", "原始正文").expect("fallback should render"),
            "原始正文"
        );
    }

    #[test]
    fn bark_body_rejects_template_expansion_bombs_and_oversized_content() {
        let template = "x".repeat(super::MAX_BARK_TEMPLATE_BYTES + 1);
        assert!(render_bark_body(Some(&template), "告警", "正文").is_err());
        let body = "x".repeat(super::MAX_BARK_BODY_BYTES + 1);
        assert!(render_bark_body(None, "告警", &body).is_err());
    }

    #[test]
    fn bark_server_url_trims_trailing_slashes() {
        assert_eq!(
            normalized_bark_server_url(" https://api.day.app/ ").expect("url should parse"),
            "https://api.day.app"
        );
    }

    #[test]
    fn bark_server_url_rejects_credentials_query_and_fragments() {
        for invalid in [
            "https://user@example.com",
            "https://example.com?target=internal",
            "https://example.com/#fragment",
        ] {
            assert!(normalized_bark_server_url(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn bark_http_transport_requires_explicit_opt_in() {
        let url = url::Url::parse("http://bark.example.com").unwrap();
        assert!(validate_bark_transport_policy(&url, false).is_err());
        assert!(validate_bark_transport_policy(&url, true).is_ok());
    }

    #[test]
    fn bark_private_targets_require_explicit_opt_in() {
        let private = [SocketAddr::from(([127, 0, 0, 1], 443))];
        assert!(validate_bark_resolved_addresses(&private, false, false).is_err());
        assert!(validate_bark_resolved_addresses(&private, true, false).is_ok());
    }

    #[test]
    fn bark_builtin_server_allows_benchmarking_ip_only_with_https_default_port() {
        let fake = [SocketAddr::from(([198, 18, 75, 234], 443))];
        assert!(validate_bark_resolved_addresses(&fake, false, true).is_ok());
        assert!(validate_bark_resolved_addresses(
            &[fake[0], SocketAddr::from(([127, 0, 0, 1], 443))],
            false,
            true,
        )
        .is_err());
        assert!(validate_bark_resolved_addresses(&fake, false, false).is_err());
    }

    #[tokio::test]
    async fn bark_transport_errors_do_not_expose_server_url_or_response_body() {
        let secret = "bark-secret-query";
        let error = reqwest::Client::new()
            .post(format!("ftp://bark.example.test/push?token={secret}"))
            .send()
            .await
            .expect_err("unsupported URL scheme should fail before network I/O");

        let message = bark_request_error_message(&error);
        assert!(!message.contains(secret));
        assert!(!message.contains("bark.example.test"));

        let body_error = aether_http::ResponseBodyReadError::Read(error);
        let message = bark_response_body_error_message(&body_error);
        assert!(!message.contains(secret));
        assert!(!message.contains("bark.example.test"));
    }
}
