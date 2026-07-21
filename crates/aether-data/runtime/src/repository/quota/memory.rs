use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;

use super::{
    ProviderQuotaReadRepository, ProviderQuotaWriteRepository, StoredProviderQuotaSnapshot,
};
use crate::DataLayerError;
use aether_wallet::{ProviderBillingType, ProviderQuotaSnapshot};

#[derive(Debug, Default)]
pub struct InMemoryProviderQuotaRepository {
    by_provider_id: RwLock<BTreeMap<String, StoredProviderQuotaSnapshot>>,
}

impl InMemoryProviderQuotaRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredProviderQuotaSnapshot>,
    {
        let mut by_provider_id = BTreeMap::new();
        for item in items {
            by_provider_id.insert(item.provider_id.clone(), item);
        }
        Self {
            by_provider_id: RwLock::new(by_provider_id),
        }
    }
}

#[async_trait]
impl ProviderQuotaReadRepository for InMemoryProviderQuotaRepository {
    async fn find_by_provider_id(
        &self,
        provider_id: &str,
    ) -> Result<Option<StoredProviderQuotaSnapshot>, DataLayerError> {
        Ok(self
            .by_provider_id
            .read()
            .expect("quota repository lock")
            .get(provider_id)
            .cloned())
    }

    async fn find_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderQuotaSnapshot>, DataLayerError> {
        let quotas = self.by_provider_id.read().expect("quota repository lock");
        Ok(provider_ids
            .iter()
            .filter_map(|provider_id| quotas.get(provider_id).cloned())
            .collect())
    }
}

#[async_trait]
impl ProviderQuotaWriteRepository for InMemoryProviderQuotaRepository {
    async fn reset_due(&self, now_unix_secs: u64) -> Result<usize, DataLayerError> {
        let mut count = 0usize;
        let mut quotas = self.by_provider_id.write().expect("quota repository lock");
        for quota in quotas.values_mut() {
            let snapshot = ProviderQuotaSnapshot {
                provider_id: quota.provider_id.clone(),
                billing_type: ProviderBillingType::parse(&quota.billing_type),
                monthly_quota_usd: quota.monthly_quota_usd,
                monthly_used_usd: quota.monthly_used_usd,
                quota_reset_day: quota.quota_reset_day,
                quota_last_reset_at_unix_secs: quota.quota_last_reset_at_unix_secs,
                quota_expires_at_unix_secs: quota.quota_expires_at_unix_secs,
                is_active: quota.is_active,
            };
            if snapshot.should_reset(now_unix_secs) {
                quota.monthly_used_usd = 0.0;
                quota.quota_last_reset_at_unix_secs = Some(now_unix_secs);
                count += 1;
            }
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryProviderQuotaRepository;
    use crate::repository::quota::{
        ProviderQuotaReadRepository, ProviderQuotaWriteRepository, StoredProviderQuotaSnapshot,
    };

    fn sample_quota() -> StoredProviderQuotaSnapshot {
        StoredProviderQuotaSnapshot::new(
            "provider-1".to_string(),
            "monthly_quota".to_string(),
            Some(20.0),
            5.0,
            Some(7),
            Some(1_000),
            None,
            true,
        )
        .expect("quota should build")
    }

    #[tokio::test]
    async fn resets_due_monthly_quota() {
        let repository = InMemoryProviderQuotaRepository::seed(vec![sample_quota()]);
        let reset = repository
            .reset_due(1_000 + 7 * 24 * 60 * 60)
            .await
            .expect("reset should succeed");
        assert_eq!(reset, 1);
        let stored = repository
            .find_by_provider_id("provider-1")
            .await
            .expect("lookup should succeed")
            .expect("quota should exist");
        assert_eq!(stored.monthly_used_usd, 0.0);
    }

    #[tokio::test]
    async fn finds_quotas_by_provider_ids() {
        let repository = InMemoryProviderQuotaRepository::seed(vec![
            sample_quota(),
            StoredProviderQuotaSnapshot::new(
                "provider-2".to_string(),
                "payg".to_string(),
                None,
                1.5,
                None,
                None,
                None,
                true,
            )
            .expect("quota should build"),
        ]);

        let stored = repository
            .find_by_provider_ids(&[
                "provider-2".to_string(),
                "missing".to_string(),
                "provider-1".to_string(),
            ])
            .await
            .expect("lookup should succeed");

        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].provider_id, "provider-2");
        assert_eq!(stored[1].provider_id, "provider-1");
    }
}
