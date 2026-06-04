use crate::provider::ProviderPoolAdapter;

#[derive(Debug, Clone, Default)]
pub struct OpenCodeFreeProviderPoolAdapter;

impl ProviderPoolAdapter for OpenCodeFreeProviderPoolAdapter {
    fn provider_type(&self) -> &'static str {
        "opencode_free"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::ProviderPoolCapability;

    #[test]
    fn test_opencode_free_provider_type() {
        let adapter = OpenCodeFreeProviderPoolAdapter;
        assert_eq!(adapter.provider_type(), "opencode_free");
    }

    #[test]
    fn test_opencode_free_no_capabilities() {
        let adapter = OpenCodeFreeProviderPoolAdapter;
        let capabilities = adapter.capabilities();

        assert!(!capabilities.supports(ProviderPoolCapability::PlanTier));
        assert!(!capabilities.supports(ProviderPoolCapability::QuotaRefresh));
    }

    #[test]
    fn test_opencode_free_no_quota_refresh() {
        let adapter = OpenCodeFreeProviderPoolAdapter;
        assert!(!adapter.supports_quota_refresh());
    }
}
