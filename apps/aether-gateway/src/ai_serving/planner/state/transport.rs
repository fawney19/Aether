use super::PlannerAppState;
pub(crate) use crate::ai_serving::transport::{
    GatewayProviderTransportSnapshot, LocalResolvedOAuthRequestAuth,
};
use crate::GatewayError;

impl<'a> PlannerAppState<'a> {
    pub(crate) async fn read_provider_transport_snapshot(
        self,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
    ) -> Result<Option<GatewayProviderTransportSnapshot>, GatewayError> {
        self.app()
            .read_provider_transport_snapshot(provider_id, endpoint_id, key_id)
            .await
    }

    pub(crate) async fn resolve_local_oauth_request_auth(
        self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Result<Option<LocalResolvedOAuthRequestAuth>, GatewayError> {
        self.app().resolve_local_oauth_request_auth(transport).await
    }
}
