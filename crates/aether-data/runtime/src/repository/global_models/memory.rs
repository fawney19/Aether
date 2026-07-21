use std::sync::RwLock;

use async_trait::async_trait;

use super::{
    AdminGlobalModelListQuery, AdminProviderModelListQuery, CreateAdminGlobalModelRecord,
    GlobalModelReadRepository, GlobalModelSnapshot, GlobalModelWriteRepository,
    PublicCatalogModelListQuery, PublicCatalogModelSearchQuery, PublicGlobalModelQuery,
    StoredAdminGlobalModel, StoredAdminGlobalModelPage, StoredAdminProviderModel,
    StoredProviderActiveGlobalModel, StoredProviderModelStats, StoredPublicCatalogModel,
    StoredPublicGlobalModel, StoredPublicGlobalModelPage, UpdateAdminGlobalModelRecord,
    UpsertAdminProviderModelRecord,
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

    fn snapshot(&self) -> GlobalModelSnapshot {
        GlobalModelSnapshot::seed(
            self.items
                .read()
                .expect("global model repository lock")
                .clone(),
        )
        .with_admin_global_models(
            self.admin_global_model_items
                .read()
                .expect("admin global model repository lock")
                .clone(),
        )
        .with_public_catalog_models(
            self.public_catalog_items
                .read()
                .expect("public catalog model repository lock")
                .clone(),
        )
        .with_admin_provider_models(
            self.admin_provider_model_items
                .read()
                .expect("admin provider model repository lock")
                .clone(),
        )
        .with_provider_model_stats(
            self.provider_model_stats
                .read()
                .expect("provider model stats repository lock")
                .clone(),
        )
        .with_active_global_model_refs(
            self.active_global_model_refs
                .read()
                .expect("active global model repository lock")
                .clone(),
        )
    }
}

#[async_trait]
impl GlobalModelReadRepository for InMemoryGlobalModelReadRepository {
    async fn list_public_models(
        &self,
        query: &PublicGlobalModelQuery,
    ) -> Result<StoredPublicGlobalModelPage, DataLayerError> {
        Ok(self.snapshot().list_public_models(query))
    }

    async fn get_public_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<StoredPublicGlobalModel>, DataLayerError> {
        Ok(self.snapshot().get_public_model_by_name(model_name))
    }

    async fn list_public_catalog_models(
        &self,
        query: &PublicCatalogModelListQuery,
    ) -> Result<Vec<StoredPublicCatalogModel>, DataLayerError> {
        Ok(self.snapshot().list_public_catalog_models(query))
    }

    async fn search_public_catalog_models(
        &self,
        query: &PublicCatalogModelSearchQuery,
    ) -> Result<Vec<StoredPublicCatalogModel>, DataLayerError> {
        Ok(self.snapshot().search_public_catalog_models(query))
    }

    async fn list_admin_global_models(
        &self,
        query: &AdminGlobalModelListQuery,
    ) -> Result<StoredAdminGlobalModelPage, DataLayerError> {
        Ok(self.snapshot().list_admin_global_models(query))
    }

    async fn list_admin_provider_models(
        &self,
        query: &AdminProviderModelListQuery,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        Ok(self.snapshot().list_admin_provider_models(query))
    }

