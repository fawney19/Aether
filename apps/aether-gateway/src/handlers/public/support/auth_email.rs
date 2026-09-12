use super::{
    escape_admin_email_template_html, json, read_admin_email_template_payload,
    render_admin_email_template_html, system_config_string, AppState, GatewayError,
    AUTH_EMAIL_VERIFICATION_PREFIX, AUTH_EMAIL_VERIFIED_PREFIX, AUTH_EMAIL_VERIFIED_TTL_SECS,
};
use crate::email_delivery::{
    read_smtp_delivery_config, send_smtp_email, ComposedEmail, SmtpDeliveryConfig,
};
use aether_admin::system::{
    admin_email_template_subject_is_valid, ADMIN_EMAIL_TEMPLATE_MAX_PREVIEW_VALUE_BYTES,
    ADMIN_EMAIL_TEMPLATE_MAX_SUBJECT_BYTES,
};
use hmac::Mac;
use sha2::{Digest, Sha256};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct StoredAuthEmailVerificationCode {
    pub(super) code_hash: String,
    pub(super) created_at: String,
    pub(super) verification_token_hash: String,
}

pub(super) type AuthSmtpConfig = SmtpDeliveryConfig;
pub(super) type AuthComposedEmail = ComposedEmail;

fn auth_email_storage_key_digest(domain: &str, parts: &[&str]) -> Result<String, GatewayError> {
    let secret = super::auth_jwt_secret().map_err(GatewayError::Internal)?;
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| GatewayError::Internal("auth email HMAC key invalid".to_string()))?;
    mac.update(b"aether-auth-email-storage-v1\0");
    mac.update(domain.as_bytes());
    for part in parts {
        mac.update(b"\0");
        mac.update(part.as_bytes());
    }
    Ok(mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub(super) fn auth_email_verification_key(email: &str) -> Result<String, GatewayError> {
    let email = email.trim().to_ascii_lowercase();
    Ok(format!(
        "{AUTH_EMAIL_VERIFICATION_PREFIX}{}",
        auth_email_storage_key_digest("pending", &[email.as_str()])?
    ))
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

pub(super) fn generate_auth_verification_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

pub(super) fn auth_verification_token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.trim().as_bytes()))
}

pub(super) fn auth_verification_code_hash(verification_token: &str, code: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            format!(
                "aether-email-verification\0{}\0{}",
                verification_token.trim(),
                code.trim()
            )
            .as_bytes()
        )
    )
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.as_bytes()
        .iter()
        .zip(right.as_bytes())
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

pub(super) fn auth_email_registration_proof_key(
    email: &str,
    verification_token: &str,
) -> Result<String, GatewayError> {
    let email = email.trim().to_ascii_lowercase();
    Ok(format!(
        "{AUTH_EMAIL_VERIFIED_PREFIX}{}",
        auth_email_storage_key_digest(
            "registration-proof",
            &[email.as_str(), verification_token.trim()]
        )?
    ))
}

pub(super) fn auth_verification_token_matches(
    stored: &StoredAuthEmailVerificationCode,
    verification_token: &str,
) -> bool {
    !verification_token.trim().is_empty()
        && constant_time_eq(
            &stored.verification_token_hash,
            &auth_verification_token_hash(verification_token),
        )
}

pub(super) fn auth_verification_code_matches(
    stored: &StoredAuthEmailVerificationCode,
    verification_token: &str,
    code: &str,
) -> bool {
    constant_time_eq(
        &stored.code_hash,
        &auth_verification_code_hash(verification_token, code),
    )
}

