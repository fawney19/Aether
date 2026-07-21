use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;

#[cfg(test)]
use super::WalletReadSeed;
use super::{
    redeem_code_credits_recharge_balance, redeem_code_payment_method,
    redeem_code_refundable_amount, AdjustWalletBalanceInput, AdminPaymentOrderListQuery,
    AdminRedeemCodeBatchListQuery, AdminRedeemCodeListQuery, AdminWalletLedgerQuery,
    AdminWalletListQuery, AdminWalletRefundRequestListQuery, CompleteAdminWalletRefundInput,
    CreateAdminRedeemCodeBatchInput, CreateAdminRedeemCodeBatchResult,
    CreateManualWalletRechargeInput, CreatePlanPurchaseOrderInput, CreatePlanPurchaseOrderOutcome,
    CreateWalletRechargeOrderInput, CreateWalletRechargeOrderOutcome,
    CreateWalletRefundRequestInput, CreateWalletRefundRequestOutcome,
    CreatedAdminRedeemCodePlaintext, CreditAdminPaymentOrderInput, DeleteAdminRedeemCodeBatchInput,
    DisableAdminRedeemCodeBatchInput, DisableAdminRedeemCodeInput, FailAdminWalletRefundInput,
    ProcessAdminWalletRefundInput, ProcessPaymentCallbackInput, ProcessPaymentCallbackOutcome,
    RedeemWalletCodeInput, RedeemWalletCodeOutcome, StoredAdminPaymentCallback,
    StoredAdminPaymentCallbackPage, StoredAdminPaymentOrder, StoredAdminPaymentOrderPage,
    StoredAdminRedeemCode, StoredAdminRedeemCodeBatch, StoredAdminRedeemCodeBatchPage,
    StoredAdminRedeemCodePage, StoredAdminWalletLedgerPage, StoredAdminWalletListItem,
    StoredAdminWalletListPage, StoredAdminWalletRefund, StoredAdminWalletRefundPage,
    StoredAdminWalletRefundRequestPage, StoredAdminWalletTransaction,
    StoredAdminWalletTransactionPage, StoredWalletDailyUsageLedger,
    StoredWalletDailyUsageLedgerPage, StoredWalletSnapshot, WalletLookupKey, WalletMutationOutcome,
    WalletReadRepository, WalletWriteRepository,
};
use crate::DataLayerError;

#[derive(Debug, Default)]
pub struct InMemoryWalletRepository {
    wallets_by_id: RwLock<BTreeMap<String, StoredWalletSnapshot>>,
    payment_orders_by_id: RwLock<BTreeMap<String, StoredAdminPaymentOrder>>,
    payment_callbacks_by_id: RwLock<BTreeMap<String, StoredAdminPaymentCallback>>,
    wallet_transactions_by_id: RwLock<BTreeMap<String, StoredAdminWalletTransaction>>,
    refunds_by_id: RwLock<BTreeMap<String, StoredAdminWalletRefund>>,
    redeem_batches_by_id: RwLock<BTreeMap<String, StoredAdminRedeemCodeBatch>>,
    redeem_codes_by_id: RwLock<BTreeMap<String, StoredAdminRedeemCode>>,
    redeem_code_hash_to_id: RwLock<BTreeMap<String, String>>,
}

impl InMemoryWalletRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredWalletSnapshot>,
    {
        let mut wallets_by_id = BTreeMap::new();
        for item in items {
            wallets_by_id.insert(item.id.clone(), item);
        }
        Self {
            wallets_by_id: RwLock::new(wallets_by_id),
            payment_orders_by_id: RwLock::new(BTreeMap::new()),
            payment_callbacks_by_id: RwLock::new(BTreeMap::new()),
            wallet_transactions_by_id: RwLock::new(BTreeMap::new()),
            refunds_by_id: RwLock::new(BTreeMap::new()),
            redeem_batches_by_id: RwLock::new(BTreeMap::new()),
            redeem_codes_by_id: RwLock::new(BTreeMap::new()),
            redeem_code_hash_to_id: RwLock::new(BTreeMap::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn seed_read_model(seed: WalletReadSeed) -> Self {
        let mut wallets_by_id = BTreeMap::new();
        for item in seed.wallets {
            wallets_by_id.insert(item.id.clone(), item);
        }
        let mut payment_orders_by_id = BTreeMap::new();
        for item in seed.payment_orders {
            payment_orders_by_id.insert(item.id.clone(), item);
        }
        let mut payment_callbacks_by_id = BTreeMap::new();
        for item in seed.payment_callbacks {
            payment_callbacks_by_id.insert(item.id.clone(), item);
        }
        let mut wallet_transactions_by_id = BTreeMap::new();
        for item in seed.wallet_transactions {
            wallet_transactions_by_id.insert(item.id.clone(), item);
        }
        let mut refunds_by_id = BTreeMap::new();
        for item in seed.refunds {
            refunds_by_id.insert(item.id.clone(), item);
        }
        let mut redeem_batches_by_id = BTreeMap::new();
        for item in seed.redeem_batches {
            redeem_batches_by_id.insert(item.id.clone(), item);
        }
        let mut redeem_codes_by_id = BTreeMap::new();
        for item in seed.redeem_codes {
            redeem_codes_by_id.insert(item.id.clone(), item);
        }

        Self {
            wallets_by_id: RwLock::new(wallets_by_id),
            payment_orders_by_id: RwLock::new(payment_orders_by_id),
            payment_callbacks_by_id: RwLock::new(payment_callbacks_by_id),
            wallet_transactions_by_id: RwLock::new(wallet_transactions_by_id),
            refunds_by_id: RwLock::new(refunds_by_id),
            redeem_batches_by_id: RwLock::new(redeem_batches_by_id),
            redeem_codes_by_id: RwLock::new(redeem_codes_by_id),
            redeem_code_hash_to_id: RwLock::new(BTreeMap::new()),
        }
    }

    pub(crate) fn with_wallets_mut<R>(
        &self,
        f: impl FnOnce(&mut BTreeMap<String, StoredWalletSnapshot>) -> R,
    ) -> R {
        let mut wallets = self.wallets_by_id.write().expect("wallet repo lock");
        f(&mut wallets)
    }
}

fn current_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn current_unix_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

struct WalletSnapshotUpdate<'a> {
    balance: f64,
    gift_balance: f64,
    limit_mode: &'a str,
    currency: &'a str,
    status: &'a str,
    total_recharged: f64,
    total_consumed: f64,
    total_refunded: f64,
    total_adjusted: f64,
    updated_at_unix_secs: Option<u64>,
}

fn update_wallet_by_owner(
    wallets_by_id: &RwLock<BTreeMap<String, StoredWalletSnapshot>>,
    matches_owner: impl Fn(&StoredWalletSnapshot) -> bool,
    update: impl FnOnce(&mut StoredWalletSnapshot),
) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
    let mut wallets = wallets_by_id.write().expect("wallet repo lock");
    let Some(wallet) = wallets.values_mut().find(|wallet| matches_owner(wallet)) else {
        return Ok(None);
    };
    update(wallet);
    Ok(Some(wallet.clone()))
}

fn update_wallet_snapshot_by_owner(
    wallets_by_id: &RwLock<BTreeMap<String, StoredWalletSnapshot>>,
    matches_owner: impl Fn(&StoredWalletSnapshot) -> bool,
    update: WalletSnapshotUpdate<'_>,
) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
    update_wallet_by_owner(wallets_by_id, matches_owner, |wallet| {
        wallet.balance = update.balance;
        wallet.gift_balance = update.gift_balance;
        wallet.limit_mode = update.limit_mode.to_string();
        wallet.currency = update.currency.to_string();
        wallet.status = update.status.to_string();
        wallet.total_recharged = update.total_recharged;
        wallet.total_consumed = update.total_consumed;
        wallet.total_refunded = update.total_refunded;
        wallet.total_adjusted = update.total_adjusted;
        wallet.updated_at_unix_secs = update
            .updated_at_unix_secs
            .unwrap_or_else(current_unix_secs);
    })
}