    async fn get_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        Ok(self
            .snapshot()
            .get_admin_provider_model(provider_id, model_id))
    }

    async fn list_admin_provider_available_source_models(
        &self,
        provider_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        Ok(self
            .snapshot()
            .list_admin_provider_available_source_models(provider_id))
    }

    async fn get_admin_global_model_by_id(
        &self,
        global_model_id: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        Ok(self
            .snapshot()
            .get_admin_global_model_by_id(global_model_id))
    }

    async fn get_admin_global_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        Ok(self.snapshot().get_admin_global_model_by_name(model_name))
    }

    async fn list_admin_provider_models_by_global_model_id(
        &self,
        global_model_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        Ok(self
            .snapshot()
            .list_admin_provider_models_by_global_model_id(global_model_id))
    }

    async fn list_provider_model_stats(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderModelStats>, DataLayerError> {
        Ok(self.snapshot().list_provider_model_stats(provider_ids))
    }

    async fn list_active_global_model_ids_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderActiveGlobalModel>, DataLayerError> {
        Ok(self
            .snapshot()
            .list_active_global_model_ids_by_provider_ids(provider_ids))
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
            global_model.supported_capabilities.clone(),
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
        existing.global_model_supported_capabilities = global_model.supported_capabilities.clone();
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
            record.usage_count.unwrap_or(0),
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
            if let Some(usage_count) = record.usage_count {
                existing.usage_count = usage_count;
            }
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
        CreateAdminGlobalModelRecord, GlobalModelReadRepository, GlobalModelWriteRepository,
        PublicCatalogModelListQuery, PublicCatalogModelSearchQuery, PublicGlobalModelQuery,
        StoredPublicCatalogModel, StoredPublicGlobalModel,
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
            Some(false),
            true,
        )
        .expect("public catalog model should build")
    }

    #[tokio::test]
    async fn embedding_model_metadata_roundtrip() {
        let repository =
            InMemoryGlobalModelReadRepository::seed(Vec::<StoredPublicGlobalModel>::new());
        let record = CreateAdminGlobalModelRecord::new(
            "gm-embedding".to_string(),
            "text-embedding-3-small".to_string(),
            "Text Embedding 3 Small".to_string(),
            true,
            None,
            Some(json!({"tiers":[{"up_to":null,"input_price_per_1m":0.02}]})),
            Some(json!(["embedding"])),
            Some(json!({
                "api_formats": ["openai:embedding"],
                "dimensions": 1536
            })),
        )
        .expect("embedding global model should validate");

        repository
            .create_admin_global_model(&record)
            .await
            .expect("embedding global model should persist")
            .expect("embedding global model should be returned");

        let stored = repository
            .get_admin_global_model_by_name("text-embedding-3-small")
            .await
            .expect("embedding global model should read")
            .expect("embedding global model should exist");

        assert_eq!(stored.supported_capabilities, Some(json!(["embedding"])));
        assert_eq!(
            stored
                .config
                .as_ref()
                .and_then(|value| value.get("dimensions")),
            Some(&json!(1536))
        );
        assert_eq!(
            stored
                .default_tiered_pricing
                .as_ref()
                .and_then(|value| value.get("tiers"))
                .and_then(serde_json::Value::as_array)
                .and_then(|tiers| tiers.first())
                .and_then(|tier| tier.get("input_price_per_1m"))
                .and_then(serde_json::Value::as_f64),
            Some(0.02)
        );
    }

    #[tokio::test]
    async fn embedding_missing_billing_config_rejected() {
        let error = CreateAdminGlobalModelRecord::new(
            "gm-embedding".to_string(),
            "text-embedding-3-small".to_string(),
            "Text Embedding 3 Small".to_string(),
            true,
            None,
            None,
            Some(json!(["embedding"])),
            None,
        )
        .expect_err("embedding metadata without billing should fail closed");

        assert!(error
            .to_string()
            .contains("embedding global model requires"));
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
    async fn public_catalog_preserves_embedding_capability_without_contaminating_chat_models() {
        let mut embedding_model = sample_public_catalog_model(
            "model-embedding",
            "provider-openai",
            "openai",
            "text-embedding-3-small",
            "text-embedding-3-small",
            "Text Embedding 3 Small",
        );
        embedding_model.supports_embedding = Some(true);
        embedding_model.supports_streaming = Some(false);
        let chat_model = sample_public_catalog_model(
            "model-chat",
            "provider-openai",
            "openai",
            "gpt-5-upstream",
            "gpt-5",
            "GPT 5",
        );
        let repository =
            InMemoryGlobalModelReadRepository::seed(Vec::<StoredPublicGlobalModel>::new())
                .with_public_catalog_models(vec![embedding_model, chat_model]);

        let items = repository
            .list_public_catalog_models(&PublicCatalogModelListQuery {
                provider_id: Some("provider-openai".to_string()),
                offset: 0,
                limit: 50,
            })
            .await
            .expect("catalog should list");

        let embedding = items
            .iter()
            .find(|item| item.name == "text-embedding-3-small")
            .expect("embedding model should be listed");
        let chat = items
            .iter()
            .find(|item| item.name == "gpt-5")
            .expect("chat model should be listed");
        assert_eq!(embedding.supports_embedding, Some(true));
        assert_eq!(embedding.supports_streaming, Some(false));
        assert_eq!(chat.supports_embedding, Some(false));
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
