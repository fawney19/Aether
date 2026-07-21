use crate::{ProviderCapability, ProviderDescriptor, ProviderHealth};

pub trait ProviderEligibilityPolicy: Send + Sync {
    fn is_eligible(&self, provider: &ProviderDescriptor, capability: ProviderCapability) -> bool {
        provider.health != ProviderHealth::Unavailable && provider.supports(capability)
    }
}
