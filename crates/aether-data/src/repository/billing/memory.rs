use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;

use super::{BillingReadRepository, StoredBillingModelContext};
use crate::DataLayerError;

type BillingContextKey = (String, String, Option<String>);
type BillingContextMap = BTreeMap<BillingContextKey, StoredBillingModelContext>;

#[derive(Debug, Default)]
pub struct InMemoryBillingReadRepository {
    by_key: RwLock<BillingContextMap>,
}

impl InMemoryBillingReadRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredBillingModelContext>,
    {
        let mut by_key = BTreeMap::new();
        for item in items {
            by_key.insert(
                (
                    item.provider_id.clone(),
                    item.global_model_name.clone(),
                    item.provider_api_key_id.clone(),
                ),
                item,
            );
        }
        Self {
            by_key: RwLock::new(by_key),
        }
    }
}

#[async_trait]
impl BillingReadRepository for InMemoryBillingReadRepository {
    async fn find_model_context(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        global_model_name: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        let by_key = self.by_key.read().expect("billing repository lock");
        if let Some(value) = find_context_by_provider_model_name(
            &by_key,
            provider_id,
            provider_api_key_id,
            global_model_name,
        ) {
            return Ok(Some(value));
        }

        let key = (
            provider_id.to_string(),
            global_model_name.to_string(),
            provider_api_key_id.map(ToOwned::to_owned),
        );
        if let Some(value) = by_key.get(&key) {
            return Ok(Some(value.clone()));
        }

        if let Some(value) = by_key
            .get(&(provider_id.to_string(), global_model_name.to_string(), None))
            .cloned()
        {
            return Ok(Some(value));
        }

        Ok(by_key
            .iter()
            .find(|((stored_provider_id, stored_model_name, _), _)| {
                stored_provider_id == provider_id && stored_model_name == global_model_name
            })
            .map(|(_, value)| value.clone()))
    }

    async fn find_model_context_by_model_id(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        model_id: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        let by_key = self.by_key.read().expect("billing repository lock");
        if let Some(value) =
            find_context_by_model_id_and_key(&by_key, provider_id, provider_api_key_id, model_id)
        {
            return Ok(Some(value));
        }
        if let Some(value) = find_context_by_model_id_and_key(&by_key, provider_id, None, model_id)
        {
            return Ok(Some(value));
        }
        Ok(by_key
            .iter()
            .find(|((stored_provider_id, _, _), value)| {
                stored_provider_id == provider_id && value.model_id.as_deref() == Some(model_id)
            })
            .map(|(_, value)| value.clone()))
    }
}

fn find_context_by_provider_model_name(
    by_key: &BillingContextMap,
    provider_id: &str,
    provider_api_key_id: Option<&str>,
    provider_model_name: &str,
) -> Option<StoredBillingModelContext> {
    find_context_by_provider_model_name_and_key(
        by_key,
        provider_id,
        provider_api_key_id,
        provider_model_name,
    )
    .or_else(|| {
        find_context_by_provider_model_name_and_key(by_key, provider_id, None, provider_model_name)
    })
    .or_else(|| {
        by_key
            .iter()
            .find(|((stored_provider_id, _, _), value)| {
                stored_provider_id == provider_id
                    && value.model_provider_model_name.as_deref() == Some(provider_model_name)
            })
            .map(|(_, value)| value.clone())
    })
}

fn find_context_by_provider_model_name_and_key(
    by_key: &BillingContextMap,
    provider_id: &str,
    provider_api_key_id: Option<&str>,
    provider_model_name: &str,
) -> Option<StoredBillingModelContext> {
    by_key
        .iter()
        .find(|((stored_provider_id, _, stored_key_id), value)| {
            stored_provider_id == provider_id
                && stored_key_id.as_deref() == provider_api_key_id
                && value.model_provider_model_name.as_deref() == Some(provider_model_name)
        })
        .map(|(_, value)| value.clone())
}

