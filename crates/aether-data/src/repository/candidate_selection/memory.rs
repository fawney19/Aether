use std::sync::RwLock;

use async_trait::async_trait;

use super::{MinimalCandidateSelectionReadRepository, StoredMinimalCandidateSelectionRow};
use crate::DataLayerError;

#[derive(Debug, Default)]
pub struct InMemoryMinimalCandidateSelectionReadRepository {
    rows: RwLock<Vec<StoredMinimalCandidateSelectionRow>>,
}

impl InMemoryMinimalCandidateSelectionReadRepository {
    pub fn seed<I>(rows: I) -> Self
    where
        I: IntoIterator<Item = StoredMinimalCandidateSelectionRow>,
    {
        Self {
            rows: RwLock::new(rows.into_iter().collect()),
        }
    }
}

#[async_trait]
impl MinimalCandidateSelectionReadRepository for InMemoryMinimalCandidateSelectionReadRepository {
    async fn list_for_exact_api_format(
        &self,
        api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let api_format = api_format.trim();
        let mut rows = self
            .rows
            .read()
            .expect("candidate selection repository lock")
            .iter()
            .filter(|row| {
                row.provider_is_active
                    && row.endpoint_is_active
                    && row.key_is_active
                    && row.model_is_active
                    && row.model_is_available
                    && api_format_matches(&row.endpoint_api_format, api_format)
                    && row.key_supports_api_format(api_format)
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            left.provider_priority
                .cmp(&right.provider_priority)
                .then(left.key_internal_priority.cmp(&right.key_internal_priority))
                .then(left.provider_id.cmp(&right.provider_id))
                .then(left.endpoint_id.cmp(&right.endpoint_id))
                .then(left.key_id.cmp(&right.key_id))
                .then(left.model_id.cmp(&right.model_id))
        });
        Ok(rows)
    }

    async fn list_for_exact_api_format_and_global_model(
        &self,
        api_format: &str,
        global_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        let rows = self.list_for_exact_api_format(api_format).await?;
        Ok(rows
            .into_iter()
            .filter(|row| row.global_model_name == global_model_name)
            .collect())
    }
}

fn normalize_api_format(value: &str) -> String {
    aether_ai_formats::normalize_legacy_openai_format_alias(value)
}

fn api_format_matches(left: &str, right: &str) -> bool {
    normalize_api_format(left) == normalize_api_format(right)
}

#[cfg(test)]
mod tests {
    use super::InMemoryMinimalCandidateSelectionReadRepository;
    use crate::repository::candidate_selection::{
        MinimalCandidateSelectionReadRepository, StoredMinimalCandidateSelectionRow,
    };

    fn sample_row(
        provider_id: &str,
        api_format: &str,
        global_model_name: &str,
        provider_priority: i32,
    ) -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: provider_id.to_string(),
            provider_name: provider_id.to_string(),
            provider_type: "custom".to_string(),
            provider_priority,
            provider_is_active: true,
            endpoint_id: format!("endpoint-{provider_id}"),
            endpoint_api_format: api_format.to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: format!("key-{provider_id}"),
            key_name: "prod".to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec![api_format.to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 50,
            key_global_priority_by_format: None,
            model_id: format!("model-{provider_id}"),
            global_model_id: "global-model-1".to_string(),
            global_model_name: global_model_name.to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: global_model_name.to_string(),
            model_provider_model_mappings: None,
            model_supports_streaming: None,
            model_is_active: true,
            model_is_available: true,
        }
    }

    #[tokio::test]
    async fn filters_by_exact_api_format_and_global_model() {
        let repository = InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_row("provider-2", "openai:chat", "gpt-4.1", 20),
            sample_row("provider-1", "openai:chat", "gpt-4.1", 10),
            sample_row("provider-3", "openai:responses", "gpt-4.1", 5),
            sample_row("provider-4", "openai:chat", "gpt-4.1-mini", 1),
        ]);

        let rows = repository
            .list_for_exact_api_format_and_global_model("openai:chat", "gpt-4.1")
            .await
            .expect("list should succeed");

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].provider_id, "provider-1");
        assert_eq!(rows[1].provider_id, "provider-2");
    }

    #[tokio::test]
    async fn filters_by_exact_api_format_only() {
        let repository = InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_row("provider-2", "openai:chat", "gpt-4.1", 20),
            sample_row("provider-1", "openai:chat", "gpt-4.1-mini", 10),
            sample_row("provider-3", "openai:responses", "gpt-4.1", 5),
        ]);

        let rows = repository
            .list_for_exact_api_format("openai:chat")
            .await
            .expect("list should succeed");

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].provider_id, "provider-1");
        assert_eq!(rows[1].provider_id, "provider-2");
    }
}
