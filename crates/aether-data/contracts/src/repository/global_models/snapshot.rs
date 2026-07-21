use std::collections::BTreeSet;

use super::{
    AdminGlobalModelListQuery, AdminProviderModelListQuery, PublicCatalogModelListQuery,
    PublicCatalogModelSearchQuery, PublicGlobalModelQuery, StoredAdminGlobalModel,
    StoredAdminGlobalModelPage, StoredAdminProviderModel, StoredProviderActiveGlobalModel,
    StoredProviderModelStats, StoredPublicCatalogModel, StoredPublicGlobalModel,
    StoredPublicGlobalModelPage,
};

/// Immutable input for the shared global-model read policy.
///
/// Database adapters can load one consistent view and apply exactly the same
/// filtering, sorting, pagination, and enrichment rules as the memory adapter
/// without depending on the `aether-data` facade.
#[derive(Debug, Clone, Default)]
pub struct GlobalModelSnapshot {
    public_global_models: Vec<StoredPublicGlobalModel>,
    admin_global_models: Vec<StoredAdminGlobalModel>,
    public_catalog_models: Vec<StoredPublicCatalogModel>,
    admin_provider_models: Vec<StoredAdminProviderModel>,
    provider_model_stats: Vec<StoredProviderModelStats>,
    active_global_model_refs: Vec<StoredProviderActiveGlobalModel>,
}

impl GlobalModelSnapshot {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredPublicGlobalModel>,
    {
        Self {
            public_global_models: items.into_iter().collect(),
            ..Self::default()
        }
    }

    pub fn with_admin_global_models<I>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredAdminGlobalModel>,
    {
        self.admin_global_models = items.into_iter().collect();
        self
    }

    pub fn with_public_catalog_models<I>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredPublicCatalogModel>,
    {
        self.public_catalog_models = items.into_iter().collect();
        self
    }

    pub fn with_admin_provider_models<I>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredAdminProviderModel>,
    {
        self.admin_provider_models = items.into_iter().collect();
        self
    }

    pub fn with_provider_model_stats<I>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredProviderModelStats>,
    {
        self.provider_model_stats = items.into_iter().collect();
        self
    }

    pub fn with_active_global_model_refs<I>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredProviderActiveGlobalModel>,
    {
        self.active_global_model_refs = items.into_iter().collect();
        self
    }

    pub fn list_public_models(
        &self,
        query: &PublicGlobalModelQuery,
    ) -> StoredPublicGlobalModelPage {
        let search = normalized_optional_search(query.search.as_deref());
        let mut filtered = self
            .public_global_models
            .iter()
            .filter(|item| match query.is_active {
                Some(is_active) => item.is_active == is_active,
                None => item.is_active,
            })
            .filter(|item| {
                let Some(search) = search.as_deref() else {
                    return true;
                };
                item.name.to_ascii_lowercase().contains(search)
                    || item
                        .display_name
                        .as_deref()
                        .is_some_and(|value| value.to_ascii_lowercase().contains(search))
            })
            .cloned()
            .collect::<Vec<_>>();
        filtered.sort_by(|left, right| left.name.cmp(&right.name));
        let total = filtered.len();
        let items = filtered
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect();
        StoredPublicGlobalModelPage { items, total }
    }

    pub fn get_public_model_by_name(&self, model_name: &str) -> Option<StoredPublicGlobalModel> {
        self.public_global_models
            .iter()
            .find(|item| item.is_active && item.name == model_name)
            .cloned()
    }

    pub fn list_public_catalog_models(
        &self,
        query: &PublicCatalogModelListQuery,
    ) -> Vec<StoredPublicCatalogModel> {
        let provider_id = normalized_optional_value(query.provider_id.as_deref());
        let mut filtered = self
            .public_catalog_models
            .iter()
            .filter(|item| item.is_active)
            .filter(|item| provider_id.is_none_or(|value| item.provider_id == value))
            .cloned()
            .collect::<Vec<_>>();
        sort_public_catalog_models(&mut filtered);
        filtered
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect()
    }

    pub fn search_public_catalog_models(
        &self,
        query: &PublicCatalogModelSearchQuery,
    ) -> Vec<StoredPublicCatalogModel> {
        let provider_id = normalized_optional_value(query.provider_id.as_deref());
        let search = query.search.trim().to_ascii_lowercase();
        let mut filtered = self
            .public_catalog_models
            .iter()
            .filter(|item| item.is_active)
            .filter(|item| provider_id.is_none_or(|value| item.provider_id == value))
            .filter(|item| {
                item.provider_model_name
                    .to_ascii_lowercase()
                    .contains(&search)
                    || item.name.to_ascii_lowercase().contains(&search)
                    || item.display_name.to_ascii_lowercase().contains(&search)
            })
            .cloned()
            .collect::<Vec<_>>();
        sort_public_catalog_models(&mut filtered);
        filtered.truncate(query.limit);
        filtered
    }

