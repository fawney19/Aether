#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GatewayCredentialCarrier {
    AuthorizationBearer,
    XApiKey,
    ApiKey,
    XGoogApiKey,
    QueryKey,
    CookieHeader,
}

impl GatewayCredentialCarrier {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::AuthorizationBearer => "authorization_bearer",
            Self::XApiKey => "x_api_key",
            Self::ApiKey => "api_key",
            Self::XGoogApiKey => "x_goog_api_key",
            Self::QueryKey => "query_key",
            Self::CookieHeader => "cookie_header",
        }
    }

    pub(crate) const fn request_auth_channel(self) -> &'static str {
        match self {
            Self::AuthorizationBearer | Self::CookieHeader => "bearer_like",
            Self::XApiKey | Self::ApiKey | Self::XGoogApiKey | Self::QueryKey => "api_key",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct GatewayTrustedAuthHeaders {
    pub(super) user_id: String,
    pub(super) api_key_id: String,
    pub(super) balance_remaining: Option<f64>,
    pub(super) access_allowed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GatewayTrustedAdminHeaders {
    pub(super) user_id: String,
    pub(super) user_role: String,
    pub(super) session_id: Option<String>,
    pub(super) management_token_id: Option<String>,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct GatewayCredentialBundle {
    pub(super) authorization_bearer: Option<String>,
    pub(super) x_api_key: Option<String>,
    pub(super) api_key: Option<String>,
    pub(super) x_goog_api_key: Option<String>,
    pub(super) query_key: Option<String>,
    pub(super) cookie_header: Option<String>,
}

impl std::fmt::Debug for GatewayCredentialBundle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = |value: &Option<String>| value.as_ref().map(|_| "[REDACTED]");
        formatter
            .debug_struct("GatewayCredentialBundle")
            .field(
                "authorization_bearer",
                &redacted(&self.authorization_bearer),
            )
            .field("x_api_key", &redacted(&self.x_api_key))
            .field("api_key", &redacted(&self.api_key))
            .field("x_goog_api_key", &redacted(&self.x_goog_api_key))
            .field("query_key", &redacted(&self.query_key))
            .field("cookie_header", &redacted(&self.cookie_header))
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum GatewayPrimaryCredential {
    ProviderApiKey {
        raw: String,
        carrier: GatewayCredentialCarrier,
    },
    BearerToken {
        raw: String,
        carrier: GatewayCredentialCarrier,
    },
    CookieHeader {
        raw: String,
        carrier: GatewayCredentialCarrier,
    },
}

impl std::fmt::Debug for GatewayPrimaryCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (variant, carrier) = match self {
            Self::ProviderApiKey { carrier, .. } => ("ProviderApiKey", carrier),
            Self::BearerToken { carrier, .. } => ("BearerToken", carrier),
            Self::CookieHeader { carrier, .. } => ("CookieHeader", carrier),
        };
        formatter
            .debug_struct(variant)
            .field("raw", &"[REDACTED]")
            .field("carrier", carrier)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct GatewayExtractedCredentials {
    pub(super) trusted_headers: Option<GatewayTrustedAuthHeaders>,
    pub(super) trusted_admin_headers: Option<GatewayTrustedAdminHeaders>,
    pub(super) bundle: GatewayCredentialBundle,
    pub(super) primary: Option<GatewayPrimaryCredential>,
}

#[derive(Clone, PartialEq)]
pub(super) enum GatewayPrincipalCandidate {
    TrustedHeaders(GatewayTrustedAuthHeaders),
    ApiKeyHash {
        key_hash: String,
        carrier: GatewayCredentialCarrier,
    },
    DeferredBearerToken {
        raw: String,
        carrier: GatewayCredentialCarrier,
    },
    DeferredCookieHeader {
        raw: String,
        carrier: GatewayCredentialCarrier,
    },
}

impl std::fmt::Debug for GatewayPrincipalCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TrustedHeaders(headers) => formatter
                .debug_tuple("TrustedHeaders")
                .field(headers)
                .finish(),
            Self::ApiKeyHash { carrier, .. } => formatter
                .debug_struct("ApiKeyHash")
                .field("key_hash", &"[REDACTED]")
                .field("carrier", carrier)
                .finish(),
            Self::DeferredBearerToken { carrier, .. } => formatter
                .debug_struct("DeferredBearerToken")
                .field("raw", &"[REDACTED]")
                .field("carrier", carrier)
                .finish(),
            Self::DeferredCookieHeader { carrier, .. } => formatter
                .debug_struct("DeferredCookieHeader")
                .field("raw", &"[REDACTED]")
                .field("carrier", carrier)
                .finish(),
        }
    }
}

#[cfg(test)]
mod debug_redaction_tests {
    use super::{GatewayCredentialBundle, GatewayCredentialCarrier, GatewayPrimaryCredential};

    #[test]
    fn gateway_credential_debug_output_redacts_raw_authorization_values() {
        let bundle = GatewayCredentialBundle {
            authorization_bearer: Some("bundle-bearer-canary".to_string()),
            api_key: Some("bundle-api-key-canary".to_string()),
            cookie_header: Some("bundle-cookie-canary".to_string()),
            ..GatewayCredentialBundle::default()
        };
        let primary = GatewayPrimaryCredential::ProviderApiKey {
            raw: "primary-api-key-canary".to_string(),
            carrier: GatewayCredentialCarrier::ApiKey,
        };
        let debug = format!("{bundle:?} {primary:?}");
        assert!(debug.contains("[REDACTED]"));
        for secret in [
            "bundle-bearer-canary",
            "bundle-api-key-canary",
            "bundle-cookie-canary",
            "primary-api-key-canary",
        ] {
            assert!(!debug.contains(secret), "debug output leaked {secret}");
        }
    }
}
