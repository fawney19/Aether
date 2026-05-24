use std::collections::BTreeMap;
use std::sync::Arc;

use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use aether_pool_core::PoolSchedulingPreset;
use serde_json::{Map, Value};

use crate::capability::ProviderPoolCapability;
use crate::presets::normalize_provider_scheduling_presets;
use crate::provider::{ProviderPoolAdapter, ProviderPoolMemberInput};
use crate::providers::{
    AntigravityProviderPoolAdapter, ChatGptWebProviderPoolAdapter, CodexProviderPoolAdapter,
    DefaultProviderPoolAdapter, GrokProviderPoolAdapter, KiroProviderPoolAdapter,
    WindsurfProviderPoolAdapter, CLAUDE_CODE_PROVIDER_POOL_ADAPTER,
    GEMINI_CLI_PROVIDER_POOL_ADAPTER, VERTEX_AI_PROVIDER_POOL_ADAPTER,
};

#[derive(Clone)]
pub struct ProviderPoolService {
    adapters: BTreeMap<String, Arc<dyn ProviderPoolAdapter>>,
    default_adapter: Arc<dyn ProviderPoolAdapter>,
}

impl std::fmt::Debug for ProviderPoolService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderPoolService")
            .field("provider_types", &self.adapters.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Default for ProviderPoolService {
    fn default() -> Self {
        Self {
            adapters: BTreeMap::new(),
            default_adapter: Arc::new(DefaultProviderPoolAdapter),
        }
    }
}

impl ProviderPoolService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_builtin_adapters() -> Self {
        Self::new()
            .with_adapter(Arc::new(AntigravityProviderPoolAdapter))
            .with_adapter(Arc::new(CLAUDE_CODE_PROVIDER_POOL_ADAPTER))
            .with_adapter(Arc::new(CodexProviderPoolAdapter))
            .with_adapter(Arc::new(GEMINI_CLI_PROVIDER_POOL_ADAPTER))
            .with_adapter(Arc::new(GrokProviderPoolAdapter))
            .with_adapter(Arc::new(KiroProviderPoolAdapter))
            .with_adapter(Arc::new(ChatGptWebProviderPoolAdapter))
            .with_adapter(Arc::new(WindsurfProviderPoolAdapter))
            .with_adapter(Arc::new(VERTEX_AI_PROVIDER_POOL_ADAPTER))
    }

    pub fn with_adapter(mut self, adapter: Arc<dyn ProviderPoolAdapter>) -> Self {
        self.adapters
            .insert(adapter.provider_type().trim().to_ascii_lowercase(), adapter);
        self
    }

    pub fn adapter(&self, provider_type: &str) -> Arc<dyn ProviderPoolAdapter> {
        self.adapters
            .get(provider_type.trim().to_ascii_lowercase().as_str())
            .cloned()
            .unwrap_or_else(|| self.default_adapter.clone())
    }

    pub fn provider_types(&self) -> impl Iterator<Item = &str> {
        self.adapters.keys().map(String::as_str)
    }

    pub fn provider_types_for_capability(&self, capability: ProviderPoolCapability) -> Vec<String> {
        self.adapters
            .iter()
            .filter(|(_, adapter)| adapter.capabilities().supports(capability))
            .map(|(provider_type, _)| provider_type.clone())
            .collect()
    }

    pub fn supports_quota_refresh(&self, provider_type: &str) -> bool {
        self.adapter(provider_type).supports_quota_refresh()
    }

    pub fn quota_refresh_endpoint_for_provider(
        &self,
        provider_type: &str,
        endpoints: &[StoredProviderCatalogEndpoint],
        include_inactive: bool,
    ) -> Option<StoredProviderCatalogEndpoint> {
        self.adapter(provider_type)
            .quota_refresh_endpoint(endpoints, include_inactive)
    }

    pub fn quota_refresh_unsupported_message(&self, provider_type: &str) -> String {
        self.adapter(provider_type)
            .quota_refresh_unsupported_message()
    }

    pub fn quota_refresh_missing_endpoint_message(&self, provider_type: &str) -> String {
        self.adapter(provider_type)
            .quota_refresh_missing_endpoint_message()
    }

    pub fn normalize_scheduling_presets(
        &self,
        provider_type: &str,
        scheduling_presets: &[PoolSchedulingPreset],
    ) -> Vec<PoolSchedulingPreset> {
        normalize_provider_scheduling_presets(
            self.adapter(provider_type).as_ref(),
            scheduling_presets,
        )
    }

    pub fn member_signals(
        &self,
        provider_type: &str,
        key: &StoredProviderCatalogKey,
        auth_config: Option<&Map<String, Value>>,
    ) -> aether_pool_core::PoolMemberSignals {
        let adapter = self.adapter(provider_type);
        let input = ProviderPoolMemberInput {
            provider_type,
            key,
            auth_config,
        };
        adapter.member_signals(&input)
    }
}
