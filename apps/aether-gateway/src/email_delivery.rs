use base64::Engine;

use crate::handlers::shared::{
    decrypt_or_migrate_smtp_password, smtp_password_binding, system_config_bool,
};
use crate::{AppState, GatewayError};

const SMTP_TIMEOUT_SECS: u64 = 30;
const SMTP_MAX_HOST_BYTES: usize = 255;
const SMTP_MAX_ADDRESS_BYTES: usize = 320;
const SMTP_MAX_HEADER_VALUE_BYTES: usize = 512;
const SMTP_MAX_USERNAME_BYTES: usize = 320;
const SMTP_MAX_PASSWORD_BYTES: usize = 16 * 1024;
const SMTP_MAX_STORED_PASSWORD_BYTES: usize = 64 * 1024;
const SMTP_MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const SMTP_MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const SMTP_MAX_DIAGNOSTIC_BYTES: usize = 4096;
// SMTP servers normally emit short ASCII status lines. Keep parser buffers
// bounded even when the peer is untrusted or compromised; these limits apply
// only to control responses, not to the message body being submitted.
const SMTP_MAX_RESPONSE_LINE_BYTES: usize = 16 * 1024;
const SMTP_MAX_RESPONSE_BYTES: usize = 256 * 1024;
const SMTP_MAX_RESPONSE_LINES: usize = 128;

#[derive(Clone)]
pub(crate) struct SmtpDeliveryConfig {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) user: Option<String>,
    pub(crate) password: Option<String>,
    pub(crate) use_tls: bool,
    pub(crate) use_ssl: bool,
    pub(crate) from_email: String,
    pub(crate) from_name: String,
}

#[derive(Clone)]
pub(crate) struct ComposedEmail {
    pub(crate) to_email: String,
    pub(crate) subject: String,
    pub(crate) html_body: String,
    pub(crate) text_body: String,
}

fn bounded_system_config_string(
    field: &str,
    value: Option<&serde_json::Value>,
    max_bytes: usize,
) -> Result<Option<String>, GatewayError> {
    let Some(serde_json::Value::String(raw)) = value else {
        return Ok(None);
    };
    let value = raw.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > max_bytes {
        return Err(GatewayError::Internal(format!(
            "smtp {field} exceeds the allowed size"
        )));
    }
    Ok(Some(value.to_string()))
}

pub(crate) async fn read_smtp_delivery_config(
    state: &AppState,
) -> Result<Option<SmtpDeliveryConfig>, GatewayError> {
    let smtp_host = state.read_system_config_json_value("smtp_host").await?;
    let smtp_from_email = state
        .read_system_config_json_value("smtp_from_email")
        .await?;
    let Some(host) = bounded_system_config_string("host", smtp_host.as_ref(), SMTP_MAX_HOST_BYTES)?
    else {
        return Ok(None);
    };
    let Some(from_email) = bounded_system_config_string(
        "from_email",
        smtp_from_email.as_ref(),
        SMTP_MAX_ADDRESS_BYTES,
    )?
    else {
        return Ok(None);
    };

    let smtp_port = state.read_system_config_json_value("smtp_port").await?;
    let smtp_user = state.read_system_config_json_value("smtp_user").await?;
    let smtp_password = state.read_system_config_json_value("smtp_password").await?;
    let smtp_use_tls = state.read_system_config_json_value("smtp_use_tls").await?;
    let smtp_use_ssl = state.read_system_config_json_value("smtp_use_ssl").await?;
    let smtp_from_name = state
        .read_system_config_json_value("smtp_from_name")
        .await?;

    let port = system_config_u16(smtp_port.as_ref(), 587);
    let user = bounded_system_config_string("user", smtp_user.as_ref(), SMTP_MAX_USERNAME_BYTES)?;
    let use_tls = system_config_bool(smtp_use_tls.as_ref(), true);
    let use_ssl = system_config_bool(smtp_use_ssl.as_ref(), false);
    let password = match (
        bounded_system_config_string(
            "stored_password",
            smtp_password.as_ref(),
            SMTP_MAX_STORED_PASSWORD_BYTES,
        )?,
        smtp_password_binding(&host, port, user.as_deref(), use_tls, use_ssl),
    ) {
        (Some(value), Some(binding)) => {
            Some(decrypt_or_migrate_smtp_password(state, &binding, value).await?)
        }
        (Some(_), None) => {
            return Err(GatewayError::Internal(
                "SMTP password binding is invalid".to_string(),
            ));
        }
        (None, _) => None,
    };

    Ok(Some(SmtpDeliveryConfig {
        host,
        port,
        user,
        password,
        use_tls,
        use_ssl,
        from_email,
        from_name: bounded_system_config_string(
            "from_name",
            smtp_from_name.as_ref(),
            SMTP_MAX_HEADER_VALUE_BYTES,
        )?
        .unwrap_or_else(|| "Aether".to_string()),
    }))
}

