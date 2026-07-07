use aether_data::repository::wallet::StoredWalletSnapshot;
use aether_wallet::{
    WalletAccessDecision, WalletAccessFailure, WalletLimitMode, WalletSnapshot, WalletStatus,
};

use crate::control::GatewayLocalAuthRejection;
use crate::data::auth::GatewayAuthApiKeySnapshot;
use crate::{AppState, GatewayError};

const DAILY_QUOTA_EPSILON_USD: f64 = 0.000_000_01;

pub(crate) async fn resolve_wallet_auth_gate(
    state: &AppState,
    auth_snapshot: &GatewayAuthApiKeySnapshot,
) -> Result<Option<WalletAccessDecision>, GatewayError> {
    resolve_wallet_auth_gate_with_cache(state, auth_snapshot, true).await
}

pub(crate) async fn resolve_wallet_auth_gate_uncached(
    state: &AppState,
    auth_snapshot: &GatewayAuthApiKeySnapshot,
) -> Result<Option<WalletAccessDecision>, GatewayError> {
    resolve_wallet_auth_gate_with_cache(state, auth_snapshot, false).await
}

async fn resolve_wallet_auth_gate_with_cache(
    state: &AppState,
    auth_snapshot: &GatewayAuthApiKeySnapshot,
    use_cache: bool,
) -> Result<Option<WalletAccessDecision>, GatewayError> {
    if !state.has_wallet_data_reader() {
        return Ok(None);
    }

    let wallet = if use_cache {
        state
            .read_wallet_snapshot_for_auth(
                &auth_snapshot.user_id,
                &auth_snapshot.api_key_id,
                auth_snapshot.api_key_is_standalone,
            )
            .await?
    } else {
        state
            .read_wallet_snapshot_for_auth_uncached(
                &auth_snapshot.user_id,
                &auth_snapshot.api_key_id,
                auth_snapshot.api_key_is_standalone,
            )
            .await?
    };

    let decision = match wallet.as_ref() {
        Some(wallet) => map_wallet_snapshot(wallet).access_decision(false),
        None => WalletAccessDecision::wallet_unavailable(None),
    };
    if !auth_snapshot.api_key_is_standalone {
        if let Some(quota) = state
            .find_user_daily_quota_availability(&auth_snapshot.user_id)
            .await?
            .filter(|quota| quota.has_active_daily_quota)
        {
            let has_remaining_quota = quota.remaining_usd > DAILY_QUOTA_EPSILON_USD;
            if decision.failure == Some(WalletAccessFailure::BalanceDenied) && has_remaining_quota {
                return Ok(Some(WalletAccessDecision::allowed(Some(
                    quota.remaining_usd,
                ))));
            }
            if decision.failure.is_none() && !quota.allow_wallet_overage && !has_remaining_quota {
                return Ok(Some(WalletAccessDecision::balance_denied(Some(0.0))));
            }
        }
    }
    Ok(Some(decision))
}

pub(crate) fn local_rejection_from_wallet_access(
    decision: &WalletAccessDecision,
) -> Option<GatewayLocalAuthRejection> {
    match decision.failure.as_ref() {
        Some(WalletAccessFailure::WalletUnavailable) => {
            Some(GatewayLocalAuthRejection::WalletUnavailable)
        }
        Some(WalletAccessFailure::BalanceDenied) => {
            Some(GatewayLocalAuthRejection::BalanceDenied {
                remaining: decision.remaining,
            })
        }
        None => None,
    }
}

