use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use aether_pool_core::{PoolMemberSignals, PoolSchedulingPreset};
use serde_json::{Map, Value};

use crate::capability::{ProviderPoolCapabilities, ProviderPoolCapability};
use crate::plan::{derive_plan_tier, normalize_provider_plan_tier};
use crate::quota::{
    provider_pool_account_blocked, provider_pool_quota_reset_seconds,
    provider_pool_quota_snapshot_exhausted_decision, provider_pool_quota_usage_ratio,
};

#[derive(Debug, Clone)]
pub struct ProviderPoolMemberInput<'a> {
    pub provider_type: &'a str,
    pub key: &'a StoredProviderCatalogKey,
    pub auth_config: Option<&'a Map<String, Value>>,
}

pub trait ProviderPoolAdapter: Send + Sync {
    fn provider_type(&self) -> &'static str;

    fn capabilities(&self) -> ProviderPoolCapabilities {
        ProviderPoolCapabilities::default()
    }

    fn default_scheduling_presets(&self) -> Vec<PoolSchedulingPreset> {
        Vec::new()
    }

    fn supports_quota_refresh(&self) -> bool {
        self.capabilities()
            .supports(ProviderPoolCapability::QuotaRefresh)
    }

    fn quota_refresh_endpoint(
        &self,
        endpoints: &[StoredProviderCatalogEndpoint],
        include_inactive: bool,
    ) -> Option<StoredProviderCatalogEndpoint> {
        if !self.supports_quota_refresh() {
            return None;
        }
        provider_pool_matching_endpoint(endpoints, include_inactive, |_| true)
    }

    fn quota_refresh_unsupported_message(&self) -> String {
        "该 Provider 暂不支持自动刷新额度".to_string()
    }

    fn quota_refresh_missing_endpoint_message(&self) -> String {
        "找不到有效端点".to_string()
    }

    fn normalize_plan_tier(&self, value: &str) -> Option<String> {
        normalize_provider_plan_tier(value, self.provider_type())
    }

    fn member_signals(&self, input: &ProviderPoolMemberInput<'_>) -> PoolMemberSignals {
        PoolMemberSignals {
            plan_tier: derive_plan_tier(input.provider_type, input.key, input.auth_config),
            quota_usage_ratio: provider_pool_quota_usage_ratio(input.key),
            quota_reset_seconds: provider_pool_quota_reset_seconds(input.key),
            account_blocked: provider_pool_account_blocked(input.key),
            quota_exhausted: self.quota_exhausted(input),
            ..PoolMemberSignals::default()
        }
    }

    fn quota_exhausted(&self, input: &ProviderPoolMemberInput<'_>) -> bool {
        provider_pool_quota_snapshot_exhausted_decision(input.key, input.provider_type)
            .unwrap_or(false)
    }
}

pub(crate) fn provider_pool_matching_endpoint<F>(
    endpoints: &[StoredProviderCatalogEndpoint],
    include_inactive: bool,
    predicate: F,
) -> Option<StoredProviderCatalogEndpoint>
where
    F: Fn(&StoredProviderCatalogEndpoint) -> bool,
{
    endpoints
        .iter()
        .find(|endpoint| endpoint.is_active && predicate(endpoint))
        .cloned()
        .or_else(|| {
            include_inactive.then(|| {
                endpoints
                    .iter()
                    .find(|endpoint| !endpoint.is_active && predicate(endpoint))
                    .cloned()
            })?
        })
}

pub(crate) fn provider_pool_endpoint_format_matches(
    endpoint: &StoredProviderCatalogEndpoint,
    expected: &str,
) -> bool {
    endpoint
        .api_format
        .trim()
        .eq_ignore_ascii_case(expected.trim())
}