pub(crate) async fn send_smtp_email(
    config: SmtpDeliveryConfig,
    email: ComposedEmail,
) -> Result<(), GatewayError> {
    validate_smtp_delivery_inputs(&config, &email)?;
    let stream = connect_tcp_stream(&config).await?;
    tokio::task::spawn_blocking(move || send_smtp_email_blocking(config, email, stream))
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
}

pub(crate) async fn probe_smtp_connection(config: SmtpDeliveryConfig) -> Result<(), GatewayError> {
    validate_smtp_config(&config)?;
    let stream = connect_tcp_stream(&config).await?;
    tokio::task::spawn_blocking(move || probe_smtp_connection_blocking(config, stream))
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
}

fn validate_smtp_control_field(field: &str, value: &str) -> Result<(), GatewayError> {
    if value.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
        return Err(GatewayError::Internal(format!(
            "smtp {field} contains forbidden control characters"
        )));
    }
    Ok(())
}

fn validate_smtp_bounded_field(
    field: &str,
    value: &str,
    max_bytes: usize,
) -> Result<(), GatewayError> {
    validate_smtp_control_field(field, value)?;
    if value.len() > max_bytes {
        return Err(GatewayError::Internal(format!(
            "smtp {field} exceeds the allowed size"
        )));
    }
    Ok(())
}

fn validate_smtp_body_field(field: &str, value: &str) -> Result<(), GatewayError> {
    // Bodies are base64 encoded before DATA is written, so line breaks and
    // tabs are valid content. NUL is still rejected because it is not valid
    // textual mail content and can confuse downstream gateways.
    if value.bytes().any(|byte| byte == 0) {
        return Err(GatewayError::Internal(format!(
            "smtp {field} contains a forbidden NUL byte"
        )));
    }
    if value.len() > SMTP_MAX_BODY_BYTES {
        return Err(GatewayError::Internal(format!(
            "smtp {field} exceeds the allowed size"
        )));
    }
    Ok(())
}

fn validate_smtp_address(field: &str, value: &str) -> Result<(), GatewayError> {
    validate_smtp_bounded_field(field, value, SMTP_MAX_ADDRESS_BYTES)?;
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_whitespace) {
        return Err(GatewayError::Internal(format!(
            "smtp {field} must be a single mailbox address"
        )));
    }
    // Addresses are inserted inside SMTP angle brackets. Reject delimiters
    // that could turn one envelope/header value into multiple fields.
    if value
        .bytes()
        .any(|byte| matches!(byte, b'<' | b'>' | b',' | b';' | b'"' | b'\\'))
    {
        return Err(GatewayError::Internal(format!(
            "smtp {field} contains invalid mailbox delimiters"
        )));
    }
    let Some((local, domain)) = value.split_once('@') else {
        return Err(GatewayError::Internal(format!(
            "smtp {field} must contain a mailbox domain"
        )));
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        return Err(GatewayError::Internal(format!(
            "smtp {field} must contain a valid mailbox domain"
        )));
    }
    Ok(())
}

fn validate_smtp_auth_config(config: &SmtpDeliveryConfig) -> Result<(), GatewayError> {
    let username = config.user.as_deref().map(str::trim);
    let has_username = username.is_some_and(|value| !value.is_empty());
    let has_password = config.password.is_some();
    if has_username != has_password {
        return Err(GatewayError::Internal(
            "smtp username and password must be configured together".to_string(),
        ));
    }
    if let Some(username) = username.filter(|value| !value.is_empty()) {
        validate_smtp_bounded_field("user", username, SMTP_MAX_USERNAME_BYTES)?;
        if !config.use_tls && !config.use_ssl {
            return Err(GatewayError::Internal(
                "smtp authentication requires TLS or SSL encryption".to_string(),
            ));
        }
    }
    if let Some(password) = config.password.as_deref() {
        validate_smtp_bounded_field("password", password, SMTP_MAX_PASSWORD_BYTES)?;
    }
    Ok(())
}

