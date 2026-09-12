use std::collections::BTreeMap;
use std::sync::{Mutex, RwLock};

use async_trait::async_trait;

#[cfg(test)]
use super::WalletReadSeed;
use super::{
    canonicalize_payment_method, canonicalize_wallet_refund_fields,
    payment_order_refund_amounts_are_consistent,
    payment_order_stripe_client_secret_cas_replacement, project_wallet_gateway_response,
    project_wallet_recharge_gateway_response, redeem_code_payment_method,
    redeem_code_refundable_amount, validate_admin_redeem_code_batch_input,
    validate_plan_purchase_order_input, validate_plan_wallet_credit_entitlements,
    validate_redeem_wallet_credit, validate_wallet_recharge_order_input,
    wallet_recharge_checkout_claim_response, wallet_recharge_checkout_claim_token,
    wallet_recharge_checkout_failed_response, wallet_recharge_checkout_uncertain_response,
    wallet_recharge_order_is_checkout_placeholder,
    wallet_recharge_order_is_reclaimable_placeholder, wallet_recharge_replay_matches,
    wallet_recharge_response_is_checkout_placeholder, wallet_refund_proof_is_success,
    AdjustWalletBalanceInput, AdminPaymentOrderListQuery, AdminRedeemCodeBatchListQuery,
    AdminRedeemCodeListQuery, AdminWalletLedgerQuery, AdminWalletListQuery,
    AdminWalletRefundRequestListQuery, CompareAndSwapPaymentOrderStripeClientSecretInput,
    CompleteAdminWalletRefundInput, CreateAdminRedeemCodeBatchInput,
    CreateAdminRedeemCodeBatchResult, CreateManualWalletRechargeInput,
    CreatePlanPurchaseOrderInput, CreatePlanPurchaseOrderOutcome, CreateWalletRechargeOrderInput,
    CreateWalletRechargeOrderOutcome, CreateWalletRefundRequestInput,
    CreateWalletRefundRequestOutcome, CreatedAdminRedeemCodePlaintext,
    CreditAdminPaymentOrderInput, DeleteAdminRedeemCodeBatchInput,
    DisableAdminRedeemCodeBatchInput, DisableAdminRedeemCodeInput, FailAdminWalletRefundInput,
    FailWalletRechargeCheckoutInput, InitializeAuthWalletOutcome, ProcessAdminWalletRefundInput,
    ProcessPaymentCallbackInput, ProcessPaymentCallbackOutcome, ReclaimWalletRechargeCheckoutInput,
    RedeemWalletCodeInput, RedeemWalletCodeOutcome, StoredAdminPaymentCallback,
    StoredAdminPaymentCallbackPage, StoredAdminPaymentOrder, StoredAdminPaymentOrderPage,
    StoredAdminRedeemCode, StoredAdminRedeemCodeBatch, StoredAdminRedeemCodeBatchPage,
    StoredAdminRedeemCodePage, StoredAdminWalletLedgerPage, StoredAdminWalletListItem,
    StoredAdminWalletListPage, StoredAdminWalletRefund, StoredAdminWalletRefundPage,
    StoredAdminWalletRefundRequestPage, StoredAdminWalletTransaction,
    StoredAdminWalletTransactionPage, StoredWalletDailyUsageLedger,
    StoredWalletDailyUsageLedgerPage, StoredWalletSnapshot, UpdateAdminWalletRefundGatewayInput,
    UpdateWalletRechargeCheckoutInput, WalletLookupKey, WalletMutationOutcome,
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
    refund_idempotency_to_id: RwLock<BTreeMap<(String, String), String>>,
    refund_creation_lock: Mutex<()>,
    // Wallet creation, order attachment, and compensation cleanup must be
    // serialized together.  Separate map locks cannot make the
    // "check references, then delete" sequence atomic.
    wallet_lifecycle_lock: Mutex<()>,
}

fn wallet_recharge_metadata_value<'a>(
    record: &'a StoredAdminPaymentOrder,
    key: &str,
) -> Option<&'a str> {
    record
        .gateway_response
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(serde_json::Value::as_str)
}

/// A compensation path may remove only a freshly initialized wallet that has
/// never carried funds or a financial reference.  Keep this predicate in one
/// place so the initial check and the final compare use exactly the same
/// definition even when another in-memory writer updates the row between
/// those checks.
fn wallet_is_untouched_for_compensation(wallet: &StoredWalletSnapshot) -> bool {
    wallet.balance == 0.0
        && wallet.gift_balance == 0.0
        && wallet.total_recharged == 0.0
        && wallet.total_consumed == 0.0
        && wallet.total_refunded == 0.0
        && wallet.total_adjusted == 0.0
        && matches!(wallet.limit_mode.as_str(), "finite" | "unlimited")
        && wallet.currency == "USD"
        && wallet.status == "active"
}

fn provisional_auth_wallet_transactions_match(
    transactions: &BTreeMap<String, StoredAdminWalletTransaction>,
    wallet: &StoredWalletSnapshot,
    user_id: &str,
) -> bool {
    let wallet_transactions = transactions
        .values()
        .filter(|transaction| transaction.wallet_id == wallet.id)
        .collect::<Vec<_>>();
    if wallet.gift_balance == 0.0 {
        return wallet_transactions.is_empty();
    }
    if wallet_transactions.len() != 1 {
        return false;
    }
    let transaction = wallet_transactions[0];
    transaction.category == "gift"
        && transaction.reason_code == "gift_initial"
        && transaction.amount == wallet.gift_balance
        && transaction.balance_before == 0.0
        && transaction.balance_after == wallet.gift_balance
        && transaction.recharge_balance_before == 0.0
        && transaction.recharge_balance_after == 0.0
        && transaction.gift_balance_before == 0.0
        && transaction.gift_balance_after == wallet.gift_balance
        && transaction.link_type.as_deref() == Some("system_task")
        && transaction.link_id.as_deref() == Some(user_id)
        && transaction.operator_id.is_none()
}

impl InMemoryWalletRepository {
    fn insert_payment_order_unique(
        &self,
        order: StoredAdminPaymentOrder,
    ) -> Result<(), DataLayerError> {
        let mut orders = self.payment_orders_by_id.write().expect("wallet repo lock");
        if orders.contains_key(&order.id)
            || orders
                .values()
                .any(|existing| existing.order_no == order.order_no)
        {
            return Err(DataLayerError::InvalidInput(
                "payment order number already belongs to another order".to_string(),
            ));
        }
        if let Some(gateway_order_id) = order.gateway_order_id.as_deref() {
            if orders.values().any(|existing| {
                existing.payment_method == order.payment_method
                    && existing.gateway_order_id.as_deref() == Some(gateway_order_id)
            }) {
                return Err(DataLayerError::InvalidInput(
                    "payment gateway order already belongs to another order".to_string(),
                ));
            }
        }
        orders.insert(order.id.clone(), order);
        Ok(())
    }

    fn insert_wallet_recharge_order_unique(
        &self,
        order: StoredAdminPaymentOrder,
        now_unix_secs: u64,
        input: &CreateWalletRechargeOrderInput,
    ) -> Result<(Option<StoredAdminPaymentOrder>, bool), DataLayerError> {
        let mut orders = self.payment_orders_by_id.write().expect("wallet repo lock");
        if let Some(existing) = orders
            .values()
            .find(|existing| existing.order_no == order.order_no)
        {
            let existing_kind = existing
                .gateway_response
                .as_ref()
                .and_then(serde_json::Value::as_object)
                .and_then(|object| object.get("order_kind"))
                .and_then(serde_json::Value::as_str);
            if existing.user_id == order.user_id && existing_kind == Some("wallet_recharge") {
                let existing_provider =
                    wallet_recharge_metadata_value(existing, "payment_provider")
                        .or_else(|| wallet_recharge_metadata_value(existing, "gateway"));
                // A seeded legacy order can outlive a deleted/provisioning
                // wallet. In that read-model-only case the incoming wallet
                // id is provisional and cannot be treated as an immutable
                // field; a real wallet remains strictly bound below.
                let replay_wallet_id = if self
                    .wallets_by_id
                    .read()
                    .expect("wallet repo lock")
                    .contains_key(&existing.wallet_id)
                {
                    order.wallet_id.as_str()
                } else {
                    existing.wallet_id.as_str()
                };
                if !wallet_recharge_replay_matches(
                    &existing.wallet_id,
                    existing.amount_usd,
                    existing.pay_amount,
                    existing.pay_currency.as_deref(),
                    existing.exchange_rate,
                    &existing.payment_method,
                    existing_provider,
                    wallet_recharge_metadata_value(existing, "payment_channel"),
                    replay_wallet_id,
                    input,
                ) {
                    return Err(DataLayerError::InvalidInput(
                        "wallet recharge replay changes immutable order fields".to_string(),
                    ));
                }
                if wallet_recharge_order_is_reclaimable_placeholder(existing, now_unix_secs) {
                    let Some(candidate_token) = order
                        .gateway_response
                        .as_ref()
                        .and_then(wallet_recharge_checkout_claim_token)
                    else {
                        return Ok((Some(existing.clone()), false));
                    };
                    let claimed = wallet_recharge_checkout_claim_response(
                        order.gateway_response.as_ref().expect("gateway response"),
                        candidate_token,
                        now_unix_secs,
                    )
                    .map_err(DataLayerError::InvalidInput)?;
                    let existing_id = existing.id.clone();
                    let existing = orders
                        .get_mut(&existing_id)
                        .expect("recharge order should remain present");
                    existing.gateway_response = Some(claimed);
                    existing.gateway_order_id = Some(existing.order_no.clone());
                    existing.status = "pending".to_string();
                    existing.expires_at_unix_secs = order.expires_at_unix_secs;
                    return Ok((Some(existing.clone()), true));
                }
                return Ok((Some(existing.clone()), false));
            }
            return Err(DataLayerError::InvalidInput(
                "payment order number already belongs to another user".to_string(),
            ));
        }
        if let Some(gateway_order_id) = order.gateway_order_id.as_deref() {
            if orders.values().any(|existing| {
                existing.payment_method == order.payment_method
                    && existing.gateway_order_id.as_deref() == Some(gateway_order_id)
            }) {
                return Err(DataLayerError::InvalidInput(
                    "payment gateway order already belongs to another order".to_string(),
                ));
            }
        }
        orders.insert(order.id.clone(), order);
        Ok((None, false))
    }