fn initialize_auth_wallet_in_memory(
    wallets_by_id: &RwLock<BTreeMap<String, StoredWalletSnapshot>>,
    wallet_transactions_by_id: &RwLock<BTreeMap<String, StoredAdminWalletTransaction>>,
    user_id: Option<&str>,
    api_key_id: Option<&str>,
    initial_gift_usd: f64,
    unlimited: bool,
) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
    let gift_amount = if unlimited {
        0.0
    } else {
        initial_gift_usd.max(0.0)
    };
    let wallet = StoredWalletSnapshot::new(
        uuid::Uuid::new_v4().to_string(),
        user_id.map(str::to_string),
        api_key_id.map(str::to_string),
        0.0,
        gift_amount,
        if unlimited { "unlimited" } else { "finite" }.to_string(),
        "USD".to_string(),
        "active".to_string(),
        0.0,
        0.0,
        0.0,
        gift_amount,
        current_unix_secs() as i64,
    )?;
    wallets_by_id
        .write()
        .expect("wallet repo lock")
        .insert(wallet.id.clone(), wallet.clone());

    if gift_amount > 0.0 {
        let link_id = user_id.or(api_key_id).unwrap_or_default().to_string();
        let description = if api_key_id.is_some() {
            "独立余额 Key 初始赠款"
        } else {
            "用户初始赠款"
        };
        let transaction = StoredAdminWalletTransaction {
            id: uuid::Uuid::new_v4().to_string(),
            wallet_id: wallet.id.clone(),
            category: "gift".to_string(),
            reason_code: "gift_initial".to_string(),
            amount: gift_amount,
            balance_before: 0.0,
            balance_after: gift_amount,
            recharge_balance_before: 0.0,
            recharge_balance_after: 0.0,
            gift_balance_before: 0.0,
            gift_balance_after: gift_amount,
            link_type: Some("system_task".to_string()),
            link_id: Some(link_id),
            operator_id: None,
            operator_name: None,
            operator_email: None,
            description: Some(description.to_string()),
            created_at_unix_ms: Some(current_unix_ms()),
        };
        wallet_transactions_by_id
            .write()
            .expect("wallet repo lock")
            .insert(transaction.id.clone(), transaction);
    }

    Ok(Some(wallet))
}

fn normalize_redeem_code(value: &str) -> Option<String> {
    let normalized = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_uppercase())
        .collect::<String>();
    if normalized.len() < 16 {
        None
    } else {
        Some(normalized)
    }
}

fn format_redeem_code(normalized: &str) -> String {
    normalized
        .as_bytes()
        .chunks(8)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("-")
}

fn hash_redeem_code(normalized: &str) -> String {
    use sha2::Digest;

    format!("{:x}", sha2::Sha256::digest(normalized.as_bytes()))
}

fn mask_redeem_code(prefix: &str, suffix: &str) -> String {
    format!("{prefix}****{suffix}")
}

fn generate_redeem_code() -> String {
    format_redeem_code(
        &uuid::Uuid::new_v4()
            .simple()
            .to_string()
            .to_ascii_uppercase(),
    )
}

#[async_trait]
impl WalletReadRepository for InMemoryWalletRepository {
    async fn find(
        &self,
        key: WalletLookupKey<'_>,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let wallets = self.wallets_by_id.read().expect("wallet repo lock");
        Ok(match key {
            WalletLookupKey::WalletId(wallet_id) => wallets.get(wallet_id).cloned(),
            WalletLookupKey::UserId(user_id) => wallets
                .values()
                .find(|wallet| wallet.user_id.as_deref() == Some(user_id))
                .cloned(),
            WalletLookupKey::ApiKeyId(api_key_id) => wallets
                .values()
                .find(|wallet| wallet.api_key_id.as_deref() == Some(api_key_id))
                .cloned(),
        })
    }

    async fn update_auth_user_wallet_limit_mode(
        &self,
        user_id: &str,
        limit_mode: &str,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        update_wallet_by_owner(
            &self.wallets_by_id,
            |wallet| wallet.user_id.as_deref() == Some(user_id),
            |wallet| {
                wallet.limit_mode = limit_mode.to_string();
                wallet.updated_at_unix_secs = current_unix_secs();
            },
        )
    }

    async fn update_auth_api_key_wallet_limit_mode(
        &self,
        api_key_id: &str,
        limit_mode: &str,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        update_wallet_by_owner(
            &self.wallets_by_id,
            |wallet| wallet.api_key_id.as_deref() == Some(api_key_id),
            |wallet| {
                wallet.limit_mode = limit_mode.to_string();
                wallet.updated_at_unix_secs = current_unix_secs();
            },
        )
    }

    async fn initialize_auth_user_wallet(
        &self,
        user_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        initialize_auth_wallet_in_memory(
            &self.wallets_by_id,
            &self.wallet_transactions_by_id,
            Some(user_id),
            None,
            initial_gift_usd,
            unlimited,
        )
    }

    async fn initialize_auth_api_key_wallet(
        &self,
        api_key_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        initialize_auth_wallet_in_memory(
            &self.wallets_by_id,
            &self.wallet_transactions_by_id,
            None,
            Some(api_key_id),
            initial_gift_usd,
            unlimited,
        )
    }

    async fn update_auth_user_wallet_snapshot(
        &self,
        user_id: &str,
        balance: f64,
        gift_balance: f64,
        limit_mode: &str,
        currency: &str,
        status: &str,
        total_recharged: f64,
        total_consumed: f64,
        total_refunded: f64,
        total_adjusted: f64,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        update_wallet_snapshot_by_owner(
            &self.wallets_by_id,
            |wallet| wallet.user_id.as_deref() == Some(user_id),
            WalletSnapshotUpdate {
                balance,
                gift_balance,
                limit_mode,
                currency,
                status,
                total_recharged,
                total_consumed,
                total_refunded,
                total_adjusted,
                updated_at_unix_secs,
            },
        )
    }

    async fn update_auth_api_key_wallet_snapshot(
        &self,
        api_key_id: &str,
        balance: f64,
        gift_balance: f64,
        limit_mode: &str,
        currency: &str,
        status: &str,
        total_recharged: f64,
        total_consumed: f64,
        total_refunded: f64,
        total_adjusted: f64,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        update_wallet_snapshot_by_owner(
            &self.wallets_by_id,
            |wallet| wallet.api_key_id.as_deref() == Some(api_key_id),
            WalletSnapshotUpdate {
                balance,
                gift_balance,
                limit_mode,
                currency,
                status,
                total_recharged,
                total_consumed,
                total_refunded,
                total_adjusted,
                updated_at_unix_secs,
            },
        )
    }

