use crate::email_delivery::{probe_smtp_connection, system_config_u16, SmtpDeliveryConfig};
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::shared::{system_config_bool, system_config_string};
use crate::GatewayError;
use axum::body::Bytes;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Default, Deserialize)]
struct AdminSmtpTestRequest {
    smtp_host: Option<serde_json::Value>,
    smtp_port: Option<serde_json::Value>,
    smtp_user: Option<serde_json::Value>,
    smtp_password: Option<serde_json::Value>,
    smtp_use_tls: Option<serde_json::Value>,
    smtp_use_ssl: Option<serde_json::Value>,
    smtp_from_email: Option<serde_json::Value>,
    smtp_from_name: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
struct ResolvedSmtpConfig {
    host: Option<String>,
    port: u16,
    user: Option<String>,
    password: Option<String>,
    use_tls: bool,
    use_ssl: bool,
    from_email: Option<String>,
    #[allow(dead_code)]
    from_name: String,
}

pub(crate) async fn build_admin_smtp_test_payload(
    state: &AdminAppState<'_>,
    request_body: Option<&Bytes>,
) -> Result<serde_json::Value, GatewayError> {
    let request = match request_body {
        Some(body) if !body.is_empty() => serde_json::from_slice::<AdminSmtpTestRequest>(body)
            .map_err(|err| GatewayError::Internal(err.to_string()))?,
        _ => AdminSmtpTestRequest::default(),
    };
    let config = resolve_admin_smtp_config(state, request).await?;
    let missing_fields = missing_smtp_fields(&config);
    if !missing_fields.is_empty() {
        return Ok(json!({
            "success": false,
            "message": format!("SMTP 配置不完整，请检查 {}", missing_fields.join(", ")),
        }));
    }

    let result = probe_smtp_connection(config.into_delivery_config()).await;
    Ok(match result {
        Ok(()) => json!({ "success": true, "message": "SMTP 连接测试成功" }),
        Err(error) => {
            json!({ "success": false, "message": translate_smtp_error(&smtp_gateway_error_message(&error)) })
        }
    })
}

async fn resolve_admin_smtp_config(
    state: &AdminAppState<'_>,
    request: AdminSmtpTestRequest,
) -> Result<ResolvedSmtpConfig, GatewayError> {
    let smtp_host = state.read_system_config_json_value("smtp_host").await?;
    let smtp_port = state.read_system_config_json_value("smtp_port").await?;
    let smtp_user = state.read_system_config_json_value("smtp_user").await?;
    let smtp_password = state.read_system_config_json_value("smtp_password").await?;
    let smtp_use_tls = state.read_system_config_json_value("smtp_use_tls").await?;
    let smtp_use_ssl = state.read_system_config_json_value("smtp_use_ssl").await?;
    let smtp_from_email = state
        .read_system_config_json_value("smtp_from_email")
        .await?;
    let smtp_from_name = state
        .read_system_config_json_value("smtp_from_name")
        .await?;

    let stored_password = system_config_string(smtp_password.as_ref()).map(|value| {
        state
            .decrypt_catalog_secret_with_fallbacks(&value)
            .unwrap_or(value)
    });

    Ok(ResolvedSmtpConfig {
        host: request
            .smtp_host
            .as_ref()
            .and_then(|value| system_config_string(Some(value)))
            .or_else(|| system_config_string(smtp_host.as_ref())),
        port: request
            .smtp_port
            .as_ref()
            .map(|value| system_config_u16(Some(value), 587))
            .unwrap_or_else(|| system_config_u16(smtp_port.as_ref(), 587)),
        user: request
            .smtp_user
            .as_ref()
            .and_then(|value| system_config_string(Some(value)))
            .or_else(|| system_config_string(smtp_user.as_ref())),
        password: request
            .smtp_password
            .as_ref()
            .and_then(|value| system_config_string(Some(value)))
            .or(stored_password),
        use_tls: request
            .smtp_use_tls
            .as_ref()
            .map(|value| system_config_bool(Some(value), true))
            .unwrap_or_else(|| system_config_bool(smtp_use_tls.as_ref(), true)),
        use_ssl: request
            .smtp_use_ssl
            .as_ref()
            .map(|value| system_config_bool(Some(value), false))
            .unwrap_or_else(|| system_config_bool(smtp_use_ssl.as_ref(), false)),
        from_email: request
            .smtp_from_email
            .as_ref()
            .and_then(|value| system_config_string(Some(value)))
            .or_else(|| system_config_string(smtp_from_email.as_ref())),
        from_name: request
            .smtp_from_name
            .as_ref()
            .and_then(|value| system_config_string(Some(value)))
            .or_else(|| system_config_string(smtp_from_name.as_ref()))
            .unwrap_or_else(|| "Aether".to_string()),
    })
}

impl ResolvedSmtpConfig {
    fn into_delivery_config(self) -> SmtpDeliveryConfig {
        SmtpDeliveryConfig {
            host: self.host.unwrap_or_default(),
            port: self.port,
            user: self.user,
            password: self.password,
            use_tls: self.use_tls,
            use_ssl: self.use_ssl,
            from_email: self.from_email.unwrap_or_default(),
            from_name: self.from_name,
        }
    }
}

fn missing_smtp_fields(config: &ResolvedSmtpConfig) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if config
        .host
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        fields.push("smtp_host");
    }
    if config
        .user
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        fields.push("smtp_user");
    }
    if config
        .password
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        fields.push("smtp_password");
    }
    if config
        .from_email
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        fields.push("smtp_from_email");
    }
    fields
}