fn validate_smtp_config(config: &SmtpDeliveryConfig) -> Result<(), GatewayError> {
    validate_smtp_bounded_field("host", &config.host, SMTP_MAX_HOST_BYTES)?;
    if config.host.is_empty() || config.host.trim() != config.host {
        return Err(GatewayError::Internal(
            "smtp host must not be empty or padded".to_string(),
        ));
    }
    if config.host.chars().any(char::is_whitespace) {
        return Err(GatewayError::Internal(
            "smtp host contains invalid whitespace".to_string(),
        ));
    }
    if config.port == 0 {
        return Err(GatewayError::Internal(
            "smtp port must be non-zero".to_string(),
        ));
    }
    if config.use_tls && config.use_ssl {
        return Err(GatewayError::Internal(
            "smtp TLS and SSL modes cannot both be enabled".to_string(),
        ));
    }
    validate_smtp_address("from_email", &config.from_email)?;
    validate_smtp_bounded_field("from_name", &config.from_name, SMTP_MAX_HEADER_VALUE_BYTES)?;
    validate_smtp_auth_config(config)
}

fn validate_smtp_delivery_inputs(
    config: &SmtpDeliveryConfig,
    email: &ComposedEmail,
) -> Result<(), GatewayError> {
    validate_smtp_config(config)?;
    validate_smtp_address("to_email", &email.to_email)?;
    validate_smtp_bounded_field("subject", &email.subject, SMTP_MAX_HEADER_VALUE_BYTES)?;
    validate_smtp_body_field("html_body", &email.html_body)?;
    validate_smtp_body_field("text_body", &email.text_body)
}

pub(crate) fn system_config_u16(value: Option<&serde_json::Value>, default: u16) -> u16 {
    match value {
        Some(serde_json::Value::Number(value)) => value
            .as_u64()
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(default),
        Some(serde_json::Value::String(value)) => value.trim().parse::<u16>().unwrap_or(default),
        _ => default,
    }
}

fn encode_mime_header(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }
    format!(
        "=?UTF-8?B?{}?=",
        base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
    )
}

fn wrap_base64(value: &str) -> String {
    let mut wrapped = String::new();
    for chunk in value.as_bytes().chunks(76) {
        wrapped.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        wrapped.push_str("\r\n");
    }
    wrapped
}

fn build_tls_config() -> std::sync::Arc<rustls::ClientConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let root_store =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    std::sync::Arc::new(config)
}

fn resolve_server_name(host: &str) -> Result<rustls::pki_types::ServerName<'static>, GatewayError> {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(rustls::pki_types::ServerName::from(ip));
    }
    rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|err| GatewayError::Internal(err.to_string()))
}

async fn connect_tcp_stream(
    config: &SmtpDeliveryConfig,
) -> Result<std::net::TcpStream, GatewayError> {
    connect_tcp_stream_with_dns(
        aether_http::lookup_host_with_limits(
            &config.host,
            config.port,
            aether_http::DEFAULT_DNS_LOOKUP_TIMEOUT,
        ),
        std::time::Duration::from_secs(SMTP_TIMEOUT_SECS),
    )
    .await
}

async fn connect_tcp_stream_with_dns(
    lookup: impl std::future::Future<Output = std::io::Result<Vec<std::net::SocketAddr>>>,
    timeout: std::time::Duration,
) -> Result<std::net::TcpStream, GatewayError> {
    let stream = tokio::time::timeout(timeout, async {
        let addresses = lookup.await.map_err(|error| {
            let message = match error.kind() {
                std::io::ErrorKind::TimedOut => "smtp DNS resolution timed out",
                std::io::ErrorKind::InvalidData => {
                    "smtp DNS resolution returned too many addresses"
                }
                _ => "smtp DNS resolution failed",
            };
            GatewayError::Internal(message.to_string())
        })?;
        if addresses.is_empty() {
            return Err(GatewayError::Internal(
                "smtp host did not resolve to an address".to_string(),
            ));
        }
        let attempts = addresses
            .into_iter()
            .map(|address| Box::pin(tokio::net::TcpStream::connect(address)));
        futures_util::future::select_ok(attempts)
            .await
            .map(|(stream, _)| stream)
            .map_err(|error| {
                GatewayError::Internal(format!("smtp connection failed ({})", error.kind()))
            })
    })
    .await
    .map_err(|_| GatewayError::Internal("smtp DNS or TCP connection timed out".to_string()))??;
    let stream = stream
        .into_std()
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    stream
        .set_nonblocking(false)
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(SMTP_TIMEOUT_SECS)))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(SMTP_TIMEOUT_SECS)))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    Ok(stream)
}