    async fn list_wallets_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredWalletSnapshot>, DataLayerError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        let user_set: std::collections::BTreeSet<&str> =
            user_ids.iter().map(String::as_str).collect();
        let wallets = self.wallets_by_id.read().expect("wallet repo lock");
        Ok(wallets
            .values()
            .filter(|wallet| {
                wallet
                    .user_id
                    .as_deref()
                    .map(|user_id| user_set.contains(user_id))
                    .unwrap_or(false)
            })
            .cloned()
            .collect())
    }

    async fn list_wallets_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredWalletSnapshot>, DataLayerError> {
        if api_key_ids.is_empty() {
            return Ok(Vec::new());
        }
        let key_set: std::collections::BTreeSet<&str> =
            api_key_ids.iter().map(String::as_str).collect();
        let wallets = self.wallets_by_id.read().expect("wallet repo lock");
        Ok(wallets
            .values()
            .filter(|wallet| {
                wallet
                    .api_key_id
                    .as_deref()
                    .map(|api_key_id| key_set.contains(api_key_id))
                    .unwrap_or(false)
            })
            .cloned()
            .collect())
    }

    async fn list_admin_wallets(
        &self,
        query: &AdminWalletListQuery,
    ) -> Result<StoredAdminWalletListPage, DataLayerError> {
        let wallets = self.wallets_by_id.read().expect("wallet repo lock");
        let mut items = wallets
            .values()
            .filter(|wallet| {
                query
                    .status
                    .as_deref()
                    .is_none_or(|expected| wallet.status == expected)
            })
            .filter(|wallet| match query.owner_type.as_deref() {
                Some("user") => wallet.user_id.is_some(),
                Some("api_key") => wallet.api_key_id.is_some(),
                _ => true,
            })
            .map(|wallet| StoredAdminWalletListItem {
                id: wallet.id.clone(),
                user_id: wallet.user_id.clone(),
                api_key_id: wallet.api_key_id.clone(),
                balance: wallet.balance,
                gift_balance: wallet.gift_balance,
                limit_mode: wallet.limit_mode.clone(),
                currency: wallet.currency.clone(),
                status: wallet.status.clone(),
                total_recharged: wallet.total_recharged,
                total_consumed: wallet.total_consumed,
                total_refunded: wallet.total_refunded,
                total_adjusted: wallet.total_adjusted,
                user_name: None,
                api_key_name: None,
                created_at_unix_ms: None,
                updated_at_unix_secs: Some(wallet.updated_at_unix_secs),
            })
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .updated_at_unix_secs
                .cmp(&left.updated_at_unix_secs)
                .then_with(|| right.id.cmp(&left.id))
        });
        let total = items.len() as u64;
        let items = items
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect::<Vec<_>>();
        Ok(StoredAdminWalletListPage { items, total })
    }

    async fn list_admin_wallet_ledger(
        &self,
        _query: &AdminWalletLedgerQuery,
    ) -> Result<StoredAdminWalletLedgerPage, DataLayerError> {
        Ok(StoredAdminWalletLedgerPage::default())
    }

    async fn list_admin_wallet_refund_requests(
        &self,
        query: &AdminWalletRefundRequestListQuery,
    ) -> Result<StoredAdminWalletRefundRequestPage, DataLayerError> {
        let wallets = self.wallets_by_id.read().expect("wallet repo lock");
        let mut items = self
            .refunds_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .filter(|refund| {
                query
                    .status
                    .as_deref()
                    .is_none_or(|expected| refund.status == expected)
            })
            .filter_map(|refund| {
                let wallet = wallets.get(&refund.wallet_id)?;
                Some(super::StoredAdminWalletRefundRequestItem {
                    id: refund.id.clone(),
                    refund_no: refund.refund_no.clone(),
                    wallet_id: refund.wallet_id.clone(),
                    user_id: refund.user_id.clone(),
                    payment_order_id: refund.payment_order_id.clone(),
                    source_type: refund.source_type.clone(),
                    source_id: refund.source_id.clone(),
                    refund_mode: refund.refund_mode.clone(),
                    amount_usd: refund.amount_usd,
                    status: refund.status.clone(),
                    reason: refund.reason.clone(),
                    failure_reason: refund.failure_reason.clone(),
                    gateway_refund_id: refund.gateway_refund_id.clone(),
                    payout_method: refund.payout_method.clone(),
                    payout_reference: refund.payout_reference.clone(),
                    payout_proof: refund.payout_proof.clone(),
                    requested_by: refund.requested_by.clone(),
                    approved_by: refund.approved_by.clone(),
                    processed_by: refund.processed_by.clone(),
                    wallet_user_id: wallet.user_id.clone(),
                    wallet_user_name: None,
                    wallet_api_key_id: wallet.api_key_id.clone(),
                    api_key_name: None,
                    wallet_status: wallet.status.clone(),
                    created_at_unix_ms: Some(refund.created_at_unix_ms),
                    updated_at_unix_secs: Some(refund.updated_at_unix_secs),
                    processed_at_unix_secs: refund.processed_at_unix_secs,
                    completed_at_unix_secs: refund.completed_at_unix_secs,
                })
            })
            .collect::<Vec<_>>();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at_unix_ms));
        let total = items.len() as u64;
        let items = items
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect();
        Ok(StoredAdminWalletRefundRequestPage { items, total })
    }

    async fn list_admin_wallet_transactions(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminWalletTransactionPage, DataLayerError> {
        let mut items = self
            .wallet_transactions_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .filter(|tx| tx.wallet_id == wallet_id)
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at_unix_ms));
        let total = items.len() as u64;
        let items = items.into_iter().skip(offset).take(limit).collect();
        Ok(StoredAdminWalletTransactionPage { items, total })
    }

    async fn find_wallet_today_usage(
        &self,
        _wallet_id: &str,
        _billing_timezone: &str,
    ) -> Result<Option<StoredWalletDailyUsageLedger>, DataLayerError> {
        Ok(None)
    }

    async fn list_wallet_daily_usage_history(
        &self,
        _wallet_id: &str,
        _billing_timezone: &str,
        _limit: usize,
    ) -> Result<StoredWalletDailyUsageLedgerPage, DataLayerError> {
        Ok(StoredWalletDailyUsageLedgerPage::default())
    }

    async fn list_admin_wallet_refunds(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminWalletRefundPage, DataLayerError> {
        let mut items = self
            .refunds_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .filter(|refund| refund.wallet_id == wallet_id)
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at_unix_ms));
        let total = items.len() as u64;
        let items = items.into_iter().skip(offset).take(limit).collect();
        Ok(StoredAdminWalletRefundPage { items, total })
    }

    async fn list_admin_payment_orders(
        &self,
        query: &AdminPaymentOrderListQuery,
    ) -> Result<StoredAdminPaymentOrderPage, DataLayerError> {
        let now = current_unix_secs();
        let mut items = self
            .payment_orders_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .filter(|order| {
                query.status.as_deref().is_none_or(|expected| {
                    let effective = if order.status == "pending"
                        && order.expires_at_unix_secs.is_some_and(|value| value < now)
                    {
                        "expired"
                    } else {
                        order.status.as_str()
                    };
                    effective == expected
                }) && query
                    .payment_method
                    .as_deref()
                    .is_none_or(|expected| order.payment_method == expected)
            })
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at_unix_ms));
        let total = items.len() as u64;
        let items = items
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect();
        Ok(StoredAdminPaymentOrderPage { items, total })
    }

    async fn find_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        Ok(self
            .payment_orders_by_id
            .read()
            .expect("wallet repo lock")
            .get(order_id)
            .cloned())
    }

    async fn list_wallet_payment_orders_by_user_id(
        &self,
        user_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminPaymentOrderPage, DataLayerError> {
        let mut items = self
            .payment_orders_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .filter(|order| order.user_id.as_deref() == Some(user_id))
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at_unix_ms));
        let total = items.len() as u64;
        let items = items.into_iter().skip(offset).take(limit).collect();
        Ok(StoredAdminPaymentOrderPage { items, total })
    }

    async fn count_pending_refunds_by_user_id(&self, user_id: &str) -> Result<u64, DataLayerError> {
        const PENDING_REFUND_STATUSES: &[&str] = &["pending_approval", "approved", "processing"];
        Ok(self
            .refunds_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .filter(|refund| {
                refund.user_id.as_deref() == Some(user_id)
                    && PENDING_REFUND_STATUSES.contains(&refund.status.as_str())
            })
            .count() as u64)
    }

    async fn count_pending_payment_orders_by_user_id(
        &self,
        user_id: &str,
    ) -> Result<u64, DataLayerError> {
        const PENDING_PAYMENT_ORDER_STATUSES: &[&str] = &["pending", "paid"];
        Ok(self
            .payment_orders_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .filter(|order| {
                order.user_id.as_deref() == Some(user_id)
                    && PENDING_PAYMENT_ORDER_STATUSES.contains(&order.status.as_str())
            })
            .count() as u64)
    }

    async fn find_wallet_payment_order_by_user_id(
        &self,
        user_id: &str,
        order_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        Ok(self
            .payment_orders_by_id
            .read()
            .expect("wallet repo lock")
            .get(order_id)
            .filter(|order| order.user_id.as_deref() == Some(user_id))
            .cloned())
    }

    async fn find_pending_plan_purchase_order_by_user_id(
        &self,
        user_id: &str,
        product_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        let now = current_unix_secs();
        Ok(self
            .payment_orders_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .filter(|order| {
                order.user_id.as_deref() == Some(user_id)
                    && order.status == "pending"
                    && order
                        .expires_at_unix_secs
                        .is_some_and(|expires_at| expires_at > now)
                    && order
                        .gateway_response
                        .as_ref()
                        .is_some_and(|gateway_response| {
                            gateway_response
                                .get("order_kind")
                                .and_then(serde_json::Value::as_str)
                                == Some("plan_purchase")
                                && gateway_response
                                    .get("product_id")
                                    .and_then(serde_json::Value::as_str)
                                    == Some(product_id)
                        })
            })
            .max_by_key(|order| order.created_at_unix_ms)
            .cloned())
    }

    async fn find_wallet_refund(
        &self,
        wallet_id: &str,
        refund_id: &str,
    ) -> Result<Option<super::StoredAdminWalletRefund>, DataLayerError> {
        Ok(self
            .refunds_by_id
            .read()
            .expect("wallet repo lock")
            .get(refund_id)
            .filter(|refund| refund.wallet_id == wallet_id)
            .cloned())
    }

    async fn list_admin_payment_callbacks(
        &self,
        payment_method: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminPaymentCallbackPage, DataLayerError> {
        let mut items = self
            .payment_callbacks_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .filter(|callback| {
                payment_method.is_none_or(|expected| callback.payment_method == expected)
            })
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at_unix_ms));
        let total = items.len() as u64;
        let items = items.into_iter().skip(offset).take(limit).collect();
        Ok(StoredAdminPaymentCallbackPage { items, total })
    }

    async fn list_admin_redeem_code_batches(
        &self,
        query: &AdminRedeemCodeBatchListQuery,
    ) -> Result<StoredAdminRedeemCodeBatchPage, DataLayerError> {
        let mut items = self
            .redeem_batches_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .filter(|batch| {
                query
                    .status
                    .as_deref()
                    .is_none_or(|expected| batch.status == expected)
            })
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at_unix_ms));
        let total = items.len() as u64;
        let items = items
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect();
        Ok(StoredAdminRedeemCodeBatchPage { items, total })
    }

    async fn find_admin_redeem_code_batch(
        &self,
        batch_id: &str,
    ) -> Result<Option<StoredAdminRedeemCodeBatch>, DataLayerError> {
        Ok(self
            .redeem_batches_by_id
            .read()
            .expect("wallet repo lock")
            .get(batch_id)
            .cloned())
    }

    async fn list_admin_redeem_codes(
        &self,
        query: &AdminRedeemCodeListQuery,
    ) -> Result<StoredAdminRedeemCodePage, DataLayerError> {
        let mut items = self
            .redeem_codes_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .filter(|code| code.batch_id == query.batch_id)
            .filter(|code| {
                query
                    .status
                    .as_deref()
                    .is_none_or(|expected| code.status == expected)
            })
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at_unix_ms));
        let total = items.len() as u64;
        let items = items
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect();
        Ok(StoredAdminRedeemCodePage { items, total })
    }
}

