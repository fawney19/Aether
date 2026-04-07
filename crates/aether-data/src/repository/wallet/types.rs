use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletLookupKey<'a> {
    WalletId(&'a str),
    UserId(&'a str),
    ApiKeyId(&'a str),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredWalletSnapshot {
    pub id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub balance: f64,
    pub gift_balance: f64,
    pub limit_mode: String,
    pub currency: String,
    pub status: String,
    pub total_recharged: f64,
    pub total_consumed: f64,
    pub total_refunded: f64,
    pub total_adjusted: f64,
    pub updated_at_unix_secs: u64,
}

impl StoredWalletSnapshot {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        user_id: Option<String>,
        api_key_id: Option<String>,
        balance: f64,
        gift_balance: f64,
        limit_mode: String,
        currency: String,
        status: String,
        total_recharged: f64,
        total_consumed: f64,
        total_refunded: f64,
        total_adjusted: f64,
        updated_at_unix_secs: i64,
    ) -> Result<Self, crate::DataLayerError> {
        if id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "wallet.id is empty".to_string(),
            ));
        }
        if limit_mode.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "wallet.limit_mode is empty".to_string(),
            ));
        }
        if currency.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "wallet.currency is empty".to_string(),
            ));
        }
        if status.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "wallet.status is empty".to_string(),
            ));
        }
        if !balance.is_finite()
            || !gift_balance.is_finite()
            || !total_recharged.is_finite()
            || !total_consumed.is_finite()
            || !total_refunded.is_finite()
            || !total_adjusted.is_finite()
        {
            return Err(crate::DataLayerError::UnexpectedValue(
                "wallet numeric value is not finite".to_string(),
            ));
        }
        Ok(Self {
            id,
            user_id,
            api_key_id,
            balance,
            gift_balance,
            limit_mode,
            currency,
            status,
            total_recharged,
            total_consumed,
            total_refunded,
            total_adjusted,
            updated_at_unix_secs: u64::try_from(updated_at_unix_secs).map_err(|_| {
                crate::DataLayerError::UnexpectedValue(
                    "wallet.updated_at_unix_secs is negative".to_string(),
                )
            })?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct AdminWalletListQuery {
    pub status: Option<String>,
    pub owner_type: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminWalletListItem {
    pub id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub balance: f64,
    pub gift_balance: f64,
    pub limit_mode: String,
    pub currency: String,
    pub status: String,
    pub total_recharged: f64,
    pub total_consumed: f64,
    pub total_refunded: f64,
    pub total_adjusted: f64,
    pub user_name: Option<String>,
    pub api_key_name: Option<String>,
    pub created_at_unix_secs: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminWalletListPage {
    pub items: Vec<StoredAdminWalletListItem>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct AdminWalletLedgerQuery {
    pub category: Option<String>,
    pub reason_code: Option<String>,
    pub owner_type: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminWalletLedgerItem {
    pub id: String,
    pub wallet_id: String,
    pub category: String,
    pub reason_code: String,
    pub amount: f64,
    pub balance_before: f64,
    pub balance_after: f64,
    pub recharge_balance_before: f64,
    pub recharge_balance_after: f64,
    pub gift_balance_before: f64,
    pub gift_balance_after: f64,
    pub link_type: Option<String>,
    pub link_id: Option<String>,
    pub operator_id: Option<String>,
    pub operator_name: Option<String>,
    pub operator_email: Option<String>,
    pub description: Option<String>,
    pub wallet_user_id: Option<String>,
    pub wallet_user_name: Option<String>,
    pub wallet_api_key_id: Option<String>,
    pub api_key_name: Option<String>,
    pub wallet_status: String,
    pub created_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminWalletLedgerPage {
    pub items: Vec<StoredAdminWalletLedgerItem>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct AdminWalletRefundRequestListQuery {
    pub status: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminWalletRefundRequestItem {
    pub id: String,
    pub refund_no: String,
    pub wallet_id: String,
    pub user_id: Option<String>,
    pub payment_order_id: Option<String>,
    pub source_type: String,
    pub source_id: Option<String>,
    pub refund_mode: String,
    pub amount_usd: f64,
    pub status: String,
    pub reason: Option<String>,
    pub failure_reason: Option<String>,
    pub gateway_refund_id: Option<String>,
    pub payout_method: Option<String>,
    pub payout_reference: Option<String>,
    pub payout_proof: Option<serde_json::Value>,
    pub requested_by: Option<String>,
    pub approved_by: Option<String>,
    pub processed_by: Option<String>,
    pub wallet_user_id: Option<String>,
    pub wallet_user_name: Option<String>,
    pub wallet_api_key_id: Option<String>,
    pub api_key_name: Option<String>,
    pub wallet_status: String,
    pub created_at_unix_secs: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
    pub processed_at_unix_secs: Option<u64>,
    pub completed_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminWalletRefundRequestPage {
    pub items: Vec<StoredAdminWalletRefundRequestItem>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminWalletTransaction {
    pub id: String,
    pub wallet_id: String,
    pub category: String,
    pub reason_code: String,
    pub amount: f64,
    pub balance_before: f64,
    pub balance_after: f64,
    pub recharge_balance_before: f64,
    pub recharge_balance_after: f64,
    pub gift_balance_before: f64,
    pub gift_balance_after: f64,
    pub link_type: Option<String>,
    pub link_id: Option<String>,
    pub operator_id: Option<String>,
    pub operator_name: Option<String>,
    pub operator_email: Option<String>,
    pub description: Option<String>,
    pub created_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdminWalletTransactionRecord {
    pub id: String,
    pub wallet_id: String,
    pub category: String,
    pub reason_code: String,
    pub amount: f64,
    pub balance_before: f64,
    pub balance_after: f64,
    pub recharge_balance_before: f64,
    pub recharge_balance_after: f64,
    pub gift_balance_before: f64,
    pub gift_balance_after: f64,
    pub link_type: Option<String>,
    pub link_id: Option<String>,
    pub operator_id: Option<String>,
    pub description: Option<String>,
    pub created_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminWalletTransactionPage {
    pub items: Vec<StoredAdminWalletTransaction>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredWalletDailyUsageLedger {
    pub id: Option<String>,
    pub billing_date: String,
    pub billing_timezone: String,
    pub total_cost_usd: f64,
    pub total_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub first_finalized_at_unix_secs: Option<u64>,
    pub last_finalized_at_unix_secs: Option<u64>,
    pub aggregated_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredWalletDailyUsageLedgerPage {
    pub items: Vec<StoredWalletDailyUsageLedger>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminWalletRefund {
    pub id: String,
    pub refund_no: String,
    pub wallet_id: String,
    pub user_id: Option<String>,
    pub payment_order_id: Option<String>,
    pub source_type: String,
    pub source_id: Option<String>,
    pub refund_mode: String,
    pub amount_usd: f64,
    pub status: String,
    pub reason: Option<String>,
    pub failure_reason: Option<String>,
    pub gateway_refund_id: Option<String>,
    pub payout_method: Option<String>,
    pub payout_reference: Option<String>,
    pub payout_proof: Option<serde_json::Value>,
    pub requested_by: Option<String>,
    pub approved_by: Option<String>,
    pub processed_by: Option<String>,
    pub created_at_unix_secs: u64,
    pub updated_at_unix_secs: u64,
    pub processed_at_unix_secs: Option<u64>,
    pub completed_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdminWalletRefundRecord {
    pub id: String,
    pub refund_no: String,
    pub wallet_id: String,
    pub user_id: Option<String>,
    pub payment_order_id: Option<String>,
    pub source_type: String,
    pub source_id: Option<String>,
    pub refund_mode: String,
    pub amount_usd: f64,
    pub status: String,
    pub reason: Option<String>,
    pub failure_reason: Option<String>,
    pub gateway_refund_id: Option<String>,
    pub payout_method: Option<String>,
    pub payout_reference: Option<String>,
    pub payout_proof: Option<serde_json::Value>,
    pub requested_by: Option<String>,
    pub approved_by: Option<String>,
    pub processed_by: Option<String>,
    pub created_at_unix_secs: u64,
    pub updated_at_unix_secs: u64,
    pub processed_at_unix_secs: Option<u64>,
    pub completed_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminWalletRefundPage {
    pub items: Vec<StoredAdminWalletRefund>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct AdminPaymentOrderListQuery {
    pub status: Option<String>,
    pub payment_method: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminPaymentOrder {
    pub id: String,
    pub order_no: String,
    pub wallet_id: String,
    pub user_id: Option<String>,
    pub amount_usd: f64,
    pub pay_amount: Option<f64>,
    pub pay_currency: Option<String>,
    pub exchange_rate: Option<f64>,
    pub refunded_amount_usd: f64,
    pub refundable_amount_usd: f64,
    pub payment_method: String,
    pub gateway_order_id: Option<String>,
    pub gateway_response: Option<serde_json::Value>,
    pub status: String,
    pub created_at_unix_secs: u64,
    pub paid_at_unix_secs: Option<u64>,
    pub credited_at_unix_secs: Option<u64>,
    pub expires_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdminWalletPaymentOrderRecord {
    pub id: String,
    pub order_no: String,
    pub wallet_id: String,
    pub user_id: Option<String>,
    pub amount_usd: f64,
    pub pay_amount: Option<f64>,
    pub pay_currency: Option<String>,
    pub exchange_rate: Option<f64>,
    pub refunded_amount_usd: f64,
    pub refundable_amount_usd: f64,
    pub payment_method: String,
    pub gateway_order_id: Option<String>,
    pub status: String,
    pub gateway_response: Option<serde_json::Value>,
    pub created_at_unix_secs: u64,
    pub paid_at_unix_secs: Option<u64>,
    pub credited_at_unix_secs: Option<u64>,
    pub expires_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminPaymentOrderPage {
    pub items: Vec<StoredAdminPaymentOrder>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminPaymentCallback {
    pub id: String,
    pub payment_order_id: Option<String>,
    pub payment_method: String,
    pub callback_key: String,
    pub order_no: Option<String>,
    pub gateway_order_id: Option<String>,
    pub payload_hash: Option<String>,
    pub signature_valid: bool,
    pub status: String,
    pub payload: Option<serde_json::Value>,
    pub error_message: Option<String>,
    pub created_at_unix_secs: u64,
    pub processed_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdminPaymentCallbackRecord {
    pub id: String,
    pub payment_order_id: Option<String>,
    pub payment_method: String,
    pub callback_key: String,
    pub order_no: Option<String>,
    pub gateway_order_id: Option<String>,
    pub payload_hash: Option<String>,
    pub signature_valid: bool,
    pub status: String,
    pub payload: Option<serde_json::Value>,
    pub error_message: Option<String>,
    pub created_at_unix_secs: u64,
    pub processed_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminPaymentCallbackPage {
    pub items: Vec<StoredAdminPaymentCallback>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateWalletRechargeOrderInput {
    pub preferred_wallet_id: Option<String>,
    pub user_id: String,
    pub amount_usd: f64,
    pub pay_amount: Option<f64>,
    pub pay_currency: Option<String>,
    pub exchange_rate: Option<f64>,
    pub payment_method: String,
    pub gateway_order_id: String,
    pub gateway_response: serde_json::Value,
    pub order_no: String,
    pub expires_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum CreateWalletRechargeOrderOutcome {
    Created(StoredAdminPaymentOrder),
    WalletInactive,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateWalletRefundRequestInput {
    pub wallet_id: String,
    pub user_id: String,
    pub amount_usd: f64,
    pub payment_order_id: Option<String>,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    pub refund_mode: Option<String>,
    pub reason: Option<String>,
    pub idempotency_key: Option<String>,
    pub refund_no: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum CreateWalletRefundRequestOutcome {
    Created(StoredAdminWalletRefund),
    Duplicate(StoredAdminWalletRefund),
    WalletMissing,
    RefundAmountExceedsAvailableBalance,
    PaymentOrderNotFound,
    PaymentOrderNotRefundable,
    RefundAmountExceedsAvailableOrderAmount,
    DuplicateRejected,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProcessPaymentCallbackInput {
    pub payment_method: String,
    pub callback_key: String,
    pub order_no: Option<String>,
    pub gateway_order_id: Option<String>,
    pub amount_usd: f64,
    pub pay_amount: Option<f64>,
    pub pay_currency: Option<String>,
    pub exchange_rate: Option<f64>,
    pub payload_hash: String,
    pub payload: serde_json::Value,
    pub signature_valid: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum ProcessPaymentCallbackOutcome {
    DuplicateProcessed {
        order_id: Option<String>,
    },
    Failed {
        duplicate: bool,
        error: String,
    },
    AlreadyCredited {
        duplicate: bool,
        order_id: String,
        order_no: String,
        wallet_id: String,
    },
    Applied {
        duplicate: bool,
        order_id: String,
        order_no: String,
        wallet_id: String,
        order: StoredAdminPaymentOrder,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum WalletMutationOutcome<T> {
    Applied(T),
    NotFound,
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdjustWalletBalanceInput {
    pub wallet_id: String,
    pub amount_usd: f64,
    pub balance_type: String,
    pub operator_id: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateManualWalletRechargeInput {
    pub wallet_id: String,
    pub amount_usd: f64,
    pub payment_method: String,
    pub operator_id: Option<String>,
    pub description: Option<String>,
    pub order_no: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProcessAdminWalletRefundInput {
    pub wallet_id: String,
    pub refund_id: String,
    pub operator_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CompleteAdminWalletRefundInput {
    pub wallet_id: String,
    pub refund_id: String,
    pub gateway_refund_id: Option<String>,
    pub payout_reference: Option<String>,
    pub payout_proof: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FailAdminWalletRefundInput {
    pub wallet_id: String,
    pub refund_id: String,
    pub reason: String,
    pub operator_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreditAdminPaymentOrderInput {
    pub order_id: String,
    pub gateway_order_id: Option<String>,
    pub pay_amount: Option<f64>,
    pub pay_currency: Option<String>,
    pub exchange_rate: Option<f64>,
    pub gateway_response_patch: Option<serde_json::Value>,
    pub operator_id: Option<String>,
}

#[async_trait]
pub trait WalletReadRepository: Send + Sync {
    async fn find(
        &self,
        key: WalletLookupKey<'_>,
    ) -> Result<Option<StoredWalletSnapshot>, crate::DataLayerError>;

    async fn list_wallets_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredWalletSnapshot>, crate::DataLayerError>;

    async fn list_wallets_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredWalletSnapshot>, crate::DataLayerError>;

    async fn list_admin_wallets(
        &self,
        query: &AdminWalletListQuery,
    ) -> Result<StoredAdminWalletListPage, crate::DataLayerError>;

    async fn list_admin_wallet_ledger(
        &self,
        query: &AdminWalletLedgerQuery,
    ) -> Result<StoredAdminWalletLedgerPage, crate::DataLayerError>;

    async fn list_admin_wallet_refund_requests(
        &self,
        query: &AdminWalletRefundRequestListQuery,
    ) -> Result<StoredAdminWalletRefundRequestPage, crate::DataLayerError>;

    async fn list_admin_wallet_transactions(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminWalletTransactionPage, crate::DataLayerError>;

    async fn find_wallet_today_usage(
        &self,
        wallet_id: &str,
        billing_timezone: &str,
    ) -> Result<Option<StoredWalletDailyUsageLedger>, crate::DataLayerError>;

    async fn list_wallet_daily_usage_history(
        &self,
        wallet_id: &str,
        billing_timezone: &str,
        limit: usize,
    ) -> Result<StoredWalletDailyUsageLedgerPage, crate::DataLayerError>;

    async fn list_admin_wallet_refunds(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminWalletRefundPage, crate::DataLayerError>;

    async fn list_admin_payment_orders(
        &self,
        query: &AdminPaymentOrderListQuery,
    ) -> Result<StoredAdminPaymentOrderPage, crate::DataLayerError>;

    async fn find_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, crate::DataLayerError>;

    async fn list_wallet_payment_orders_by_user_id(
        &self,
        user_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminPaymentOrderPage, crate::DataLayerError>;

    async fn find_wallet_payment_order_by_user_id(
        &self,
        user_id: &str,
        order_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, crate::DataLayerError>;

    async fn find_wallet_refund(
        &self,
        wallet_id: &str,
        refund_id: &str,
    ) -> Result<Option<StoredAdminWalletRefund>, crate::DataLayerError>;

    async fn list_admin_payment_callbacks(
        &self,
        payment_method: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminPaymentCallbackPage, crate::DataLayerError>;
}

#[async_trait]
pub trait WalletWriteRepository: Send + Sync {
    async fn create_wallet_recharge_order(
        &self,
        input: CreateWalletRechargeOrderInput,
    ) -> Result<CreateWalletRechargeOrderOutcome, crate::DataLayerError>;

    async fn create_wallet_refund_request(
        &self,
        input: CreateWalletRefundRequestInput,
    ) -> Result<CreateWalletRefundRequestOutcome, crate::DataLayerError>;

    async fn process_payment_callback(
        &self,
        input: ProcessPaymentCallbackInput,
    ) -> Result<ProcessPaymentCallbackOutcome, crate::DataLayerError>;

    async fn adjust_wallet_balance(
        &self,
        input: AdjustWalletBalanceInput,
    ) -> Result<Option<(StoredWalletSnapshot, StoredAdminWalletTransaction)>, crate::DataLayerError>;

    async fn create_manual_wallet_recharge(
        &self,
        input: CreateManualWalletRechargeInput,
    ) -> Result<Option<(StoredWalletSnapshot, StoredAdminPaymentOrder)>, crate::DataLayerError>;

    async fn process_admin_wallet_refund(
        &self,
        input: ProcessAdminWalletRefundInput,
    ) -> Result<
        WalletMutationOutcome<(
            StoredWalletSnapshot,
            StoredAdminWalletRefund,
            StoredAdminWalletTransaction,
        )>,
        crate::DataLayerError,
    >;

    async fn complete_admin_wallet_refund(
        &self,
        input: CompleteAdminWalletRefundInput,
    ) -> Result<WalletMutationOutcome<StoredAdminWalletRefund>, crate::DataLayerError>;

    async fn fail_admin_wallet_refund(
        &self,
        input: FailAdminWalletRefundInput,
    ) -> Result<
        WalletMutationOutcome<(
            StoredWalletSnapshot,
            StoredAdminWalletRefund,
            Option<StoredAdminWalletTransaction>,
        )>,
        crate::DataLayerError,
    >;

    async fn expire_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<WalletMutationOutcome<(StoredAdminPaymentOrder, bool)>, crate::DataLayerError>;

    async fn fail_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<WalletMutationOutcome<StoredAdminPaymentOrder>, crate::DataLayerError>;

    async fn credit_admin_payment_order(
        &self,
        input: CreditAdminPaymentOrderInput,
    ) -> Result<WalletMutationOutcome<(StoredAdminPaymentOrder, bool)>, crate::DataLayerError>;
}

pub trait WalletRepository: WalletReadRepository + WalletWriteRepository + Send + Sync {}

impl<T> WalletRepository for T where T: WalletReadRepository + WalletWriteRepository + Send + Sync {}

#[cfg(test)]
mod tests {
    use super::StoredWalletSnapshot;
    use crate::repository::settlement::UsageSettlementInput;

    #[test]
    fn rejects_invalid_wallet_snapshot() {
        assert!(StoredWalletSnapshot::new(
            "".to_string(),
            None,
            None,
            1.0,
            0.0,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            0.0,
            0.0,
            0.0,
            0.0,
            1,
        )
        .is_err());
    }

    #[test]
    fn rejects_invalid_settlement_input() {
        let input = UsageSettlementInput {
            request_id: "".to_string(),
            user_id: None,
            api_key_id: None,
            provider_id: None,
            status: "completed".to_string(),
            billing_status: "pending".to_string(),
            total_cost_usd: 0.1,
            actual_total_cost_usd: 0.1,
            finalized_at_unix_secs: None,
        };
        assert!(input.validate().is_err());
    }
}
