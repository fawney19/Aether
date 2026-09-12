use super::{
    build_auth_error_response, decrypt_or_migrate_system_config_secret, http, system_config_bool,
    system_config_string, system_config_string_list, AppState, Body, Response,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::warn;

const TURNSTILE_SITEVERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";
const MAX_TURNSTILE_RESPONSE_BYTES: usize = 64 * 1024;
const TURNSTILE_TOKEN_MAX_LEN: usize = 2048;
const TURNSTILE_SITEVERIFY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy)]
pub(super) enum AuthTurnstileAction {
    SendVerificationCode,
    Register,
}

impl AuthTurnstileAction {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::SendVerificationCode => "send_verification_code",
            Self::Register => "register",
        }
    }
}

struct AuthTurnstileConfig {
    enabled: bool,
    site_key: Option<String>,
    secret_key: Option<String>,
    allowed_hostnames: Vec<String>,
}

#[derive(Serialize)]
struct TurnstileSiteverifyRequest<'a> {
    secret: &'a str,
    response: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    remoteip: Option<&'a str>,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
struct TurnstileSiteverifyResponse {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    hostname: Option<String>,
    #[serde(default, rename = "error-codes")]
    error_codes: Vec<String>,
}

enum AuthTurnstileFailure {
    BadRequest(&'static str),
    ServiceUnavailable(&'static str),
}

impl AuthTurnstileFailure {
    fn into_response(self) -> Response<Body> {
        match self {
            Self::BadRequest(detail) => {
                build_auth_error_response(http::StatusCode::BAD_REQUEST, detail, false)
            }
            Self::ServiceUnavailable(detail) => {
                build_auth_error_response(http::StatusCode::SERVICE_UNAVAILABLE, detail, false)
            }
        }
    }
}

pub(super) async fn verify_auth_turnstile(
    state: &AppState,
    client_ip: std::net::IpAddr,
    token: Option<&str>,
    action: AuthTurnstileAction,
) -> Result<(), Response<Body>> {
    match verify_auth_turnstile_inner(state, client_ip, token, action).await {
        Ok(()) => Ok(()),
        Err(err) => Err(err.into_response()),
    }
}

async fn verify_auth_turnstile_inner(
    state: &AppState,
    client_ip: std::net::IpAddr,
    token: Option<&str>,
    action: AuthTurnstileAction,
) -> Result<(), AuthTurnstileFailure> {
    let config = read_auth_turnstile_config(state).await?;
    if !config.enabled {
        return Ok(());
    }

    let (Some(_site_key), Some(secret_key)) =
        (config.site_key.as_deref(), config.secret_key.as_deref())
    else {
        warn!("turnstile is enabled but site key or secret key is missing");
        return Err(AuthTurnstileFailure::ServiceUnavailable(
            "人机验证服务暂不可用，请稍后重试",
        ));
    };

    let token = token
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(AuthTurnstileFailure::BadRequest("请先完成人机验证"))?;
    if token.len() > TURNSTILE_TOKEN_MAX_LEN {
        warn!(
            token_len = token.len(),
            "turnstile token exceeds maximum length"
        );
        return Err(AuthTurnstileFailure::BadRequest("人机验证失败，请重试"));
    }

    let remoteip = client_ip.to_string();
    let siteverify_request = TurnstileSiteverifyRequest {
        secret: secret_key,
        response: token,
        remoteip: Some(remoteip.as_str()),
        idempotency_key: uuid::Uuid::new_v4().to_string(),
    };
    let siteverify_url = turnstile_siteverify_url(state);
    let response = tokio::time::timeout(
        turnstile_siteverify_timeout(state),
        state
            .client
            .post(siteverify_url)
            .form(&siteverify_request)
            .send(),
    )
    .await
    .map_err(|_| {
        warn!("turnstile siteverify request timed out");
        AuthTurnstileFailure::ServiceUnavailable("人机验证服务暂不可用，请稍后重试")
    })?
    .map_err(|_| {
        warn!("turnstile siteverify request failed");
        AuthTurnstileFailure::ServiceUnavailable("人机验证服务暂不可用，请稍后重试")
    })?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        warn!(
            status,
            "turnstile siteverify returned non-success HTTP status"
        );
        return Err(AuthTurnstileFailure::ServiceUnavailable(
            "人机验证服务暂不可用，请稍后重试",
        ));
    }
    let body = aether_http::read_response_bytes_with_limit(response, MAX_TURNSTILE_RESPONSE_BYTES)
        .await
        .map_err(|_| {
            warn!("turnstile siteverify response read failed");
            AuthTurnstileFailure::ServiceUnavailable("人机验证服务暂不可用，请稍后重试")
        })?;
    let payload = serde_json::from_slice::<TurnstileSiteverifyResponse>(&body).map_err(|_| {
        warn!("turnstile siteverify response decode failed");
        AuthTurnstileFailure::ServiceUnavailable("人机验证服务暂不可用，请稍后重试")
    })?;

    if !payload.success {
        warn!(
            error_codes = ?payload.error_codes,
            action = ?payload.action,
            hostname = ?payload.hostname,
            "turnstile siteverify rejected token"
        );
        if turnstile_siteverify_error_is_service_unavailable(&payload.error_codes) {
            return Err(AuthTurnstileFailure::ServiceUnavailable(
                "人机验证服务暂不可用，请稍后重试",
            ));
        }
        return Err(AuthTurnstileFailure::BadRequest("人机验证失败，请重试"));
    }
    if payload.action.as_deref() != Some(action.as_str()) {
        warn!(
            expected_action = action.as_str(),
            actual_action = ?payload.action,
            "turnstile siteverify action mismatch"
        );
        return Err(AuthTurnstileFailure::BadRequest("人机验证失败，请重试"));
    }
    if !config.allowed_hostnames.is_empty() {
        let Some(hostname) = payload.hostname.as_deref().map(str::to_ascii_lowercase) else {
            warn!("turnstile siteverify response missing hostname");
            return Err(AuthTurnstileFailure::BadRequest("人机验证失败，请重试"));
        };
        if !config
            .allowed_hostnames
            .iter()
            .any(|allowed| allowed == &hostname)
        {
            warn!(
                hostname = %hostname,
                allowed_hostnames = ?config.allowed_hostnames,
                "turnstile siteverify hostname mismatch"
            );
            return Err(AuthTurnstileFailure::BadRequest("人机验证失败，请重试"));
        }
    }

    Ok(())
}

fn turnstile_siteverify_error_is_service_unavailable(error_codes: &[String]) -> bool {
    error_codes.iter().any(|code| {
        matches!(
            code.trim().to_ascii_lowercase().as_str(),
            "missing-input-secret" | "invalid-input-secret" | "internal-error"
        )
    })
}

async fn read_auth_turnstile_config(
    state: &AppState,
) -> Result<AuthTurnstileConfig, AuthTurnstileFailure> {
    let enabled = state
        .read_system_config_json_value("turnstile_enabled")
        .await
        .map_err(|err| {
            warn!(error = %crate::error::redact_error_debug(&err), "turnstile enabled config lookup failed");
            AuthTurnstileFailure::ServiceUnavailable("人机验证服务暂不可用，请稍后重试")
        })?;
    let site_key = state
        .read_system_config_json_value("turnstile_site_key")
        .await
        .map_err(|err| {
            warn!(error = %crate::error::redact_error_debug(&err), "turnstile site key config lookup failed");
            AuthTurnstileFailure::ServiceUnavailable("人机验证服务暂不可用，请稍后重试")
        })?;
    let secret_key = state
        .read_system_config_json_value("turnstile_secret_key")
        .await
        .map_err(|err| {
            warn!(error = %crate::error::redact_error_debug(&err), "turnstile secret key config lookup failed");
            AuthTurnstileFailure::ServiceUnavailable("人机验证服务暂不可用，请稍后重试")
        })?;
    let allowed_hostnames = state
        .read_system_config_json_value("turnstile_allowed_hostnames")
        .await
        .map_err(|err| {
            warn!(error = %crate::error::redact_error_debug(&err), "turnstile hostname config lookup failed");
            AuthTurnstileFailure::ServiceUnavailable("人机验证服务暂不可用，请稍后重试")
        })?;

    let secret_key = match system_config_string(secret_key.as_ref()) {
        Some(value) => Some(
            decrypt_or_migrate_system_config_secret(state, "turnstile_secret_key", value)
                .await
                .map_err(|error| {
                    warn!(error = %crate::error::redact_error_debug(&error), "turnstile secret key migration failed");
                    AuthTurnstileFailure::ServiceUnavailable("人机验证服务暂不可用，请稍后重试")
                })?,
        ),
        None => None,
    };

    Ok(AuthTurnstileConfig {
        enabled: system_config_bool(enabled.as_ref(), false),
        site_key: system_config_string(site_key.as_ref()),
        secret_key,
        allowed_hostnames: system_config_string_list(allowed_hostnames.as_ref()),
    })
}

fn turnstile_siteverify_url(state: &AppState) -> &str {
    #[cfg(test)]
    if let Some(url) = state.turnstile_siteverify_url_override.as_deref() {
        return url;
    }
    TURNSTILE_SITEVERIFY_URL
}

fn turnstile_siteverify_timeout(state: &AppState) -> Duration {
    #[cfg(test)]
    if let Some(timeout) = state.turnstile_siteverify_timeout_override {
        return timeout;
    }
    TURNSTILE_SITEVERIFY_TIMEOUT
}
