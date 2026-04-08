use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredPublicGlobalModel {
    pub id: String,
    pub name: String,
    pub display_name: Option<String>,
    pub is_active: bool,
    pub default_price_per_request: Option<f64>,
    pub default_tiered_pricing: Option<Value>,
    pub supported_capabilities: Option<Value>,
    pub config: Option<Value>,
    pub usage_count: u64,
}

impl StoredPublicGlobalModel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        name: String,
        display_name: Option<String>,
        is_active: bool,
        default_price_per_request: Option<f64>,
        default_tiered_pricing: Option<Value>,
        supported_capabilities: Option<Value>,
        config: Option<Value>,
        usage_count: u64,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.id is empty".to_string(),
            ));
        }
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.name is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            name,
            display_name,
            is_active,
            default_price_per_request,
            default_tiered_pricing,
            supported_capabilities,
            config,
            usage_count,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PublicGlobalModelQuery {
    pub offset: usize,
    pub limit: usize,
    pub is_active: Option<bool>,
    pub search: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredPublicCatalogModel {
    pub id: String,
    pub provider_id: String,
    pub provider_name: String,
    pub provider_model_name: String,
    pub name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub icon_url: Option<String>,
    pub input_price_per_1m: Option<f64>,
    pub output_price_per_1m: Option<f64>,
    pub cache_creation_price_per_1m: Option<f64>,
    pub cache_read_price_per_1m: Option<f64>,
    pub supports_vision: Option<bool>,
    pub supports_function_calling: Option<bool>,
    pub supports_streaming: Option<bool>,
    pub is_active: bool,
}

impl StoredPublicCatalogModel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        provider_id: String,
        provider_name: String,
        provider_model_name: String,
        name: String,
        display_name: String,
        description: Option<String>,
        icon_url: Option<String>,
        input_price_per_1m: Option<f64>,
        output_price_per_1m: Option<f64>,
        cache_creation_price_per_1m: Option<f64>,
        cache_read_price_per_1m: Option<f64>,
        supports_vision: Option<bool>,
        supports_function_calling: Option<bool>,
        supports_streaming: Option<bool>,
        is_active: bool,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.id is empty".to_string(),
            ));
        }
        if provider_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.provider_id is empty".to_string(),
            ));
        }
        if provider_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "providers.name is empty".to_string(),
            ));
        }
        if provider_model_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.provider_model_name is empty".to_string(),
            ));
        }
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "public model name is empty".to_string(),
            ));
        }
        if display_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "public model display_name is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            provider_id,
            provider_name,
            provider_model_name,
            name,
            display_name,
            description,
            icon_url,
            input_price_per_1m,
            output_price_per_1m,
            cache_creation_price_per_1m,
            cache_read_price_per_1m,
            supports_vision,
            supports_function_calling,
            supports_streaming,
            is_active,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PublicCatalogModelListQuery {
    pub provider_id: Option<String>,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicCatalogModelSearchQuery {
    pub search: String,
    pub provider_id: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminProviderModelListQuery {
    pub provider_id: String,
    pub is_active: Option<bool>,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminGlobalModel {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub is_active: bool,
    pub default_price_per_request: Option<f64>,
    pub default_tiered_pricing: Option<Value>,
    pub supported_capabilities: Option<Value>,
    pub config: Option<Value>,
    pub provider_count: u64,
    pub active_provider_count: u64,
    pub usage_count: u64,
    pub created_at_unix_secs: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
}

impl StoredAdminGlobalModel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        name: String,
        display_name: String,
        is_active: bool,
        default_price_per_request: Option<f64>,
        default_tiered_pricing: Option<Value>,
        supported_capabilities: Option<Value>,
        config: Option<Value>,
        provider_count: u64,
        active_provider_count: u64,
        usage_count: u64,
        created_at_unix_secs: Option<u64>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.id is empty".to_string(),
            ));
        }
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.name is empty".to_string(),
            ));
        }
        if display_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.display_name is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            name,
            display_name,
            is_active,
            default_price_per_request,
            default_tiered_pricing,
            supported_capabilities,
            config,
            provider_count,
            active_provider_count,
            usage_count,
            created_at_unix_secs,
            updated_at_unix_secs,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminGlobalModelListQuery {
    pub offset: usize,
    pub limit: usize,
    pub is_active: Option<bool>,
    pub search: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminProviderModel {
    pub id: String,
    pub provider_id: String,
    pub global_model_id: String,
    pub provider_model_name: String,
    pub provider_model_mappings: Option<Value>,
    pub price_per_request: Option<f64>,
    pub tiered_pricing: Option<Value>,
    pub supports_vision: Option<bool>,
    pub supports_function_calling: Option<bool>,
    pub supports_streaming: Option<bool>,
    pub supports_extended_thinking: Option<bool>,
    pub supports_image_generation: Option<bool>,
    pub is_active: bool,
    pub is_available: bool,
    pub config: Option<Value>,
    pub created_at_unix_secs: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
    pub global_model_name: Option<String>,
    pub global_model_display_name: Option<String>,
    pub global_model_default_price_per_request: Option<f64>,
    pub global_model_default_tiered_pricing: Option<Value>,
    pub global_model_config: Option<Value>,
}

impl StoredAdminProviderModel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        provider_id: String,
        global_model_id: String,
        provider_model_name: String,
        provider_model_mappings: Option<Value>,
        price_per_request: Option<f64>,
        tiered_pricing: Option<Value>,
        supports_vision: Option<bool>,
        supports_function_calling: Option<bool>,
        supports_streaming: Option<bool>,
        supports_extended_thinking: Option<bool>,
        supports_image_generation: Option<bool>,
        is_active: bool,
        is_available: bool,
        config: Option<Value>,
        created_at_unix_secs: Option<u64>,
        updated_at_unix_secs: Option<u64>,
        global_model_name: Option<String>,
        global_model_display_name: Option<String>,
        global_model_default_price_per_request: Option<f64>,
        global_model_default_tiered_pricing: Option<Value>,
        global_model_config: Option<Value>,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.id is empty".to_string(),
            ));
        }
        if provider_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.provider_id is empty".to_string(),
            ));
        }
        if global_model_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.global_model_id is empty".to_string(),
            ));
        }
        if provider_model_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.provider_model_name is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            provider_id,
            global_model_id,
            provider_model_name,
            provider_model_mappings,
            price_per_request,
            tiered_pricing,
            supports_vision,
            supports_function_calling,
            supports_streaming,
            supports_extended_thinking,
            supports_image_generation,
            is_active,
            is_available,
            config,
            created_at_unix_secs,
            updated_at_unix_secs,
            global_model_name,
            global_model_display_name,
            global_model_default_price_per_request,
            global_model_default_tiered_pricing,
            global_model_config,
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpsertAdminProviderModelRecord {
    pub id: String,
    pub provider_id: String,
    pub global_model_id: String,
    pub provider_model_name: String,
    pub provider_model_mappings: Option<Value>,
    pub price_per_request: Option<f64>,
    pub tiered_pricing: Option<Value>,
    pub supports_vision: Option<bool>,
    pub supports_function_calling: Option<bool>,
    pub supports_streaming: Option<bool>,
    pub supports_extended_thinking: Option<bool>,
    pub supports_image_generation: Option<bool>,
    pub is_active: bool,
    pub is_available: bool,
    pub config: Option<Value>,
}

impl UpsertAdminProviderModelRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        provider_id: String,
        global_model_id: String,
        provider_model_name: String,
        provider_model_mappings: Option<Value>,
        price_per_request: Option<f64>,
        tiered_pricing: Option<Value>,
        supports_vision: Option<bool>,
        supports_function_calling: Option<bool>,
        supports_streaming: Option<bool>,
        supports_extended_thinking: Option<bool>,
        supports_image_generation: Option<bool>,
        is_active: bool,
        is_available: bool,
        config: Option<Value>,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.id is empty".to_string(),
            ));
        }
        if provider_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.provider_id is empty".to_string(),
            ));
        }
        if global_model_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.global_model_id is empty".to_string(),
            ));
        }
        if provider_model_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "models.provider_model_name is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            provider_id,
            global_model_id,
            provider_model_name,
            provider_model_mappings,
            price_per_request,
            tiered_pricing,
            supports_vision,
            supports_function_calling,
            supports_streaming,
            supports_extended_thinking,
            supports_image_generation,
            is_active,
            is_available,
            config,
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateAdminGlobalModelRecord {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub is_active: bool,
    pub default_price_per_request: Option<f64>,
    pub default_tiered_pricing: Option<Value>,
    pub supported_capabilities: Option<Value>,
    pub config: Option<Value>,
}

impl CreateAdminGlobalModelRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        name: String,
        display_name: String,
        is_active: bool,
        default_price_per_request: Option<f64>,
        default_tiered_pricing: Option<Value>,
        supported_capabilities: Option<Value>,
        config: Option<Value>,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.id is empty".to_string(),
            ));
        }
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.name is empty".to_string(),
            ));
        }
        if display_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.display_name is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            name,
            display_name,
            is_active,
            default_price_per_request,
            default_tiered_pricing,
            supported_capabilities,
            config,
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpdateAdminGlobalModelRecord {
    pub id: String,
    pub display_name: String,
    pub is_active: bool,
    pub default_price_per_request: Option<f64>,
    pub default_tiered_pricing: Option<Value>,
    pub supported_capabilities: Option<Value>,
    pub config: Option<Value>,
}

impl UpdateAdminGlobalModelRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        display_name: String,
        is_active: bool,
        default_price_per_request: Option<f64>,
        default_tiered_pricing: Option<Value>,
        supported_capabilities: Option<Value>,
        config: Option<Value>,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.id is empty".to_string(),
            ));
        }
        if display_name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "global_models.display_name is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            display_name,
            is_active,
            default_price_per_request,
            default_tiered_pricing,
            supported_capabilities,
            config,
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredPublicGlobalModelPage {
    pub items: Vec<StoredPublicGlobalModel>,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminGlobalModelPage {
    pub items: Vec<StoredAdminGlobalModel>,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderModelStats {
    pub provider_id: String,
    pub total_models: u64,
    pub active_models: u64,
}

impl StoredProviderModelStats {
    pub fn new(
        provider_id: String,
        total_models: i64,
        active_models: i64,
    ) -> Result<Self, crate::DataLayerError> {
        if provider_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider model stats provider_id is empty".to_string(),
            ));
        }
        if total_models < 0 || active_models < 0 {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider model stats count is negative".to_string(),
            ));
        }
        Ok(Self {
            provider_id,
            total_models: total_models as u64,
            active_models: active_models as u64,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderActiveGlobalModel {
    pub provider_id: String,
    pub global_model_id: String,
}

impl StoredProviderActiveGlobalModel {
    pub fn new(
        provider_id: String,
        global_model_id: String,
    ) -> Result<Self, crate::DataLayerError> {
        if provider_id.trim().is_empty() || global_model_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider active global model identity is empty".to_string(),
            ));
        }
        Ok(Self {
            provider_id,
            global_model_id,
        })
    }
}

#[async_trait]
pub trait GlobalModelReadRepository: Send + Sync {
    async fn list_public_models(
        &self,
        query: &PublicGlobalModelQuery,
    ) -> Result<StoredPublicGlobalModelPage, crate::DataLayerError>;

    async fn get_public_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<StoredPublicGlobalModel>, crate::DataLayerError>;

    async fn list_public_catalog_models(
        &self,
        query: &PublicCatalogModelListQuery,
    ) -> Result<Vec<StoredPublicCatalogModel>, crate::DataLayerError>;

    async fn search_public_catalog_models(
        &self,
        query: &PublicCatalogModelSearchQuery,
    ) -> Result<Vec<StoredPublicCatalogModel>, crate::DataLayerError>;

    async fn list_admin_global_models(
        &self,
        query: &AdminGlobalModelListQuery,
    ) -> Result<StoredAdminGlobalModelPage, crate::DataLayerError>;

    async fn list_admin_provider_models(
        &self,
        query: &AdminProviderModelListQuery,
    ) -> Result<Vec<StoredAdminProviderModel>, crate::DataLayerError>;

    async fn list_admin_provider_available_source_models(
        &self,
        provider_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, crate::DataLayerError>;

    async fn get_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Option<StoredAdminProviderModel>, crate::DataLayerError>;

    async fn get_admin_global_model_by_id(
        &self,
        global_model_id: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, crate::DataLayerError>;

    async fn get_admin_global_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, crate::DataLayerError>;

    async fn list_admin_provider_models_by_global_model_id(
        &self,
        global_model_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, crate::DataLayerError>;

    async fn list_provider_model_stats(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderModelStats>, crate::DataLayerError>;

    async fn list_active_global_model_ids_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderActiveGlobalModel>, crate::DataLayerError>;
}

#[async_trait]
pub trait GlobalModelWriteRepository: Send + Sync {
    async fn create_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, crate::DataLayerError>;

    async fn update_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, crate::DataLayerError>;

    async fn delete_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<bool, crate::DataLayerError>;

    async fn create_admin_global_model(
        &self,
        record: &CreateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, crate::DataLayerError>;

    async fn update_admin_global_model(
        &self,
        record: &UpdateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, crate::DataLayerError>;

    async fn delete_admin_global_model(
        &self,
        global_model_id: &str,
    ) -> Result<bool, crate::DataLayerError>;
}