#[async_trait]
impl WalletWriteRepository for InMemoryWalletRepository {
    async fn create_wallet_recharge_order(
        &self,
        input: CreateWalletRechargeOrderInput,
    ) -> Result<CreateWalletRechargeOrderOutcome, DataLayerError> {
        let now_secs = current_unix_secs();
        let wallet_id = {
            let mut wallets = self.wallets_by_id.write().expect("wallet repo lock");
            let wallet = wallets
                .values_mut()
                .find(|wallet| wallet.user_id.as_deref() == Some(input.user_id.as_str()));
            if wallet
                .as_ref()
                .is_some_and(|wallet| wallet.status != "active")
            {
                return Ok(CreateWalletRechargeOrderOutcome::WalletInactive);
            }

            match wallet {
                Some(wallet) => wallet.id.clone(),
                None => {
                    let wallet_id = input
                        .preferred_wallet_id
                        .clone()
                        .unwrap_or_else(|| format!("wallet-{}", uuid::Uuid::new_v4()));
                    let wallet = StoredWalletSnapshot::new(
                        wallet_id.clone(),
                        Some(input.user_id.clone()),
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
                        now_secs as i64,
                    )?;
                    wallets.insert(wallet_id.clone(), wallet);
                    wallet_id
                }
            }
        };

        let order = StoredAdminPaymentOrder {
            id: format!("payment-order-{}", uuid::Uuid::new_v4()),
            order_no: input.order_no,
            wallet_id,
            user_id: Some(input.user_id),
            amount_usd: input.amount_usd,
            pay_amount: input.pay_amount,
            pay_currency: input.pay_currency,
            exchange_rate: input.exchange_rate,
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            payment_method: input.payment_method,
            gateway_order_id: Some(input.gateway_order_id),
            gateway_response: Some(input.gateway_response),
            status: "pending".to_string(),
            created_at_unix_ms: current_unix_ms(),
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: Some(input.expires_at_unix_secs),
        };
        self.payment_orders_by_id
            .write()
            .expect("wallet repo lock")
            .insert(order.id.clone(), order.clone());
        Ok(CreateWalletRechargeOrderOutcome::Created(order))
    }

    async fn create_plan_purchase_order(
        &self,
        input: CreatePlanPurchaseOrderInput,
    ) -> Result<CreatePlanPurchaseOrderOutcome, DataLayerError> {
        let wallet_id = {
            let wallets = self.wallets_by_id.read().expect("wallet repo lock");
            let Some(wallet) = wallets
                .values()
                .find(|wallet| wallet.user_id.as_deref() == Some(input.user_id.as_str()))
            else {
                return Ok(CreatePlanPurchaseOrderOutcome::WalletInactive);
            };
            if wallet.status != "active" {
                return Ok(CreatePlanPurchaseOrderOutcome::WalletInactive);
            }
            wallet.id.clone()
        };
        let max_active_per_user = input
            .product_snapshot
            .get("max_active_per_user")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(1)
            .max(1);
        let purchase_limit_scope = input
            .product_snapshot
            .get("purchase_limit_scope")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("active_period");
        if purchase_limit_scope != "unlimited" {
            let now_secs = current_unix_secs();
            let existing_count = self
                .payment_orders_by_id
                .read()
                .expect("wallet repo lock")
                .values()
                .filter(|order| order.user_id.as_deref() == Some(input.user_id.as_str()))
                .filter(|order| {
                    let Some(gateway_response) = order.gateway_response.as_ref() else {
                        return false;
                    };
                    gateway_response
                        .get("order_kind")
                        .and_then(serde_json::Value::as_str)
                        == Some("plan_purchase")
                        && gateway_response
                            .get("product_id")
                            .and_then(serde_json::Value::as_str)
                            == Some(input.product_id.as_str())
                })
                .filter(|order| {
                    if order.status == "pending" {
                        return order
                            .expires_at_unix_secs
                            .is_some_and(|expires_at| expires_at > now_secs);
                    }
                    if purchase_limit_scope == "lifetime" {
                        return order.status == "credited";
                    }
                    order.status == "credited"
                        && order
                            .expires_at_unix_secs
                            .is_some_and(|expires_at| expires_at > now_secs)
                })
                .count() as i64;
            if existing_count >= max_active_per_user {
                return Ok(CreatePlanPurchaseOrderOutcome::ActivePlanLimitReached);
            }
        }
        let mut gateway_response = match input.gateway_response {
            serde_json::Value::Object(map) => map,
            value => {
                let mut map = serde_json::Map::new();
                map.insert("raw".to_string(), value);
                map
            }
        };
        gateway_response.insert(
            "order_kind".to_string(),
            serde_json::Value::String("plan_purchase".to_string()),
        );
        gateway_response.insert(
            "product_id".to_string(),
            serde_json::Value::String(input.product_id),
        );
        gateway_response.insert("product_snapshot".to_string(), input.product_snapshot);
        let order = StoredAdminPaymentOrder {
            id: format!("payment-order-{}", uuid::Uuid::new_v4()),
            order_no: input.order_no,
            wallet_id,
            user_id: Some(input.user_id),
            amount_usd: input.amount_usd,
            pay_amount: Some(input.pay_amount),
            pay_currency: Some(input.pay_currency),
            exchange_rate: Some(input.exchange_rate),
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            payment_method: input.payment_method,
            gateway_order_id: Some(input.gateway_order_id),
            gateway_response: Some(serde_json::Value::Object(gateway_response)),
            status: "pending".to_string(),
            created_at_unix_ms: current_unix_ms(),
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: Some(input.expires_at_unix_secs),
        };
        self.payment_orders_by_id
            .write()
            .expect("wallet repo lock")
            .insert(order.id.clone(), order.clone());
        Ok(CreatePlanPurchaseOrderOutcome::Created(order))
    }

