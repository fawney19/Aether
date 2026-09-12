use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest as _, Sha256};

use crate::AppState;

use aether_contracts::internal_gateway::{
    verify_internal_gateway_request_signature, INTERNAL_GATEWAY_AUTH_NONCE_HEADER,
    INTERNAL_GATEWAY_AUTH_SIGNATURE_HEADER, INTERNAL_GATEWAY_AUTH_TIMESTAMP_HEADER,
};

pub(crate) const INTERNAL_GATEWAY_AUTH_SECRET_ENV: &str = "AETHER_INTERNAL_GATEWAY_AUTH_SECRET";
const INTERNAL_GATEWAY_AUTH_SECRET_MIN_BYTES: usize = 32;
const INTERNAL_GATEWAY_AUTH_SECRET_MAX_BYTES: usize = 4096;
const INTERNAL_GATEWAY_AUTH_CLOCK_SKEW_SECS: u64 = 300;
const INTERNAL_GATEWAY_AUTH_NONCE_MIN_BYTES: usize = 16;
const INTERNAL_GATEWAY_AUTH_NONCE_MAX_BYTES: usize = 128;
const INTERNAL_GATEWAY_AUTH_NONCE_TTL: Duration =
    Duration::from_secs(INTERNAL_GATEWAY_AUTH_CLOCK_SKEW_SECS * 2 + 30);
const INTERNAL_GATEWAY_AUTH_NONCE_KEY_PREFIX: &str = "internal:gateway:auth:nonce:";

#[derive(Clone)]
pub(crate) struct InternalGatewayAuthConfig {
    mode: InternalGatewayAuthMode,
}

#[derive(Clone)]
enum InternalGatewayAuthMode {
    Disabled,
    Misconfigured,
    Hmac {
        secret: Arc<[u8]>,
    },
    #[cfg(test)]
    LoopbackTestCompatibility,
}

impl fmt::Debug for InternalGatewayAuthConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InternalGatewayAuthConfig")
            .field("status", &self.status())
            .finish()
    }
}

impl InternalGatewayAuthConfig {
    pub(crate) fn for_process() -> Self {
        #[cfg(test)]
        {
            Self {
                mode: InternalGatewayAuthMode::LoopbackTestCompatibility,
            }
        }
        #[cfg(not(test))]
        {
            Self::from_environment()
        }
    }

    fn from_environment() -> Self {
        match std::env::var(INTERNAL_GATEWAY_AUTH_SECRET_ENV) {
            Ok(value) => Self::from_secret_value(Some(value.as_str())),
            Err(std::env::VarError::NotPresent) => Self::from_secret_value(None),
            Err(std::env::VarError::NotUnicode(_)) => Self {
                mode: InternalGatewayAuthMode::Misconfigured,
            },
        }
    }

    fn from_secret_value(value: Option<&str>) -> Self {
        let Some(value) = value else {
            return Self {
                mode: InternalGatewayAuthMode::Disabled,
            };
        };
        let secret = value.trim().as_bytes();
        if !(INTERNAL_GATEWAY_AUTH_SECRET_MIN_BYTES..=INTERNAL_GATEWAY_AUTH_SECRET_MAX_BYTES)
            .contains(&secret.len())
        {
            return Self {
                mode: InternalGatewayAuthMode::Misconfigured,
            };
        }
        Self {
            mode: InternalGatewayAuthMode::Hmac {
                secret: Arc::from(secret),
            },
        }
    }

