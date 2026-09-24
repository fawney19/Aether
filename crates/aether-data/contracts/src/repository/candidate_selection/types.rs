use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderModelMapping {
    pub name: String,
    pub priority: i32,
    pub api_formats: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_ids: Option<Vec<String>>,
    /// Optional request-operation scope. An omitted scope applies to every
    /// operation supported by the selected API format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operations: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredMinimalCandidateSelectionRow {
    pub provider_id: String,
    pub provider_name: String,
    pub provider_type: String,
    pub provider_priority: i32,
    pub provider_is_active: bool,
    #[serde(default)]
    pub provider_pool_enabled: bool,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StoredPoolKeyCandidateOrder {
    #[default]
    InternalPriority,
    Lru,
    CacheAffinity,
    SingleAccount,
    LoadBalance {
        seed: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredPoolKeyCandidateRowsQuery {
    pub api_format: String,
    pub provider_id: String,
    pub endpoint_id: String,
    pub model_id: String,
    pub selected_provider_model_name: String,
    #[serde(default)]
    pub order: StoredPoolKeyCandidateOrder,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredPoolKeyCandidateRowsByKeyIdsQuery {
    pub api_format: String,
    pub provider_id: String,
    pub endpoint_id: String,
    pub model_id: String,
    pub selected_provider_model_name: String,
    pub key_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredRequestedModelCandidateRowsQuery {
    pub api_format: String,
    pub requested_model_name: String,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredApiFormatCandidateRowsQuery {
    pub api_format: String,
    pub offset: u32,
    pub limit: u32,
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
                .any(|value| api_format_permission_covers(value, api_format)),
        }
    }
}

/// Evaluates the API-format scope on a provider-model mapping.
///
/// Codex Live was introduced after existing Codex model associations had
/// already stored their source-model scope as `openai:responses`. Preserve
/// those associations for the same Codex provider without treating the two
/// formats as globally interchangeable. Endpoint and key permissions remain
/// independently scoped to `codex:live`.
pub fn provider_model_mapping_api_format_covers(
    provider_type: &str,
    mapping_api_format: &str,
    requested_api_format: &str,
) -> bool {
    if aether_ai_formats::api_format_permission_covers(mapping_api_format, requested_api_format) {
        return true;
    }

    provider_type.trim().eq_ignore_ascii_case("codex")
        && aether_ai_formats::normalize_api_format_alias(requested_api_format) == "codex:live"
        && aether_ai_formats::normalize_api_format_alias(mapping_api_format) == "openai:responses"
}

fn api_format_permission_covers(allowed: &str, requested: &str) -> bool {
    aether_ai_formats::api_format_permission_covers(allowed, requested)
}

#[async_trait]
pub trait MinimalCandidateSelectionReadRepository: Send + Sync {
    fn clear_local_cache(&self) {}

    async fn list_for_exact_api_format(
        &self,
        api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, crate::DataLayerError>;

    async fn list_for_exact_api_format_page(
        &self,
        query: &StoredApiFormatCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, crate::DataLayerError> {
        Ok(self
            .list_for_exact_api_format(&query.api_format)
            .await?
            .into_iter()
            .skip(query.offset as usize)
            .take(query.limit as usize)
            .collect())
    }

    async fn list_for_exact_api_format_and_global_model(
        &self,
        api_format: &str,
        global_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, crate::DataLayerError>;

    async fn list_for_exact_api_format_and_requested_model(
        &self,
        api_format: &str,
        requested_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, crate::DataLayerError>;

    async fn list_for_exact_api_format_and_requested_model_page(
        &self,
        query: &StoredRequestedModelCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, crate::DataLayerError>;

    async fn list_pool_key_rows_for_group(
        &self,
        query: &StoredPoolKeyCandidateRowsQuery,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, crate::DataLayerError>;

    async fn list_pool_key_rows_for_group_key_ids(
        &self,
        query: &StoredPoolKeyCandidateRowsByKeyIdsQuery,
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

#[cfg(test)]
mod tests {
    use super::provider_model_mapping_api_format_covers;

    #[test]
    fn legacy_responses_mapping_is_only_compatible_with_codex_live() {
        assert!(provider_model_mapping_api_format_covers(
            "codex",
            "openai:responses",
            "codex:live"
        ));
        assert!(provider_model_mapping_api_format_covers(
            " CoDeX ",
            "/v1/responses",
            "codex:live"
        ));

        for provider_type in ["openai", "custom", "chatgpt_web"] {
            assert!(!provider_model_mapping_api_format_covers(
                provider_type,
                "openai:responses",
                "codex:live"
            ));
        }
        for requested_api_format in ["openai:chat", "claude:messages", "openai:image"] {
            assert!(!provider_model_mapping_api_format_covers(
                "codex",
                "openai:responses",
                requested_api_format
            ));
        }
    }
}
