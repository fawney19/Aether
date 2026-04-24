use super::{
    decrypt_catalog_secret_with_fallbacks, escape_admin_email_template_html, json,
    read_admin_email_template_payload, render_admin_email_template_html, system_config_bool,
    system_config_string, system_config_u16, AppState, GatewayError,
    AUTH_EMAIL_VERIFICATION_PREFIX, AUTH_EMAIL_VERIFIED_PREFIX, AUTH_EMAIL_VERIFIED_TTL_SECS,
    AUTH_SMTP_TIMEOUT_SECS,
};
use base64::Engine;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct StoredAuthEmailVerificationCode {
    pub(super) code: String,
    pub(super) created_at: String,
}

#[derive(Debug, Clone)]
pub(super) struct AuthSmtpConfig {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) user: Option<String>,
    pub(super) password: Option<String>,
    pub(super) use_tls: bool,
    pub(super) use_ssl: bool,
    pub(super) from_email: String,
    pub(super) from_name: String,
}

#[derive(Debug, Clone)]
pub(super) struct AuthComposedEmail {
    pub(super) to_email: String,
    pub(super) subject: String,
    pub(super) html_body: String,
    pub(super) text_body: String,
}

pub(super) fn auth_email_verification_key(email: &str) -> String {
    format!("{AUTH_EMAIL_VERIFICATION_PREFIX}{email}")
}

pub(super) fn auth_email_verified_key(email: &str) -> String {
    format!("{AUTH_EMAIL_VERIFIED_PREFIX}{email}")
}

pub(super) fn load_auth_email_verification_entry_for_tests(
    _state: &AppState,
    _key: &str,
) -> Option<String> {
    #[cfg(test)]
    {
        return _state
            .auth_email_verification_store
            .as_ref()
            .and_then(|store| {
                store
                    .lock()
                    .expect("auth email verification store should lock")
                    .get(_key)
                    .cloned()
            });
    }

    #[allow(unreachable_code)]
    None
}

pub(super) fn save_auth_email_verification_entry_for_tests(
    _state: &AppState,
    _key: &str,
    _value: &str,
) -> bool {
    #[cfg(test)]
    {
        if let Some(store) = _state.auth_email_verification_store.as_ref() {
            store
                .lock()
                .expect("auth email verification store should lock")
                .insert(_key.to_string(), _value.to_string());
            return true;
        }
    }

    false
}

pub(super) fn delete_auth_email_verification_entries_for_tests(
    _state: &AppState,
    _keys: &[String],
) -> bool {
    #[cfg(test)]
    {
        if let Some(store) = _state.auth_email_verification_store.as_ref() {
            let mut guard = store
                .lock()
                .expect("auth email verification store should lock");
            for key in _keys {
                guard.remove(key);
            }
            return true;
        }
    }

    false
}

pub(super) fn record_auth_email_delivery_for_tests(
    _state: &AppState,
    _payload: serde_json::Value,
) -> bool {
    #[cfg(test)]
    {
        if let Some(store) = _state.auth_email_delivery_store.as_ref() {
            store
                .lock()
                .expect("auth email delivery store should lock")
                .push(_payload);
            return true;
        }
    }

    false
}

pub(super) fn generate_auth_verification_code() -> String {
    format!("{:06}", uuid::Uuid::new_v4().as_u128() % 1_000_000)
}

fn render_auth_template_string(
    template: &str,
    variables: &std::collections::BTreeMap<String, String>,
    escape_html: bool,
) -> Result<String, GatewayError> {
    let mut rendered = template.to_string();
    for (key, value) in variables {
        let pattern = regex::Regex::new(&format!(r"\{{\{{\s*{}\s*\}}\}}", regex::escape(key)))
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let replacement = if escape_html {
            escape_admin_email_template_html(value)
        } else {
            value.clone()
        };
        rendered = pattern
            .replace_all(&rendered, replacement.as_str())
            .into_owned();
    }
    Ok(rendered)
}

fn auth_encode_mime_header(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }
    format!(
        "=?UTF-8?B?{}?=",
        base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
    )
}

fn auth_wrap_base64(value: &str) -> String {
    let mut wrapped = String::new();
    for chunk in value.as_bytes().chunks(76) {
        wrapped.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        wrapped.push_str("\r\n");
    }
    wrapped
}