    async fn create_wallet_refund_request(
        &self,
        input: CreateWalletRefundRequestInput,
    ) -> Result<CreateWalletRefundRequestOutcome, DataLayerError> {
        let wallets = self.wallets_by_id.read().expect("wallet repo lock");
        let Some(wallet) = wallets.get(&input.wallet_id) else {
            return Ok(CreateWalletRefundRequestOutcome::WalletMissing);
        };
        let reserved_amount = self
            .refunds_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .filter(|refund| {
                refund.wallet_id == input.wallet_id
                    && matches!(refund.status.as_str(), "pending_approval" | "approved")
            })
            .map(|refund| refund.amount_usd)
            .sum::<f64>();
        if input.amount_usd > (wallet.balance - reserved_amount) {
            return Ok(CreateWalletRefundRequestOutcome::RefundAmountExceedsAvailableBalance);
        }

        if let Some(order_id) = input.payment_order_id.as_deref() {
            let orders = self.payment_orders_by_id.read().expect("wallet repo lock");
            let Some(order) = orders.get(order_id) else {
                return Ok(CreateWalletRefundRequestOutcome::PaymentOrderNotFound);
            };
            if order.wallet_id != input.wallet_id || order.status != "credited" {
                return Ok(CreateWalletRefundRequestOutcome::PaymentOrderNotRefundable);
            }
            let reserved_for_order = self
                .refunds_by_id
                .read()
                .expect("wallet repo lock")
                .values()
                .filter(|refund| {
                    refund.payment_order_id.as_deref() == Some(order_id)
                        && matches!(refund.status.as_str(), "pending_approval" | "approved")
                })
                .map(|refund| refund.amount_usd)
                .sum::<f64>();
            if input.amount_usd > (order.refundable_amount_usd - reserved_for_order) {
                return Ok(
                    CreateWalletRefundRequestOutcome::RefundAmountExceedsAvailableOrderAmount,
                );
            }
        }

        let refund = StoredAdminWalletRefund {
            id: format!("refund-{}", uuid::Uuid::new_v4()),
            refund_no: input.refund_no,
            wallet_id: input.wallet_id,
            user_id: Some(input.user_id),
            payment_order_id: input.payment_order_id.clone(),
            source_type: input
                .payment_order_id
                .clone()
                .map(|_| "payment_order".to_string())
                .or(input.source_type)
                .unwrap_or_else(|| "wallet_balance".to_string()),
            source_id: input.payment_order_id.clone().or(input.source_id),
            refund_mode: input
                .refund_mode
                .unwrap_or_else(|| "offline_payout".to_string()),
            amount_usd: input.amount_usd,
            status: "pending_approval".to_string(),
            reason: input.reason,
            failure_reason: None,
            gateway_refund_id: None,
            payout_method: None,
            payout_reference: input.idempotency_key,
            payout_proof: None,
            requested_by: None,
            approved_by: None,
            processed_by: None,
            created_at_unix_ms: current_unix_ms(),
            updated_at_unix_secs: current_unix_secs(),
            processed_at_unix_secs: None,
            completed_at_unix_secs: None,
        };
        self.refunds_by_id
            .write()
            .expect("wallet repo lock")
            .insert(refund.id.clone(), refund.clone());
        Ok(CreateWalletRefundRequestOutcome::Created(refund))
    }

    async fn process_payment_callback(
        &self,
        _input: ProcessPaymentCallbackInput,
    ) -> Result<ProcessPaymentCallbackOutcome, DataLayerError> {
        Ok(ProcessPaymentCallbackOutcome::Failed {
            duplicate: false,
            error: "payment callback is not supported in memory wallet repository".to_string(),
        })
    }

    async fn adjust_wallet_balance(
        &self,
        _input: AdjustWalletBalanceInput,
    ) -> Result<Option<(StoredWalletSnapshot, super::StoredAdminWalletTransaction)>, DataLayerError>
    {
        Ok(None)
    }

    async fn create_manual_wallet_recharge(
        &self,
        _input: CreateManualWalletRechargeInput,
    ) -> Result<Option<(StoredWalletSnapshot, StoredAdminPaymentOrder)>, DataLayerError> {
        Ok(None)
    }

    async fn process_admin_wallet_refund(
        &self,
        _input: ProcessAdminWalletRefundInput,
    ) -> Result<
        WalletMutationOutcome<(
            StoredWalletSnapshot,
            super::StoredAdminWalletRefund,
            super::StoredAdminWalletTransaction,
        )>,
        DataLayerError,
    > {
        Ok(WalletMutationOutcome::NotFound)
    }

    async fn complete_admin_wallet_refund(
        &self,
        _input: CompleteAdminWalletRefundInput,
    ) -> Result<WalletMutationOutcome<super::StoredAdminWalletRefund>, DataLayerError> {
        Ok(WalletMutationOutcome::NotFound)
    }

    async fn fail_admin_wallet_refund(
        &self,
        _input: FailAdminWalletRefundInput,
    ) -> Result<
        WalletMutationOutcome<(
            StoredWalletSnapshot,
            super::StoredAdminWalletRefund,
            Option<super::StoredAdminWalletTransaction>,
        )>,
        DataLayerError,
    > {
        Ok(WalletMutationOutcome::NotFound)
    }

    async fn expire_admin_payment_order(
        &self,
        _order_id: &str,
    ) -> Result<WalletMutationOutcome<(StoredAdminPaymentOrder, bool)>, DataLayerError> {
        Ok(WalletMutationOutcome::NotFound)
    }

    async fn fail_admin_payment_order(
        &self,
        _order_id: &str,
    ) -> Result<WalletMutationOutcome<StoredAdminPaymentOrder>, DataLayerError> {
        Ok(WalletMutationOutcome::NotFound)
    }

    async fn credit_admin_payment_order(
        &self,
        _input: CreditAdminPaymentOrderInput,
    ) -> Result<WalletMutationOutcome<(StoredAdminPaymentOrder, bool)>, DataLayerError> {
        Ok(WalletMutationOutcome::NotFound)
    }

