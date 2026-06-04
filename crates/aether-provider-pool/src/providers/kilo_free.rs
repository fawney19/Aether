use crate::provider::ProviderPoolAdapter;

#[derive(Debug, Clone, Default)]
pub struct KiloFreeProviderPoolAdapter;

impl ProviderPoolAdapter for KiloFreeProviderPoolAdapter {
    fn provider_type(&self) -> &'static str {
        "kilo_free"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{ProviderPoolCapability, ProviderPoolCapabilities};

    #[test]
    fn test_kilo_free_provider_type() {
        let adapter = KiloFreeProviderPoolAdapter;
        assert_eq!(adapter.provider_type(), "kilo_free");
    }

    #[test]
    fn test_kilo_free_no_capabilities() {
        let adapter = KiloFreeProviderPoolAdapter;
        let caps: ProviderPoolCapabilities = adapter.capabilities();
        assert!(!caps.plan_tier);
        assert!(!caps.quota_reset);
        assert!(!caps.quota_refresh);
        assert!(!caps.supports(ProviderPoolCapability::PlanTier));
        assert!(!caps.supports(ProviderPoolCapability::QuotaReset));
        assert!(!caps.supports(ProviderPoolCapability::QuotaRefresh));
    }

    #[test]
    fn test_kilo_free_no_quota_refresh() {
        let adapter = KiloFreeProviderPoolAdapter;
        assert!(!adapter.supports_quota_refresh());
    }
}
