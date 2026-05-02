use std::collections::BTreeMap;

use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
use aether_data_contracts::repository::quota::StoredProviderQuotaSnapshot;
use aether_wallet::{ProviderBillingType, ProviderQuotaSnapshot};

pub fn should_skip_provider_quota(
    quota: &StoredProviderQuotaSnapshot,
    _now_unix_secs: u64,
) -> bool {
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

    match snapshot.billing_type {
        ProviderBillingType::MonthlyQuota | ProviderBillingType::FreeTier => snapshot
            .remaining_quota_usd()
            .is_some_and(|remaining| remaining <= 0.0),
        ProviderBillingType::PayAsYouGo | ProviderBillingType::Unknown => false,
    }
}

pub fn build_provider_concurrent_limit_map(
    providers: Vec<StoredProviderCatalogProvider>,
) -> BTreeMap<String, usize> {
    providers
        .into_iter()
        .filter_map(|provider| {
            provider
                .concurrent_limit
                .and_then(|limit| usize::try_from(limit).ok())
                .filter(|limit| *limit > 0)
                .map(|limit| (provider.id, limit))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{build_provider_concurrent_limit_map, should_skip_provider_quota};
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
    use aether_data_contracts::repository::quota::StoredProviderQuotaSnapshot;

    fn sample_provider(id: &str, concurrent_limit: Option<i32>) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            id.to_string(),
            format!("provider-{id}"),
            Some("https://example.com".to_string()),
            "custom".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            false,
            concurrent_limit,
            None,
            None,
            None,
            None,
            None,
        )
    }

    #[test]
    fn skips_only_exhausted_monthly_quota_provider() {
        let inactive = StoredProviderQuotaSnapshot::new(
            "provider-1".to_string(),
            "monthly_quota".to_string(),
            Some(10.0),
            1.0,
            Some(30),
            Some(1_000),
            None,
            false,
        )
        .expect("quota should build");
        assert!(!should_skip_provider_quota(&inactive, 2_000));

        let expired = StoredProviderQuotaSnapshot::new(
            "provider-1".to_string(),
            "monthly_quota".to_string(),
            Some(10.0),
            1.0,
            Some(30),
            Some(1_000),
            Some(1_500),
            true,
        )
        .expect("quota should build");
        assert!(!should_skip_provider_quota(&expired, 2_000));

        let exhausted = StoredProviderQuotaSnapshot::new(
            "provider-1".to_string(),
            "monthly_quota".to_string(),
            Some(10.0),
            10.0,
            Some(30),
            Some(1_000),
            None,
            true,
        )
        .expect("quota should build");
        assert!(should_skip_provider_quota(&exhausted, 2_000));

        let payg = StoredProviderQuotaSnapshot::new(
            "provider-1".to_string(),
            "pay_as_you_go".to_string(),
            None,
            10.0,
            None,
            None,
            None,
            true,
        )
        .expect("quota should build");
        assert!(!should_skip_provider_quota(&payg, 2_000));
    }

    #[test]
    fn builds_provider_concurrent_limit_map_for_positive_limits_only() {
        let limits = build_provider_concurrent_limit_map(vec![
            sample_provider("provider-a", Some(10)),
            sample_provider("provider-b", Some(0)),
            sample_provider("provider-c", None),
        ]);

        assert_eq!(limits.get("provider-a"), Some(&10));
        assert!(!limits.contains_key("provider-b"));
        assert!(!limits.contains_key("provider-c"));
    }
}