fn smtp_gateway_error_message(error: &GatewayError) -> String {
    match error {
        GatewayError::Internal(message) => message.clone(),
        _ => format!("{error:?}"),
    }
}

fn translate_smtp_error(error: &str) -> String {
    let error_lower = error.to_ascii_lowercase();

    if error_lower.contains("username and password not accepted") {
        return "用户名或密码错误，请检查 SMTP 凭据".to_string();
    }
    if error_lower.contains("authentication failed")
        || error_lower.contains("auth") && error_lower.contains("535")
    {
        return "认证失败，请检查用户名和密码".to_string();
    }
    if error_lower.contains("invalid credentials") || error_lower.contains("badcredentials") {
        return "凭据无效，请检查用户名和密码".to_string();
    }
    if error_lower.contains("smtp auth extension is not supported") {
        return "服务器不支持认证，请尝试使用 TLS 或 SSL 加密".to_string();
    }
    if error_lower.contains("connection refused") || error_lower.contains("os error 61") {
        return "连接被拒绝，请检查服务器地址和端口".to_string();
    }
    if error_lower.contains("connection timed out")
        || error_lower.contains("timed out")
        || error_lower.contains("operation timed out")
    {
        return "连接超时，请检查网络或服务器地址".to_string();
    }
    if error_lower.contains("name or service not known")
        || error_lower.contains("getaddrinfo failed")
        || error_lower.contains("nodename nor servname provided")
        || error_lower.contains("failed to lookup address information")
    {
        return "无法解析服务器地址，请检查 SMTP 服务器地址".to_string();
    }
    if error_lower.contains("network is unreachable") {
        return "网络不可达，请检查网络连接".to_string();
    }
    if error_lower.contains("certificate") && error_lower.contains("verify") {
        return "SSL 证书验证失败，请检查服务器证书或尝试其他加密方式".to_string();
    }
    if error_lower.contains("ssl") && error_lower.contains("wrong version") {
        return "SSL 版本不匹配，请尝试其他加密方式".to_string();
    }
    if error_lower.contains("starttls") {
        return "STARTTLS 握手失败，请检查加密设置".to_string();
    }
    if error_lower.contains("sender address rejected") {
        return "发件人地址被拒绝，请检查发件人邮箱设置".to_string();
    }
    if error_lower.contains("relay access denied") {
        return "中继访问被拒绝，请检查 SMTP 服务器配置".to_string();
    }

    error.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_missing_python_required_fields() {
        let config = ResolvedSmtpConfig {
            host: None,
            port: 587,
            user: None,
            password: None,
            use_tls: true,
            use_ssl: false,
            from_email: None,
            from_name: "Aether".to_string(),
        };
        assert_eq!(
            missing_smtp_fields(&config),
            vec!["smtp_host", "smtp_user", "smtp_password", "smtp_from_email"]
        );
    }

    #[test]
    fn translates_common_smtp_errors() {
        assert_eq!(
            translate_smtp_error("connection refused"),
            "连接被拒绝，请检查服务器地址和端口"
        );
        assert_eq!(
            translate_smtp_error("535 authentication failed"),
            "认证失败，请检查用户名和密码"
        );
        assert_eq!(
            translate_smtp_error("nodename nor servname provided"),
            "无法解析服务器地址，请检查 SMTP 服务器地址"
        );
    }
}