    fn remove_created_wallet_if_unreferenced(&self, wallet_id: &str) {
        // The caller holds wallet_lifecycle_lock, as do all in-memory paths
        // that attach a payment order.  The reference check and compensation
        // delete are therefore atomic with respect to a new order attachment.
        let wallet_is_referenced = self
            .payment_orders_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .any(|existing| existing.wallet_id == wallet_id);
        if !wallet_is_referenced {
            let mut wallets = self.wallets_by_id.write().expect("wallet repo lock");
            // A regular wallet snapshot update does not need the lifecycle
            // lock. Re-check the complete pristine shape while holding the
            // write lock so a concurrent credit cannot be deleted as part of
            // compensation.
            if wallets
                .get(wallet_id)
                .is_some_and(wallet_is_untouched_for_compensation)
            {
                wallets.remove(wallet_id);
            }
        }
    }

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
            refund_idempotency_to_id: RwLock::new(BTreeMap::new()),
            refund_creation_lock: Mutex::new(()),
            wallet_lifecycle_lock: Mutex::new(()),
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
        let refund_idempotency_to_id = seed
            .refund_idempotency
            .into_iter()
            .map(|(user_id, idempotency_key, refund_id)| ((user_id, idempotency_key), refund_id))
            .collect();
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
            refund_idempotency_to_id: RwLock::new(refund_idempotency_to_id),
            refund_creation_lock: Mutex::new(()),
            wallet_lifecycle_lock: Mutex::new(()),
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
) -> Result<Option<(StoredWalletSnapshot, bool)>, DataLayerError> {
    let owner_id = user_id
        .or(api_key_id)
        .filter(|value| !value.trim().is_empty());
    if owner_id.is_none() || (user_id.is_some() && api_key_id.is_some()) {
        return Err(DataLayerError::InvalidInput(
            "wallet owner must be exactly one non-empty user or API-key id".to_string(),
        ));
    }
    if !initial_gift_usd.is_finite() {
        return Err(DataLayerError::InvalidInput(
            "initial gift amount must be finite".to_string(),
        ));
    }

    // Initialization is intentionally idempotent. Database backends enforce this
    // with the owner unique indexes; perform the same lookup before creating the
    // in-memory row so retries cannot mint another wallet or gift transaction.
    {
        let wallets = wallets_by_id.read().expect("wallet repo lock");
        let existing = wallets.values().find(|wallet| {
            if let Some(user_id) = user_id {
                wallet.user_id.as_deref() == Some(user_id) && wallet.api_key_id.is_none()
            } else if let Some(api_key_id) = api_key_id {
                wallet.api_key_id.as_deref() == Some(api_key_id) && wallet.user_id.is_none()
            } else {
                false
            }
        });
        if let Some(wallet) = existing {
            return Ok(Some((wallet.clone(), false)));
        }
    }

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

    Ok(Some((wallet, true)))
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
        if user_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "user id is required to initialize a wallet".to_string(),
            ));
        }
        let _lifecycle_guard = self
            .wallet_lifecycle_lock
            .lock()
            .expect("wallet lifecycle lock");
        initialize_auth_wallet_in_memory(
            &self.wallets_by_id,
            &self.wallet_transactions_by_id,
            Some(user_id),
            None,
            initial_gift_usd,
            unlimited,
        )
        .map(|result| result.map(|(wallet, _created)| wallet))
    }

    async fn initialize_auth_user_wallet_with_outcome(
        &self,
        user_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<InitializeAuthWalletOutcome>, DataLayerError> {
        if user_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "user id is required to initialize a wallet".to_string(),
            ));
        }
        let _lifecycle_guard = self
            .wallet_lifecycle_lock
            .lock()
            .expect("wallet lifecycle lock");
        initialize_auth_wallet_in_memory(
            &self.wallets_by_id,
            &self.wallet_transactions_by_id,
            Some(user_id),
            None,
            initial_gift_usd,
            unlimited,
        )
        .map(|result| {
            result.map(|(wallet, created)| InitializeAuthWalletOutcome { wallet, created })
        })
    }

    async fn initialize_auth_api_key_wallet(
        &self,
        api_key_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        if api_key_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "api key id is required to initialize a wallet".to_string(),
            ));
        }
        let _lifecycle_guard = self
            .wallet_lifecycle_lock
            .lock()
            .expect("wallet lifecycle lock");
        initialize_auth_wallet_in_memory(
            &self.wallets_by_id,
            &self.wallet_transactions_by_id,
            None,
            Some(api_key_id),
            initial_gift_usd,
            unlimited,
        )
        .map(|result| result.map(|(wallet, _created)| wallet))
    }

    async fn initialize_auth_api_key_wallet_with_outcome(
        &self,
        api_key_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<InitializeAuthWalletOutcome>, DataLayerError> {
        if api_key_id.trim().is_empty() {
            return Err(DataLayerError::InvalidInput(
                "api key id is required to initialize a wallet".to_string(),
            ));
        }
        let _lifecycle_guard = self
            .wallet_lifecycle_lock
            .lock()
            .expect("wallet lifecycle lock");
        initialize_auth_wallet_in_memory(
            &self.wallets_by_id,
            &self.wallet_transactions_by_id,
            None,
            Some(api_key_id),
            initial_gift_usd,
            unlimited,
        )
        .map(|result| {
            result.map(|(wallet, created)| InitializeAuthWalletOutcome { wallet, created })
        })
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
        let wallets = self.wallets_by_id.read().expect("wallet repo lock").clone();
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
                        && order.expires_at_unix_secs.is_some_and(|value| value <= now)
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
            .filter(|order| {
                order.user_id.as_deref() == Some(user_id)
                    && order
                        .gateway_response
                        .as_ref()
                        .and_then(|value| value.get("order_kind"))
                        .and_then(serde_json::Value::as_str)
                        != Some("plan_purchase")
            })
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
            .filter(|order| {
                order.user_id.as_deref() == Some(user_id)
                    && order
                        .gateway_response
                        .as_ref()
                        .and_then(|value| value.get("order_kind"))
                        .and_then(serde_json::Value::as_str)
                        != Some("plan_purchase")
            })
            .cloned())
    }

    async fn find_wallet_recharge_order_by_order_no(
        &self,
        user_id: &str,
        order_no: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        Ok(self
            .payment_orders_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .find(|order| {
                order.user_id.as_deref() == Some(user_id)
                    && order.order_no == order_no
                    && order
                        .gateway_response
                        .as_ref()
                        .and_then(serde_json::Value::as_object)
                        .and_then(|object| object.get("order_kind"))
                        .and_then(serde_json::Value::as_str)
                        == Some("wallet_recharge")
            })
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

    async fn find_payment_order_by_order_no(
        &self,
        order_no: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        Ok(self
            .payment_orders_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .find(|order| order.order_no == order_no)
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
    async fn delete_wallet_if_unreferenced(
        &self,
        wallet_id: &str,
        owner: WalletLookupKey<'_>,
    ) -> Result<bool, DataLayerError> {
        if wallet_id.trim().is_empty() {
            return Ok(false);
        }
        let owner_matches = |wallet: &StoredWalletSnapshot| match owner {
            WalletLookupKey::UserId(user_id) => {
                !user_id.trim().is_empty()
                    && wallet.id == wallet_id
                    && wallet.user_id.as_deref() == Some(user_id)
                    && wallet.api_key_id.is_none()
            }
            WalletLookupKey::ApiKeyId(api_key_id) => {
                !api_key_id.trim().is_empty()
                    && wallet.id == wallet_id
                    && wallet.api_key_id.as_deref() == Some(api_key_id)
                    && wallet.user_id.is_none()
            }
            WalletLookupKey::WalletId(_) => false,
        };
        if matches!(owner, WalletLookupKey::WalletId(_)) {
            return Err(DataLayerError::InvalidInput(
                "wallet compensation requires an explicit user or API-key owner".to_string(),
            ));
        }
        let _lifecycle_guard = self
            .wallet_lifecycle_lock
            .lock()
            .expect("wallet lifecycle lock");

        let wallet = {
            let wallets = self.wallets_by_id.read().expect("wallet repo lock");
            wallets
                .values()
                .find(|wallet| owner_matches(wallet))
                .cloned()
        };
        let Some(wallet) = wallet else {
            return Ok(false);
        };

        // Compensation is only allowed for an untouched, freshly-created wallet.  A journal
        // entry can race with an existing wallet lookup, and deleting a zero-reference wallet
        // with persisted funds would otherwise destroy those funds.
        if !wallet_is_untouched_for_compensation(&wallet) {
            return Ok(false);
        }
        let referenced = self
            .payment_orders_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .any(|order| order.wallet_id == wallet.id)
            || self
                .refunds_by_id
                .read()
                .expect("wallet repo lock")
                .values()
                .any(|refund| refund.wallet_id == wallet.id)
            || self
                .wallet_transactions_by_id
                .read()
                .expect("wallet repo lock")
                .values()
                .any(|transaction| transaction.wallet_id == wallet.id)
            || self
                .redeem_codes_by_id
                .read()
                .expect("wallet repo lock")
                .values()
                .any(|code| code.redeemed_wallet_id.as_deref() == Some(wallet.id.as_str()));
        if referenced {
            return Ok(false);
        }

        let mut wallets = self.wallets_by_id.write().expect("wallet repo lock");
        let removable = wallets.get(&wallet.id).is_some_and(|current| {
            owner_matches(current)
                && current == &wallet
                && wallet_is_untouched_for_compensation(current)
        });
        if !removable {
            return Ok(false);
        }
        Ok(wallets.remove(&wallet.id).is_some())
    }

    async fn delete_wallet_if_snapshot_matches_and_unreferenced(
        &self,
        expected: &StoredWalletSnapshot,
        owner: WalletLookupKey<'_>,
    ) -> Result<bool, DataLayerError> {
        if expected.id.trim().is_empty() {
            return Ok(false);
        }
        let owner_matches = |wallet: &StoredWalletSnapshot| match owner {
            WalletLookupKey::UserId(user_id) => {
                !user_id.trim().is_empty()
                    && wallet.id == expected.id
                    && wallet.user_id.as_deref() == Some(user_id)
                    && wallet.api_key_id.is_none()
            }
            WalletLookupKey::ApiKeyId(api_key_id) => {
                !api_key_id.trim().is_empty()
                    && wallet.id == expected.id
                    && wallet.api_key_id.as_deref() == Some(api_key_id)
                    && wallet.user_id.is_none()
            }
            WalletLookupKey::WalletId(_) => false,
        };
        if matches!(owner, WalletLookupKey::WalletId(_)) {
            return Err(DataLayerError::InvalidInput(
                "wallet compensation requires an explicit user or API-key owner".to_string(),
            ));
        }
        let _lifecycle_guard = self
            .wallet_lifecycle_lock
            .lock()
            .expect("wallet lifecycle lock");

        let current = {
            let wallets = self.wallets_by_id.read().expect("wallet repo lock");
            wallets
                .get(&expected.id)
                .filter(|wallet| owner_matches(wallet))
                .cloned()
        };
        // Compare every field, including the owner and update timestamp.  A
        // mismatch means another operation touched the wallet, so compensation
        // must fail closed and preserve its funds.
        if current.as_ref() != Some(expected) {
            return Ok(false);
        }

        let referenced = self
            .payment_orders_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .any(|order| order.wallet_id == expected.id)
            || self
                .refunds_by_id
                .read()
                .expect("wallet repo lock")
                .values()
                .any(|refund| refund.wallet_id == expected.id)
            || self
                .wallet_transactions_by_id
                .read()
                .expect("wallet repo lock")
                .values()
                .any(|transaction| transaction.wallet_id == expected.id)
            || self
                .redeem_codes_by_id
                .read()
                .expect("wallet repo lock")
                .values()
                .any(|code| code.redeemed_wallet_id.as_deref() == Some(expected.id.as_str()));
        if referenced {
            return Ok(false);
        }

        let mut wallets = self.wallets_by_id.write().expect("wallet repo lock");
        if wallets
            .get(&expected.id)
            .is_some_and(|wallet| owner_matches(wallet) && wallet == expected)
        {
            return Ok(wallets.remove(&expected.id).is_some());
        }
        Ok(false)
    }

    async fn restore_wallet_if_snapshot_matches(
        &self,
        before: &StoredWalletSnapshot,
        after: &StoredWalletSnapshot,
        owner: WalletLookupKey<'_>,
    ) -> Result<bool, DataLayerError> {
        if before.id.trim().is_empty() || after.id.trim().is_empty() {
            return Ok(false);
        }
        if before.id != after.id {
            return Err(DataLayerError::InvalidInput(
                "wallet restore snapshots must reference the same wallet".to_string(),
            ));
        }
        let owner_matches = |wallet: &StoredWalletSnapshot| match owner {
            WalletLookupKey::UserId(user_id) => {
                !user_id.trim().is_empty()
                    && wallet.user_id.as_deref() == Some(user_id)
                    && wallet.api_key_id.is_none()
            }
            WalletLookupKey::ApiKeyId(api_key_id) => {
                !api_key_id.trim().is_empty()
                    && wallet.api_key_id.as_deref() == Some(api_key_id)
                    && wallet.user_id.is_none()
            }
            WalletLookupKey::WalletId(_) => false,
        };
        if matches!(owner, WalletLookupKey::WalletId(_)) {
            return Err(DataLayerError::InvalidInput(
                "wallet restore requires an explicit user or API-key owner".to_string(),
            ));
        }

        // Keep the compare and replacement atomic with the lifecycle operations. The map write
        // lock also prevents an ordinary wallet mutation from interleaving between the compare
        // and restore; a changed snapshot therefore fails closed instead of being overwritten.
        let _lifecycle_guard = self
            .wallet_lifecycle_lock
            .lock()
            .expect("wallet lifecycle lock");
        let mut wallets = self.wallets_by_id.write().expect("wallet repo lock");
        let Some(current) = wallets.get(&after.id) else {
            return Ok(false);
        };
        if current != after || !owner_matches(current) || !owner_matches(before) {
            return Ok(false);
        }
        wallets.insert(before.id.clone(), before.clone());
        Ok(true)
    }

    async fn delete_provisional_auth_user_wallet(
        &self,
        wallet_id: &str,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        if wallet_id.trim().is_empty() || user_id.trim().is_empty() {
            return Ok(false);
        }
        let _lifecycle_guard = self
            .wallet_lifecycle_lock
            .lock()
            .expect("wallet lifecycle lock");

        // Provisioning rollback is deliberately fail-closed. The only
        // transaction that may exist is the deterministic initial gift entry;
        // any other financial artifact makes the wallet ineligible for purge.
        let wallet = {
            let wallets = self.wallets_by_id.read().expect("wallet repo lock");
            wallets
                .values()
                .find(|wallet| {
                    wallet.id == wallet_id
                        && wallet.user_id.as_deref() == Some(user_id)
                        && wallet.api_key_id.is_none()
                        && wallet.balance == 0.0
                        && wallet.total_recharged == 0.0
                        && wallet.total_consumed == 0.0
                        && wallet.total_refunded == 0.0
                        && wallet.total_adjusted == wallet.gift_balance
                        && wallet.gift_balance >= 0.0
                        && wallet.status == "active"
                        && matches!(wallet.limit_mode.as_str(), "finite" | "unlimited")
                        && wallet.currency == "USD"
                })
                .cloned()
        };
        let Some(wallet) = wallet else {
            return Ok(false);
        };

        let transactions = self
            .wallet_transactions_by_id
            .read()
            .expect("wallet repo lock");
        let transaction_matches =
            provisional_auth_wallet_transactions_match(&transactions, &wallet, user_id);
        drop(transactions);
        if !transaction_matches {
            return Ok(false);
        }

        if self
            .payment_orders_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .any(|order| order.wallet_id == wallet.id)
            || self
                .refunds_by_id
                .read()
                .expect("wallet repo lock")
                .values()
                .any(|refund| refund.wallet_id == wallet.id)
            || self
                .redeem_codes_by_id
                .read()
                .expect("wallet repo lock")
                .values()
                .any(|code| code.redeemed_wallet_id.as_deref() == Some(wallet.id.as_str()))
        {
            return Ok(false);
        }

        // Snapshot updates do not take `wallet_lifecycle_lock`; hold the
        // wallet write lock through the final compare and removal so a credit
        // cannot land after the check but before deletion. Re-check the
        // transaction set under its write lock as well, since a concurrent
        // financial entry must make this compensation fail closed.
        let mut wallets = self.wallets_by_id.write().expect("wallet repo lock");
        if wallets.get(&wallet.id) != Some(&wallet) {
            return Ok(false);
        }
        let mut transactions = self
            .wallet_transactions_by_id
            .write()
            .expect("wallet repo lock");
        if !provisional_auth_wallet_transactions_match(&transactions, &wallet, user_id) {
            return Ok(false);
        }
        transactions.retain(|_, transaction| transaction.wallet_id != wallet.id);
        Ok(wallets.remove(&wallet.id).is_some())
    }

    async fn create_wallet_recharge_order(
        &self,
        mut input: CreateWalletRechargeOrderInput,
    ) -> Result<CreateWalletRechargeOrderOutcome, DataLayerError> {
        input.payment_method = canonicalize_payment_method(&input.payment_method)
            .map_err(DataLayerError::InvalidInput)?;
        validate_wallet_recharge_order_input(&input).map_err(DataLayerError::InvalidInput)?;
        if !input.amount_usd.is_finite()
            || input.amount_usd <= 0.0
            || input
                .pay_amount
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
            || input
                .exchange_rate
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
            || input.expires_at_unix_secs > i64::MAX as u64
        {
            return Err(DataLayerError::InvalidInput(
                "invalid wallet recharge numeric fields".to_string(),
            ));
        }
        let mut gateway_response =
            project_wallet_recharge_gateway_response(&input.gateway_response)
                .map_err(DataLayerError::InvalidInput)?;
        if !gateway_response.is_object() {
            return Err(DataLayerError::InvalidInput(
                "wallet recharge gateway response must be an object".to_string(),
            ));
        }
        let gateway_object = gateway_response
            .as_object_mut()
            .expect("validated wallet recharge gateway response");
        if let Some(provider) = input.payment_provider.as_deref() {
            gateway_object.insert(
                "payment_provider".to_string(),
                serde_json::Value::String(provider.trim().to_ascii_lowercase()),
            );
        }
        if let Some(channel) = input.payment_channel.as_deref() {
            gateway_object.insert(
                "payment_channel".to_string(),
                serde_json::Value::String(channel.trim().to_ascii_lowercase()),
            );
        }
        let _lifecycle_guard = self
            .wallet_lifecycle_lock
            .lock()
            .expect("wallet lifecycle lock");
        let now_secs = current_unix_secs();
        // Keep the wallet and payment-order locks in separate scopes.  The
        // repository stores them in independent maps, and holding one while
        // acquiring the other can deadlock with refund/order readers.
        let (wallet_id, created_wallet) = {
            let mut wallets = self.wallets_by_id.write().expect("wallet repo lock");
            let existing_wallet = wallets
                .values()
                .find(|wallet| wallet.user_id.as_deref() == Some(input.user_id.as_str()))
                .map(|wallet| (wallet.id.clone(), wallet.status.clone()));
            if existing_wallet
                .as_ref()
                .is_some_and(|(_, status)| status != "active")
            {
                return Ok(CreateWalletRechargeOrderOutcome::WalletInactive);
            }

            match existing_wallet {
                Some((wallet_id, _)) => (wallet_id, false),
                None => {
                    let wallet_id = input
                        .preferred_wallet_id
                        .clone()
                        .unwrap_or_else(|| format!("wallet-{}", uuid::Uuid::new_v4()));
                    if wallets.contains_key(&wallet_id) {
                        return Err(DataLayerError::InvalidInput(
                            "wallet identifier already belongs to another owner".to_string(),
                        ));
                    }
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
                    (wallet_id, true)
                }
            }
        };

        let replay_input = input.clone();
        let order = StoredAdminPaymentOrder {
            id: format!("payment-order-{}", uuid::Uuid::new_v4()),
            order_no: input.order_no,
            wallet_id: wallet_id.clone(),
            user_id: Some(input.user_id),
            amount_usd: input.amount_usd,
            pay_amount: input.pay_amount,
            pay_currency: input.pay_currency,
            exchange_rate: input.exchange_rate,
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            payment_method: input.payment_method,
            payment_provider: input.payment_provider,
            order_kind: "wallet_recharge".to_string(),
            gateway_order_id: Some(input.gateway_order_id),
            gateway_response: Some(gateway_response),
            status: "pending".to_string(),
            created_at_unix_ms: current_unix_ms(),
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: Some(input.expires_at_unix_secs),
        };
        match self.insert_wallet_recharge_order_unique(order.clone(), now_secs, &replay_input) {
            Ok((Some(existing), true)) => {
                if created_wallet {
                    self.remove_created_wallet_if_unreferenced(&wallet_id);
                }
                return Ok(CreateWalletRechargeOrderOutcome::Created(existing));
            }
            Ok((Some(existing), false)) => {
                if created_wallet {
                    self.remove_created_wallet_if_unreferenced(&wallet_id);
                }
                return Ok(CreateWalletRechargeOrderOutcome::Existing(existing));
            }
            Ok((None, false)) => {}
            Ok((None, true)) => {
                if created_wallet {
                    self.remove_created_wallet_if_unreferenced(&wallet_id);
                }
                return Err(DataLayerError::InvalidInput(
                    "reclaimed recharge order disappeared".to_string(),
                ));
            }
            Err(error) => {
                if created_wallet {
                    self.remove_created_wallet_if_unreferenced(&wallet_id);
                }
                return Err(error);
            }
        }
        Ok(CreateWalletRechargeOrderOutcome::Created(order))
    }

    async fn update_wallet_recharge_checkout(
        &self,
        input: UpdateWalletRechargeCheckoutInput,
    ) -> Result<WalletMutationOutcome<StoredAdminPaymentOrder>, DataLayerError> {
        if input.order_id.trim().is_empty() || input.gateway_order_id.trim().is_empty() {
            return Ok(WalletMutationOutcome::Invalid(
                "wallet recharge checkout identifiers are required".to_string(),
            ));
        }
        let gateway_response =
            match project_wallet_recharge_gateway_response(&input.gateway_response) {
                Ok(value) => value,
                Err(error) => return Ok(WalletMutationOutcome::Invalid(error)),
            };
        let mut orders = self.payment_orders_by_id.write().expect("wallet repo lock");
        let Some(current_order) = orders.get(&input.order_id) else {
            return Ok(WalletMutationOutcome::NotFound);
        };
        let is_wallet_recharge = current_order
            .gateway_response
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .and_then(|object| object.get("order_kind"))
            .and_then(serde_json::Value::as_str)
            == Some("wallet_recharge");
        if !is_wallet_recharge {
            return Ok(WalletMutationOutcome::Invalid(
                "payment order is not a wallet recharge".to_string(),
            ));
        }
        let current_is_checkout_placeholder =
            wallet_recharge_order_is_checkout_placeholder(current_order);
        let current_token = current_order
            .gateway_response
            .as_ref()
            .and_then(wallet_recharge_checkout_claim_token);
        let requested_token = wallet_recharge_checkout_claim_token(&gateway_response);
        if current_token.is_some() && current_token != requested_token {
            return Ok(WalletMutationOutcome::Invalid(
                "wallet recharge checkout claim is no longer current".to_string(),
            ));
        }
        if current_order.status != "pending" {
            if current_order.gateway_order_id.as_deref() == Some(input.gateway_order_id.as_str()) {
                return Ok(WalletMutationOutcome::Applied(current_order.clone()));
            }
            return Ok(WalletMutationOutcome::Invalid(
                "wallet recharge order is no longer pending".to_string(),
            ));
        }
        let now_secs = current_unix_secs();
        if current_order
            .expires_at_unix_secs
            .is_none_or(|expires_at| expires_at <= now_secs)
        {
            return Ok(WalletMutationOutcome::Invalid(
                "wallet recharge order is expired".to_string(),
            ));
        }
        // The initial row stores the order number as a temporary gateway id.
        // Once a provider checkout is persisted, a concurrent creator must
        // not overwrite that evidence with a second provider checkout.
        if current_order
            .gateway_order_id
            .as_deref()
            .is_some_and(|existing| {
                existing != input.gateway_order_id.as_str()
                    && existing != current_order.order_no.as_str()
                    && !current_is_checkout_placeholder
            })
        {
            return Ok(WalletMutationOutcome::Invalid(
                "wallet recharge checkout is already bound".to_string(),
            ));
        }
        let payment_method = current_order.payment_method.clone();
        if orders.values().any(|existing| {
            existing.id != input.order_id
                && existing.payment_method == payment_method
                && existing.gateway_order_id.as_deref() == Some(input.gateway_order_id.as_str())
        }) {
            return Ok(WalletMutationOutcome::Invalid(
                "payment gateway order already belongs to another order".to_string(),
            ));
        }
        let order = orders
            .get_mut(&input.order_id)
            .expect("wallet recharge order disappeared while write lock held");
        order.gateway_order_id = Some(input.gateway_order_id);
        order.gateway_response = Some(gateway_response);
        Ok(WalletMutationOutcome::Applied(order.clone()))
    }

    async fn compare_and_swap_payment_order_stripe_client_secret(
        &self,
        input: CompareAndSwapPaymentOrderStripeClientSecretInput,
    ) -> Result<bool, DataLayerError> {
        let mut orders = self.payment_orders_by_id.write().expect("wallet repo lock");
        let Some(current) = orders.get(&input.order_id) else {
            return Ok(false);
        };
        let Some(replacement) = payment_order_stripe_client_secret_cas_replacement(current, &input)
            .map_err(DataLayerError::InvalidInput)?
        else {
            return Ok(false);
        };
        let current = orders
            .get_mut(&input.order_id)
            .expect("payment order disappeared while write lock held");
        current.gateway_response = Some(replacement);
        Ok(true)
    }

    async fn fail_wallet_recharge_checkout(
        &self,
        input: FailWalletRechargeCheckoutInput,
    ) -> Result<WalletMutationOutcome<StoredAdminPaymentOrder>, DataLayerError> {
        if input.order_id.trim().is_empty()
            || input.claim_token.trim().is_empty()
            || input.claim_token.len() > 128
        {
            return Ok(WalletMutationOutcome::Invalid(
                "wallet recharge checkout failure identifiers are required".to_string(),
            ));
        }
        let mut orders = self.payment_orders_by_id.write().expect("wallet repo lock");
        let Some(order) = orders.get_mut(&input.order_id) else {
            return Ok(WalletMutationOutcome::NotFound);
        };
        if !wallet_recharge_order_is_checkout_placeholder(order) {
            return Ok(WalletMutationOutcome::Invalid(
                "payment order is not a checkout placeholder".to_string(),
            ));
        }
        let current_token = order
            .gateway_response
            .as_ref()
            .and_then(wallet_recharge_checkout_claim_token);
        if current_token != Some(input.claim_token.trim()) {
            return Ok(WalletMutationOutcome::Invalid(
                "wallet recharge checkout claim is no longer current".to_string(),
            ));
        }
        if order.status != "pending" {
            return Ok(WalletMutationOutcome::Applied(order.clone()));
        }
        let failed = if input.provider_request_may_have_succeeded {
            wallet_recharge_checkout_uncertain_response(
                order.gateway_response.as_ref(),
                &input.reason,
                current_unix_secs(),
            )
        } else {
            wallet_recharge_checkout_failed_response(
                order.gateway_response.as_ref(),
                &input.reason,
                current_unix_secs(),
            )
        };
        order.gateway_response = Some(failed);
        order.status = "failed".to_string();
        Ok(WalletMutationOutcome::Applied(order.clone()))
    }

    async fn reclaim_wallet_recharge_checkout(
        &self,
        input: ReclaimWalletRechargeCheckoutInput,
    ) -> Result<WalletMutationOutcome<StoredAdminPaymentOrder>, DataLayerError> {
        if input.order_id.trim().is_empty()
            || input.claim_token.trim().is_empty()
            || input.claim_token.len() > 128
            || input.expires_at_unix_secs <= current_unix_secs()
        {
            return Ok(WalletMutationOutcome::Invalid(
                "wallet recharge checkout reclaim identifiers are invalid".to_string(),
            ));
        }
        // Keep the in-memory backend aligned with SQL backends: a reclaim may
        // only install a server-created placeholder, never provider checkout
        // evidence supplied by an internal caller.
        if !wallet_recharge_response_is_checkout_placeholder(&input.gateway_response) {
            return Ok(WalletMutationOutcome::Invalid(
                "wallet recharge reclaim response must be a placeholder".to_string(),
            ));
        }
        let now = current_unix_secs();
        let response = wallet_recharge_checkout_claim_response(
            &input.gateway_response,
            &input.claim_token,
            now,
        )
        .map_err(DataLayerError::InvalidInput)?;
        let mut orders = self.payment_orders_by_id.write().expect("wallet repo lock");
        let Some(order) = orders.get_mut(&input.order_id) else {
            return Ok(WalletMutationOutcome::NotFound);
        };
        if !wallet_recharge_order_is_reclaimable_placeholder(order, now) {
            return Ok(WalletMutationOutcome::Invalid(
                "wallet recharge checkout is still in progress or already completed".to_string(),
            ));
        }
        order.gateway_response = Some(response);
        order.gateway_order_id = Some(order.order_no.clone());
        order.status = "pending".to_string();
        order.expires_at_unix_secs = Some(input.expires_at_unix_secs);
        Ok(WalletMutationOutcome::Applied(order.clone()))
    }

    async fn create_plan_purchase_order(
        &self,
        mut input: CreatePlanPurchaseOrderInput,
    ) -> Result<CreatePlanPurchaseOrderOutcome, DataLayerError> {
        input.payment_method = canonicalize_payment_method(&input.payment_method)
            .map_err(DataLayerError::InvalidInput)?;
        validate_plan_purchase_order_input(&input).map_err(DataLayerError::InvalidInput)?;
        let projected_gateway_response = project_wallet_gateway_response(&input.gateway_response)
            .map_err(DataLayerError::InvalidInput)?;
        let entitlements = input
            .product_snapshot
            .get("entitlements")
            .or_else(|| input.product_snapshot.get("entitlements_json"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        validate_plan_wallet_credit_entitlements(&entitlements)
            .map_err(DataLayerError::InvalidInput)?;
        let _lifecycle_guard = self
            .wallet_lifecycle_lock
            .lock()
            .expect("wallet lifecycle lock");
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
        let mut gateway_response = match projected_gateway_response {
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
            payment_provider: input.payment_provider,
            order_kind: "plan_purchase".to_string(),
            gateway_order_id: Some(input.gateway_order_id),
            gateway_response: Some(serde_json::Value::Object(gateway_response)),
            status: "pending".to_string(),
            created_at_unix_ms: current_unix_ms(),
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: Some(input.expires_at_unix_secs),
        };
        self.insert_payment_order_unique(order.clone())?;
        Ok(CreatePlanPurchaseOrderOutcome::Created(order))
    }

    async fn create_wallet_refund_request(
        &self,
        input: CreateWalletRefundRequestInput,
    ) -> Result<CreateWalletRefundRequestOutcome, DataLayerError> {
        if !input.amount_usd.is_finite() || input.amount_usd <= 0.0 {
            return Ok(CreateWalletRefundRequestOutcome::InvalidInput(
                "refund amount must be finite and greater than zero".to_string(),
            ));
        }
        if input
            .idempotency_key
            .as_deref()
            .is_some_and(|key| key.trim().is_empty() || key.chars().count() > 128)
        {
            return Ok(CreateWalletRefundRequestOutcome::InvalidInput(
                "refund idempotency key is invalid".to_string(),
            ));
        }
        // SQL backends lock the wallet row while reserving a refund. Serialize
        // the in-memory equivalent so concurrent requests cannot both spend
        // the same available balance or race compensation cleanup. Keep both
        // locks in this order everywhere: lifecycle first, reservation second.
        let _lifecycle_guard = self
            .wallet_lifecycle_lock
            .lock()
            .expect("wallet lifecycle lock");
        let _reservation_guard = self
            .refund_creation_lock
            .lock()
            .expect("wallet refund creation lock");
        let wallet = {
            let wallets = self.wallets_by_id.read().expect("wallet repo lock");
            let Some(wallet) = wallets.get(&input.wallet_id) else {
                return Ok(CreateWalletRefundRequestOutcome::WalletMissing);
            };
            if wallet.user_id.as_deref() != Some(input.user_id.as_str()) {
                return Ok(CreateWalletRefundRequestOutcome::WalletMissing);
            }
            wallet.clone()
        };
        if !wallet.balance.is_finite() {
            return Ok(CreateWalletRefundRequestOutcome::InvalidInput(
                "wallet recharge balance is invalid".to_string(),
            ));
        }

        if let Some(idempotency_key) = input.idempotency_key.as_deref() {
            let key = (input.user_id.clone(), idempotency_key.to_string());
            if let Some(refund_id) = self
                .refund_idempotency_to_id
                .read()
                .expect("wallet repo lock")
                .get(&key)
                .cloned()
            {
                if let Some(refund) = self
                    .refunds_by_id
                    .read()
                    .expect("wallet repo lock")
                    .get(&refund_id)
                    .cloned()
                {
                    return Ok(CreateWalletRefundRequestOutcome::Duplicate(refund));
                }
                return Ok(CreateWalletRefundRequestOutcome::DuplicateRejected);
            }
        }

        let reserved_amount = self
            .refunds_by_id
            .read()
            .expect("wallet repo lock")
            .values()
            .filter(|refund| {
                refund.wallet_id == input.wallet_id
                    && matches!(refund.status.as_str(), "pending_approval" | "approved")
            })
            .try_fold(0.0_f64, |total, refund| {
                let amount = refund.amount_usd;
                if !amount.is_finite() || amount <= 0.0 {
                    return None;
                }
                let next = total + amount;
                next.is_finite().then_some(next)
            });
        let Some(reserved_amount) = reserved_amount else {
            return Ok(CreateWalletRefundRequestOutcome::InvalidInput(
                "wallet refund reservation is invalid".to_string(),
            ));
        };
        let available_balance = wallet.balance - reserved_amount;
        if !available_balance.is_finite() || input.amount_usd > available_balance {
            return Ok(CreateWalletRefundRequestOutcome::RefundAmountExceedsAvailableBalance);
        }

        let mut resolved_payment_method: Option<String> = None;
        if let Some(order_id) = input.payment_order_id.as_deref() {
            let order = {
                let orders = self.payment_orders_by_id.read().expect("wallet repo lock");
                let Some(order) = orders.get(order_id) else {
                    return Ok(CreateWalletRefundRequestOutcome::PaymentOrderNotFound);
                };
                if order.wallet_id != input.wallet_id || order.status != "credited" {
                    return Ok(CreateWalletRefundRequestOutcome::PaymentOrderNotRefundable);
                }
                order.clone()
            };
            let reserved_for_order = self
                .refunds_by_id
                .read()
                .expect("wallet repo lock")
                .values()
                .filter(|refund| {
                    refund.payment_order_id.as_deref() == Some(order_id)
                        && matches!(refund.status.as_str(), "pending_approval" | "approved")
                })
                .try_fold(0.0_f64, |total, refund| {
                    let amount = refund.amount_usd;
                    if !amount.is_finite() || amount <= 0.0 {
                        return None;
                    }
                    let next = total + amount;
                    next.is_finite().then_some(next)
                });
            if !payment_order_refund_amounts_are_consistent(
                order.amount_usd,
                order.refunded_amount_usd,
                order.refundable_amount_usd,
            ) {
                return Ok(CreateWalletRefundRequestOutcome::InvalidInput(
                    "payment order refund amounts are invalid".to_string(),
                ));
            }
            let Some(reserved_for_order) = reserved_for_order else {
                return Ok(CreateWalletRefundRequestOutcome::InvalidInput(
                    "payment order refund reservation is invalid".to_string(),
                ));
            };
            let available_order_amount = order.refundable_amount_usd - reserved_for_order;
            if !available_order_amount.is_finite() || input.amount_usd > available_order_amount {
                return Ok(
                    CreateWalletRefundRequestOutcome::RefundAmountExceedsAvailableOrderAmount,
                );
            }
            resolved_payment_method = Some(order.payment_method.clone());
        }

        let canonical = canonicalize_wallet_refund_fields(
            input.payment_order_id.as_deref(),
            input.source_type.as_deref(),
            input.source_id.as_deref(),
            input.refund_mode.as_deref(),
            resolved_payment_method.as_deref(),
        )
        .map_err(DataLayerError::InvalidInput)?;
        let idempotency_key = input.idempotency_key.clone();
        let user_id = input.user_id.clone();

        let refund = StoredAdminWalletRefund {
            id: format!("refund-{}", uuid::Uuid::new_v4()),
            refund_no: input.refund_no,
            wallet_id: input.wallet_id,
            user_id: Some(input.user_id),
            payment_order_id: input.payment_order_id.clone(),
            source_type: canonical.source_type,
            source_id: canonical.source_id,
            refund_mode: canonical.refund_mode,
            amount_usd: input.amount_usd,
            status: "pending_approval".to_string(),
            reason: input.reason,
            failure_reason: None,
            gateway_refund_id: None,
            payout_method: None,
            payout_reference: None,
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
        if let Some(idempotency_key) = idempotency_key {
            self.refund_idempotency_to_id
                .write()
                .expect("wallet repo lock")
                .insert((user_id, idempotency_key), refund.id.clone());
        }
        Ok(CreateWalletRefundRequestOutcome::Created(refund))
    }

    async fn process_payment_callback(
        &self,
        mut input: ProcessPaymentCallbackInput,
    ) -> Result<ProcessPaymentCallbackOutcome, DataLayerError> {
        input
            .canonicalize_and_validate()
            .map_err(DataLayerError::InvalidInput)?;
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

    async fn update_admin_wallet_refund_gateway(
        &self,
        input: UpdateAdminWalletRefundGatewayInput,
    ) -> Result<WalletMutationOutcome<super::StoredAdminWalletRefund>, DataLayerError> {
        if input.gateway_refund_id.trim().is_empty() || input.gateway_refund_id.len() > 128 {
            return Ok(WalletMutationOutcome::Invalid(
                "gateway refund identifier is invalid".to_string(),
            ));
        }
        if input
            .payout_proof
            .as_ref()
            .is_some_and(|proof| !proof.is_object())
        {
            return Ok(WalletMutationOutcome::Invalid(
                "gateway refund proof must be an object".to_string(),
            ));
        }
        let mut refunds = self.refunds_by_id.write().expect("wallet repo lock");
        let Some(refund) = refunds.get_mut(&input.refund_id) else {
            return Ok(WalletMutationOutcome::NotFound);
        };
        if refund.wallet_id != input.wallet_id {
            return Ok(WalletMutationOutcome::NotFound);
        }
        if !refund.amount_usd.is_finite() || refund.amount_usd <= 0.0 {
            return Ok(WalletMutationOutcome::Invalid(
                "refund amount must be finite and greater than zero".to_string(),
            ));
        }
        if let Some(existing_id) = refund.gateway_refund_id.as_deref() {
            if existing_id != input.gateway_refund_id {
                return Ok(WalletMutationOutcome::Invalid(
                    "gateway refund identifier conflicts with existing evidence".to_string(),
                ));
            }
        }
        if refund.status == "succeeded" {
            return Ok(WalletMutationOutcome::Applied(refund.clone()));
        }
        if refund.status != "processing" {
            return Ok(WalletMutationOutcome::Invalid(
                "refund status must be processing before gateway update".to_string(),
            ));
        }
        if refund.gateway_refund_id.is_none() {
            refund.gateway_refund_id = Some(input.gateway_refund_id);
        }
        // Preserve a processing proof for ordinary replays, but allow an
        // explicit successful gateway proof to upgrade it.
        if refund.payout_proof.is_none()
            || input
                .payout_proof
                .as_ref()
                .is_some_and(wallet_refund_proof_is_success)
        {
            refund.payout_proof = input.payout_proof;
        }
        refund.updated_at_unix_secs = current_unix_secs();
        Ok(WalletMutationOutcome::Applied(refund.clone()))
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
        validate_admin_redeem_code_batch_input(&input).map_err(DataLayerError::InvalidInput)?;
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
        let _lifecycle_guard = self
            .wallet_lifecycle_lock
            .lock()
            .expect("wallet lifecycle lock");
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
        let (wallet, balance_before, gift_before) = {
            let wallets = self.wallets_by_id.read().expect("wallet repo lock");
            if let Some(wallet) = wallets
                .values()
                .find(|wallet| wallet.user_id.as_deref() == Some(input.user_id.as_str()))
            {
                if wallet.status != "active" {
                    return Ok(RedeemWalletCodeOutcome::WalletInactive);
                }
                let balance_before = wallet.balance;
                let gift_before = wallet.gift_balance;
                let (after_recharge, after_gift, after_total_recharged) =
                    validate_redeem_wallet_credit(
                        &balance_bucket,
                        amount_usd,
                        balance_before,
                        gift_before,
                        wallet.total_recharged,
                    )
                    .map_err(DataLayerError::UnexpectedValue)?;
                let mut wallet = wallet.clone();
                wallet.balance = after_recharge;
                wallet.gift_balance = after_gift;
                wallet.total_recharged = after_total_recharged;
                wallet.updated_at_unix_secs = now_secs;
                (wallet, balance_before, gift_before)
            } else {
                let (after_recharge, after_gift, after_total_recharged) =
                    validate_redeem_wallet_credit(&balance_bucket, amount_usd, 0.0, 0.0, 0.0)
                        .map_err(DataLayerError::UnexpectedValue)?;
                let wallet = StoredWalletSnapshot::new(
                    format!("wallet-{}", uuid::Uuid::new_v4()),
                    Some(input.user_id.clone()),
                    None,
                    after_recharge,
                    after_gift,
                    "finite".to_string(),
                    "USD".to_string(),
                    "active".to_string(),
                    after_total_recharged,
                    0.0,
                    0.0,
                    0.0,
                    now_secs as i64,
                )?;
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
            payment_provider: Some("redeem_code".to_string()),
            order_kind: "wallet_recharge".to_string(),
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
        // Reserve the globally unique payment identity before publishing any
        // wallet mutation. Every operation after this point is infallible map
        // replacement, so a duplicate order cannot leave credited funds.
        self.insert_payment_order_unique(order.clone())?;
        self.wallets_by_id
            .write()
            .expect("wallet repo lock")
            .insert(wallet.id.clone(), wallet.clone());

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
            batch.redeemed_count = batch.redeemed_count.saturating_add(1);
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
        AdminWalletListQuery, CompareAndSwapPaymentOrderStripeClientSecretInput,
        CreatePlanPurchaseOrderInput, CreatePlanPurchaseOrderOutcome,
        CreateWalletRechargeOrderInput, CreateWalletRechargeOrderOutcome,
        CreateWalletRefundRequestInput, CreateWalletRefundRequestOutcome,
        FailWalletRechargeCheckoutInput, StoredAdminPaymentOrder, StoredAdminWalletRefund,
        StoredWalletSnapshot, UpdateWalletRechargeCheckoutInput, WalletLookupKey,
        WalletMutationOutcome, WalletReadRepository, WalletWriteRepository,
    };
    use crate::DataLayerError;
    use serde_json::json;
    use std::sync::Arc;

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
    async fn compensation_delete_preserves_wallet_with_persisted_balance_in_memory() {
        let wallet = StoredWalletSnapshot::new(
            "funded-wallet".to_string(),
            Some("funded-user".to_string()),
            None,
            1.0,
            0.0,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            1.0,
            0.0,
            0.0,
            1.0,
            100,
        )
        .expect("wallet should build");
        let repository = InMemoryWalletRepository::seed(vec![wallet]);

        assert!(!repository
            .delete_wallet_if_unreferenced("funded-wallet", WalletLookupKey::UserId("funded-user"))
            .await
            .expect("funded wallet must not be deleted"));
        assert!(repository
            .find(WalletLookupKey::UserId("funded-user"))
            .await
            .expect("wallet lookup should succeed")
            .is_some());
    }

    #[tokio::test]
    async fn provisional_recharge_cleanup_preserves_wallet_changed_after_initial_check_in_memory() {
        let wallet = StoredWalletSnapshot::new(
            "provisional-recharge-wallet".to_string(),
            Some("provisional-recharge-user".to_string()),
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
        let repository = InMemoryWalletRepository::seed(vec![wallet]);
        repository.with_wallets_mut(|wallets| {
            let wallet = wallets
                .get_mut("provisional-recharge-wallet")
                .expect("wallet should be seeded");
            wallet.balance = 2.0;
            wallet.total_recharged = 2.0;
            wallet.updated_at_unix_secs = 200;
        });

        // This is the same cleanup helper used when a recharge order loses a
        // uniqueness race after creating a provisional wallet. A wallet that
        // acquired funds must survive the compensation attempt.
        repository.remove_created_wallet_if_unreferenced("provisional-recharge-wallet");
        let retained = repository
            .find(WalletLookupKey::UserId("provisional-recharge-user"))
            .await
            .expect("wallet lookup should succeed")
            .expect("changed wallet should remain");
        assert_eq!(retained.balance, 2.0);
        assert_eq!(retained.total_recharged, 2.0);
    }

    #[tokio::test]
    async fn snapshot_compensation_deletes_matching_funded_wallet_and_preserves_changed_one() {
        let wallet = StoredWalletSnapshot::new(
            "import-funded-wallet".to_string(),
            Some("import-funded-user".to_string()),
            None,
            12.5,
            3.25,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            12.5,
            0.0,
            0.0,
            0.0,
            4242,
        )
        .expect("wallet should build");
        let repository = InMemoryWalletRepository::seed(vec![wallet.clone()]);
        assert!(repository
            .delete_wallet_if_snapshot_matches_and_unreferenced(
                &wallet,
                WalletLookupKey::UserId("import-funded-user"),
            )
            .await
            .expect("matching snapshot should delete"));
        assert!(repository
            .find(WalletLookupKey::WalletId("import-funded-wallet"))
            .await
            .expect("wallet lookup should succeed")
            .is_none());

        let repository = InMemoryWalletRepository::seed(vec![wallet.clone()]);
        repository.with_wallets_mut(|wallets| {
            wallets
                .get_mut("import-funded-wallet")
                .expect("wallet should be seeded")
                .balance = 99.0;
        });
        assert!(!repository
            .delete_wallet_if_snapshot_matches_and_unreferenced(
                &wallet,
                WalletLookupKey::UserId("import-funded-user"),
            )
            .await
            .expect("changed snapshot should be retained"));
        assert!(repository
            .find(WalletLookupKey::WalletId("import-funded-wallet"))
            .await
            .expect("wallet lookup should succeed")
            .is_some());
    }

    #[tokio::test]
    async fn snapshot_restore_is_compare_and_swap_in_memory() {
        let before = StoredWalletSnapshot::new(
            "existing-wallet".to_string(),
            Some("existing-user".to_string()),
            None,
            4.0,
            2.0,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            7.0,
            3.0,
            0.0,
            0.0,
            100,
        )
        .expect("wallet should build");
        let mut after = before.clone();
        after.balance = 25.0;
        after.gift_balance = 5.0;
        after.total_recharged = 28.0;
        after.updated_at_unix_secs = 200;

        let repository = InMemoryWalletRepository::seed(vec![after.clone()]);
        assert!(repository
            .restore_wallet_if_snapshot_matches(
                &before,
                &after,
                WalletLookupKey::UserId("existing-user"),
            )
            .await
            .expect("matching post-state should restore"));
        assert_eq!(
            repository
                .find(WalletLookupKey::UserId("existing-user"))
                .await
                .expect("wallet lookup should succeed"),
            Some(before.clone())
        );

        let repository = InMemoryWalletRepository::seed(vec![after.clone()]);
        repository.with_wallets_mut(|wallets| {
            let wallet = wallets
                .get_mut("existing-wallet")
                .expect("wallet should be seeded");
            wallet.balance = 99.0;
            wallet.updated_at_unix_secs = 300;
        });
        assert!(!repository
            .restore_wallet_if_snapshot_matches(
                &before,
                &after,
                WalletLookupKey::UserId("existing-user"),
            )
            .await
            .expect("changed post-state should fail closed"));
        let retained = repository
            .find(WalletLookupKey::UserId("existing-user"))
            .await
            .expect("wallet lookup should succeed")
            .expect("changed wallet should remain");
        assert_eq!(retained.balance, 99.0);
        assert_eq!(retained.updated_at_unix_secs, 300);
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
            payment_provider: Some("stripe".to_string()),
            order_kind: "wallet_recharge".to_string(),
            gateway_order_id: None,
            gateway_response: None,
            status: status.to_string(),
            created_at_unix_ms: 100,
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: None,
        }
    }

    fn stripe_secret_cas_input(
        order: &StoredAdminPaymentOrder,
        expected_gateway_response: serde_json::Value,
        expected_ciphertext: &str,
        replacement_ciphertext: &str,
    ) -> CompareAndSwapPaymentOrderStripeClientSecretInput {
        CompareAndSwapPaymentOrderStripeClientSecretInput {
            order_id: order.id.clone(),
            order_no: order.order_no.clone(),
            wallet_id: order.wallet_id.clone(),
            user_id: order.user_id.clone(),
            payment_method: order.payment_method.clone(),
            payment_provider: order.payment_provider.clone(),
            order_kind: order.order_kind.clone(),
            gateway_order_id: order.gateway_order_id.clone(),
            expected_status: order.status.clone(),
            expected_expires_at_unix_secs: order.expires_at_unix_secs,
            expected_gateway_response,
            expected_client_secret_encrypted: expected_ciphertext.to_string(),
            replacement_client_secret_encrypted: replacement_ciphertext.to_string(),
        }
    }

    #[tokio::test]
    async fn stripe_secret_cas_is_exact_and_never_overwrites_a_newer_value_in_memory() {
        let legacy = "gAAAAABlegacy";
        let replacement = concat!(
            "aether-payment-order-stripe-client-secret-v2:",
            "aether-runtime-secret-v1:gAAAAABreplacement"
        );
        let mut order = sample_payment_order("stripe-cas-order", Some("user-1"), "pending");
        order.gateway_order_id = Some("pi-cas".to_string());
        order.expires_at_unix_secs = Some(4_102_444_800);
        order.gateway_response = Some(json!({
            "gateway": "stripe",
            "publishable_key": "pk_test_public",
            "_stripe_client_secret_encrypted": legacy,
        }));
        let observed = order
            .gateway_response
            .clone()
            .expect("fixture response should exist");
        let repository = InMemoryWalletRepository::seed_read_model(WalletReadSeed {
            payment_orders: vec![order.clone()],
            ..WalletReadSeed::default()
        });
        let input = stripe_secret_cas_input(&order, observed.clone(), legacy, replacement);

        let mut stale_json = input.clone();
        stale_json.expected_gateway_response["publishable_key"] = json!("pk_test_changed");
        assert!(!repository
            .compare_and_swap_payment_order_stripe_client_secret(stale_json)
            .await
            .expect("stale JSON should be a normal CAS miss"));

        let mut stale_ciphertext = input.clone();
        stale_ciphertext.expected_client_secret_encrypted = "gAAAAABother".to_string();
        assert!(!repository
            .compare_and_swap_payment_order_stripe_client_secret(stale_ciphertext)
            .await
            .expect("stale ciphertext should be a normal CAS miss"));

        let mut foreign_identity = input.clone();
        foreign_identity.order_no = "order-no-foreign".to_string();
        assert!(!repository
            .compare_and_swap_payment_order_stripe_client_secret(foreign_identity)
            .await
            .expect("foreign identity should be a normal CAS miss"));

        assert!(repository
            .compare_and_swap_payment_order_stripe_client_secret(input.clone())
            .await
            .expect("exact CAS should succeed"));
        assert!(!repository
            .compare_and_swap_payment_order_stripe_client_secret(input)
            .await
            .expect("an old migration must not overwrite the new value"));

        let stored = repository
            .find_admin_payment_order(&order.id)
            .await
            .expect("stored order should be readable")
            .expect("stored order should remain");
        let response = stored
            .gateway_response
            .expect("stored response should remain");
        assert_eq!(
            response["_stripe_client_secret_encrypted"].as_str(),
            Some(replacement)
        );
        assert_eq!(response["publishable_key"], "pk_test_public");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_stripe_secret_migrations_have_exactly_one_winner_in_memory() {
        let legacy = "gAAAAABlegacy-race";
        let mut order = sample_payment_order("stripe-cas-race", Some("user-1"), "pending");
        order.expires_at_unix_secs = Some(4_102_444_800);
        order.gateway_response = Some(json!({
            "gateway": "stripe",
            "_stripe_client_secret_encrypted": legacy,
        }));
        let observed = order
            .gateway_response
            .clone()
            .expect("fixture response should exist");
        let repository = Arc::new(InMemoryWalletRepository::seed_read_model(WalletReadSeed {
            payment_orders: vec![order.clone()],
            ..WalletReadSeed::default()
        }));
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let mut tasks = Vec::new();
        for suffix in ["winner-a", "winner-b"] {
            let repository = Arc::clone(&repository);
            let barrier = Arc::clone(&barrier);
            let input = stripe_secret_cas_input(
                &order,
                observed.clone(),
                legacy,
                &format!(
                    "aether-payment-order-stripe-client-secret-v2:aether-runtime-secret-v1:gAAAAAB{suffix}"
                ),
            );
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                repository
                    .compare_and_swap_payment_order_stripe_client_secret(input)
                    .await
            }));
        }
        barrier.wait().await;

        let mut winners = 0;
        for task in tasks {
            if task
                .await
                .expect("migration task should join")
                .expect("migration should not error")
            {
                winners += 1;
            }
        }
        assert_eq!(winners, 1);
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
    async fn deletes_only_untouched_provisional_auth_wallets_in_memory() {
        let repository = InMemoryWalletRepository::seed(Vec::new());
        let provisional_wallet = repository
            .initialize_auth_user_wallet("provisional-user", 10.0, false)
            .await
            .expect("wallet initialization should succeed")
            .expect("provisional wallet should exist");

        assert!(repository
            .delete_provisional_auth_user_wallet(&provisional_wallet.id, "provisional-user")
            .await
            .expect("provisional cleanup should succeed"));
        assert!(repository
            .find(WalletLookupKey::UserId("provisional-user"))
            .await
            .expect("wallet lookup should succeed")
            .is_none());
    }

    #[tokio::test]
    async fn provisional_cleanup_keeps_wallet_with_financial_activity_in_memory() {
        let wallet = StoredWalletSnapshot::new(
            "active-wallet".to_string(),
            Some("active-user".to_string()),
            None,
            0.0,
            10.0,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            0.0,
            1.0,
            0.0,
            10.0,
            1,
        )
        .expect("wallet should build");
        let repository = InMemoryWalletRepository::seed([wallet]);

        assert!(!repository
            .delete_provisional_auth_user_wallet("active-wallet", "active-user")
            .await
            .expect("provisional cleanup should succeed"));
        assert!(repository
            .find(WalletLookupKey::UserId("active-user"))
            .await
            .expect("wallet lookup should succeed")
            .is_some());
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
    async fn plan_purchase_rejects_malformed_wallet_credit_in_memory() {
        let repository = InMemoryWalletRepository::seed(vec![sample_wallet()]);
        let result = repository
            .create_plan_purchase_order(CreatePlanPurchaseOrderInput {
                preferred_wallet_id: None,
                user_id: "user-1".to_string(),
                amount_usd: 1.0,
                pay_amount: 1.0,
                pay_currency: "USD".to_string(),
                exchange_rate: 1.0,
                payment_method: "stripe".to_string(),
                payment_provider: Some("stripe".to_string()),
                payment_channel: Some("card".to_string()),
                gateway_order_id: "gateway-invalid-wallet-credit".to_string(),
                gateway_response: json!({ "checkout": true }),
                order_no: "order-invalid-wallet-credit".to_string(),
                product_id: "invalid-wallet-credit-plan".to_string(),
                product_snapshot: json!({
                    "id": "invalid-wallet-credit-plan",
                    "purchase_limit_scope": "unlimited",
                    "entitlements": [{
                        "type": "wallet_credit",
                        "amount_usd": 1.0,
                        "balance_bucket": "unknown"
                    }]
                }),
                expires_at_unix_secs: 4_102_444_800,
            })
            .await;
        assert!(matches!(result, Err(DataLayerError::InvalidInput(_))));
        assert!(repository
            .payment_orders_by_id
            .read()
            .expect("wallet repo lock")
            .is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gateway_order_uniqueness_is_atomic_in_memory() {
        let repository = Arc::new(InMemoryWalletRepository::seed(Vec::new()));
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let mut tasks = Vec::new();
        for index in 0..2 {
            let repository = Arc::clone(&repository);
            let barrier = Arc::clone(&barrier);
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                repository
                    .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
                        preferred_wallet_id: Some(format!("wallet-concurrent-{index}")),
                        user_id: format!("user-concurrent-{index}"),
                        amount_usd: 1.0,
                        pay_amount: Some(1.0),
                        pay_currency: Some("USD".to_string()),
                        exchange_rate: Some(1.0),
                        payment_method: " EPAY ".to_string(),
                        payment_provider: None,
                        payment_channel: None,
                        gateway_order_id: "shared-memory-gateway-id".to_string(),
                        gateway_response: json!({ "checkout": true }),
                        order_no: format!("order-concurrent-{index}"),
                        expires_at_unix_secs: 4_102_444_800,
                    })
                    .await
            }));
        }
        barrier.wait().await;

        let mut created = 0;
        let mut rejected = 0;
        for task in tasks {
            match task.await.expect("order task should join") {
                Ok(CreateWalletRechargeOrderOutcome::Created(order)) => {
                    assert_eq!(order.payment_method, "epay");
                    created += 1;
                }
                Err(DataLayerError::InvalidInput(_)) => rejected += 1,
                other => panic!("unexpected concurrent order result: {other:?}"),
            }
        }
        assert_eq!((created, rejected), (1, 1));
        assert_eq!(
            repository
                .payment_orders_by_id
                .read()
                .expect("wallet repo lock")
                .len(),
            1
        );
        assert_eq!(
            repository
                .wallets_by_id
                .read()
                .expect("wallet repo lock")
                .len(),
            1,
            "the rejected order must not leave a provisional wallet behind"
        );
        let orders = repository
            .payment_orders_by_id
            .read()
            .expect("wallet repo lock");
        let wallets = repository.wallets_by_id.read().expect("wallet repo lock");
        let stored_order = orders.values().next().expect("winning order should remain");
        let stored_wallet = wallets
            .get(&stored_order.wallet_id)
            .expect("winning order must not reference a removed wallet");
        assert_eq!(stored_wallet.user_id, stored_order.user_id);
    }

    #[tokio::test]
    async fn recharge_order_conflict_removes_unreferenced_provisional_wallet_in_memory() {
        let mut existing = sample_payment_order(
            "existing-recharge-order",
            Some("recharge-conflict-user"),
            "pending",
        );
        existing.order_no = "recharge-conflict-order-no".to_string();
        existing.wallet_id = "wallet-from-old-read-model".to_string();
        existing.payment_method = "epay".to_string();
        existing.gateway_order_id = Some("gateway-from-old-read-model".to_string());
        existing.gateway_response = Some(json!({
            "order_kind": "wallet_recharge",
            "integration_status": "checkout_pending"
        }));
        let repository = InMemoryWalletRepository::seed_read_model(WalletReadSeed {
            payment_orders: vec![existing.clone()],
            ..WalletReadSeed::default()
        });

        let outcome = repository
            .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
                preferred_wallet_id: Some("wallet-provisional-conflict".to_string()),
                user_id: "recharge-conflict-user".to_string(),
                amount_usd: 10.0,
                pay_amount: None,
                pay_currency: None,
                exchange_rate: None,
                payment_method: "epay".to_string(),
                payment_provider: Some("epay".to_string()),
                payment_channel: None,
                gateway_order_id: "gateway-retry".to_string(),
                gateway_response: json!({ "integration_status": "checkout_pending" }),
                order_no: "recharge-conflict-order-no".to_string(),
                expires_at_unix_secs: 4_102_444_800,
            })
            .await
            .expect("existing recharge order should be returned");

        assert!(matches!(
            outcome,
            CreateWalletRechargeOrderOutcome::Existing(order)
                if order.id == "existing-recharge-order"
        ));
        assert!(repository
            .find(WalletLookupKey::UserId("recharge-conflict-user"))
            .await
            .expect("wallet lookup should succeed")
            .is_none());
        assert_eq!(
            repository
                .payment_orders_by_id
                .read()
                .expect("wallet repo lock")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn recharge_rejects_preferred_wallet_id_owned_by_another_user_in_memory() {
        let repository = InMemoryWalletRepository::seed(vec![sample_wallet()]);

        let result = repository
            .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
                preferred_wallet_id: Some("wallet-1".to_string()),
                user_id: "different-owner".to_string(),
                amount_usd: 5.0,
                pay_amount: Some(5.0),
                pay_currency: Some("USD".to_string()),
                exchange_rate: Some(1.0),
                payment_method: "stripe".to_string(),
                payment_provider: Some("stripe".to_string()),
                payment_channel: Some("card".to_string()),
                gateway_order_id: "gateway-wallet-id-collision".to_string(),
                gateway_response: json!({ "checkout": true }),
                order_no: "order-wallet-id-collision".to_string(),
                expires_at_unix_secs: 4_102_444_800,
            })
            .await;

        assert!(matches!(
            result,
            Err(DataLayerError::InvalidInput(message))
                if message.contains("wallet identifier already belongs")
        ));
        let original = repository
            .find(WalletLookupKey::UserId("user-1"))
            .await
            .expect("original wallet lookup should succeed")
            .expect("original wallet should remain present");
        assert_eq!(original.id, "wallet-1");
        assert!(repository
            .find(WalletLookupKey::UserId("different-owner"))
            .await
            .expect("new owner lookup should succeed")
            .is_none());
        assert!(repository
            .payment_orders_by_id
            .read()
            .expect("wallet repo lock")
            .is_empty());
    }

    #[tokio::test]
    async fn recharge_checkout_update_preserves_order_kind_in_memory() {
        let repository = InMemoryWalletRepository::seed(Vec::new());
        let created = repository
            .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
                preferred_wallet_id: None,
                user_id: "user-recharge-kind".to_string(),
                amount_usd: 3.0,
                pay_amount: Some(3.0),
                pay_currency: Some("USD".to_string()),
                exchange_rate: Some(1.0),
                payment_method: "epay".to_string(),
                payment_provider: Some("epay".to_string()),
                payment_channel: Some("alipay".to_string()),
                gateway_order_id: "placeholder-order-kind".to_string(),
                gateway_response: json!({
                    "gateway": "epay",
                    "integration_status": "checkout_pending"
                }),
                order_no: "order-recharge-kind".to_string(),
                expires_at_unix_secs: 4_102_444_800,
            })
            .await
            .expect("recharge order should be created");
        let CreateWalletRechargeOrderOutcome::Created(order) = created else {
            panic!("expected a newly created recharge order");
        };

        let updated = repository
            .update_wallet_recharge_checkout(UpdateWalletRechargeCheckoutInput {
                order_id: order.id.clone(),
                gateway_order_id: "provider-order-kind".to_string(),
                gateway_response: json!({
                    "gateway": "epay",
                    "payment_url": "https://pay.example.test/order"
                }),
            })
            .await
            .expect("checkout update should succeed");
        assert!(matches!(updated, WalletMutationOutcome::Applied(_)));

        let replay = repository
            .find_wallet_recharge_order_by_order_no("user-recharge-kind", "order-recharge-kind")
            .await
            .expect("recharge lookup should succeed")
            .expect("updated order should remain discoverable");
        assert_eq!(
            replay.gateway_order_id.as_deref(),
            Some("provider-order-kind")
        );
        assert_eq!(
            replay
                .gateway_response
                .as_ref()
                .and_then(|value| value.get("order_kind"))
                .and_then(serde_json::Value::as_str),
            Some("wallet_recharge")
        );
    }

    #[tokio::test]
    async fn recharge_checkout_update_rejects_expired_order_in_memory() {
        let repository = InMemoryWalletRepository::seed(Vec::new());
        let created = repository
            .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
                preferred_wallet_id: None,
                user_id: "user-expired-checkout".to_string(),
                amount_usd: 3.0,
                pay_amount: Some(3.0),
                pay_currency: Some("USD".to_string()),
                exchange_rate: Some(1.0),
                payment_method: "epay".to_string(),
                payment_provider: Some("epay".to_string()),
                payment_channel: Some("alipay".to_string()),
                gateway_order_id: "order-expired-checkout".to_string(),
                gateway_response: serde_json::json!({
                    "order_kind": "wallet_recharge",
                    "integration_status": "checkout_pending"
                }),
                order_no: "order-expired-checkout".to_string(),
                expires_at_unix_secs: 1,
            })
            .await
            .expect("expired recharge order should be creatable for regression setup");
        let CreateWalletRechargeOrderOutcome::Created(order) = created else {
            panic!("expected a newly created recharge order");
        };

        let result = repository
            .update_wallet_recharge_checkout(UpdateWalletRechargeCheckoutInput {
                order_id: order.id.clone(),
                gateway_order_id: "provider-expired-checkout".to_string(),
                gateway_response: serde_json::json!({
                    "order_kind": "wallet_recharge",
                    "payment_url": "https://pay.example.test/expired"
                }),
            })
            .await
            .expect("expired checkout update should resolve");
        assert!(matches!(result, WalletMutationOutcome::Invalid(_)));

        let replay = repository
            .find_wallet_recharge_order_by_order_no(
                "user-expired-checkout",
                "order-expired-checkout",
            )
            .await
            .expect("expired recharge lookup should succeed")
            .expect("expired recharge order should remain stored");
        assert_eq!(
            replay.gateway_order_id.as_deref(),
            Some("order-expired-checkout")
        );
    }

    #[tokio::test]
    async fn failed_recharge_without_channel_can_be_reclaimed_in_memory() {
        let repository = InMemoryWalletRepository::seed(Vec::new());
        let first = repository
            .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
                preferred_wallet_id: None,
                user_id: "user-reclaim-no-channel".to_string(),
                amount_usd: 3.0,
                pay_amount: None,
                pay_currency: None,
                exchange_rate: None,
                payment_method: "stripe".to_string(),
                payment_provider: Some("stripe".to_string()),
                payment_channel: None,
                gateway_order_id: "order-reclaim-no-channel".to_string(),
                gateway_response: json!({
                    "gateway": "stripe",
                    "order_kind": "wallet_recharge",
                    "integration_status": "checkout_pending",
                    "checkout_claim_token": "first-claim"
                }),
                order_no: "order-reclaim-no-channel".to_string(),
                expires_at_unix_secs: 4_102_444_800,
            })
            .await
            .expect("initial recharge should be created");
        let CreateWalletRechargeOrderOutcome::Created(first) = first else {
            panic!("expected initial recharge order");
        };

        let failed = repository
            .fail_wallet_recharge_checkout(FailWalletRechargeCheckoutInput {
                order_id: first.id.clone(),
                claim_token: "first-claim".to_string(),
                reason: "provider unavailable".to_string(),
                provider_request_may_have_succeeded: false,
            })
            .await
            .expect("checkout failure should resolve");
        assert!(matches!(failed, WalletMutationOutcome::Applied(_)));

        let retry = repository
            .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
                preferred_wallet_id: None,
                user_id: "user-reclaim-no-channel".to_string(),
                amount_usd: 3.0,
                pay_amount: None,
                pay_currency: None,
                exchange_rate: None,
                payment_method: "stripe".to_string(),
                payment_provider: Some("stripe".to_string()),
                payment_channel: None,
                gateway_order_id: "order-reclaim-no-channel".to_string(),
                gateway_response: json!({
                    "gateway": "stripe",
                    "order_kind": "wallet_recharge",
                    "integration_status": "checkout_pending",
                    "checkout_claim_token": "retry-claim"
                }),
                order_no: "order-reclaim-no-channel".to_string(),
                expires_at_unix_secs: 4_102_444_800,
            })
            .await
            .expect("failed placeholder should be reclaimable");

        let CreateWalletRechargeOrderOutcome::Created(reclaimed) = retry else {
            panic!("expected failed placeholder to be reclaimed");
        };
        assert_eq!(reclaimed.id, first.id);
        assert_eq!(
            reclaimed
                .gateway_response
                .as_ref()
                .and_then(|value| value.get("checkout_claim_token"))
                .and_then(serde_json::Value::as_str),
            Some("retry-claim")
        );
        assert_eq!(reclaimed.status, "pending");
    }

    #[tokio::test]
    async fn recharge_order_rejects_non_finite_numeric_fields_in_memory() {
        let invalid_inputs = [
            (f64::NAN, Some(1.0), Some(1.0), 4_102_444_800),
            (1.0, Some(f64::INFINITY), Some(1.0), 4_102_444_800),
            (1.0, Some(1.0), Some(0.0), 4_102_444_800),
            (1.0, Some(1.0), Some(1.0), i64::MAX as u64 + 1),
        ];
        for (index, (amount_usd, pay_amount, exchange_rate, expires_at)) in
            invalid_inputs.into_iter().enumerate()
        {
            let repository = InMemoryWalletRepository::seed(Vec::new());
            let result = repository
                .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
                    preferred_wallet_id: None,
                    user_id: format!("invalid-recharge-user-{index}"),
                    amount_usd,
                    pay_amount,
                    pay_currency: Some("USD".to_string()),
                    exchange_rate,
                    payment_method: "stripe".to_string(),
                    payment_provider: Some("stripe".to_string()),
                    payment_channel: Some("card".to_string()),
                    gateway_order_id: format!("invalid-recharge-gateway-{index}"),
                    gateway_response: json!({"payment_url": "https://pay.example.test"}),
                    order_no: format!("invalid-recharge-order-{index}"),
                    expires_at_unix_secs: expires_at,
                })
                .await;
            assert!(matches!(result, Err(DataLayerError::InvalidInput(_))));
            assert!(repository
                .find(WalletLookupKey::UserId(&format!(
                    "invalid-recharge-user-{index}"
                )))
                .await
                .expect("wallet lookup should succeed")
                .is_none());
        }
    }

    #[tokio::test]
    async fn refund_creation_is_idempotent_and_rejects_corrupt_wallet_balance_in_memory() {
        let repository = InMemoryWalletRepository::seed(vec![sample_wallet()]);
        let first = repository
            .create_wallet_refund_request(CreateWalletRefundRequestInput {
                wallet_id: "wallet-1".to_string(),
                user_id: "user-1".to_string(),
                amount_usd: 4.0,
                payment_order_id: None,
                source_type: None,
                source_id: None,
                refund_mode: None,
                reason: Some("first request".to_string()),
                idempotency_key: Some("memory-refund-idempotency".to_string()),
                refund_no: "memory-refund-1".to_string(),
            })
            .await
            .expect("first refund should resolve");
        let CreateWalletRefundRequestOutcome::Created(first) = first else {
            panic!("expected first refund to be created");
        };
        let replay = repository
            .create_wallet_refund_request(CreateWalletRefundRequestInput {
                wallet_id: "wallet-1".to_string(),
                user_id: "user-1".to_string(),
                amount_usd: 9.0,
                payment_order_id: None,
                source_type: None,
                source_id: None,
                refund_mode: None,
                reason: Some("replayed request".to_string()),
                idempotency_key: Some("memory-refund-idempotency".to_string()),
                refund_no: "memory-refund-2".to_string(),
            })
            .await
            .expect("refund replay should resolve");
        assert!(matches!(
            replay,
            CreateWalletRefundRequestOutcome::Duplicate(refund) if refund.id == first.id
        ));
        assert_eq!(
            repository
                .refunds_by_id
                .read()
                .expect("wallet repo lock")
                .len(),
            1
        );

        repository.with_wallets_mut(|wallets| {
            wallets
                .get_mut("wallet-1")
                .expect("sample wallet should exist")
                .balance = f64::NAN;
        });
        let corrupt = repository
            .create_wallet_refund_request(CreateWalletRefundRequestInput {
                wallet_id: "wallet-1".to_string(),
                user_id: "user-1".to_string(),
                amount_usd: 1.0,
                payment_order_id: None,
                source_type: None,
                source_id: None,
                refund_mode: None,
                reason: None,
                idempotency_key: Some("memory-refund-corrupt".to_string()),
                refund_no: "memory-refund-corrupt".to_string(),
            })
            .await
            .expect("corrupt wallet refund should resolve");
        assert!(matches!(
            corrupt,
            CreateWalletRefundRequestOutcome::InvalidInput(_)
        ));
    }

    #[tokio::test]
    async fn seeded_refund_idempotency_replays_only_explicit_mappings_in_memory() {
        let seeded = sample_refund("refund-seeded-idempotency", Some("user-1"), "approved");
        let mapped_repository = InMemoryWalletRepository::seed_read_model(WalletReadSeed {
            wallets: vec![sample_wallet()],
            refunds: vec![seeded.clone()],
            refund_idempotency: vec![(
                "user-1".to_string(),
                "seeded-refund-key".to_string(),
                seeded.id.clone(),
            )],
            ..WalletReadSeed::default()
        });
        let replay = mapped_repository
            .create_wallet_refund_request(CreateWalletRefundRequestInput {
                wallet_id: "wallet-1".to_string(),
                user_id: "user-1".to_string(),
                amount_usd: 1.0,
                payment_order_id: None,
                source_type: None,
                source_id: None,
                refund_mode: None,
                reason: Some("seeded replay".to_string()),
                idempotency_key: Some("seeded-refund-key".to_string()),
                refund_no: "seeded-refund-replay".to_string(),
            })
            .await
            .expect("seeded refund replay should resolve");
        assert!(matches!(
            replay,
            CreateWalletRefundRequestOutcome::Duplicate(refund) if refund.id == seeded.id
        ));
        assert_eq!(
            mapped_repository
                .refunds_by_id
                .read()
                .expect("wallet repo lock")
                .len(),
            1
        );

        let unmapped_repository = InMemoryWalletRepository::seed_read_model(WalletReadSeed {
            wallets: vec![sample_wallet()],
            refunds: vec![seeded],
            ..WalletReadSeed::default()
        });
        let created = unmapped_repository
            .create_wallet_refund_request(CreateWalletRefundRequestInput {
                wallet_id: "wallet-1".to_string(),
                user_id: "user-1".to_string(),
                amount_usd: 1.0,
                payment_order_id: None,
                source_type: None,
                source_id: None,
                refund_mode: None,
                reason: Some("unmapped seed".to_string()),
                idempotency_key: Some("seeded-refund-key".to_string()),
                refund_no: "seeded-refund-unmapped".to_string(),
            })
            .await
            .expect("unmapped seeded refund request should resolve");
        assert!(matches!(
            created,
            CreateWalletRefundRequestOutcome::Created(_)
        ));
        assert_eq!(
            unmapped_repository
                .refunds_by_id
                .read()
                .expect("wallet repo lock")
                .len(),
            2
        );
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
            refund_idempotency: Vec::new(),
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

    #[tokio::test]
    async fn refund_reservation_rejects_invalid_active_amounts_in_memory() {
        for (label, amount) in [
            ("negative", -100.0),
            ("zero", 0.0),
            ("infinite", f64::INFINITY),
            ("nan", f64::NAN),
        ] {
            let mut invalid = sample_refund(
                &format!("refund-invalid-{label}"),
                Some("user-1"),
                "pending_approval",
            );
            invalid.amount_usd = amount;
            let repository = InMemoryWalletRepository::seed_read_model(WalletReadSeed {
                wallets: vec![sample_wallet()],
                refunds: vec![invalid],
                ..WalletReadSeed::default()
            });
            let outcome = repository
                .create_wallet_refund_request(CreateWalletRefundRequestInput {
                    wallet_id: "wallet-1".to_string(),
                    user_id: "user-1".to_string(),
                    amount_usd: 1.0,
                    payment_order_id: None,
                    source_type: None,
                    source_id: None,
                    refund_mode: None,
                    reason: Some(format!("invalid reservation: {label}")),
                    idempotency_key: Some(format!("reservation-invalid-{label}")),
                    refund_no: format!("reservation-invalid-{label}"),
                })
                .await
                .expect("reservation request should resolve");
            assert!(
                matches!(outcome, CreateWalletRefundRequestOutcome::InvalidInput(_)),
                "active {label} reservation must fail closed: {outcome:?}"
            );
        }

        // Completed refunds do not reserve balance and remain ignored.
        let mut completed = sample_refund("refund-completed", Some("user-1"), "completed");
        completed.amount_usd = 100.0;
        let repository = InMemoryWalletRepository::seed_read_model(WalletReadSeed {
            wallets: vec![sample_wallet()],
            refunds: vec![completed],
            ..WalletReadSeed::default()
        });
        let outcome = repository
            .create_wallet_refund_request(CreateWalletRefundRequestInput {
                wallet_id: "wallet-1".to_string(),
                user_id: "user-1".to_string(),
                amount_usd: 1.0,
                payment_order_id: None,
                source_type: None,
                source_id: None,
                refund_mode: None,
                reason: Some("completed reservation is ignored".to_string()),
                idempotency_key: Some("reservation-completed-ignored".to_string()),
                refund_no: "reservation-completed-ignored".to_string(),
            })
            .await
            .expect("completed reservation should not block request");
        assert!(matches!(
            outcome,
            CreateWalletRefundRequestOutcome::Created(_)
        ));
    }
}
