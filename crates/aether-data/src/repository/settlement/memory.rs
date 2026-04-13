use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use super::{SettlementWriteRepository, StoredUsageSettlement, UsageSettlementInput};
use crate::repository::wallet::{InMemoryWalletRepository, StoredWalletSnapshot};
use crate::DataLayerError;

#[derive(Debug)]
enum InMemorySettlementWalletStore {
    Owned(RwLock<BTreeMap<String, StoredWalletSnapshot>>),
    Shared(Arc<InMemoryWalletRepository>),
}

impl Default for InMemorySettlementWalletStore {
    fn default() -> Self {
        Self::Owned(RwLock::new(BTreeMap::new()))
    }
}

impl InMemorySettlementWalletStore {
    fn seeded<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredWalletSnapshot>,
    {
        let mut wallets_by_id = BTreeMap::new();
        for item in items {
            wallets_by_id.insert(item.id.clone(), item);
        }
        Self::Owned(RwLock::new(wallets_by_id))
    }

    fn with_mut<R>(&self, f: impl FnOnce(&mut BTreeMap<String, StoredWalletSnapshot>) -> R) -> R {
        match self {
            Self::Owned(wallets_by_id) => {
                let mut wallets = wallets_by_id.write().expect("settlement repo lock");
                f(&mut wallets)
            }
            Self::Shared(repository) => repository.with_wallets_mut(f),
        }
    }
}

#[derive(Debug, Default)]
pub struct InMemorySettlementRepository {
    wallets: InMemorySettlementWalletStore,
    provider_monthly_used: RwLock<BTreeMap<String, f64>>,
    settlements: RwLock<BTreeMap<String, StoredUsageSettlement>>,
}