fn wrap_tls_stream(
    stream: std::net::TcpStream,
    host: &str,
) -> Result<rustls::StreamOwned<rustls::ClientConnection, std::net::TcpStream>, GatewayError> {
    let server_name = resolve_server_name(host)?;
    let connection = rustls::ClientConnection::new(build_tls_config(), server_name)
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    Ok(rustls::StreamOwned::new(connection, stream))
}

fn smtp_read_response<T: std::io::BufRead>(reader: &mut T) -> Result<(u16, String), GatewayError> {
    let mut message = String::new();
    let mut expected_code = None;
    for line_number in 0..SMTP_MAX_RESPONSE_LINES {
        let mut line = Vec::new();
        let bytes = read_smtp_response_line(reader, &mut line)
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if bytes == 0 {
            return Err(GatewayError::Internal(
                "smtp connection closed unexpectedly".to_string(),
            ));
        }
        if line.len() > SMTP_MAX_RESPONSE_LINE_BYTES {
            return Err(GatewayError::Internal(
                "smtp response line exceeds the allowed size".to_string(),
            ));
        }

        // Strip only the protocol line ending. The remaining bytes are kept
        // for diagnostics after validating that they are UTF-8.
        while matches!(line.last(), Some(b'\r' | b'\n')) {
            line.pop();
        }
        if line.len() < 3 || !line[..3].iter().all(|byte| byte.is_ascii_digit()) {
            return Err(GatewayError::Internal("invalid smtp response".to_string()));
        }
        let parsed_code = u16::from(line[0] - b'0') * 100
            + u16::from(line[1] - b'0') * 10
            + u16::from(line[2] - b'0');
        if let Some(expected_code) = expected_code {
            if parsed_code != expected_code {
                return Err(GatewayError::Internal(
                    "smtp response continuation code changed".to_string(),
                ));
            }
        } else {
            expected_code = Some(parsed_code);
        }
        let separator = line.get(3).copied().unwrap_or(b' ');
        if separator != b'-' && separator != b' ' {
            return Err(GatewayError::Internal("invalid smtp response".to_string()));
        }

        let trimmed = std::str::from_utf8(&line)
            .map_err(|_| GatewayError::Internal("smtp response is not valid UTF-8".to_string()))?;
        let additional = trimmed.len() + usize::from(!message.is_empty());
        if message
            .len()
            .checked_add(additional)
            .is_none_or(|length| length > SMTP_MAX_RESPONSE_BYTES)
        {
            return Err(GatewayError::Internal(
                "smtp response exceeds the allowed size".to_string(),
            ));
        }
        if !message.is_empty() {
            message.push('\n');
        }
        message.push_str(trimmed);

        if separator != b'-' {
            return Ok((parsed_code, message));
        }

        if line_number + 1 == SMTP_MAX_RESPONSE_LINES {
            return Err(GatewayError::Internal(
                "smtp response has too many continuation lines".to_string(),
            ));
        }
    }

    Err(GatewayError::Internal(
        "smtp response has too many continuation lines".to_string(),
    ))
}

/// Read one SMTP response line without allowing `BufRead::read_until` to
/// allocate an attacker-controlled amount of memory before a size check.
fn read_smtp_response_line<T: std::io::BufRead>(
    reader: &mut T,
    line: &mut Vec<u8>,
) -> std::io::Result<usize> {
    loop {
        let buffered = reader.fill_buf()?;
        if buffered.is_empty() {
            return Ok(line.len());
        }
        let newline = buffered.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(buffered.len(), |index| index + 1);
        if line
            .len()
            .checked_add(take)
            .is_none_or(|length| length > SMTP_MAX_RESPONSE_LINE_BYTES)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "smtp response line exceeds the allowed size",
            ));
        }
        line.extend_from_slice(&buffered[..take]);
        reader.consume(take);
        if newline.is_some() {
            return Ok(line.len());
        }
    }
}

fn smtp_expect<T: std::io::BufRead>(
    reader: &mut T,
    allowed_codes: &[u16],
) -> Result<String, GatewayError> {
    let (code, message) = smtp_read_response(reader)?;
    if allowed_codes.contains(&code) {
        return Ok(message);
    }
    let message = sanitize_smtp_diagnostic(&message);
    Err(GatewayError::Internal(format!(
        "unexpected smtp response {code}: {message}"
    )))
}

