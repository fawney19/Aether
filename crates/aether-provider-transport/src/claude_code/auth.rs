use super::super::auth::resolve_local_standard_auth;
use super::super::snapshot::GatewayProviderTransportSnapshot;
use super::super::supports_local_oauth_request_auth_resolution;

pub fn supports_local_claude_code_auth(transport: &GatewayProviderTransportSnapshot) -> bool {
    resolve_local_standard_auth(transport).is_some_and(|(_, value)| !value.trim().is_empty())
        || supports_local_oauth_request_auth_resolution(transport)
}