fn render_auth_template_string(
    template: &str,
    variables: &std::collections::BTreeMap<String, String>,
    escape_html: bool,
) -> Result<String, GatewayError> {
    if template.len() > ADMIN_EMAIL_TEMPLATE_MAX_SUBJECT_BYTES
        || template.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
    {
        return Err(GatewayError::Internal(
            "email template subject is invalid or oversized".to_string(),
        ));
    }
    let mut rendered = template.to_string();
    for (key, value) in variables {
        if value.len() > ADMIN_EMAIL_TEMPLATE_MAX_PREVIEW_VALUE_BYTES {
            return Err(GatewayError::Internal(
                "email template variable is oversized".to_string(),
            ));
        }
        let pattern = regex::Regex::new(&format!(r"\{{\{{\s*{}\s*\}}\}}", regex::escape(key)))
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let replacement = if escape_html {
            escape_admin_email_template_html(value)
        } else {
            value.clone()
        };
        let (matched_bytes, occurrences) = pattern
            .find_iter(&rendered)
            .fold((0usize, 0usize), |(matched, count), found| {
                (matched.saturating_add(found.as_str().len()), count + 1)
            });
        let prospective_len = occurrences
            .checked_mul(replacement.len())
            .and_then(|bytes| {
                rendered
                    .len()
                    .checked_sub(matched_bytes)?
                    .checked_add(bytes)
            });
        if prospective_len.is_none_or(|length| length > ADMIN_EMAIL_TEMPLATE_MAX_SUBJECT_BYTES) {
            return Err(GatewayError::Internal(
                "rendered email template subject is oversized".to_string(),
            ));
        }
        rendered = pattern
            .replace_all(&rendered, regex::NoExpand(replacement.as_str()))
            .into_owned();
        if rendered.len() > ADMIN_EMAIL_TEMPLATE_MAX_SUBJECT_BYTES {
            return Err(GatewayError::Internal(
                "rendered email template subject is oversized".to_string(),
            ));
        }
    }
    if !admin_email_template_subject_is_valid(&rendered) {
        return Err(GatewayError::Internal(
            "rendered email template subject is invalid or oversized".to_string(),
        ));
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
    let key = auth_email_verification_key(email)?;
    let raw = state.runtime_kv_get(&key).await?;
    raw.map(|value| {
        serde_json::from_str::<StoredAuthEmailVerificationCode>(&value)
            .map_err(|err| GatewayError::Internal(err.to_string()))
    })
    .transpose()
}

pub(in crate::handlers::public::support) async fn auth_email_is_verified(
    state: &AppState,
    email: &str,
    verification_token: &str,
) -> Result<bool, GatewayError> {
    let key = auth_email_registration_proof_key(email, verification_token)?;
    state.runtime_kv_exists(&key).await
}

pub(super) async fn mark_auth_email_verified(
    state: &AppState,
    email: &str,
    verification_token: &str,
) -> Result<bool, GatewayError> {
    let key = auth_email_registration_proof_key(email, verification_token)?;
    state
        .runtime_kv_setex(&key, "verified", AUTH_EMAIL_VERIFIED_TTL_SECS)
        .await?;
    Ok(true)
}

pub(super) async fn consume_auth_email_verification_code(
    state: &AppState,
    email: &str,
) -> Result<Option<StoredAuthEmailVerificationCode>, GatewayError> {
    let key = auth_email_verification_key(email)?;
    state
        .runtime_kv_getdel(&key)
        .await?
        .map(|value| {
            serde_json::from_str::<StoredAuthEmailVerificationCode>(&value)
                .map_err(|err| GatewayError::Internal(err.to_string()))
        })
        .transpose()
}

pub(in crate::handlers::public::support) async fn consume_auth_email_registration_proof(
    state: &AppState,
    email: &str,
    verification_token: &str,
) -> Result<bool, GatewayError> {
    let key = auth_email_registration_proof_key(email, verification_token)?;
    Ok(state.runtime_kv_getdel(&key).await?.as_deref() == Some("verified"))
}

pub(super) async fn clear_auth_email_pending_code(
    state: &AppState,
    email: &str,
) -> Result<bool, GatewayError> {
    let verification_key = auth_email_verification_key(email)?;
    state.runtime_kv_del(&verification_key).await
}

pub(super) async fn store_auth_email_verification_code(
    state: &AppState,
    email: &str,
    code: &str,
    verification_token: &str,
    created_at: chrono::DateTime<chrono::Utc>,
    ttl_seconds: u64,
) -> Result<bool, GatewayError> {
    let key = auth_email_verification_key(email)?;
    let value = json!({
        "code_hash": auth_verification_code_hash(verification_token, code),
        "created_at": created_at.to_rfc3339(),
        "verification_token_hash": auth_verification_token_hash(verification_token),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_email_runtime_keys_are_keyed_and_contain_no_plaintext_identifiers() {
        let email = "Alice+security@example.com";
        let token = "verification-token-security-test";
        let pending = auth_email_verification_key(email).expect("pending key should build");
        let proof =
            auth_email_registration_proof_key(email, token).expect("proof key should build");

        for key in [&pending, &proof] {
            assert!(!key.to_ascii_lowercase().contains("alice"));
            assert!(!key.contains("example.com"));
            assert!(!key.contains(token));
        }
        let plain_email_hash = format!("{:x}", Sha256::digest(email.to_ascii_lowercase()));
        assert!(!pending.contains(&plain_email_hash));
        assert_ne!(pending, proof);
    }

    #[tokio::test]
    async fn auth_email_challenges_and_registration_proofs_are_consumed_once_concurrently() {
        let state = AppState::new().expect("app state should build");
        let email = "atomic@example.com";
        let token = "atomic-verification-token";
        store_auth_email_verification_code(&state, email, "123456", token, chrono::Utc::now(), 300)
            .await
            .expect("challenge should store");

        let (first_challenge, second_challenge) = tokio::join!(
            consume_auth_email_verification_code(&state, email),
            consume_auth_email_verification_code(&state, email)
        );
        assert_eq!(
            [first_challenge, second_challenge]
                .into_iter()
                .filter(|result| result.as_ref().is_ok_and(Option::is_some))
                .count(),
            1
        );

        mark_auth_email_verified(&state, email, token)
            .await
            .expect("proof should store");
        let (first_proof, second_proof) = tokio::join!(
            consume_auth_email_registration_proof(&state, email, token),
            consume_auth_email_registration_proof(&state, email, token)
        );
        assert_eq!(
            [first_proof, second_proof]
                .into_iter()
                .filter(|result| result.as_ref().is_ok_and(|consumed| *consumed))
                .count(),
            1
        );
    }
}
