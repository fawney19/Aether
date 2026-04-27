use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderModelMapping {
    pub name: String,
    pub priority: i32,
    pub api_formats: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredMinimalCandidateSelectionRow {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_type: String,
    pub provider_priority: i32,
    pub provider_is_active: bool,
    pub endpoint_id: String,
    pub endpoint_api_format: String,
    pub endpoint_api_family: Option<String>,
    pub endpoint_kind: Option<String>,
    pub endpoint_is_active: bool,
    pub key_id: String,
    pub key_name: String,
    pub key_auth_type: String,
    pub key_is_active: bool,
    pub key_api_formats: Option<Vec<String>>,
    pub key_allowed_models: Option<Vec<String>>,
    pub key_capabilities: Option<serde_json::Value>,
    pub key_internal_priority: i32,
    pub key_global_priority_by_format: Option<serde_json::Value>,
    pub model_id: String,
    pub global_model_id: String,
    pub global_model_name: String,
    pub global_model_mappings: Option<Vec<String>>,
    pub global_model_supports_streaming: Option<bool>,
    pub model_provider_model_name: String,
    pub model_provider_model_mappings: Option<Vec<StoredProviderModelMapping>>,
    pub model_supports_streaming: Option<bool>,
    pub model_is_active: bool,
    pub model_is_available: bool,
}

impl StoredMinimalCandidateSelectionRow {
    pub fn supports_streaming(&self) -> bool {
        self.model_supports_streaming
            .or(self.global_model_supports_streaming)
            .unwrap_or(true)
    }

    pub fn key_supports_api_format(&self, api_format: &str) -> bool {
        match self.key_api_formats.as_deref() {
            None => true,
            Some(formats) => formats
                .iter()
                .any(|value| api_format_matches(value, api_format)),
        }
    }
}

fn normalize_api_format(value: &str) -> String {
    aether_ai_formats::normalize_legacy_openai_format_alias(value)
}

fn api_format_matches(left: &str, right: &str) -> bool {
    normalize_api_format(left) == normalize_api_format(right)
}

#[async_trait]
pub trait MinimalCandidateSelectionReadRepository: Send + Sync {
    async fn list_for_exact_api_format(
        &self,
        api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, crate::DataLayerError>;

    async fn list_for_exact_api_format_and_global_model(
        &self,
        api_format: &str,
        global_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, crate::DataLayerError>;
}

pub trait MinimalCandidateSelectionRepository:
    MinimalCandidateSelectionReadRepository + Send + Sync
{
}

impl<T> MinimalCandidateSelectionRepository for T where
    T: MinimalCandidateSelectionReadRepository + Send + Sync
{
}
