use base64::Engine;

use crate::handlers::shared::{
    decrypt_catalog_secret_with_fallbacks, system_config_bool, system_config_string,
};
use crate::{AppState, GatewayError};

const SMTP_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
pub(crate) struct ComposedEmail {
    pub(crate) to_email: String,
    pub(crate) subject: String,
    pub(crate) html_body: String,
    pub(crate) text_body: String,
}

pub(crate) async fn read_smtp_delivery_config(
    state: &AppState,
) -> Result<Option<SmtpDeliveryConfig>, GatewayError> {
    let smtp_host = state.read_system_config_json_value("smtp_host").await?;
    let smtp_from_email = state
        .read_system_config_json_value("smtp_from_email")
        .await?;
    let Some(host) = system_config_string(smtp_host.as_ref()) else {
        return Ok(None);
    };
    let Some(from_email) = system_config_string(smtp_from_email.as_ref()) else {
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

    let password = system_config_string(smtp_password.as_ref()).map(|value| {
        decrypt_catalog_secret_with_fallbacks(state.encryption_key(), &value).unwrap_or(value)
    });

    Ok(Some(SmtpDeliveryConfig {
        host,
        port: system_config_u16(smtp_port.as_ref(), 587),
        user: system_config_string(smtp_user.as_ref()),
        password,
        use_tls: system_config_bool(smtp_use_tls.as_ref(), true),
        use_ssl: system_config_bool(smtp_use_ssl.as_ref(), false),
        from_email,
        from_name: system_config_string(smtp_from_name.as_ref())
            .unwrap_or_else(|| "Aether".to_string()),
    }))
}

pub(crate) async fn send_smtp_email(
    config: SmtpDeliveryConfig,
    email: ComposedEmail,
) -> Result<(), GatewayError> {
    tokio::task::spawn_blocking(move || send_smtp_email_blocking(config, email))
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
}

pub(crate) async fn probe_smtp_connection(config: SmtpDeliveryConfig) -> Result<(), GatewayError> {
    tokio::task::spawn_blocking(move || probe_smtp_connection_blocking(config))
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
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

fn connect_tcp_stream(config: &SmtpDeliveryConfig) -> Result<std::net::TcpStream, GatewayError> {
    let stream = std::net::TcpStream::connect((config.host.as_str(), config.port))
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
    let code = loop {
        let parsed_code;
        let continuation;
        let trimmed;
        {
            let mut line = String::new();
            let bytes = reader
                .read_line(&mut line)
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
            if bytes == 0 {
                return Err(GatewayError::Internal(
                    "smtp connection closed unexpectedly".to_string(),
                ));
            }
            trimmed = line.trim_end_matches(['\r', '\n']).to_string();
            if trimmed.len() < 3 {
                return Err(GatewayError::Internal("invalid smtp response".to_string()));
            }
            parsed_code = trimmed[..3]
                .parse::<u16>()
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
            continuation = trimmed.as_bytes().get(3).copied() == Some(b'-');
        }
        if !message.is_empty() {
            message.push('\n');
        }
        message.push_str(&trimmed);
        if !continuation {
            break parsed_code;
        }
    };
    Ok((code, message))
}

fn smtp_expect<T: std::io::BufRead>(
    reader: &mut T,
    allowed_codes: &[u16],
) -> Result<String, GatewayError> {
    let (code, message) = smtp_read_response(reader)?;
    if allowed_codes.contains(&code) {
        return Ok(message);
    }
    Err(GatewayError::Internal(format!(
        "unexpected smtp response {code}: {message}"
    )))
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

fn build_email_message(config: &SmtpDeliveryConfig, email: &ComposedEmail) -> String {
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
    format!(
        "From: {from_header}\r\nTo: <{to_email}>\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\nContent-Type: text/plain; charset=\"utf-8\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{text_body}--{boundary}\r\nContent-Type: text/html; charset=\"utf-8\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{html_body}--{boundary}--\r\n",
        to_email = email.to_email,
        subject = encode_mime_header(&email.subject),
    )
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
    let message = build_email_message(config, email);
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
) -> Result<(), GatewayError> {
    if config.use_ssl {
        let stream = connect_tcp_stream(&config)?;
        let tls_stream = wrap_tls_stream(stream, &config.host)?;
        let mut reader = std::io::BufReader::new(tls_stream);
        let _ = smtp_expect(&mut reader, &[220])?;
        return smtp_send_message(&mut reader, &config, &email);
    }

    let stream = connect_tcp_stream(&config)?;
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

fn probe_smtp_connection_blocking(config: SmtpDeliveryConfig) -> Result<(), GatewayError> {
    if config.use_ssl {
        let stream = connect_tcp_stream(&config)?;
        let tls_stream = wrap_tls_stream(stream, &config.host)?;
        let mut reader = std::io::BufReader::new(tls_stream);
        let _ = smtp_expect(&mut reader, &[220])?;
        return smtp_probe_connection(&mut reader, &config);
    }

    let stream = connect_tcp_stream(&config)?;
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