    pub fn list_admin_global_models(
        &self,
        query: &AdminGlobalModelListQuery,
    ) -> StoredAdminGlobalModelPage {
        let search = normalized_optional_search(query.search.as_deref());
        let mut filtered = self
            .admin_global_models
            .iter()
            .filter(|item| query.is_active.is_none_or(|value| item.is_active == value))
            .filter(|item| {
                let Some(search) = search.as_deref() else {
                    return true;
                };
                item.name.to_ascii_lowercase().contains(search)
                    || item.display_name.to_ascii_lowercase().contains(search)
            })
            .cloned()
            .collect::<Vec<_>>();
        filtered.sort_by(|left, right| left.name.cmp(&right.name));
        let total = filtered.len();
        let items = filtered
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .map(|item| self.enrich_admin_global_model(&item))
            .collect();
        StoredAdminGlobalModelPage { items, total }
    }

    pub fn list_admin_provider_models(
        &self,
        query: &AdminProviderModelListQuery,
    ) -> Vec<StoredAdminProviderModel> {
        let mut filtered = self
            .admin_provider_models
            .iter()
            .filter(|item| item.provider_id == query.provider_id)
            .filter(|item| query.is_active.is_none_or(|value| item.is_active == value))
            .cloned()
            .collect::<Vec<_>>();
        sort_admin_provider_models(&mut filtered);
        filtered
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect()
    }

    pub fn get_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Option<StoredAdminProviderModel> {
        self.admin_provider_models
            .iter()
            .find(|item| item.provider_id == provider_id && item.id == model_id)
            .cloned()
    }

    pub fn list_admin_provider_available_source_models(
        &self,
        provider_id: &str,
    ) -> Vec<StoredAdminProviderModel> {
        let mut filtered = self
            .admin_provider_models
            .iter()
            .filter(|item| item.provider_id == provider_id && item.is_active)
            .filter(|item| {
                self.admin_global_models
                    .iter()
                    .find(|global| global.id == item.global_model_id)
                    .is_some_and(|global| global.is_active)
            })
            .cloned()
            .collect::<Vec<_>>();
        filtered.sort_by(|left, right| {
            left.global_model_name
                .cmp(&right.global_model_name)
                .then_with(|| right.created_at_unix_ms.cmp(&left.created_at_unix_ms))
                .then_with(|| left.id.cmp(&right.id))
        });
        filtered
    }

    pub fn get_admin_global_model_by_id(
        &self,
        global_model_id: &str,
    ) -> Option<StoredAdminGlobalModel> {
        self.admin_global_models
            .iter()
            .find(|item| item.id == global_model_id)
            .map(|item| self.enrich_admin_global_model(item))
    }

    pub fn get_admin_global_model_by_name(
        &self,
        model_name: &str,
    ) -> Option<StoredAdminGlobalModel> {
        self.admin_global_models
            .iter()
            .find(|item| item.name == model_name)
            .map(|item| self.enrich_admin_global_model(item))
    }

    pub fn list_admin_provider_models_by_global_model_id(
        &self,
        global_model_id: &str,
    ) -> Vec<StoredAdminProviderModel> {
        let mut filtered = self
            .admin_provider_models
            .iter()
            .filter(|item| item.global_model_id == global_model_id)
            .cloned()
            .collect::<Vec<_>>();
        sort_admin_provider_models(&mut filtered);
        filtered
    }

    pub fn list_provider_model_stats(
        &self,
        provider_ids: &[String],
    ) -> Vec<StoredProviderModelStats> {
        let provider_ids = provider_ids.iter().collect::<BTreeSet<_>>();
        self.provider_model_stats
            .iter()
            .filter(|item| provider_ids.contains(&item.provider_id))
            .cloned()
            .collect()
    }

    pub fn list_active_global_model_ids_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Vec<StoredProviderActiveGlobalModel> {
        let provider_ids = provider_ids.iter().collect::<BTreeSet<_>>();
        self.active_global_model_refs
            .iter()
            .filter(|item| provider_ids.contains(&item.provider_id))
            .cloned()
            .collect()
    }

    fn enrich_admin_global_model(&self, item: &StoredAdminGlobalModel) -> StoredAdminGlobalModel {
        let mut enriched = item.clone();
        let providers = self
            .admin_provider_models
            .iter()
            .filter(|model| model.global_model_id == item.id)
            .map(|model| model.provider_id.as_str())
            .collect::<BTreeSet<_>>();
        let active_providers = self
            .admin_provider_models
            .iter()
            .filter(|model| {
                model.global_model_id == item.id && model.is_active && model.is_available
            })
            .map(|model| model.provider_id.as_str())
            .collect::<BTreeSet<_>>();
        enriched.provider_count = providers.len() as u64;
        enriched.active_provider_count = active_providers.len() as u64;
        enriched
    }
}

fn normalized_optional_value(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn normalized_optional_search(value: Option<&str>) -> Option<String> {
    normalized_optional_value(value).map(str::to_ascii_lowercase)
}

fn sort_public_catalog_models(items: &mut [StoredPublicCatalogModel]) {
    items.sort_by(|left, right| {
        left.provider_name
            .cmp(&right.provider_name)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn sort_admin_provider_models(items: &mut [StoredAdminProviderModel]) {
    items.sort_by(|left, right| {
        right
            .created_at_unix_ms
            .unwrap_or_default()
            .cmp(&left.created_at_unix_ms.unwrap_or_default())
            .then_with(|| left.id.cmp(&right.id))
    });
}