fn find_context_by_model_id_and_key(
    by_key: &BillingContextMap,
    provider_id: &str,
    provider_api_key_id: Option<&str>,
    model_id: &str,
) -> Option<StoredBillingModelContext> {
    by_key
        .iter()
        .find(|((stored_provider_id, _, stored_key_id), value)| {
            stored_provider_id == provider_id
                && stored_key_id.as_deref() == provider_api_key_id
                && value.model_id.as_deref() == Some(model_id)
        })
        .map(|(_, value)| value.clone())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::InMemoryBillingReadRepository;
    use crate::repository::billing::{BillingReadRepository, StoredBillingModelContext};

    fn sample_context() -> StoredBillingModelContext {
        StoredBillingModelContext::new(
            "provider-1".to_string(),
            Some("pay_as_you_go".to_string()),
            Some("key-1".to_string()),
            Some(json!({"openai:chat": 0.8})),
            Some(60),
            "global-model-1".to_string(),
            "gpt-5".to_string(),
            Some(json!({"streaming": true})),
            Some(0.02),
            Some(json!({"tiers":[{"up_to":null,"input_price_per_1m":3.0,"output_price_per_1m":15.0}]})),
            Some("model-1".to_string()),
            Some("gpt-5-upstream".to_string()),
            None,
            Some(0.01),
            None,
        )
        .expect("billing context should build")
    }

    #[tokio::test]
    async fn falls_back_to_provider_without_key_scope() {
        let repository = InMemoryBillingReadRepository::seed(vec![sample_context()]);
        let stored = repository
            .find_model_context("provider-1", Some("key-2"), "gpt-5")
            .await
            .expect("lookup should succeed")
            .expect("context should exist");

        assert_eq!(stored.provider_id, "provider-1");
        assert_eq!(stored.global_model_name, "gpt-5");
    }

    #[tokio::test]
    async fn resolves_by_provider_model_name_before_global_name_collision() {
        let global_named_context = StoredBillingModelContext::new(
            "provider-1".to_string(),
            Some("pay_as_you_go".to_string()),
            Some("key-1".to_string()),
            None,
            Some(60),
            "global-model-blank".to_string(),
            "claude-sonnet-4-6".to_string(),
            None,
            None,
            None,
            Some("model-blank".to_string()),
            Some("blank-upstream".to_string()),
            None,
            None,
            None,
        )
        .expect("blank billing context should build");
        let provider_priced_context = StoredBillingModelContext::new(
            "provider-1".to_string(),
            Some("pay_as_you_go".to_string()),
            Some("key-1".to_string()),
            None,
            Some(60),
            "global-model-priced".to_string(),
            "claude-opus-4-6".to_string(),
            None,
            None,
            Some(json!({"tiers":[{"up_to":null,"input_price_per_1m":3.0,"output_price_per_1m":15.0}]})),
            Some("model-priced".to_string()),
            Some("claude-sonnet-4-6".to_string()),
            None,
            None,
            None,
        )
        .expect("priced billing context should build");
        let repository = InMemoryBillingReadRepository::seed(vec![
            global_named_context,
            provider_priced_context,
        ]);

        let stored = repository
            .find_model_context("provider-1", Some("key-1"), "claude-sonnet-4-6")
            .await
            .expect("lookup should succeed")
            .expect("context should exist");

        assert_eq!(stored.global_model_name, "claude-opus-4-6");
        assert_eq!(
            stored.model_provider_model_name.as_deref(),
            Some("claude-sonnet-4-6")
        );
        assert!(stored.default_tiered_pricing.is_some());
    }

    #[tokio::test]
    async fn resolves_by_model_id() {
        let repository = InMemoryBillingReadRepository::seed(vec![sample_context()]);
        let stored = repository
            .find_model_context_by_model_id("provider-1", Some("key-1"), "model-1")
            .await
            .expect("lookup should succeed")
            .expect("context should exist");

        assert_eq!(stored.global_model_name, "gpt-5");
        assert_eq!(
            stored.model_provider_model_name.as_deref(),
            Some("gpt-5-upstream")
        );
    }
}