/// SMTP responses are controlled by a remote server. Keep diagnostics useful
/// while preventing terminal escapes, log/UI line injection, and oversized
/// error payloads from crossing the API boundary.
fn sanitize_smtp_diagnostic(message: &str) -> String {
    let mut sanitized = String::new();
    let mut previous_space = false;
    for character in message.chars() {
        if character == '\u{1b}' || character.is_control() {
            if !previous_space {
                sanitized.push(' ');
                previous_space = true;
            }
            continue;
        }
        if sanitized.len() + character.len_utf8() > SMTP_MAX_DIAGNOSTIC_BYTES {
            break;
        }
        if character.is_whitespace() {
            if !previous_space {
                sanitized.push(' ');
                previous_space = true;
            }
        } else {
            sanitized.push(character);
            previous_space = false;
        }
    }
    sanitized.trim().to_string()
}

fn smtp_write_line<T: std::io::Write>(writer: &mut T, line: &str) -> Result<(), GatewayError> {
    writer
        .write_all(line.as_bytes())
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    writer
        .write_all(b"\r\n")
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    writer
        .flush()
        .map_err(|err| GatewayError::Internal(err.to_string()))
}

fn smtp_send_command<S: std::io::Read + std::io::Write>(
    reader: &mut std::io::BufReader<S>,
    command: &str,
    allowed_codes: &[u16],
) -> Result<String, GatewayError> {
    smtp_write_line(reader.get_mut(), command)?;
    smtp_expect(reader, allowed_codes)
}

fn build_email_message(
    config: &SmtpDeliveryConfig,
    email: &ComposedEmail,
) -> Result<String, GatewayError> {
    validate_smtp_delivery_inputs(config, email)?;
    let boundary = format!("aether-{}", uuid::Uuid::new_v4().simple());
    let text_body =
        wrap_base64(&base64::engine::general_purpose::STANDARD.encode(email.text_body.as_bytes()));
    let html_body =
        wrap_base64(&base64::engine::general_purpose::STANDARD.encode(email.html_body.as_bytes()));
    let from_header = if config.from_name.trim().is_empty() {
        format!("<{}>", config.from_email)
    } else {
        format!(
            "{} <{}>",
            encode_mime_header(config.from_name.trim()),
            config.from_email
        )
    };
    let message = format!(
        "From: {from_header}\r\nTo: <{to_email}>\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\nContent-Type: text/plain; charset=\"utf-8\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{text_body}--{boundary}\r\nContent-Type: text/html; charset=\"utf-8\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{html_body}--{boundary}--\r\n",
        to_email = email.to_email,
        subject = encode_mime_header(&email.subject),
    );
    if message.len() > SMTP_MAX_MESSAGE_BYTES {
        return Err(GatewayError::Internal(
            "smtp message exceeds the allowed size".to_string(),
        ));
    }
    Ok(message)
}

fn smtp_authenticate<S: std::io::Read + std::io::Write>(
    reader: &mut std::io::BufReader<S>,
    config: &SmtpDeliveryConfig,
) -> Result<(), GatewayError> {
    let Some(username) = config
        .user
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    if !config.use_tls && !config.use_ssl {
        return Err(GatewayError::Internal(
            "smtp authentication requires TLS or SSL encryption".to_string(),
        ));
    }
    let password = config.password.as_deref().unwrap_or("");
    smtp_send_command(reader, "AUTH LOGIN", &[334])?;
    smtp_send_command(
        reader,
        &base64::engine::general_purpose::STANDARD.encode(username.as_bytes()),
        &[334],
    )?;
    smtp_send_command(
        reader,
        &base64::engine::general_purpose::STANDARD.encode(password.as_bytes()),
        &[235],
    )?;
    Ok(())
}