    pub(crate) fn status(&self) -> &'static str {
        match &self.mode {
            InternalGatewayAuthMode::Disabled => "disabled",
            InternalGatewayAuthMode::Misconfigured => "misconfigured",
            InternalGatewayAuthMode::Hmac { .. } => "hmac_authenticated",
            #[cfg(test)]
            InternalGatewayAuthMode::LoopbackTestCompatibility => "test_loopback_compatibility",
        }
    }

    #[cfg(test)]
    pub(crate) fn disabled_for_tests() -> Self {
        Self {
            mode: InternalGatewayAuthMode::Disabled,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_secret_for_tests(secret: &str) -> Self {
        let config = Self::from_secret_value(Some(secret));
        assert_eq!(config.status(), "hmac_authenticated");
        config
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InternalGatewayAuthError {
    Disabled,
    Invalid,
    Unavailable,
}

pub(crate) async fn authenticate_internal_gateway_request(
    state: &AppState,
    remote_addr: &std::net::SocketAddr,
    method: &http::Method,
    path_and_query: &str,
    headers: &http::HeaderMap,
    body: &[u8],
) -> Result<(), InternalGatewayAuthError> {
    let secret = match &state.internal_gateway_auth.mode {
        InternalGatewayAuthMode::Disabled => return Err(InternalGatewayAuthError::Disabled),
        InternalGatewayAuthMode::Misconfigured => {
            return Err(InternalGatewayAuthError::Unavailable)
        }
        InternalGatewayAuthMode::Hmac { secret } => Arc::clone(secret),
        #[cfg(test)]
        InternalGatewayAuthMode::LoopbackTestCompatibility => {
            return remote_addr
                .ip()
                .is_loopback()
                .then_some(())
                .ok_or(InternalGatewayAuthError::Invalid)
        }
    };

    let timestamp = unique_header(headers, INTERNAL_GATEWAY_AUTH_TIMESTAMP_HEADER)
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or(InternalGatewayAuthError::Invalid)?;
    let nonce = unique_header(headers, INTERNAL_GATEWAY_AUTH_NONCE_HEADER)
        .filter(|value| valid_nonce(value))
        .ok_or(InternalGatewayAuthError::Invalid)?;
    let signature = unique_header(headers, INTERNAL_GATEWAY_AUTH_SIGNATURE_HEADER)
        .ok_or(InternalGatewayAuthError::Invalid)?;
    let now = current_unix_secs().ok_or(InternalGatewayAuthError::Unavailable)?;
    if now.abs_diff(timestamp) > INTERNAL_GATEWAY_AUTH_CLOCK_SKEW_SECS {
        return Err(InternalGatewayAuthError::Invalid);
    }
    if !verify_internal_gateway_request_signature(
        secret.as_ref(),
        method.as_str(),
        path_and_query,
        timestamp,
        nonce,
        body,
        signature,
    ) {
        return Err(InternalGatewayAuthError::Invalid);
    }

    let nonce_digest = Sha256::digest(nonce.as_bytes());
    let nonce_key = format!("{INTERNAL_GATEWAY_AUTH_NONCE_KEY_PREFIX}{nonce_digest:x}");
    match state
        .runtime_state
        .kv_set_if_absent(
            &nonce_key,
            timestamp.to_string(),
            INTERNAL_GATEWAY_AUTH_NONCE_TTL,
        )
        .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(InternalGatewayAuthError::Invalid),
        Err(_) => Err(InternalGatewayAuthError::Unavailable),
    }
}

fn unique_header<'a>(headers: &'a http::HeaderMap, name: &'static str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?.trim();
    if value.is_empty() || values.next().is_some() {
        return None;
    }
    Some(value)
}

fn valid_nonce(nonce: &str) -> bool {
    (INTERNAL_GATEWAY_AUTH_NONCE_MIN_BYTES..=INTERNAL_GATEWAY_AUTH_NONCE_MAX_BYTES)
        .contains(&nonce.len())
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn current_unix_secs() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|v| v.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_configuration_fails_closed_and_debug_never_contains_secret() {
        assert_eq!(
            InternalGatewayAuthConfig::from_secret_value(None).status(),
            "disabled"
        );
        assert_eq!(
            InternalGatewayAuthConfig::from_secret_value(Some("short")).status(),
            "misconfigured"
        );
        let secret = "internal-gateway-test-secret-32-bytes-minimum";
        let config = InternalGatewayAuthConfig::from_secret_value(Some(secret));
        assert_eq!(config.status(), "hmac_authenticated");
        assert!(!format!("{config:?}").contains(secret));
    }
}
