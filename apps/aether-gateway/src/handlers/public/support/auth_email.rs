use super::{
    escape_admin_email_template_html, json, read_admin_email_template_payload,
    render_admin_email_template_html, system_config_string, AppState, GatewayError,
    AUTH_EMAIL_VERIFICATION_PREFIX, AUTH_EMAIL_VERIFIED_PREFIX, AUTH_EMAIL_VERIFIED_TTL_SECS,
};
use crate::email_delivery::{
    read_smtp_delivery_config, send_smtp_email, ComposedEmail, SmtpDeliveryConfig,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct StoredAuthEmailVerificationCode {
    pub(super) code: String,
    pub(super) created_at: String,
}

pub(super) type AuthSmtpConfig = SmtpDeliveryConfig;
pub(super) type AuthComposedEmail = ComposedEmail;

pub(super) fn auth_email_verification_key(email: &str) -> String {
    format!("{AUTH_EMAIL_VERIFICATION_PREFIX}{email}")
}

pub(super) fn auth_email_verified_key(email: &str) -> String {
    format!("{AUTH_EMAIL_VERIFIED_PREFIX}{email}")
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

pub(super) async fn read_auth_email_verification_code(
    state: &AppState,
    email: &str,
) -> Result<Option<StoredAuthEmailVerificationCode>, GatewayError> {
    let key = auth_email_verification_key(email);
    let raw = state.runtime_kv_get(&key).await?;
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
    state.runtime_kv_exists(&key).await
}

pub(super) async fn mark_auth_email_verified(
    state: &AppState,
    email: &str,
) -> Result<bool, GatewayError> {
    let key = auth_email_verified_key(email);
    state
        .runtime_kv_setex(&key, "verified", AUTH_EMAIL_VERIFIED_TTL_SECS)
        .await?;
    Ok(true)
}

pub(super) async fn clear_auth_email_pending_code(
    state: &AppState,
    email: &str,
) -> Result<bool, GatewayError> {
    let verification_key = auth_email_verification_key(email);
    state.runtime_kv_del(&verification_key).await
}

pub(super) async fn clear_auth_email_verification(
    state: &AppState,
    email: &str,
) -> Result<bool, GatewayError> {
    let verification_key = auth_email_verification_key(email);
    let verified_key = auth_email_verified_key(email);
    let deleted_pending = state.runtime_kv_del(&verification_key).await?;
    let deleted_verified = state.runtime_kv_del(&verified_key).await?;
    Ok(deleted_pending || deleted_verified)
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
    state.runtime_kv_setex(&key, &value, ttl_seconds).await?;
    Ok(true)
}

pub(super) async fn read_auth_smtp_config(
    state: &AppState,
) -> Result<Option<AuthSmtpConfig>, GatewayError> {
    read_smtp_delivery_config(state).await
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
            "to_email": email.to_email.clone(),
            "subject": email.subject.clone(),
            "html_body": email.html_body.clone(),
            "text_body": email.text_body.clone(),
        }),
    ) {
        return Ok(());
    }

    send_smtp_email(config, email).await
}

pub(super) async fn auth_registration_email_configured(
    state: &AppState,
) -> Result<bool, GatewayError> {
    Ok(read_smtp_delivery_config(state).await?.is_some())
}
