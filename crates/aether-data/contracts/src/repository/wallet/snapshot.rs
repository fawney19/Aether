use std::collections::{BTreeMap, BTreeSet};

use super::{
    AdminPaymentOrderListQuery, AdminRedeemCodeBatchListQuery, AdminRedeemCodeListQuery,
    AdminWalletLedgerQuery, AdminWalletListQuery, AdminWalletRefundRequestListQuery,
    StoredAdminPaymentCallback, StoredAdminPaymentCallbackPage, StoredAdminPaymentOrder,
    StoredAdminPaymentOrderPage, StoredAdminRedeemCode, StoredAdminRedeemCodeBatch,
    StoredAdminRedeemCodeBatchPage, StoredAdminRedeemCodePage, StoredAdminWalletLedgerPage,
    StoredAdminWalletListItem, StoredAdminWalletListPage, StoredAdminWalletRefund,
    StoredAdminWalletRefundPage, StoredAdminWalletRefundRequestItem,
    StoredAdminWalletRefundRequestPage, StoredAdminWalletTransaction,
    StoredAdminWalletTransactionPage, StoredWalletSnapshot, WalletLookupKey,
};

#[derive(Debug, Clone, Default)]
pub struct WalletReadSeed {
    pub wallets: Vec<StoredWalletSnapshot>,
    pub payment_orders: Vec<StoredAdminPaymentOrder>,
    pub payment_callbacks: Vec<StoredAdminPaymentCallback>,
    pub wallet_transactions: Vec<StoredAdminWalletTransaction>,
    pub refunds: Vec<StoredAdminWalletRefund>,
    pub redeem_batches: Vec<StoredAdminRedeemCodeBatch>,
    pub redeem_codes: Vec<StoredAdminRedeemCode>,
}

/// Immutable wallet read model shared by memory and SQL adapters.
#[derive(Debug, Clone, Default)]
pub struct WalletReadSnapshot {
    wallets: BTreeMap<String, StoredWalletSnapshot>,
    payment_orders: BTreeMap<String, StoredAdminPaymentOrder>,
    payment_callbacks: BTreeMap<String, StoredAdminPaymentCallback>,
    wallet_transactions: BTreeMap<String, StoredAdminWalletTransaction>,
    refunds: BTreeMap<String, StoredAdminWalletRefund>,
    redeem_batches: BTreeMap<String, StoredAdminRedeemCodeBatch>,
    redeem_codes: BTreeMap<String, StoredAdminRedeemCode>,
}

impl WalletReadSnapshot {
    pub fn new(seed: WalletReadSeed) -> Self {
        Self {
            wallets: by_id(seed.wallets, |item| &item.id),
            payment_orders: by_id(seed.payment_orders, |item| &item.id),
            payment_callbacks: by_id(seed.payment_callbacks, |item| &item.id),
            wallet_transactions: by_id(seed.wallet_transactions, |item| &item.id),
            refunds: by_id(seed.refunds, |item| &item.id),
            redeem_batches: by_id(seed.redeem_batches, |item| &item.id),
            redeem_codes: by_id(seed.redeem_codes, |item| &item.id),
        }
    }

    pub fn find(&self, key: WalletLookupKey<'_>) -> Option<StoredWalletSnapshot> {
        match key {
            WalletLookupKey::WalletId(wallet_id) => self.wallets.get(wallet_id).cloned(),
            WalletLookupKey::UserId(user_id) => self
                .wallets
                .values()
                .find(|wallet| wallet.user_id.as_deref() == Some(user_id))
                .cloned(),
            WalletLookupKey::ApiKeyId(api_key_id) => self
                .wallets
                .values()
                .find(|wallet| wallet.api_key_id.as_deref() == Some(api_key_id))
                .cloned(),
        }
    }

    pub fn list_wallets_by_user_ids(&self, user_ids: &[String]) -> Vec<StoredWalletSnapshot> {
        let ids = user_ids.iter().map(String::as_str).collect::<BTreeSet<_>>();
        self.wallets
            .values()
            .filter(|wallet| wallet.user_id.as_deref().is_some_and(|id| ids.contains(id)))
            .cloned()
            .collect()
    }

