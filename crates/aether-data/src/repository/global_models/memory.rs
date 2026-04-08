use std::collections::BTreeSet;
use std::sync::RwLock;

use async_trait::async_trait;

use super::{
    AdminGlobalModelListQuery, AdminProviderModelListQuery, CreateAdminGlobalModelRecord,
    GlobalModelReadRepository, GlobalModelWriteRepository, PublicCatalogModelListQuery,
    PublicCatalogModelSearchQuery, PublicGlobalModelQuery, StoredAdminGlobalModel,
    StoredAdminGlobalModelPage, StoredAdminProviderModel, StoredProviderActiveGlobalModel,
    StoredProviderModelStats, StoredPublicCatalogModel, StoredPublicGlobalModel,
    StoredPublicGlobalModelPage, UpdateAdminGlobalModelRecord, UpsertAdminProviderModelRecord,
};
use crate::DataLayerError;

#[derive(Debug, Default)]
pub struct InMemoryGlobalModelReadRepository {
    items: RwLock<Vec<StoredPublicGlobalModel>>,
    admin_global_model_items: RwLock<Vec<StoredAdminGlobalModel>>,
    public_catalog_items: RwLock<Vec<StoredPublicCatalogModel>>,
    admin_provider_model_items: RwLock<Vec<StoredAdminProviderModel>>,
    provider_model_stats: RwLock<Vec<StoredProviderModelStats>>,
    active_global_model_refs: RwLock<Vec<StoredProviderActiveGlobalModel>>,
}

impl InMemoryGlobalModelReadRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredPublicGlobalModel>,
    {
        Self {
            items: RwLock::new(items.into_iter().collect()),
            admin_global_model_items: RwLock::new(Vec::new()),
            public_catalog_items: RwLock::new(Vec::new()),
            admin_provider_model_items: RwLock::new(Vec::new()),
            provider_model_stats: RwLock::new(Vec::new()),
            active_global_model_refs: RwLock::new(Vec::new()),
        }
    }

    pub fn with_public_catalog_models<I>(self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredPublicCatalogModel>,
    {
        *self
            .public_catalog_items
            .write()
            .expect("public catalog model repository lock") = items.into_iter().collect();
        self
    }

    pub fn with_provider_model_stats<I>(self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredProviderModelStats>,
    {
        *self
            .provider_model_stats
            .write()
            .expect("provider model stats repository lock") = items.into_iter().collect();
        self
    }

    pub fn with_admin_provider_models<I>(self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredAdminProviderModel>,
    {
        *self
            .admin_provider_model_items
            .write()
            .expect("admin provider model repository lock") = items.into_iter().collect();
        self
    }

    pub fn with_active_global_model_refs<I>(self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredProviderActiveGlobalModel>,
    {
        *self
            .active_global_model_refs
            .write()
            .expect("active global model repository lock") = items.into_iter().collect();
        self
    }

    pub fn with_admin_global_models<I>(self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredAdminGlobalModel>,
    {
        *self
            .admin_global_model_items
            .write()
            .expect("admin global model repository lock") = items.into_iter().collect();
        self
    }

    fn admin_global_model_provider_counts(&self, global_model_id: &str) -> (u64, u64) {
        let items = self
            .admin_provider_model_items
            .read()
            .expect("admin provider model repository lock");
        let provider_count = items
            .iter()
            .filter(|item| item.global_model_id == global_model_id)
            .map(|item| item.provider_id.clone())
            .collect::<BTreeSet<_>>()
            .len() as u64;
        let active_provider_count = items
            .iter()
            .filter(|item| item.global_model_id == global_model_id && item.is_active)
            .map(|item| item.provider_id.clone())
            .collect::<BTreeSet<_>>()
            .len() as u64;
        (provider_count, active_provider_count)
    }

    fn enrich_admin_global_model(&self, item: &StoredAdminGlobalModel) -> StoredAdminGlobalModel {
        let mut enriched = item.clone();
        let (provider_count, active_provider_count) =
            self.admin_global_model_provider_counts(&item.id);
        enriched.provider_count = provider_count;
        enriched.active_provider_count = active_provider_count;
        enriched
    }
}

#[async_trait]
impl GlobalModelReadRepository for InMemoryGlobalModelReadRepository {
    async fn list_public_models(
        &self,
        query: &PublicGlobalModelQuery,
    ) -> Result<StoredPublicGlobalModelPage, DataLayerError> {
        let items = self.items.read().expect("global model repository lock");
        let search = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase());

        let mut filtered = items
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
                        .map(|value| value.to_ascii_lowercase().contains(search))
                        .unwrap_or(false)
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
        Ok(StoredPublicGlobalModelPage { items, total })
    }

    async fn get_public_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<StoredPublicGlobalModel>, DataLayerError> {
        let items = self.items.read().expect("global model repository lock");
        Ok(items
            .iter()
            .find(|item| item.is_active && item.name == model_name)
            .cloned())
    }

    async fn list_public_catalog_models(
        &self,
        query: &PublicCatalogModelListQuery,
    ) -> Result<Vec<StoredPublicCatalogModel>, DataLayerError> {
        let items = self
            .public_catalog_items
            .read()
            .expect("public catalog model repository lock");
        let provider_id = query
            .provider_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());

        let mut filtered = items
            .iter()
            .filter(|item| item.is_active)
            .filter(|item| match provider_id {
                Some(provider_id) => item.provider_id == provider_id,
                None => true,
            })
            .cloned()
            .collect::<Vec<_>>();
        filtered.sort_by(|left, right| {
            left.provider_name
                .cmp(&right.provider_name)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(filtered
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect())
    }

    async fn search_public_catalog_models(
        &self,
        query: &PublicCatalogModelSearchQuery,
    ) -> Result<Vec<StoredPublicCatalogModel>, DataLayerError> {
        let items = self
            .public_catalog_items
            .read()
            .expect("public catalog model repository lock");
        let provider_id = query
            .provider_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let search = query.search.trim().to_ascii_lowercase();

        let mut filtered = items
            .iter()
            .filter(|item| item.is_active)
            .filter(|item| match provider_id {
                Some(provider_id) => item.provider_id == provider_id,
                None => true,
            })
            .filter(|item| {
                item.provider_model_name
                    .to_ascii_lowercase()
                    .contains(&search)
                    || item.name.to_ascii_lowercase().contains(&search)
                    || item.display_name.to_ascii_lowercase().contains(&search)
            })
            .cloned()
            .collect::<Vec<_>>();
        filtered.sort_by(|left, right| {
            left.provider_name
                .cmp(&right.provider_name)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.id.cmp(&right.id))
        });
        filtered.truncate(query.limit);
        Ok(filtered)
    }

    async fn list_admin_global_models(
        &self,
        query: &AdminGlobalModelListQuery,
    ) -> Result<StoredAdminGlobalModelPage, DataLayerError> {
        let items = self
            .admin_global_model_items
            .read()
            .expect("admin global model repository lock");
        let search = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase());
        let mut filtered = items
            .iter()
            .filter(|item| match query.is_active {
                Some(is_active) => item.is_active == is_active,
                None => true,
            })
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
        Ok(StoredAdminGlobalModelPage { items, total })
    }

    async fn list_admin_provider_models(
        &self,
        query: &AdminProviderModelListQuery,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        let items = self
            .admin_provider_model_items
            .read()
            .expect("admin provider model repository lock");
        let mut filtered = items
            .iter()
            .filter(|item| item.provider_id == query.provider_id)
            .filter(|item| match query.is_active {
                Some(is_active) => item.is_active == is_active,
                None => true,
            })
            .cloned()
            .collect::<Vec<_>>();
        filtered.sort_by(|left, right| {
            right
                .created_at_unix_secs
                .unwrap_or_default()
                .cmp(&left.created_at_unix_secs.unwrap_or_default())
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(filtered
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect())
    }

    async fn get_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        Ok(self
            .admin_provider_model_items
            .read()
            .expect("admin provider model repository lock")
            .iter()
            .find(|item| item.provider_id == provider_id && item.id == model_id)
            .cloned())
    }

    async fn list_admin_provider_available_source_models(
        &self,
        provider_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        let items = self
            .admin_provider_model_items
            .read()
            .expect("admin provider model repository lock");
        let active_globals = self
            .admin_global_model_items
            .read()
            .expect("admin global model repository lock");

        let mut filtered = items
            .iter()
            .filter(|item| item.provider_id == provider_id && item.is_active)
            .filter(|item| {
                active_globals
                    .iter()
                    .find(|global| global.id == item.global_model_id)
                    .map(|global| global.is_active)
                    .unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>();
        filtered.sort_by(|left, right| {
            left.global_model_name
                .cmp(&right.global_model_name)
                .then_with(|| right.created_at_unix_secs.cmp(&left.created_at_unix_secs))
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(filtered)
    }

    async fn get_admin_global_model_by_id(
        &self,
        global_model_id: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        let items = self
            .admin_global_model_items
            .read()
            .expect("admin global model repository lock");
        Ok(items
            .iter()
            .find(|item| item.id == global_model_id)
            .map(|item| self.enrich_admin_global_model(item)))
    }

    async fn get_admin_global_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        let items = self
            .admin_global_model_items
            .read()
            .expect("admin global model repository lock");
        Ok(items
            .iter()
            .find(|item| item.name == model_name)
            .map(|item| self.enrich_admin_global_model(item)))
    }

    async fn list_admin_provider_models_by_global_model_id(
        &self,
        global_model_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        let items = self
            .admin_provider_model_items
            .read()
            .expect("admin provider model repository lock");
        let mut filtered = items
            .iter()
            .filter(|item| item.global_model_id == global_model_id)
            .cloned()
            .collect::<Vec<_>>();
        filtered.sort_by(|left, right| {
            right
                .created_at_unix_secs
                .unwrap_or_default()
                .cmp(&left.created_at_unix_secs.unwrap_or_default())
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(filtered)
    }

    async fn list_provider_model_stats(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderModelStats>, DataLayerError> {
        let provider_ids = provider_ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        Ok(self
            .provider_model_stats
            .read()
            .expect("provider model stats repository lock")
            .iter()
            .filter(|item| provider_ids.contains(&item.provider_id))
            .cloned()
            .collect())
    }

    async fn list_active_global_model_ids_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderActiveGlobalModel>, DataLayerError> {
        let provider_ids = provider_ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        Ok(self
            .active_global_model_refs
            .read()
            .expect("active global model repository lock")
            .iter()
            .filter(|item| provider_ids.contains(&item.provider_id))
            .cloned()
            .collect())
    }
}

#[async_trait]
impl GlobalModelWriteRepository for InMemoryGlobalModelReadRepository {
    async fn create_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        let global_model = self
            .get_admin_global_model_by_id(&record.global_model_id)
            .await?
            .ok_or_else(|| DataLayerError::UnexpectedValue("global model not found".to_string()))?;

        let stored = StoredAdminProviderModel::new(
            record.id.clone(),
            record.provider_id.clone(),
            record.global_model_id.clone(),
            record.provider_model_name.clone(),
            record.provider_model_mappings.clone(),
            record.price_per_request,
            record.tiered_pricing.clone(),
            record.supports_vision,
            record.supports_function_calling,
            record.supports_streaming,
            record.supports_extended_thinking,
            record.supports_image_generation,
            record.is_active,
            record.is_available,
            record.config.clone(),
            Some(1_711_000_000),
            Some(1_711_000_000),
            Some(global_model.name.clone()),
            Some(global_model.display_name.clone()),
            global_model.default_price_per_request,
            global_model.default_tiered_pricing.clone(),
            global_model.config.clone(),
        )?;
        self.admin_provider_model_items
            .write()
            .expect("admin provider model repository lock")
            .push(stored.clone());
        Ok(Some(stored))
    }

    async fn update_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        let global_model = self
            .get_admin_global_model_by_id(&record.global_model_id)
            .await?
            .ok_or_else(|| DataLayerError::UnexpectedValue("global model not found".to_string()))?;
        let mut items = self
            .admin_provider_model_items
            .write()
            .expect("admin provider model repository lock");
        let Some(existing) = items
            .iter_mut()
            .find(|item| item.id == record.id && item.provider_id == record.provider_id)
        else {
            return Ok(None);
        };
        existing.global_model_id = record.global_model_id.clone();
        existing.provider_model_name = record.provider_model_name.clone();
        existing.provider_model_mappings = record.provider_model_mappings.clone();
        existing.price_per_request = record.price_per_request;
        existing.tiered_pricing = record.tiered_pricing.clone();
        existing.supports_vision = record.supports_vision;
        existing.supports_function_calling = record.supports_function_calling;
        existing.supports_streaming = record.supports_streaming;
        existing.supports_extended_thinking = record.supports_extended_thinking;
        existing.supports_image_generation = record.supports_image_generation;
        existing.is_active = record.is_active;
        existing.is_available = record.is_available;
        existing.config = record.config.clone();
        existing.updated_at_unix_secs = Some(1_711_000_100);
        existing.global_model_name = Some(global_model.name.clone());
        existing.global_model_display_name = Some(global_model.display_name.clone());
        existing.global_model_default_price_per_request = global_model.default_price_per_request;
        existing.global_model_default_tiered_pricing = global_model.default_tiered_pricing.clone();
        existing.global_model_config = global_model.config.clone();
        Ok(Some(existing.clone()))
    }

    async fn delete_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<bool, DataLayerError> {
        let mut items = self
            .admin_provider_model_items
            .write()
            .expect("admin provider model repository lock");
        let original_len = items.len();
        items.retain(|item| !(item.provider_id == provider_id && item.id == model_id));
        Ok(items.len() != original_len)
    }

    async fn create_admin_global_model(
        &self,
        record: &CreateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        let stored = StoredAdminGlobalModel::new(
            record.id.clone(),
            record.name.clone(),
            record.display_name.clone(),
            record.is_active,
            record.default_price_per_request,
            record.default_tiered_pricing.clone(),
            record.supported_capabilities.clone(),
            record.config.clone(),
            0,
            0,
            0,
            Some(1_711_000_000),
            Some(1_711_000_000),
        )?;
        self.admin_global_model_items
            .write()
            .expect("admin global model repository lock")
            .push(stored);
        self.get_admin_global_model_by_id(&record.id).await
    }

    async fn update_admin_global_model(
        &self,
        record: &UpdateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        {
            let mut items = self
                .admin_global_model_items
                .write()
                .expect("admin global model repository lock");
            let Some(existing) = items.iter_mut().find(|item| item.id == record.id) else {
                return Ok(None);
            };
            existing.display_name = record.display_name.clone();
            existing.is_active = record.is_active;
            existing.default_price_per_request = record.default_price_per_request;
            existing.default_tiered_pricing = record.default_tiered_pricing.clone();
            existing.supported_capabilities = record.supported_capabilities.clone();
            existing.config = record.config.clone();
            existing.updated_at_unix_secs = Some(1_711_000_100);
        }
        self.get_admin_global_model_by_id(&record.id).await
    }

    async fn delete_admin_global_model(
        &self,
        global_model_id: &str,
    ) -> Result<bool, DataLayerError> {
        let mut globals = self
            .admin_global_model_items
            .write()
            .expect("admin global model repository lock");
        let original_len = globals.len();
        globals.retain(|item| item.id != global_model_id);
        drop(globals);
        self.admin_provider_model_items
            .write()
            .expect("admin provider model repository lock")
            .retain(|item| item.global_model_id != global_model_id);
        Ok(original_len
            != self
                .admin_global_model_items
                .read()
                .expect("admin global model repository lock")
                .len())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::InMemoryGlobalModelReadRepository;
    use crate::repository::global_models::{
        GlobalModelReadRepository, PublicCatalogModelListQuery, PublicCatalogModelSearchQuery,
        PublicGlobalModelQuery, StoredPublicCatalogModel, StoredPublicGlobalModel,
    };

    fn sample_model(
        id: &str,
        name: &str,
        display_name: &str,
        is_active: bool,
    ) -> StoredPublicGlobalModel {
        StoredPublicGlobalModel::new(
            id.to_string(),
            name.to_string(),
            Some(display_name.to_string()),
            is_active,
            Some(0.02),
            Some(json!({"tiers":[{"up_to": null, "input_price_per_1m": 3.0, "output_price_per_1m": 15.0}]})),
            Some(json!(["vision"])),
            Some(json!({"family": "test"})),
            0,
        )
        .expect("global model should build")
    }

    fn sample_public_catalog_model(
        id: &str,
        provider_id: &str,
        provider_name: &str,
        provider_model_name: &str,
        name: &str,
        display_name: &str,
    ) -> StoredPublicCatalogModel {
        StoredPublicCatalogModel::new(
            id.to_string(),
            provider_id.to_string(),
            provider_name.to_string(),
            provider_model_name.to_string(),
            name.to_string(),
            display_name.to_string(),
            Some(format!("{display_name} description")),
            Some(format!("https://cdn.example/{name}.png")),
            Some(3.0),
            Some(15.0),
            Some(1.5),
            Some(0.3),
            Some(true),
            Some(true),
            Some(true),
            true,
        )
        .expect("public catalog model should build")
    }

    #[tokio::test]
    async fn defaults_to_active_models_only() {
        let repository = InMemoryGlobalModelReadRepository::seed(vec![
            sample_model("gm-1", "claude-sonnet-4-5", "Claude Sonnet 4.5", true),
            sample_model("gm-2", "legacy-model", "Legacy Model", false),
        ]);

        let page = repository
            .list_public_models(&PublicGlobalModelQuery {
                offset: 0,
                limit: 50,
                is_active: None,
                search: None,
            })
            .await
            .expect("list should succeed");

        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].name, "claude-sonnet-4-5");
    }

    #[tokio::test]
    async fn search_matches_name_and_display_name() {
        let repository = InMemoryGlobalModelReadRepository::seed(vec![
            sample_model("gm-1", "gpt-5", "GPT 5", true),
            sample_model("gm-2", "claude-sonnet-4-5", "Claude Sonnet 4.5", true),
        ]);

        let page = repository
            .list_public_models(&PublicGlobalModelQuery {
                offset: 0,
                limit: 50,
                is_active: None,
                search: Some("sonnet".to_string()),
            })
            .await
            .expect("list should succeed");

        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].name, "claude-sonnet-4-5");
    }

    #[tokio::test]
    async fn get_public_model_by_name_only_returns_active_exact_match() {
        let repository = InMemoryGlobalModelReadRepository::seed(vec![
            sample_model("gm-1", "gpt-5", "GPT 5", true),
            sample_model("gm-2", "gpt-5-old", "GPT 5 Old", false),
        ]);

        let model = repository
            .get_public_model_by_name("gpt-5")
            .await
            .expect("lookup should succeed");
        assert_eq!(model.expect("model should exist").name, "gpt-5");

        let missing = repository
            .get_public_model_by_name("gpt-5-old")
            .await
            .expect("lookup should succeed");
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn lists_public_catalog_models_with_provider_filter() {
        let repository =
            InMemoryGlobalModelReadRepository::seed(Vec::<StoredPublicGlobalModel>::new())
                .with_public_catalog_models(vec![
                    sample_public_catalog_model(
                        "model-1",
                        "provider-openai",
                        "openai",
                        "gpt-5-preview",
                        "gpt-5",
                        "GPT 5",
                    ),
                    sample_public_catalog_model(
                        "model-2",
                        "provider-claude",
                        "claude",
                        "claude-3-7-sonnet",
                        "claude-3-7-sonnet",
                        "Claude 3.7 Sonnet",
                    ),
                ]);

        let items = repository
            .list_public_catalog_models(&PublicCatalogModelListQuery {
                provider_id: Some("provider-openai".to_string()),
                offset: 0,
                limit: 50,
            })
            .await
            .expect("list should succeed");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].provider_id, "provider-openai");
        assert_eq!(items[0].name, "gpt-5");
    }

    #[tokio::test]
    async fn searches_public_catalog_models_by_provider_and_display_name() {
        let repository =
            InMemoryGlobalModelReadRepository::seed(Vec::<StoredPublicGlobalModel>::new())
                .with_public_catalog_models(vec![
                    sample_public_catalog_model(
                        "model-1",
                        "provider-openai",
                        "openai",
                        "gpt-5-preview",
                        "gpt-5",
                        "GPT 5",
                    ),
                    sample_public_catalog_model(
                        "model-2",
                        "provider-claude",
                        "claude",
                        "claude-3-7-sonnet",
                        "claude-3-7-sonnet",
                        "Claude 3.7 Sonnet",
                    ),
                ]);

        let items = repository
            .search_public_catalog_models(&PublicCatalogModelSearchQuery {
                search: "sonnet".to_string(),
                provider_id: Some("provider-claude".to_string()),
                limit: 20,
            })
            .await
            .expect("search should succeed");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].provider_name, "claude");
        assert_eq!(items[0].display_name, "Claude 3.7 Sonnet");
    }
}