fn smtp_deliver_message<S: std::io::Read + std::io::Write>(
    reader: &mut std::io::BufReader<S>,
    config: &SmtpDeliveryConfig,
    email: &ComposedEmail,
) -> Result<(), GatewayError> {
    // Keep this check next to command construction for callers that bypass
    // the async delivery wrapper.
    validate_smtp_delivery_inputs(config, email)?;
    smtp_send_command(
        reader,
        &format!("MAIL FROM:<{}>", config.from_email),
        &[250],
    )?;
    smtp_send_command(
        reader,
        &format!("RCPT TO:<{}>", email.to_email),
        &[250, 251],
    )?;
    smtp_send_command(reader, "DATA", &[354])?;
    let message = build_email_message(config, email)?;
    reader
        .get_mut()
        .write_all(message.as_bytes())
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    reader
        .get_mut()
        .write_all(b"\r\n.\r\n")
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    reader
        .get_mut()
        .flush()
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let _ = smtp_expect(reader, &[250])?;
    let _ = smtp_send_command(reader, "QUIT", &[221]);
    Ok(())
}

fn smtp_send_message<S: std::io::Read + std::io::Write>(
    reader: &mut std::io::BufReader<S>,
    config: &SmtpDeliveryConfig,
    email: &ComposedEmail,
) -> Result<(), GatewayError> {
    smtp_send_command(reader, "EHLO aether.local", &[250])?;
    smtp_authenticate(reader, config)?;
    smtp_deliver_message(reader, config, email)
}

fn smtp_probe_connection<S: std::io::Read + std::io::Write>(
    reader: &mut std::io::BufReader<S>,
    config: &SmtpDeliveryConfig,
) -> Result<(), GatewayError> {
    smtp_send_command(reader, "EHLO aether.local", &[250])?;
    smtp_authenticate(reader, config)?;
    let _ = smtp_send_command(reader, "QUIT", &[221]);
    Ok(())
}

fn send_smtp_email_blocking(
    config: SmtpDeliveryConfig,
    email: ComposedEmail,
    stream: std::net::TcpStream,
) -> Result<(), GatewayError> {
    if config.use_ssl {
        let tls_stream = wrap_tls_stream(stream, &config.host)?;
        let mut reader = std::io::BufReader::new(tls_stream);
        let _ = smtp_expect(&mut reader, &[220])?;
        return smtp_send_message(&mut reader, &config, &email);
    }

    let mut reader = std::io::BufReader::new(stream);
    let _ = smtp_expect(&mut reader, &[220])?;
    let _ = smtp_send_command(&mut reader, "EHLO aether.local", &[250])?;
    if config.use_tls {
        let _ = smtp_send_command(&mut reader, "STARTTLS", &[220])?;
        let stream = reader.into_inner();
        let tls_stream = wrap_tls_stream(stream, &config.host)?;
        let mut reader = std::io::BufReader::new(tls_stream);
        return smtp_send_message(&mut reader, &config, &email);
    }

    smtp_authenticate(&mut reader, &config)?;
    smtp_deliver_message(&mut reader, &config, &email)
}