fn map_wallet_snapshot(snapshot: &StoredWalletSnapshot) -> WalletSnapshot {
    WalletSnapshot {
        wallet_id: snapshot.id.clone(),
        user_id: snapshot.user_id.clone(),
        api_key_id: snapshot.api_key_id.clone(),
        recharge_balance: snapshot.balance,
        gift_balance: snapshot.gift_balance,
        limit_mode: WalletLimitMode::parse(&snapshot.limit_mode),
        currency: snapshot.currency.clone(),
        status: WalletStatus::parse(&snapshot.status),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_data::repository::usage::InMemoryUsageReadRepository;
    use aether_data::repository::wallet::{InMemoryWalletRepository, StoredWalletSnapshot};
    use aether_data_contracts::repository::billing::{
        BillingReadRepository, StoredBillingModelContext, UserDailyQuotaAvailabilityRecord,
    };
    use aether_data_contracts::DataLayerError;
    use aether_wallet::{WalletAccessFailure, WalletLimitMode, WalletSnapshot, WalletStatus};
    use async_trait::async_trait;

    use super::{
        local_rejection_from_wallet_access, map_wallet_snapshot, resolve_wallet_auth_gate,
    };
    use crate::control::GatewayLocalAuthRejection;
    use crate::data::auth::GatewayAuthApiKeySnapshot;
    use crate::data::GatewayDataState;
    use crate::AppState;

    #[derive(Debug)]
    struct FixedQuotaBillingReadRepository {
        quota: Option<UserDailyQuotaAvailabilityRecord>,
    }

    #[async_trait]
    impl BillingReadRepository for FixedQuotaBillingReadRepository {
        async fn find_model_context(
            &self,
            _provider_id: &str,
            _provider_api_key_id: Option<&str>,
            _global_model_name: &str,
        ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
            Ok(None)
        }

        async fn find_user_daily_quota_availability(
            &self,
            _user_id: &str,
        ) -> Result<Option<UserDailyQuotaAvailabilityRecord>, DataLayerError> {
            Ok(self.quota.clone())
        }
    }

    #[test]
    fn maps_wallet_snapshot_and_derives_balance_denied() {
        let stored = StoredWalletSnapshot::new(
            "wallet-1".to_string(),
            Some("user-1".to_string()),
            None,
            0.0,
            0.0,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            0.0,
            0.0,
            0.0,
            0.0,
            100,
        )
        .expect("wallet should build");

        let decision = map_wallet_snapshot(&stored).access_decision(false);
        assert_eq!(decision.failure, Some(WalletAccessFailure::BalanceDenied));
        assert_eq!(
            local_rejection_from_wallet_access(&decision),
            Some(GatewayLocalAuthRejection::BalanceDenied {
                remaining: Some(0.0),
            })
        );
    }

    #[test]
    fn unlimited_admin_wallet_gate_allows_without_remaining() {
        let decision = WalletSnapshot {
            wallet_id: "wallet-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: None,
            recharge_balance: 0.0,
            gift_balance: 0.0,
            limit_mode: WalletLimitMode::Unlimited,
            currency: "USD".to_string(),
            status: WalletStatus::Active,
        }
        .access_decision(true);

        assert!(decision.allowed);
        assert_eq!(decision.remaining, None);
    }

    #[test]
    fn admin_user_with_empty_finite_wallet_is_balance_denied() {
        let stored = StoredWalletSnapshot::new(
            "wallet-1".to_string(),
            Some("admin-1".to_string()),
            None,
            0.0,
            0.0,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            0.0,
            0.0,
            0.0,
            0.0,
            100,
        )
        .expect("wallet should build");

        let decision = map_wallet_snapshot(&stored).access_decision(true);

        assert_eq!(decision.failure, Some(WalletAccessFailure::BalanceDenied));
    }

    #[tokio::test]
    async fn ordinary_user_key_without_quota_denies_empty_wallet() {
        let state = state_with_wallet_and_quota(empty_user_wallet(), None);
        let auth_snapshot = ordinary_user_api_key_snapshot();

        let decision = resolve_wallet_auth_gate(&state, &auth_snapshot)
            .await
            .expect("wallet gate should resolve")
            .expect("wallet gate should return a decision");

        assert!(!decision.allowed);
        assert_eq!(decision.failure, Some(WalletAccessFailure::BalanceDenied));
        assert_eq!(
            local_rejection_from_wallet_access(&decision),
            Some(GatewayLocalAuthRejection::BalanceDenied {
                remaining: Some(0.0),
            })
        );
    }

    #[tokio::test]
    async fn ordinary_user_key_with_remaining_quota_allows_empty_wallet() {
        let state = state_with_wallet_and_quota(
            empty_user_wallet(),
            Some(quota_availability(10.0, 4.0, false)),
        );
        let auth_snapshot = ordinary_user_api_key_snapshot();

        let decision = resolve_wallet_auth_gate(&state, &auth_snapshot)
            .await
            .expect("wallet gate should resolve")
            .expect("wallet gate should return a decision");

        assert!(decision.allowed);
        assert_eq!(decision.failure, None);
        assert_eq!(decision.remaining, Some(4.0));
    }

    #[tokio::test]
    async fn admin_wallet_recharge_invalidates_cached_auth_capacity_state() {
        let wallet = empty_user_wallet();
        let state =
            state_with_wallet_and_quota(wallet.clone(), None).with_auth_wallets_for_tests([wallet]);
        let auth_snapshot = ordinary_user_api_key_snapshot();

        let denied = resolve_wallet_auth_gate(&state, &auth_snapshot)
            .await
            .expect("wallet gate should resolve")
            .expect("wallet gate should return a decision");

        assert!(!denied.allowed);
        assert_eq!(denied.failure, Some(WalletAccessFailure::BalanceDenied));

        let recharge = state
            .admin_create_manual_wallet_recharge(
                "wallet-user-1",
                10.0,
                "admin_manual",
                Some("admin-1"),
                Some("manual recharge"),
            )
            .await
            .expect("wallet recharge should complete");

        assert!(recharge.is_some());

        let refreshed = resolve_wallet_auth_gate(&state, &auth_snapshot)
            .await
            .expect("wallet gate should resolve after recharge")
            .expect("wallet gate should return a decision after recharge");

        assert!(refreshed.allowed);
        assert_eq!(refreshed.failure, None);
        assert_eq!(refreshed.remaining, Some(10.0));
    }

    fn state_with_wallet_and_quota(
        wallet: StoredWalletSnapshot,
        quota: Option<UserDailyQuotaAvailabilityRecord>,
    ) -> AppState {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let billing_repository: Arc<dyn BillingReadRepository> =
            Arc::new(FixedQuotaBillingReadRepository { quota });
        let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![wallet]));
        let data = GatewayDataState::with_usage_billing_and_wallet_for_tests(
            usage_repository,
            billing_repository,
            wallet_repository,
        );
        AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data)
    }

    fn empty_user_wallet() -> StoredWalletSnapshot {
        StoredWalletSnapshot::new(
            "wallet-user-1".to_string(),
            Some("user-1".to_string()),
            None,
            0.0,
            0.0,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            0.0,
            0.0,
            0.0,
            0.0,
            100,
        )
        .expect("wallet should build")
    }

    fn quota_availability(
        total_quota_usd: f64,
        remaining_usd: f64,
        allow_wallet_overage: bool,
    ) -> UserDailyQuotaAvailabilityRecord {
        UserDailyQuotaAvailabilityRecord {
            has_active_daily_quota: true,
            total_quota_usd,
            used_usd: total_quota_usd - remaining_usd,
            remaining_usd,
            allow_wallet_overage,
        }
    }

    fn ordinary_user_api_key_snapshot() -> GatewayAuthApiKeySnapshot {
        GatewayAuthApiKeySnapshot {
            user_id: "user-1".to_string(),
            username: "ordinary-user".to_string(),
            email: Some("ordinary@example.com".to_string()),
            user_role: "user".to_string(),
            user_auth_source: "local".to_string(),
            user_is_active: true,
            user_is_deleted: false,
            user_rate_limit: None,
            user_allowed_providers: None,
            user_allowed_api_formats: None,
            user_allowed_models: None,
            api_key_id: "api-key-1".to_string(),
            api_key_name: Some("admin-created-key".to_string()),
            api_key_is_active: true,
            api_key_is_locked: false,
            api_key_is_standalone: false,
            api_key_rate_limit: None,
            api_key_concurrent_limit: None,
            api_key_expires_at_unix_secs: None,
            api_key_allowed_providers: None,
            api_key_allowed_api_formats: None,
            api_key_allowed_models: None,
            api_key_ip_rules: None,
            currently_usable: true,
        }
    }
}