    pub fn list_wallets_by_api_key_ids(&self, api_key_ids: &[String]) -> Vec<StoredWalletSnapshot> {
        let ids = api_key_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        self.wallets
            .values()
            .filter(|wallet| {
                wallet
                    .api_key_id
                    .as_deref()
                    .is_some_and(|id| ids.contains(id))
            })
            .cloned()
            .collect()
    }

    pub fn list_admin_wallets(&self, query: &AdminWalletListQuery) -> StoredAdminWalletListPage {
        let mut items = self
            .wallets
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
            .collect();
        StoredAdminWalletListPage { items, total }
    }

    pub fn list_admin_wallet_ledger(
        &self,
        _query: &AdminWalletLedgerQuery,
    ) -> StoredAdminWalletLedgerPage {
        StoredAdminWalletLedgerPage::default()
    }

    pub fn list_admin_wallet_refund_requests(
        &self,
        query: &AdminWalletRefundRequestListQuery,
    ) -> StoredAdminWalletRefundRequestPage {
        let mut items = self
            .refunds
            .values()
            .filter(|refund| {
                query
                    .status
                    .as_deref()
                    .is_none_or(|expected| refund.status == expected)
            })
            .filter_map(|refund| {
                let wallet = self.wallets.get(&refund.wallet_id)?;
                Some(StoredAdminWalletRefundRequestItem {
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
        page(items, query.offset, query.limit, |items, total| {
            StoredAdminWalletRefundRequestPage { items, total }
        })
    }

    pub fn list_admin_wallet_transactions(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> StoredAdminWalletTransactionPage {
        let mut items = self
            .wallet_transactions
            .values()
            .filter(|item| item.wallet_id == wallet_id)
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at_unix_ms));
        page(items, offset, limit, |items, total| {
            StoredAdminWalletTransactionPage { items, total }
        })
    }

    pub fn list_admin_wallet_refunds(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> StoredAdminWalletRefundPage {
        let mut items = self
            .refunds
            .values()
            .filter(|item| item.wallet_id == wallet_id)
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at_unix_ms));
        page(items, offset, limit, |items, total| {
            StoredAdminWalletRefundPage { items, total }
        })
    }

    pub fn list_admin_payment_orders(
        &self,
        query: &AdminPaymentOrderListQuery,
        now_unix_secs: u64,
    ) -> StoredAdminPaymentOrderPage {
        let mut items = self
            .payment_orders
            .values()
            .filter(|order| {
                query.status.as_deref().is_none_or(|expected| {
                    let effective = if order.status == "pending"
                        && order
                            .expires_at_unix_secs
                            .is_some_and(|value| value < now_unix_secs)
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
        page(items, query.offset, query.limit, |items, total| {
            StoredAdminPaymentOrderPage { items, total }
        })
    }

    pub fn find_admin_payment_order(&self, order_id: &str) -> Option<StoredAdminPaymentOrder> {
        self.payment_orders.get(order_id).cloned()
    }

    pub fn list_wallet_payment_orders_by_user_id(
        &self,
        user_id: &str,
        limit: usize,
        offset: usize,
    ) -> StoredAdminPaymentOrderPage {
        let mut items = self
            .payment_orders
            .values()
            .filter(|order| order.user_id.as_deref() == Some(user_id))
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at_unix_ms));
        page(items, offset, limit, |items, total| {
            StoredAdminPaymentOrderPage { items, total }
        })
    }

    pub fn count_pending_refunds_by_user_id(&self, user_id: &str) -> u64 {
        const STATUSES: &[&str] = &["pending_approval", "approved", "processing"];
        self.refunds
            .values()
            .filter(|item| {
                item.user_id.as_deref() == Some(user_id) && STATUSES.contains(&item.status.as_str())
            })
            .count() as u64
    }

    pub fn count_pending_payment_orders_by_user_id(&self, user_id: &str) -> u64 {
        const STATUSES: &[&str] = &["pending", "paid"];
        self.payment_orders
            .values()
            .filter(|item| {
                item.user_id.as_deref() == Some(user_id) && STATUSES.contains(&item.status.as_str())
            })
            .count() as u64
    }

    pub fn find_wallet_payment_order_by_user_id(
        &self,
        user_id: &str,
        order_id: &str,
    ) -> Option<StoredAdminPaymentOrder> {
        self.payment_orders
            .get(order_id)
            .filter(|order| order.user_id.as_deref() == Some(user_id))
            .cloned()
    }

    pub fn find_pending_plan_purchase_order_by_user_id(
        &self,
        user_id: &str,
        product_id: &str,
        now_unix_secs: u64,
    ) -> Option<StoredAdminPaymentOrder> {
        self.payment_orders
            .values()
            .filter(|order| {
                order.user_id.as_deref() == Some(user_id)
                    && order.status == "pending"
                    && order
                        .expires_at_unix_secs
                        .is_some_and(|expires_at| expires_at > now_unix_secs)
                    && order.gateway_response.as_ref().is_some_and(|response| {
                        response
                            .get("order_kind")
                            .and_then(serde_json::Value::as_str)
                            == Some("plan_purchase")
                            && response
                                .get("product_id")
                                .and_then(serde_json::Value::as_str)
                                == Some(product_id)
                    })
            })
            .max_by_key(|order| order.created_at_unix_ms)
            .cloned()
    }

    pub fn find_wallet_refund(
        &self,
        wallet_id: &str,
        refund_id: &str,
    ) -> Option<StoredAdminWalletRefund> {
        self.refunds
            .get(refund_id)
            .filter(|refund| refund.wallet_id == wallet_id)
            .cloned()
    }

    pub fn list_admin_payment_callbacks(
        &self,
        payment_method: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> StoredAdminPaymentCallbackPage {
        let mut items = self
            .payment_callbacks
            .values()
            .filter(|item| payment_method.is_none_or(|value| item.payment_method == value))
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at_unix_ms));
        page(items, offset, limit, |items, total| {
            StoredAdminPaymentCallbackPage { items, total }
        })
    }

    pub fn list_admin_redeem_code_batches(
        &self,
        query: &AdminRedeemCodeBatchListQuery,
    ) -> StoredAdminRedeemCodeBatchPage {
        let mut items = self
            .redeem_batches
            .values()
            .filter(|item| {
                query
                    .status
                    .as_deref()
                    .is_none_or(|value| item.status == value)
            })
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at_unix_ms));
        page(items, query.offset, query.limit, |items, total| {
            StoredAdminRedeemCodeBatchPage { items, total }
        })
    }

    pub fn find_admin_redeem_code_batch(
        &self,
        batch_id: &str,
    ) -> Option<StoredAdminRedeemCodeBatch> {
        self.redeem_batches.get(batch_id).cloned()
    }

    pub fn list_admin_redeem_codes(
        &self,
        query: &AdminRedeemCodeListQuery,
    ) -> StoredAdminRedeemCodePage {
        let mut items = self
            .redeem_codes
            .values()
            .filter(|item| item.batch_id == query.batch_id)
            .filter(|item| {
                query
                    .status
                    .as_deref()
                    .is_none_or(|value| item.status == value)
            })
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| std::cmp::Reverse(item.created_at_unix_ms));
        page(items, query.offset, query.limit, |items, total| {
            StoredAdminRedeemCodePage { items, total }
        })
    }
}

fn by_id<T>(items: Vec<T>, id: impl Fn(&T) -> &str) -> BTreeMap<String, T> {
    items
        .into_iter()
        .map(|item| (id(&item).to_string(), item))
        .collect()
}

fn page<T, P>(items: Vec<T>, offset: usize, limit: usize, build: impl Fn(Vec<T>, u64) -> P) -> P {
    let total = items.len() as u64;
    build(items.into_iter().skip(offset).take(limit).collect(), total)
}