fn probe_smtp_connection_blocking(
    config: SmtpDeliveryConfig,
    stream: std::net::TcpStream,
) -> Result<(), GatewayError> {
    if config.use_ssl {
        let tls_stream = wrap_tls_stream(stream, &config.host)?;
        let mut reader = std::io::BufReader::new(tls_stream);
        let _ = smtp_expect(&mut reader, &[220])?;
        return smtp_probe_connection(&mut reader, &config);
    }

    let mut reader = std::io::BufReader::new(stream);
    let _ = smtp_expect(&mut reader, &[220])?;
    let _ = smtp_send_command(&mut reader, "EHLO aether.local", &[250])?;
    if config.use_tls {
        let _ = smtp_send_command(&mut reader, "STARTTLS", &[220])?;
        let stream = reader.into_inner();
        let tls_stream = wrap_tls_stream(stream, &config.host)?;
        let mut reader = std::io::BufReader::new(tls_stream);
        return smtp_probe_connection(&mut reader, &config);
    }

    smtp_authenticate(&mut reader, &config)?;
    let _ = smtp_send_command(&mut reader, "QUIT", &[221]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SmtpDeliveryConfig {
        SmtpDeliveryConfig {
            host: "smtp.example.com".to_string(),
            port: 587,
            user: Some("user@example.com".to_string()),
            password: Some("password".to_string()),
            use_tls: true,
            use_ssl: false,
            from_email: "sender@example.com".to_string(),
            from_name: "Aether".to_string(),
        }
    }

    fn email() -> ComposedEmail {
        ComposedEmail {
            to_email: "recipient@example.com".to_string(),
            subject: "Subject".to_string(),
            html_body: "<p>hello</p>".to_string(),
            text_body: "hello".to_string(),
        }
    }

    #[test]
    fn rejects_crlf_in_smtp_envelope_and_header_fields() {
        let mut malicious_config = config();
        malicious_config.from_email =
            "sender@example.com\r\nRCPT TO:<attacker@example.com>".to_string();
        let error = validate_smtp_delivery_inputs(&malicious_config, &email())
            .expect_err("CRLF in an envelope address must be rejected");
        assert!(format!("{error:?}").contains("from_email"));

        let mut malicious_email = email();
        malicious_email.subject = "Subject\nX-Injected: yes".to_string();
        let error = validate_smtp_delivery_inputs(&config(), &malicious_email)
            .expect_err("CRLF in a header value must be rejected");
        assert!(format!("{error:?}").contains("subject"));
    }

    #[test]
    fn allows_normal_smtp_values() {
        assert!(validate_smtp_delivery_inputs(&config(), &email()).is_ok());
    }

    #[tokio::test]
    async fn smtp_connection_deadline_includes_a_stalled_dns_lookup() {
        let error = connect_tcp_stream_with_dns(
            std::future::pending(),
            std::time::Duration::from_millis(5),
        )
        .await
        .expect_err("DNS must not outlive the connection deadline");
        assert!(format!("{error:?}").contains("smtp DNS or TCP connection timed out"));
    }

    #[tokio::test]
    async fn smtp_dns_errors_and_empty_answers_fail_without_connecting() {
        for (addresses, expected) in [
            (Ok(Vec::new()), "smtp host did not resolve to an address"),
            (
                Err(std::io::Error::other("sensitive-dns-detail")),
                "smtp DNS resolution failed",
            ),
            (
                Err(std::io::Error::from(std::io::ErrorKind::InvalidData)),
                "smtp DNS resolution returned too many addresses",
            ),
            (
                Err(std::io::Error::from(std::io::ErrorKind::TimedOut)),
                "smtp DNS resolution timed out",
            ),
        ] {
            let error = connect_tcp_stream_with_dns(
                std::future::ready(addresses),
                std::time::Duration::from_secs(1),
            )
            .await
            .expect_err("invalid DNS answers must fail before TCP connect");
            assert!(format!("{error:?}").contains(expected));
            assert!(!format!("{error:?}").contains("sensitive-dns-detail"));
        }
    }

    #[tokio::test]
    async fn smtp_connection_tries_answers_beyond_the_old_sixteen_address_limit() {
        let unavailable = tokio::net::TcpSocket::new_v4().unwrap();
        unavailable.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let listener = crate::test_support::bind_loopback_listener().await.unwrap();
        let available = listener.local_addr().unwrap();
        let mut addresses = vec![unavailable.local_addr().unwrap(); 16];
        addresses.push(available);
        let stream = connect_tcp_stream_with_dns(
            std::future::ready(Ok(addresses)),
            std::time::Duration::from_secs(5),
        )
        .await
        .expect("later DNS answers should remain available for fallback");
        assert_eq!(stream.peer_addr().unwrap(), available);
        assert_eq!(
            stream.read_timeout().unwrap(),
            Some(std::time::Duration::from_secs(SMTP_TIMEOUT_SECS))
        );
    }

    #[tokio::test]
    async fn smtp_probe_and_delivery_use_the_preconnected_stream() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

        for deliver in [false, true] {
            let listener = crate::test_support::bind_loopback_listener().await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut reader = tokio::io::BufReader::new(stream);
                reader
                    .get_mut()
                    .write_all(b"220 mock SMTP ready\r\n")
                    .await
                    .unwrap();
                let mut delivered = false;
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).await.unwrap() > 0);
                    let response = if line.starts_with("EHLO ")
                        || line.starts_with("MAIL FROM:")
                        || line.starts_with("RCPT TO:")
                    {
                        &b"250 OK\r\n"[..]
                    } else if line == "DATA\r\n" {
                        reader
                            .get_mut()
                            .write_all(b"354 End with dot\r\n")
                            .await
                            .unwrap();
                        loop {
                            line.clear();
                            assert!(reader.read_line(&mut line).await.unwrap() > 0);
                            if line == ".\r\n" {
                                break;
                            }
                        }
                        delivered = true;
                        &b"250 Accepted\r\n"[..]
                    } else {
                        assert_eq!(line, "QUIT\r\n");
                        reader
                            .get_mut()
                            .write_all(b"221 Goodbye\r\n")
                            .await
                            .unwrap();
                        break;
                    };
                    reader.get_mut().write_all(response).await.unwrap();
                }
                assert_eq!(delivered, deliver);
            });
            let config = SmtpDeliveryConfig {
                host: "127.0.0.1".to_string(),
                port,
                user: None,
                password: None,
                use_tls: false,
                use_ssl: false,
                ..config()
            };
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                if deliver {
                    send_smtp_email(config, email()).await.unwrap();
                } else {
                    probe_smtp_connection(config).await.unwrap();
                }
                server.await.unwrap();
            })
            .await
            .expect("local SMTP probe and delivery should complete");
        }
    }

    #[test]
    fn rejects_authentication_over_plaintext_smtp() {
        let mut insecure = config();
        insecure.use_tls = false;
        insecure.use_ssl = false;
        let error = validate_smtp_delivery_inputs(&insecure, &email())
            .expect_err("SMTP credentials must never be sent over plaintext");
        assert!(format!("{error:?}").contains("requires TLS or SSL"));
    }

    #[test]
    fn rejects_malformed_mailboxes_and_non_protocol_delimiters() {
        let mut malicious = email();
        malicious.to_email = "recipient@example.com>\x01RCPT TO:<attacker@example.com>".to_string();
        let error = validate_smtp_delivery_inputs(&config(), &malicious)
            .expect_err("control characters and envelope delimiters must be rejected");
        assert!(format!("{error:?}").contains("to_email"));

        let mut malformed = email();
        malformed.to_email = "not-an-email".to_string();
        assert!(validate_smtp_delivery_inputs(&config(), &malformed).is_err());
    }

    #[test]
    fn bounds_message_bodies_before_smtp_submission() {
        let mut oversized = email();
        oversized.html_body = "x".repeat(SMTP_MAX_BODY_BYTES + 1);
        let error = validate_smtp_delivery_inputs(&config(), &oversized)
            .expect_err("oversized message bodies must be rejected");
        assert!(format!("{error:?}").contains("html_body"));

        let mut textual = email();
        textual.text_body = "line one\nline two\t✓".to_string();
        assert!(validate_smtp_delivery_inputs(&config(), &textual).is_ok());

        textual.text_body.push('\0');
        assert!(validate_smtp_delivery_inputs(&config(), &textual).is_err());
    }

    #[test]
    fn sanitizes_remote_response_diagnostics() {
        let mut reader = std::io::BufReader::new("550 bad\u{1b}[31m\r\n".as_bytes());
        let error = smtp_expect(&mut reader, &[250]).expect_err("unexpected response must fail");
        let GatewayError::Internal(message) = error else {
            panic!("expected internal SMTP error");
        };
        assert!(!message.contains('\u{1b}'));
        assert!(!message.contains('\n'));
        assert!(message.len() < SMTP_MAX_DIAGNOSTIC_BYTES);
    }

    #[test]
    fn smtp_response_rejects_non_ascii_status_prefix_without_panicking() {
        let mut reader = std::io::BufReader::new("é00 greeting\r\n".as_bytes());
        let error = smtp_read_response(&mut reader)
            .expect_err("a non-ASCII status prefix must be rejected");
        assert!(format!("{error:?}").contains("invalid smtp response"));
    }

    #[test]
    fn smtp_response_rejects_oversized_lines_before_allocating_unbounded_memory() {
        let mut input = vec![b'2'; SMTP_MAX_RESPONSE_LINE_BYTES + 1];
        input.push(b'\n');
        let mut reader = std::io::BufReader::new(input.as_slice());
        let error = smtp_read_response(&mut reader).expect_err("oversized line must be rejected");
        assert!(format!("{error:?}").contains("response line exceeds"));
    }

    #[test]
    fn smtp_response_bounds_continuation_lines_and_code_changes() {
        let repeated = (0..=SMTP_MAX_RESPONSE_LINES)
            .map(|_| "250-more\r\n")
            .collect::<String>();
        let mut reader = std::io::BufReader::new(repeated.as_bytes());
        let error = smtp_read_response(&mut reader)
            .expect_err("too many continuation lines must be rejected");
        assert!(format!("{error:?}").contains("too many continuation lines"));

        let mut reader = std::io::BufReader::new("250-more\r\n550 done\r\n".as_bytes());
        let error = smtp_read_response(&mut reader)
            .expect_err("continuation response code changes must be rejected");
        assert!(format!("{error:?}").contains("continuation code changed"));
    }
}
