pub(crate) type AdminGatewayProviderTransportSnapshot =
    crate::provider_transport::GatewayProviderTransportSnapshot;
pub(crate) type AdminLocalOAuthRefreshError = crate::provider_transport::LocalOAuthRefreshError;
pub(crate) type AdminKiroRequestAuth = crate::provider_transport::kiro::KiroRequestAuth;
pub(crate) type AdminKiroAuthConfig = crate::provider_transport::kiro::KiroAuthConfig;
pub(crate) type AdminKiroOAuthRefreshAdapter =
    crate::provider_transport::kiro::KiroOAuthRefreshAdapter;
pub(crate) type AdminProviderOAuthTemplate =
    crate::provider_transport::provider_types::ProviderOAuthTemplate;

pub(crate) fn is_fixed_provider_type_for_admin_oauth(provider_type: &str) -> bool {
    crate::provider_transport::provider_types::provider_type_is_fixed_for_admin_oauth(provider_type)
}

pub(crate) fn admin_provider_oauth_template(
    provider_type: &str,
) -> Option<AdminProviderOAuthTemplate> {
    crate::provider_transport::provider_types::provider_type_admin_oauth_template(provider_type)
}

pub(crate) fn admin_provider_oauth_template_types() -> impl Iterator<Item = &'static str> {
    crate::provider_transport::provider_types::ADMIN_PROVIDER_OAUTH_TEMPLATE_TYPES
        .iter()
        .copied()
}