impl InMemorySettlementRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredWalletSnapshot>,
    {
        Self {
            wallets: InMemorySettlementWalletStore::seeded(items),
            provider_monthly_used: RwLock::new(BTreeMap::new()),
            settlements: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn from_wallet_repository(wallet_repository: Arc<InMemoryWalletRepository>) -> Self {
        Self {
            wallets: InMemorySettlementWalletStore::Shared(wallet_repository),
            provider_monthly_used: RwLock::new(BTreeMap::new()),
            settlements: RwLock::new(BTreeMap::new()),
        }
    }
}

#[async_trait]
impl SettlementWriteRepository for InMemorySettlementRepository {
    async fn settle_usage(
        &self,
        input: UsageSettlementInput,
    ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
        input.validate()?;
        if input.billing_status != "pending" {
            let existing = self
                .settlements
                .read()
                .expect("settlement snapshot lock")
                .get(&input.request_id)
                .cloned();
            return Ok(Some(existing.unwrap_or(StoredUsageSettlement {
                request_id: input.request_id,
                wallet_id: None,
                billing_status: input.billing_status,
                wallet_balance_before: None,
                wallet_balance_after: None,
                wallet_recharge_balance_before: None,
                wallet_recharge_balance_after: None,
                wallet_gift_balance_before: None,
                wallet_gift_balance_after: None,
                provider_monthly_used_usd: None,
                finalized_at_unix_secs: input.finalized_at_unix_secs,
            })));
        }

        let final_billing_status = if input.status == "completed" {
            "settled"
        } else {
            "void"
        };
        let mut settlement = self.wallets.with_mut(|wallets| {
            let wallet_id = input
                .api_key_id
                .as_deref()
                .and_then(|api_key_id| {
                    wallets
                        .values()
                        .find(|wallet| wallet.api_key_id.as_deref() == Some(api_key_id))
                        .map(|wallet| wallet.id.clone())
                })
                .or_else(|| {
                    input.user_id.as_deref().and_then(|user_id| {
                        wallets
                            .values()
                            .find(|wallet| wallet.user_id.as_deref() == Some(user_id))
                            .map(|wallet| wallet.id.clone())
                    })
                });
            let wallet = wallet_id
                .as_deref()
                .and_then(|wallet_id| wallets.get_mut(wallet_id));

            let mut settlement = StoredUsageSettlement {
                request_id: input.request_id.clone(),
                wallet_id: None,
                billing_status: final_billing_status.to_string(),
                wallet_balance_before: None,
                wallet_balance_after: None,
                wallet_recharge_balance_before: None,
                wallet_recharge_balance_after: None,
                wallet_gift_balance_before: None,
                wallet_gift_balance_after: None,
                provider_monthly_used_usd: None,
                finalized_at_unix_secs: input.finalized_at_unix_secs,
            };

            if let Some(wallet) = wallet {
                let before_recharge = wallet.balance;
                let before_gift = wallet.gift_balance;
                let before_total = before_recharge + before_gift;
                settlement.wallet_id = Some(wallet.id.clone());
                settlement.wallet_balance_before = Some(before_total);
                settlement.wallet_recharge_balance_before = Some(before_recharge);
                settlement.wallet_gift_balance_before = Some(before_gift);

                if final_billing_status == "settled" {
                    if wallet.limit_mode.eq_ignore_ascii_case("unlimited") {
                        wallet.total_consumed += input.total_cost_usd;
                    } else {
                        let gift_deduction = before_gift.max(0.0).min(input.total_cost_usd);
                        let recharge_deduction = input.total_cost_usd - gift_deduction;
                        wallet.gift_balance = before_gift - gift_deduction;
                        wallet.balance = before_recharge - recharge_deduction;
                        wallet.total_consumed += input.total_cost_usd;
                    }
                }

                settlement.wallet_recharge_balance_after = Some(wallet.balance);
                settlement.wallet_gift_balance_after = Some(wallet.gift_balance);
                settlement.wallet_balance_after = Some(wallet.balance + wallet.gift_balance);
            }

            settlement
        });

        if final_billing_status == "settled" {
            if let Some(provider_id) = input.provider_id {
                let mut quotas = self
                    .provider_monthly_used
                    .write()
                    .expect("provider quota lock");
                let value = quotas.entry(provider_id).or_insert(0.0);
                *value += input.actual_total_cost_usd;
                settlement.provider_monthly_used_usd = Some(*value);
            }
        }

        self.settlements
            .write()
            .expect("settlement snapshot lock")
            .insert(settlement.request_id.clone(), settlement.clone());

        Ok(Some(settlement))
    }
}

#[cfg(test)]
mod tests {
    use super::InMemorySettlementRepository;
    use crate::repository::settlement::{SettlementWriteRepository, UsageSettlementInput};
    use crate::repository::wallet::StoredWalletSnapshot;

    fn sample_wallet() -> StoredWalletSnapshot {
        StoredWalletSnapshot::new(
            "wallet-1".to_string(),
            Some("user-1".to_string()),
            Some("key-1".to_string()),
            10.0,
            2.0,
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

    #[tokio::test]
    async fn settles_usage_against_wallet_and_provider_quota() {
        let repository = InMemorySettlementRepository::seed(vec![sample_wallet()]);
        let settlement = repository
            .settle_usage(UsageSettlementInput {
                request_id: "req-1".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("key-1".to_string()),
                provider_id: Some("provider-1".to_string()),
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                total_cost_usd: 3.0,
                actual_total_cost_usd: 1.5,
                finalized_at_unix_secs: Some(200),
            })
            .await
            .expect("settlement should succeed")
            .expect("settlement should exist");

        assert_eq!(settlement.billing_status, "settled");
        assert_eq!(settlement.wallet_balance_before, Some(12.0));
        assert_eq!(settlement.wallet_balance_after, Some(9.0));
        assert_eq!(settlement.provider_monthly_used_usd, Some(1.5));
    }

    #[tokio::test]
    async fn returns_stored_snapshot_when_usage_is_already_finalized() {
        let repository = InMemorySettlementRepository::seed(vec![sample_wallet()]);
        let settled = repository
            .settle_usage(UsageSettlementInput {
                request_id: "req-2".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("key-1".to_string()),
                provider_id: Some("provider-1".to_string()),
                status: "completed".to_string(),
                billing_status: "pending".to_string(),
                total_cost_usd: 2.0,
                actual_total_cost_usd: 1.0,
                finalized_at_unix_secs: Some(250),
            })
            .await
            .expect("settlement should succeed")
            .expect("settlement should exist");

        let replay = repository
            .settle_usage(UsageSettlementInput {
                request_id: "req-2".to_string(),
                user_id: Some("user-1".to_string()),
                api_key_id: Some("key-1".to_string()),
                provider_id: Some("provider-1".to_string()),
                status: "completed".to_string(),
                billing_status: "settled".to_string(),
                total_cost_usd: 2.0,
                actual_total_cost_usd: 1.0,
                finalized_at_unix_secs: Some(250),
            })
            .await
            .expect("replay should succeed")
            .expect("snapshot should exist");

        assert_eq!(replay, settled);
    }
}