    async fn create_admin_redeem_code_batch(
        &self,
        input: CreateAdminRedeemCodeBatchInput,
    ) -> Result<CreateAdminRedeemCodeBatchResult, DataLayerError> {
        let now_ms = current_unix_ms();
        let now_secs = current_unix_secs();
        let batch_id = format!("redeem-batch-{}", uuid::Uuid::new_v4());
        let mut plaintext_codes = Vec::with_capacity(input.total_count);
        let mut codes_by_id = self.redeem_codes_by_id.write().expect("wallet repo lock");
        let mut code_hash_to_id = self
            .redeem_code_hash_to_id
            .write()
            .expect("wallet repo lock");

        for _ in 0..input.total_count {
            loop {
                let code = generate_redeem_code();
                let normalized =
                    normalize_redeem_code(&code).expect("generated code should normalize");
                let code_hash = hash_redeem_code(&normalized);
                if code_hash_to_id.contains_key(&code_hash) {
                    continue;
                }
                let code_id = format!("redeem-code-{}", uuid::Uuid::new_v4());
                let prefix = normalized.chars().take(4).collect::<String>();
                let suffix = normalized
                    .chars()
                    .rev()
                    .take(4)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<String>();
                let masked_code = mask_redeem_code(&prefix, &suffix);
                codes_by_id.insert(
                    code_id.clone(),
                    StoredAdminRedeemCode {
                        id: code_id.clone(),
                        batch_id: batch_id.clone(),
                        batch_name: Some(input.name.clone()),
                        code_prefix: prefix.clone(),
                        code_suffix: suffix.clone(),
                        masked_code: masked_code.clone(),
                        status: "active".to_string(),
                        redeemed_by_user_id: None,
                        redeemed_by_user_name: None,
                        redeemed_wallet_id: None,
                        redeemed_payment_order_id: None,
                        redeemed_order_no: None,
                        redeemed_at_unix_secs: None,
                        disabled_by: None,
                        expires_at_unix_secs: input.expires_at_unix_secs,
                        created_at_unix_ms: now_ms,
                        updated_at_unix_secs: now_secs,
                    },
                );
                code_hash_to_id.insert(code_hash, code_id.clone());
                plaintext_codes.push(CreatedAdminRedeemCodePlaintext {
                    code_id,
                    code,
                    masked_code,
                });
                break;
            }
        }

        let batch = StoredAdminRedeemCodeBatch {
            id: batch_id.clone(),
            name: input.name,
            amount_usd: input.amount_usd,
            currency: input.currency,
            balance_bucket: input.balance_bucket,
            total_count: input.total_count as u64,
            redeemed_count: 0,
            active_count: input.total_count as u64,
            status: "active".to_string(),
            description: input.description,
            created_by: input.created_by,
            expires_at_unix_secs: input.expires_at_unix_secs,
            created_at_unix_ms: now_ms,
            updated_at_unix_secs: now_secs,
        };
        self.redeem_batches_by_id
            .write()
            .expect("wallet repo lock")
            .insert(batch_id, batch.clone());
        Ok(CreateAdminRedeemCodeBatchResult {
            batch,
            codes: plaintext_codes,
        })
    }

    async fn disable_admin_redeem_code_batch(
        &self,
        input: DisableAdminRedeemCodeBatchInput,
    ) -> Result<WalletMutationOutcome<StoredAdminRedeemCodeBatch>, DataLayerError> {
        let now_secs = current_unix_secs();
        let updated = {
            let mut batches = self.redeem_batches_by_id.write().expect("wallet repo lock");
            let Some(batch) = batches.get_mut(&input.batch_id) else {
                return Ok(WalletMutationOutcome::NotFound);
            };
            batch.status = "disabled".to_string();
            batch.updated_at_unix_secs = now_secs;
            batch.clone()
        };

        let mut codes = self.redeem_codes_by_id.write().expect("wallet repo lock");
        for code in codes
            .values_mut()
            .filter(|code| code.batch_id == input.batch_id)
        {
            if code.status == "active" {
                code.status = "disabled".to_string();
                code.disabled_by = input.operator_id.clone();
                code.updated_at_unix_secs = now_secs;
            }
        }
        if let Some(batch) = self
            .redeem_batches_by_id
            .write()
            .expect("wallet repo lock")
            .get_mut(&input.batch_id)
        {
            batch.active_count = 0;
        }

        Ok(WalletMutationOutcome::Applied(updated))
    }

    async fn delete_admin_redeem_code_batch(
        &self,
        input: DeleteAdminRedeemCodeBatchInput,
    ) -> Result<WalletMutationOutcome<StoredAdminRedeemCodeBatch>, DataLayerError> {
        let batch = {
            let batches = self.redeem_batches_by_id.read().expect("wallet repo lock");
            let Some(batch) = batches.get(&input.batch_id) else {
                return Ok(WalletMutationOutcome::NotFound);
            };
            batch.clone()
        };
        let _ = input.operator_id;

        if batch.status != "disabled" {
            return Ok(WalletMutationOutcome::Invalid(
                "only disabled redeem code batch can be deleted".to_string(),
            ));
        }

        let codes = self.redeem_codes_by_id.read().expect("wallet repo lock");
        if codes
            .values()
            .any(|code| code.batch_id == input.batch_id && code.status == "redeemed")
        {
            return Ok(WalletMutationOutcome::Invalid(
                "redeemed batch cannot be deleted".to_string(),
            ));
        }
        let code_ids = codes
            .values()
            .filter(|code| code.batch_id == input.batch_id)
            .map(|code| code.id.clone())
            .collect::<Vec<_>>();
        drop(codes);

        self.redeem_batches_by_id
            .write()
            .expect("wallet repo lock")
            .remove(&input.batch_id);
        self.redeem_codes_by_id
            .write()
            .expect("wallet repo lock")
            .retain(|code_id, _| !code_ids.contains(code_id));
        self.redeem_code_hash_to_id
            .write()
            .expect("wallet repo lock")
            .retain(|_, code_id| !code_ids.contains(code_id));

        Ok(WalletMutationOutcome::Applied(batch))
    }

    async fn disable_admin_redeem_code(
        &self,
        input: DisableAdminRedeemCodeInput,
    ) -> Result<WalletMutationOutcome<StoredAdminRedeemCode>, DataLayerError> {
        let now_secs = current_unix_secs();
        let updated = {
            let mut codes = self.redeem_codes_by_id.write().expect("wallet repo lock");
            let Some(code) = codes.get_mut(&input.code_id) else {
                return Ok(WalletMutationOutcome::NotFound);
            };
            if code.status == "redeemed" {
                return Ok(WalletMutationOutcome::Invalid(
                    "redeemed code cannot be disabled".to_string(),
                ));
            }
            code.status = "disabled".to_string();
            code.disabled_by = input.operator_id;
            code.updated_at_unix_secs = now_secs;
            code.clone()
        };

        if let Some(batch) = self
            .redeem_batches_by_id
            .write()
            .expect("wallet repo lock")
            .get_mut(&updated.batch_id)
        {
            batch.active_count = self
                .redeem_codes_by_id
                .read()
                .expect("wallet repo lock")
                .values()
                .filter(|code| code.batch_id == updated.batch_id && code.status == "active")
                .count() as u64;
            batch.updated_at_unix_secs = now_secs;
        }

        Ok(WalletMutationOutcome::Applied(updated))
    }