fn auth_build_verification_text_body(
    app_name: &str,
    email: &str,
    code: &str,
    expire_minutes: i64,
) -> String {
    format!(
        "{app_name}\n\n您的验证码是：{code}\n目标邮箱：{email}\n有效期：{expire_minutes} 分钟\n\n如果这不是您本人的操作，请忽略此邮件。"
    )
}

fn auth_build_tls_config() -> std::sync::Arc<rustls::ClientConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let root_store =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    std::sync::Arc::new(config)
}

fn auth_resolve_server_name(
    host: &str,
) -> Result<rustls::pki_types::ServerName<'static>, GatewayError> {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(rustls::pki_types::ServerName::from(ip));
    }
    rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|err| GatewayError::Internal(err.to_string()))
}

fn auth_connect_tcp_stream(config: &AuthSmtpConfig) -> Result<std::net::TcpStream, GatewayError> {
    let stream = std::net::TcpStream::connect((config.host.as_str(), config.port))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(AUTH_SMTP_TIMEOUT_SECS)))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(AUTH_SMTP_TIMEOUT_SECS)))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    Ok(stream)
}

fn auth_wrap_tls_stream(
    stream: std::net::TcpStream,
    host: &str,
) -> Result<rustls::StreamOwned<rustls::ClientConnection, std::net::TcpStream>, GatewayError> {
    let server_name = auth_resolve_server_name(host)?;
    let connection = rustls::ClientConnection::new(auth_build_tls_config(), server_name)
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    Ok(rustls::StreamOwned::new(connection, stream))
}

