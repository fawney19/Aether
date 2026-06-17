use super::super::snapshot::GatewayProviderTransportSnapshot;

pub const OPENCODE_FREE_PROVIDER_TYPE: &str = "opencode_free";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpencodeFreeRequestAuth;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpencodeFreeRequestAuthSupport {
    Supported(OpencodeFreeRequestAuth),
    Unsupported(OpencodeFreeNoAuthReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpencodeFreeNoAuthReason {
    WrongProviderType,
}

pub fn resolve_local_opencode_free_request_auth(
    transport: &GatewayProviderTransportSnapshot,
) -> OpencodeFreeRequestAuthSupport {
    if !transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case(OPENCODE_FREE_PROVIDER_TYPE)
    {
        return OpencodeFreeRequestAuthSupport::Unsupported(OpencodeFreeNoAuthReason::WrongProviderType);
    }

    OpencodeFreeRequestAuthSupport::Supported(OpencodeFreeRequestAuth)
}