    async fn redeem_wallet_code(
        &self,
        input: RedeemWalletCodeInput,
    ) -> Result<RedeemWalletCodeOutcome, DataLayerError> {
        let Some(normalized) = normalize_redeem_code(&input.code) else {
            return Ok(RedeemWalletCodeOutcome::InvalidCode);
        };
        let code_hash = hash_redeem_code(&normalized);
        let Some(code_id) = self
            .redeem_code_hash_to_id
            .read()
            .expect("wallet repo lock")
            .get(&code_hash)
            .cloned()
        else {
            return Ok(RedeemWalletCodeOutcome::CodeNotFound);
        };

        let now_secs = current_unix_secs();
        let now_ms = current_unix_ms();
        let (batch_id, batch_name, balance_bucket, amount_usd) = {
            let batches = self.redeem_batches_by_id.read().expect("wallet repo lock");
            let codes = self.redeem_codes_by_id.read().expect("wallet repo lock");
            let Some(code) = codes.get(&code_id) else {
                return Ok(RedeemWalletCodeOutcome::CodeNotFound);
            };
            match code.status.as_str() {
                "disabled" => return Ok(RedeemWalletCodeOutcome::CodeDisabled),
                "redeemed" => return Ok(RedeemWalletCodeOutcome::CodeRedeemed),
                _ => {}
            }
            if code
                .expires_at_unix_secs
                .is_some_and(|value| value <= now_secs)
            {
                return Ok(RedeemWalletCodeOutcome::CodeExpired);
            }
            let Some(batch) = batches.get(&code.batch_id) else {
                return Ok(RedeemWalletCodeOutcome::CodeNotFound);
            };
            if batch.status != "active" {
                return Ok(RedeemWalletCodeOutcome::BatchDisabled);
            }
            if batch
                .expires_at_unix_secs
                .is_some_and(|value| value <= now_secs)
            {
                return Ok(RedeemWalletCodeOutcome::CodeExpired);
            }
            (
                code.batch_id.clone(),
                batch.name.clone(),
                batch.balance_bucket.clone(),
                batch.amount_usd,
            )
        };
        let credits_recharge_balance = redeem_code_credits_recharge_balance(&balance_bucket);

        let (wallet, balance_before, gift_before) = {
            let mut wallets = self.wallets_by_id.write().expect("wallet repo lock");
            if let Some(wallet) = wallets
                .values_mut()
                .find(|wallet| wallet.user_id.as_deref() == Some(input.user_id.as_str()))
            {
                if wallet.status != "active" {
                    return Ok(RedeemWalletCodeOutcome::WalletInactive);
                }
                let balance_before = wallet.balance;
                let gift_before = wallet.gift_balance;
                if credits_recharge_balance {
                    wallet.balance += amount_usd;
                } else {
                    wallet.gift_balance += amount_usd;
                }
                wallet.total_recharged += amount_usd;
                wallet.updated_at_unix_secs = now_secs;
                (wallet.clone(), balance_before, gift_before)
            } else {
                let wallet = StoredWalletSnapshot::new(
                    format!("wallet-{}", uuid::Uuid::new_v4()),
                    Some(input.user_id.clone()),
                    None,
                    if credits_recharge_balance {
                        amount_usd
                    } else {
                        0.0
                    },
                    if credits_recharge_balance {
                        0.0
                    } else {
                        amount_usd
                    },
                    "finite".to_string(),
                    "USD".to_string(),
                    "active".to_string(),
                    amount_usd,
                    0.0,
                    0.0,
                    0.0,
                    now_secs as i64,
                )?;
                wallets.insert(wallet.id.clone(), wallet.clone());
                (wallet, 0.0, 0.0)
            }
        };

        let order = StoredAdminPaymentOrder {
            id: format!("payment-order-{}", uuid::Uuid::new_v4()),
            order_no: input.order_no,
            wallet_id: wallet.id.clone(),
            user_id: Some(input.user_id.clone()),
            amount_usd,
            pay_amount: None,
            pay_currency: None,
            exchange_rate: None,
            refunded_amount_usd: 0.0,
            refundable_amount_usd: redeem_code_refundable_amount(&balance_bucket, amount_usd),
            payment_method: redeem_code_payment_method(&balance_bucket).to_string(),
            gateway_order_id: Some(format!("card_{}", uuid::Uuid::new_v4().simple())),
            gateway_response: Some(serde_json::json!({
                "source": "redeem_code",
                "batch_id": batch_id,
                "batch_name": batch_name,
                "balance_bucket": balance_bucket,
            })),
            status: "credited".to_string(),
            created_at_unix_ms: now_ms,
            paid_at_unix_secs: Some(now_secs),
            credited_at_unix_secs: Some(now_secs),
            expires_at_unix_secs: None,
        };
        self.payment_orders_by_id
            .write()
            .expect("wallet repo lock")
            .insert(order.id.clone(), order.clone());

        let tx = StoredAdminWalletTransaction {
            id: format!("wallet-tx-{}", uuid::Uuid::new_v4()),
            wallet_id: wallet.id.clone(),
            category: "recharge".to_string(),
            reason_code: "topup_card_code".to_string(),
            amount: amount_usd,
            balance_before: balance_before + gift_before,
            balance_after: wallet.balance + wallet.gift_balance,
            recharge_balance_before: balance_before,
            recharge_balance_after: wallet.balance,
            gift_balance_before: gift_before,
            gift_balance_after: wallet.gift_balance,
            link_type: Some("payment_order".to_string()),
            link_id: Some(order.id.clone()),
            operator_id: None,
            operator_name: None,
            operator_email: None,
            description: Some("兑换码充值".to_string()),
            created_at_unix_ms: Some(now_ms),
        };
        self.wallet_transactions_by_id
            .write()
            .expect("wallet repo lock")
            .insert(tx.id.clone(), tx);

        if let Some(code) = self
            .redeem_codes_by_id
            .write()
            .expect("wallet repo lock")
            .get_mut(&code_id)
        {
            code.status = "redeemed".to_string();
            code.redeemed_by_user_id = Some(input.user_id);
            code.redeemed_wallet_id = Some(wallet.id.clone());
            code.redeemed_payment_order_id = Some(order.id.clone());
            code.redeemed_order_no = Some(order.order_no.clone());
            code.redeemed_at_unix_secs = Some(now_secs);
            code.updated_at_unix_secs = now_secs;
        }
        if let Some(batch) = self
            .redeem_batches_by_id
            .write()
            .expect("wallet repo lock")
            .get_mut(&batch_id)
        {
            batch.redeemed_count += 1;
            batch.active_count = batch.active_count.saturating_sub(1);
            batch.updated_at_unix_secs = now_secs;
        }

        Ok(RedeemWalletCodeOutcome::Redeemed {
            wallet,
            order,
            amount_usd,
            batch_name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{InMemoryWalletRepository, WalletReadSeed};
    use crate::repository::wallet::{
        AdminWalletListQuery, CreatePlanPurchaseOrderInput, CreatePlanPurchaseOrderOutcome,
        StoredAdminPaymentOrder, StoredAdminWalletRefund, StoredWalletSnapshot, WalletLookupKey,
        WalletReadRepository, WalletWriteRepository,
    };
    use serde_json::json;

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
    async fn updates_auth_wallet_limit_mode_and_snapshot_in_memory() {
        let repository = InMemoryWalletRepository::seed(vec![sample_wallet()]);

        let limit_updated = repository
            .update_auth_user_wallet_limit_mode("user-1", "unlimited")
            .await
            .expect("limit mode update should succeed")
            .expect("wallet should update");
        assert_eq!(limit_updated.limit_mode, "unlimited");

        let snapshot_updated = repository
            .update_auth_api_key_wallet_snapshot(
                "key-1",
                20.0,
                4.0,
                "finite",
                "USD",
                "active",
                30.0,
                5.0,
                1.0,
                2.0,
                Some(777),
            )
            .await
            .expect("snapshot update should succeed")
            .expect("wallet should update");
        assert_eq!(snapshot_updated.balance, 20.0);
        assert_eq!(snapshot_updated.gift_balance, 4.0);
        assert_eq!(snapshot_updated.total_recharged, 30.0);
        assert_eq!(snapshot_updated.total_consumed, 5.0);
        assert_eq!(snapshot_updated.total_refunded, 1.0);
        assert_eq!(snapshot_updated.total_adjusted, 2.0);
        assert_eq!(snapshot_updated.updated_at_unix_secs, 777);

        assert!(repository
            .update_auth_user_wallet_limit_mode("missing-user", "finite")
            .await
            .expect("missing limit mode update should succeed")
            .is_none());

        let user_wallet = repository
            .initialize_auth_user_wallet("user-2", 7.0, false)
            .await
            .expect("user wallet init should succeed")
            .expect("user wallet should initialize");
        assert_eq!(user_wallet.user_id.as_deref(), Some("user-2"));
        assert_eq!(user_wallet.gift_balance, 7.0);
        assert_eq!(user_wallet.total_adjusted, 7.0);

        let api_key_wallet = repository
            .initialize_auth_api_key_wallet("key-2", 7.0, true)
            .await
            .expect("api key wallet init should succeed")
            .expect("api key wallet should initialize");
        assert_eq!(api_key_wallet.api_key_id.as_deref(), Some("key-2"));
        assert_eq!(api_key_wallet.limit_mode, "unlimited");
        assert_eq!(api_key_wallet.gift_balance, 0.0);
    }

    fn sample_payment_order(
        id: &str,
        user_id: Option<&str>,
        status: &str,
    ) -> StoredAdminPaymentOrder {
        StoredAdminPaymentOrder {
            id: id.to_string(),
            order_no: format!("order-no-{id}"),
            wallet_id: "wallet-1".to_string(),
            user_id: user_id.map(str::to_string),
            amount_usd: 10.0,
            pay_amount: None,
            pay_currency: None,
            exchange_rate: None,
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 10.0,
            payment_method: "stripe".to_string(),
            gateway_order_id: None,
            gateway_response: None,
            status: status.to_string(),
            created_at_unix_ms: 100,
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: None,
        }
    }

    fn sample_refund(id: &str, user_id: Option<&str>, status: &str) -> StoredAdminWalletRefund {
        StoredAdminWalletRefund {
            id: id.to_string(),
            refund_no: format!("refund-no-{id}"),
            wallet_id: "wallet-1".to_string(),
            user_id: user_id.map(str::to_string),
            payment_order_id: None,
            source_type: "wallet_balance".to_string(),
            source_id: None,
            refund_mode: "offline_payout".to_string(),
            amount_usd: 3.0,
            status: status.to_string(),
            reason: None,
            failure_reason: None,
            gateway_refund_id: None,
            payout_method: None,
            payout_reference: None,
            payout_proof: None,
            requested_by: None,
            approved_by: None,
            processed_by: None,
            created_at_unix_ms: 100,
            updated_at_unix_secs: 100,
            processed_at_unix_secs: None,
            completed_at_unix_secs: None,
        }
    }

    #[tokio::test]
    async fn finds_wallet_by_owner() {
        let repository = InMemoryWalletRepository::seed(vec![sample_wallet()]);
        let wallet = repository
            .find(WalletLookupKey::UserId("user-1"))
            .await
            .expect("lookup should succeed")
            .expect("wallet should exist");
        assert_eq!(wallet.id, "wallet-1");
    }

    #[tokio::test]
    async fn lists_admin_wallets_with_filters_and_pagination() {
        let repository = InMemoryWalletRepository::seed(vec![
            sample_wallet(),
            StoredWalletSnapshot::new(
                "wallet-2".to_string(),
                Some("user-2".to_string()),
                None,
                3.0,
                1.0,
                "finite".to_string(),
                "USD".to_string(),
                "inactive".to_string(),
                0.0,
                0.0,
                0.0,
                0.0,
                90,
            )
            .expect("wallet should build"),
            StoredWalletSnapshot::new(
                "wallet-3".to_string(),
                None,
                Some("key-3".to_string()),
                5.0,
                0.0,
                "unlimited".to_string(),
                "USD".to_string(),
                "active".to_string(),
                0.0,
                0.0,
                0.0,
                0.0,
                110,
            )
            .expect("wallet should build"),
        ]);

        let page = repository
            .list_admin_wallets(&AdminWalletListQuery {
                status: Some("active".to_string()),
                owner_type: Some("api_key".to_string()),
                limit: 1,
                offset: 0,
            })
            .await
            .expect("list should succeed");

        assert_eq!(page.total, 2);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, "wallet-3");
        assert_eq!(page.items[0].updated_at_unix_secs, Some(110));
    }

    #[tokio::test]
    async fn daily_usage_queries_default_to_empty_in_memory() {
        let repository = InMemoryWalletRepository::seed(vec![sample_wallet()]);
        let today = repository
            .find_wallet_today_usage("wallet-1", "Asia/Shanghai")
            .await
            .expect("lookup should succeed");
        let history = repository
            .list_wallet_daily_usage_history("wallet-1", "Asia/Shanghai", 20)
            .await
            .expect("history should succeed");

        assert!(today.is_none());
        assert_eq!(history.total, 0);
        assert!(history.items.is_empty());
    }

    #[tokio::test]
    async fn lifetime_plan_purchase_blocks_duplicate_pending_order_in_memory() {
        let repository = InMemoryWalletRepository::seed(vec![sample_wallet()]);
        let snapshot = json!({
            "id": "first-plan",
            "duration_unit": "month",
            "duration_value": 1,
            "max_active_per_user": 1,
            "purchase_limit_scope": "lifetime",
            "entitlements": [
                {
                    "type": "wallet_credit",
                    "amount_usd": 1.0,
                    "balance_bucket": "gift"
                }
            ]
        });

        let first = repository
            .create_plan_purchase_order(CreatePlanPurchaseOrderInput {
                preferred_wallet_id: None,
                user_id: "user-1".to_string(),
                amount_usd: 1.0,
                pay_amount: 7.2,
                pay_currency: "CNY".to_string(),
                exchange_rate: 7.2,
                payment_method: "alipay".to_string(),
                payment_provider: Some("epay".to_string()),
                payment_channel: Some("alipay".to_string()),
                gateway_order_id: "gateway-first-plan-1".to_string(),
                gateway_response: json!({ "checkout": true }),
                order_no: "order-first-plan-1".to_string(),
                product_id: "first-plan".to_string(),
                product_snapshot: snapshot.clone(),
                expires_at_unix_secs: 4_102_444_800,
            })
            .await
            .expect("first plan purchase should resolve");
        assert!(matches!(first, CreatePlanPurchaseOrderOutcome::Created(_)));

        let duplicate = repository
            .create_plan_purchase_order(CreatePlanPurchaseOrderInput {
                preferred_wallet_id: None,
                user_id: "user-1".to_string(),
                amount_usd: 1.0,
                pay_amount: 7.2,
                pay_currency: "CNY".to_string(),
                exchange_rate: 7.2,
                payment_method: "alipay".to_string(),
                payment_provider: Some("epay".to_string()),
                payment_channel: Some("alipay".to_string()),
                gateway_order_id: "gateway-first-plan-2".to_string(),
                gateway_response: json!({ "checkout": true }),
                order_no: "order-first-plan-2".to_string(),
                product_id: "first-plan".to_string(),
                product_snapshot: snapshot,
                expires_at_unix_secs: 4_102_444_800,
            })
            .await
            .expect("duplicate plan purchase should resolve");
        assert!(matches!(
            duplicate,
            CreatePlanPurchaseOrderOutcome::ActivePlanLimitReached
        ));

        let unlimited_snapshot = json!({
            "id": "unlimited-plan",
            "duration_unit": "month",
            "duration_value": 1,
            "max_active_per_user": 1,
            "purchase_limit_scope": "unlimited",
            "entitlements": [
                {
                    "type": "wallet_credit",
                    "amount_usd": 1.0,
                    "balance_bucket": "gift"
                }
            ]
        });
        for index in 1..=2 {
            let order = repository
                .create_plan_purchase_order(CreatePlanPurchaseOrderInput {
                    preferred_wallet_id: None,
                    user_id: "user-1".to_string(),
                    amount_usd: 1.0,
                    pay_amount: 7.2,
                    pay_currency: "CNY".to_string(),
                    exchange_rate: 7.2,
                    payment_method: "alipay".to_string(),
                    payment_provider: Some("epay".to_string()),
                    payment_channel: Some("alipay".to_string()),
                    gateway_order_id: format!("gateway-unlimited-plan-{index}"),
                    gateway_response: json!({ "checkout": true }),
                    order_no: format!("order-unlimited-plan-{index}"),
                    product_id: "unlimited-plan".to_string(),
                    product_snapshot: unlimited_snapshot.clone(),
                    expires_at_unix_secs: 4_102_444_800,
                })
                .await
                .expect("unlimited plan purchase should resolve");
            assert!(matches!(order, CreatePlanPurchaseOrderOutcome::Created(_)));
        }
    }

    #[tokio::test]
    async fn counts_pending_user_refunds_and_payment_orders() {
        let repository = InMemoryWalletRepository::seed_read_model(WalletReadSeed {
            wallets: vec![sample_wallet()],
            payment_orders: vec![
                sample_payment_order("order-1", Some("user-1"), "pending"),
                sample_payment_order("order-2", Some("user-1"), "paid"),
                sample_payment_order("order-3", Some("user-1"), "credited"),
                sample_payment_order("order-4", Some("user-2"), "pending"),
            ],
            payment_callbacks: Vec::new(),
            wallet_transactions: Vec::new(),
            refunds: vec![
                sample_refund("refund-1", Some("user-1"), "pending_approval"),
                sample_refund("refund-2", Some("user-1"), "processing"),
                sample_refund("refund-3", Some("user-1"), "completed"),
                sample_refund("refund-4", Some("user-2"), "approved"),
            ],
            redeem_batches: Vec::new(),
            redeem_codes: Vec::new(),
        });

        assert_eq!(
            repository
                .count_pending_payment_orders_by_user_id("user-1")
                .await
                .expect("payment order count should succeed"),
            2
        );
        assert_eq!(
            repository
                .count_pending_refunds_by_user_id("user-1")
                .await
                .expect("refund count should succeed"),
            2
        );
    }
}