fn auth_smtp_read_response<T: std::io::BufRead>(
    reader: &mut T,
) -> Result<(u16, String), GatewayError> {
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

fn auth_smtp_expect<T: std::io::BufRead>(
    reader: &mut T,
    allowed_codes: &[u16],
) -> Result<String, GatewayError> {
    let (code, message) = auth_smtp_read_response(reader)?;
    if allowed_codes.contains(&code) {
        return Ok(message);
    }
    Err(GatewayError::Internal(format!(
        "unexpected smtp response {code}: {message}"
    )))
}

fn auth_smtp_write_line<T: std::io::Write>(writer: &mut T, line: &str) -> Result<(), GatewayError> {
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

fn auth_smtp_send_command<S: std::io::Read + std::io::Write>(
    reader: &mut std::io::BufReader<S>,
    command: &str,
    allowed_codes: &[u16],
) -> Result<String, GatewayError> {
    auth_smtp_write_line(reader.get_mut(), command)?;
    auth_smtp_expect(reader, allowed_codes)
}

fn auth_build_email_message(config: &AuthSmtpConfig, email: &AuthComposedEmail) -> String {
    let boundary = format!("aether-{}", uuid::Uuid::new_v4().simple());
    let text_body = auth_wrap_base64(
        &base64::engine::general_purpose::STANDARD.encode(email.text_body.as_bytes()),
    );
    let html_body = auth_wrap_base64(
        &base64::engine::general_purpose::STANDARD.encode(email.html_body.as_bytes()),
    );
    let from_header = if config.from_name.trim().is_empty() {
        format!("<{}>", config.from_email)
    } else {
        format!(
            "{} <{}>",
            auth_encode_mime_header(config.from_name.trim()),
            config.from_email
        )
    };
    format!(
        "From: {from_header}\r\nTo: <{to_email}>\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\nContent-Type: text/plain; charset=\"utf-8\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{text_body}--{boundary}\r\nContent-Type: text/html; charset=\"utf-8\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{html_body}--{boundary}--\r\n",
        to_email = email.to_email,
        subject = auth_encode_mime_header(&email.subject),
    )
}

fn auth_smtp_authenticate<S: std::io::Read + std::io::Write>(
    reader: &mut std::io::BufReader<S>,
    config: &AuthSmtpConfig,
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
    auth_smtp_send_command(reader, "AUTH LOGIN", &[334])?;
    auth_smtp_send_command(
        reader,
        &base64::engine::general_purpose::STANDARD.encode(username.as_bytes()),
        &[334],
    )?;
    auth_smtp_send_command(
        reader,
        &base64::engine::general_purpose::STANDARD.encode(password.as_bytes()),
        &[235],
    )?;
    Ok(())
}

fn auth_smtp_deliver_message<S: std::io::Read + std::io::Write>(
    reader: &mut std::io::BufReader<S>,
    config: &AuthSmtpConfig,
    email: &AuthComposedEmail,
) -> Result<(), GatewayError> {
    auth_smtp_send_command(
        reader,
        &format!("MAIL FROM:<{}>", config.from_email),
        &[250],
    )?;
    auth_smtp_send_command(
        reader,
        &format!("RCPT TO:<{}>", email.to_email),
        &[250, 251],
    )?;
    auth_smtp_send_command(reader, "DATA", &[354])?;
    let message = auth_build_email_message(config, email);
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
    let _ = auth_smtp_expect(reader, &[250])?;
    let _ = auth_smtp_send_command(reader, "QUIT", &[221]);
    Ok(())
}

fn auth_smtp_send_message<S: std::io::Read + std::io::Write>(
    reader: &mut std::io::BufReader<S>,
    config: &AuthSmtpConfig,
    email: &AuthComposedEmail,
) -> Result<(), GatewayError> {
    auth_smtp_send_command(reader, "EHLO aether.local", &[250])?;
    auth_smtp_authenticate(reader, config)?;
    auth_smtp_deliver_message(reader, config, email)
}

fn send_auth_email_blocking(
    config: AuthSmtpConfig,
    email: AuthComposedEmail,
) -> Result<(), GatewayError> {
    if config.use_ssl {
        let stream = auth_connect_tcp_stream(&config)?;
        let tls_stream = auth_wrap_tls_stream(stream, &config.host)?;
        let mut reader = std::io::BufReader::new(tls_stream);
        let _ = auth_smtp_expect(&mut reader, &[220])?;
        return auth_smtp_send_message(&mut reader, &config, &email);
    }

    let stream = auth_connect_tcp_stream(&config)?;
    let mut reader = std::io::BufReader::new(stream);
    let _ = auth_smtp_expect(&mut reader, &[220])?;
    let _ = auth_smtp_send_command(&mut reader, "EHLO aether.local", &[250])?;
    if config.use_tls {
        let _ = auth_smtp_send_command(&mut reader, "STARTTLS", &[220])?;
        let stream = reader.into_inner();
        let tls_stream = auth_wrap_tls_stream(stream, &config.host)?;
        let mut reader = std::io::BufReader::new(tls_stream);
        return auth_smtp_send_message(&mut reader, &config, &email);
    }

    auth_smtp_authenticate(&mut reader, &config)?;
    auth_smtp_deliver_message(&mut reader, &config, &email)
}

pub(super) async fn read_auth_email_verification_code(
    state: &AppState,
    email: &str,
) -> Result<Option<StoredAuthEmailVerificationCode>, GatewayError> {
    let key = auth_email_verification_key(email);
    let raw = if let Some(runner) = state.redis_kv_runner() {
        let mut connection = runner
            .client()
            .get_multiplexed_async_connection()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let namespaced_key = runner.keyspace().key(&key);
        redis::cmd("GET")
            .arg(&namespaced_key)
            .query_async::<Option<String>>(&mut connection)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
    } else {
        load_auth_email_verification_entry_for_tests(state, &key)
    };
    raw.map(|value| {
        serde_json::from_str::<StoredAuthEmailVerificationCode>(&value)
            .map_err(|err| GatewayError::Internal(err.to_string()))
    })
    .transpose()
}

pub(super) async fn auth_email_is_verified(
    state: &AppState,
    email: &str,
) -> Result<bool, GatewayError> {
    let key = auth_email_verified_key(email);
    if let Some(runner) = state.redis_kv_runner() {
        let mut connection = runner
            .client()
            .get_multiplexed_async_connection()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let namespaced_key = runner.keyspace().key(&key);
        let exists = redis::cmd("EXISTS")
            .arg(&namespaced_key)
            .query_async::<i64>(&mut connection)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok(exists > 0);
    }
    Ok(load_auth_email_verification_entry_for_tests(state, &key).is_some())
}

pub(super) async fn mark_auth_email_verified(
    state: &AppState,
    email: &str,
) -> Result<bool, GatewayError> {
    let key = auth_email_verified_key(email);
    if let Some(runner) = state.redis_kv_runner() {
        runner
            .setex(&key, "verified", Some(AUTH_EMAIL_VERIFIED_TTL_SECS))
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok(true);
    }
    Ok(save_auth_email_verification_entry_for_tests(
        state, &key, "verified",
    ))
}

pub(super) async fn clear_auth_email_pending_code(
    state: &AppState,
    email: &str,
) -> Result<bool, GatewayError> {
    let verification_key = auth_email_verification_key(email);
    if let Some(runner) = state.redis_kv_runner() {
        let _ = runner
            .del(&verification_key)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok(true);
    }
    Ok(delete_auth_email_verification_entries_for_tests(
        state,
        &[verification_key],
    ))
}

pub(super) async fn clear_auth_email_verification(
    state: &AppState,
    email: &str,
) -> Result<bool, GatewayError> {
    let verification_key = auth_email_verification_key(email);
    let verified_key = auth_email_verified_key(email);
    if let Some(runner) = state.redis_kv_runner() {
        let _ = runner
            .del(&verification_key)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let _ = runner
            .del(&verified_key)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok(true);
    }
    Ok(delete_auth_email_verification_entries_for_tests(
        state,
        &[verification_key, verified_key],
    ))
}

pub(super) async fn store_auth_email_verification_code(
    state: &AppState,
    email: &str,
    code: &str,
    created_at: chrono::DateTime<chrono::Utc>,
    ttl_seconds: u64,
) -> Result<bool, GatewayError> {
    let key = auth_email_verification_key(email);
    let value = json!({
        "code": code,
        "created_at": created_at.to_rfc3339(),
    })
    .to_string();
    if let Some(runner) = state.redis_kv_runner() {
        runner
            .setex(&key, &value, Some(ttl_seconds))
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok(true);
    }
    Ok(save_auth_email_verification_entry_for_tests(
        state, &key, &value,
    ))
}

pub(super) async fn read_auth_smtp_config(
    state: &AppState,
) -> Result<Option<AuthSmtpConfig>, GatewayError> {
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

    Ok(Some(AuthSmtpConfig {
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

pub(super) async fn auth_email_app_name(state: &AppState) -> Result<String, GatewayError> {
    let email_app_name = state
        .read_system_config_json_value("email_app_name")
        .await?;
    let site_name = state.read_system_config_json_value("site_name").await?;
    let smtp_from_name = state
        .read_system_config_json_value("smtp_from_name")
        .await?;
    Ok(system_config_string(email_app_name.as_ref())
        .or_else(|| system_config_string(site_name.as_ref()))
        .or_else(|| system_config_string(smtp_from_name.as_ref()))
        .unwrap_or_else(|| "Aether".to_string()))
}

pub(super) async fn build_auth_verification_email(
    state: &AppState,
    email: &str,
    code: &str,
    expire_minutes: i64,
) -> Result<AuthComposedEmail, GatewayError> {
    let template = read_admin_email_template_payload(state, "verification")
        .await?
        .ok_or_else(|| GatewayError::Internal("verification email template missing".to_string()))?;
    let subject_template = template
        .get("subject")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("邮箱验证码");
    let html_template = template
        .get("html")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let app_name = auth_email_app_name(state).await?;
    let variables = std::collections::BTreeMap::from([
        ("app_name".to_string(), app_name.clone()),
        ("code".to_string(), code.to_string()),
        ("expire_minutes".to_string(), expire_minutes.to_string()),
        ("email".to_string(), email.to_string()),
    ]);
    let subject = render_auth_template_string(subject_template, &variables, false)?;
    let html_body = render_admin_email_template_html(html_template, &variables)?;
    let text_body = auth_build_verification_text_body(&app_name, email, code, expire_minutes);
    Ok(AuthComposedEmail {
        to_email: email.to_string(),
        subject,
        html_body,
        text_body,
    })
}

pub(super) async fn send_auth_email(
    state: &AppState,
    config: AuthSmtpConfig,
    email: AuthComposedEmail,
) -> Result<(), GatewayError> {
    if record_auth_email_delivery_for_tests(
        state,
        json!({
            "to_email": email.to_email,
            "subject": email.subject,
            "html_body": email.html_body,
            "text_body": email.text_body,
        }),
    ) {
        return Ok(());
    }

    tokio::task::spawn_blocking(move || send_auth_email_blocking(config, email))
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
}

pub(super) async fn auth_registration_email_configured(
    state: &AppState,
) -> Result<bool, GatewayError> {
    let smtp_host = state.read_system_config_json_value("smtp_host").await?;
    let smtp_from_email = state
        .read_system_config_json_value("smtp_from_email")
        .await?;
    Ok(system_config_string(smtp_host.as_ref()).is_some()
        && system_config_string(smtp_from_email.as_ref()).is_some())
}
