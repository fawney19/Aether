use serde_json::Value;

use super::super::snapshot::GatewayProviderTransportSnapshot;
use super::auth::{
    resolve_local_opencode_free_request_auth, OpencodeFreeNoAuthReason, OpencodeFreeRequestAuth,
    OpencodeFreeRequestAuthSupport, OPENCODE_FREE_PROVIDER_TYPE,
};
use crate::rules::{body_rules_have_enabled_rules, header_rules_have_enabled_rules};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpencodeFreeRequestSideSpec {
    pub auth: OpencodeFreeRequestAuth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpencodeFreeRequestSideSupport {
    Supported(OpencodeFreeRequestSideSpec),
    Unsupported(OpencodeFreeRequestSideUnsupportedReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpencodeFreeRequestSideUnsupportedReason {
    InactiveTransport,
    WrongProviderType,
    UnsupportedApiFormat,
    UnsupportedHeaderRules,
    UnsupportedBodyRules,
    UnsupportedNetworkConfig,
    UnsupportedAuth(OpencodeFreeNoAuthReason),
}

pub fn is_opencode_free_provider_transport(transport: &GatewayProviderTransportSnapshot) -> bool {
    transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(OPENCODE_FREE_PROVIDER_TYPE)
}

pub fn classify_local_opencode_free_request_support(
    transport: &GatewayProviderTransportSnapshot,
    _request_body: &Value,
) -> OpencodeFreeRequestSideSupport {
    if !transport.provider.is_active || !transport.endpoint.is_active || !transport.key.is_active {
        return OpencodeFreeRequestSideSupport::Unsupported(
            OpencodeFreeRequestSideUnsupportedReason::InactiveTransport,
        );
    }
    if !is_opencode_free_provider_transport(transport) {
        return OpencodeFreeRequestSideSupport::Unsupported(
            OpencodeFreeRequestSideUnsupportedReason::WrongProviderType,
        );
    }

    let endpoint_format =
        aether_ai_formats::normalize_api_format_alias(&transport.endpoint.api_format);
    if endpoint_format != "openai:chat" {
        return OpencodeFreeRequestSideSupport::Unsupported(
            OpencodeFreeRequestSideUnsupportedReason::UnsupportedApiFormat,
        );
    }
    if header_rules_have_enabled_rules(transport.endpoint.header_rules.as_ref()) {
        return OpencodeFreeRequestSideSupport::Unsupported(
            OpencodeFreeRequestSideUnsupportedReason::UnsupportedHeaderRules,
        );
    }
    if body_rules_have_enabled_rules(transport.endpoint.body_rules.as_ref()) {
        return OpencodeFreeRequestSideSupport::Unsupported(
            OpencodeFreeRequestSideUnsupportedReason::UnsupportedBodyRules,
        );
    }
    if transport.provider.proxy.is_some()
        || transport.endpoint.proxy.is_some()
        || transport.key.proxy.is_some()
        || transport.key.fingerprint.is_some()
    {
        return OpencodeFreeRequestSideSupport::Unsupported(
            OpencodeFreeRequestSideUnsupportedReason::UnsupportedNetworkConfig,
        );
    }

    let auth = match resolve_local_opencode_free_request_auth(transport) {
        OpencodeFreeRequestAuthSupport::Supported(auth) => auth,
        OpencodeFreeRequestAuthSupport::Unsupported(reason) => {
            return OpencodeFreeRequestSideSupport::Unsupported(
                OpencodeFreeRequestSideUnsupportedReason::UnsupportedAuth(reason),
            );
        }
    };

    OpencodeFreeRequestSideSupport::Supported(OpencodeFreeRequestSideSpec { auth })
}
