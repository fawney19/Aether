use async_trait::async_trait;

const WALLET_REDACTED_DEBUG_VALUE: &str = "[REDACTED]";

fn wallet_redacted_debug_option<T>(value: &Option<T>) -> Option<&'static str> {
    value.as_ref().map(|_| WALLET_REDACTED_DEBUG_VALUE)
}

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

/// Result of an idempotent authentication-wallet initialization.
///
/// Callers that may need to compensate a partially completed operation must
/// know whether the returned row was created by that operation or was already
/// present.  Returning this bit from the same atomic repository operation
/// avoids the racy `find -> initialize` ownership inference used previously.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InitializeAuthWalletOutcome {
    pub wallet: StoredWalletSnapshot,
    pub created: bool,
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
    pub created_at_unix_ms: Option<u64>,
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
    pub created_at_unix_ms: Option<u64>,
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
    pub created_at_unix_ms: Option<u64>,
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
    pub created_at_unix_ms: Option<u64>,
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
    pub created_at_unix_ms: u64,
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
    pub created_at_unix_ms: u64,
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
    pub created_at_unix_ms: u64,
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

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    #[serde(default)]
    pub payment_provider: Option<String>,
    #[serde(default)]
    pub order_kind: String,
    pub gateway_order_id: Option<String>,
    pub gateway_response: Option<serde_json::Value>,
    pub status: String,
    pub created_at_unix_ms: u64,
    pub paid_at_unix_secs: Option<u64>,
    pub credited_at_unix_secs: Option<u64>,
    pub expires_at_unix_secs: Option<u64>,
}

impl std::fmt::Debug for StoredAdminPaymentOrder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredAdminPaymentOrder")
            .field("id", &self.id)
            .field("order_no", &self.order_no)
            .field("wallet_id", &self.wallet_id)
            .field("user_id", &self.user_id)
            .field("amount_usd", &self.amount_usd)
            .field("payment_method", &self.payment_method)
            .field("payment_provider", &self.payment_provider)
            .field("order_kind", &self.order_kind)
            .field("gateway_order_id", &self.gateway_order_id)
            .field(
                "gateway_response",
                &wallet_redacted_debug_option(&self.gateway_response),
            )
            .field("status", &self.status)
            .field("created_at_unix_ms", &self.created_at_unix_ms)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    pub created_at_unix_ms: u64,
    pub paid_at_unix_secs: Option<u64>,
    pub credited_at_unix_secs: Option<u64>,
    pub expires_at_unix_secs: Option<u64>,
}

impl std::fmt::Debug for AdminWalletPaymentOrderRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminWalletPaymentOrderRecord")
            .field("id", &self.id)
            .field("order_no", &self.order_no)
            .field("wallet_id", &self.wallet_id)
            .field("user_id", &self.user_id)
            .field("amount_usd", &self.amount_usd)
            .field("payment_method", &self.payment_method)
            .field("gateway_order_id", &self.gateway_order_id)
            .field("status", &self.status)
            .field(
                "gateway_response",
                &wallet_redacted_debug_option(&self.gateway_response),
            )
            .field("created_at_unix_ms", &self.created_at_unix_ms)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminPaymentOrderPage {
    pub items: Vec<StoredAdminPaymentOrder>,
    pub total: u64,
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    pub created_at_unix_ms: u64,
    pub processed_at_unix_secs: Option<u64>,
}

impl std::fmt::Debug for StoredAdminPaymentCallback {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredAdminPaymentCallback")
            .field("id", &self.id)
            .field("payment_order_id", &self.payment_order_id)
            .field("payment_method", &self.payment_method)
            .field("callback_key", &WALLET_REDACTED_DEBUG_VALUE)
            .field("order_no", &self.order_no)
            .field("gateway_order_id", &self.gateway_order_id)
            .field(
                "payload_hash",
                &wallet_redacted_debug_option(&self.payload_hash),
            )
            .field("signature_valid", &self.signature_valid)
            .field("status", &self.status)
            .field("payload", &wallet_redacted_debug_option(&self.payload))
            .field(
                "error_message",
                &wallet_redacted_debug_option(&self.error_message),
            )
            .field("created_at_unix_ms", &self.created_at_unix_ms)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    pub created_at_unix_ms: u64,
    pub processed_at_unix_secs: Option<u64>,
}

impl std::fmt::Debug for AdminPaymentCallbackRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdminPaymentCallbackRecord")
            .field("id", &self.id)
            .field("payment_order_id", &self.payment_order_id)
            .field("payment_method", &self.payment_method)
            .field("callback_key", &WALLET_REDACTED_DEBUG_VALUE)
            .field("order_no", &self.order_no)
            .field("gateway_order_id", &self.gateway_order_id)
            .field(
                "payload_hash",
                &wallet_redacted_debug_option(&self.payload_hash),
            )
            .field("signature_valid", &self.signature_valid)
            .field("status", &self.status)
            .field("payload", &wallet_redacted_debug_option(&self.payload))
            .field(
                "error_message",
                &wallet_redacted_debug_option(&self.error_message),
            )
            .field("created_at_unix_ms", &self.created_at_unix_ms)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminPaymentCallbackPage {
    pub items: Vec<StoredAdminPaymentCallback>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct AdminRedeemCodeBatchListQuery {
    pub status: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminRedeemCodeBatch {
    pub id: String,
    pub name: String,
    pub amount_usd: f64,
    pub currency: String,
    pub balance_bucket: String,
    pub total_count: u64,
    pub redeemed_count: u64,
    pub active_count: u64,
    pub status: String,
    pub description: Option<String>,
    pub created_by: Option<String>,
    pub expires_at_unix_secs: Option<u64>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminRedeemCodeBatchPage {
    pub items: Vec<StoredAdminRedeemCodeBatch>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct AdminRedeemCodeListQuery {
    pub batch_id: String,
    pub status: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminRedeemCode {
    pub id: String,
    pub batch_id: String,
    pub batch_name: Option<String>,
    pub code_prefix: String,
    pub code_suffix: String,
    pub masked_code: String,
    pub status: String,
    pub redeemed_by_user_id: Option<String>,
    pub redeemed_by_user_name: Option<String>,
    pub redeemed_wallet_id: Option<String>,
    pub redeemed_payment_order_id: Option<String>,
    pub redeemed_order_no: Option<String>,
    pub redeemed_at_unix_secs: Option<u64>,
    pub disabled_by: Option<String>,
    pub expires_at_unix_secs: Option<u64>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredAdminRedeemCodePage {
    pub items: Vec<StoredAdminRedeemCode>,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreatedAdminRedeemCodePlaintext {
    pub code_id: String,
    pub code: String,
    pub masked_code: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateAdminRedeemCodeBatchInput {
    pub name: String,
    pub amount_usd: f64,
    pub currency: String,
    pub balance_bucket: String,
    pub total_count: usize,
    pub expires_at_unix_secs: Option<u64>,
    pub description: Option<String>,
    pub created_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateAdminRedeemCodeBatchResult {
    pub batch: StoredAdminRedeemCodeBatch,
    pub codes: Vec<CreatedAdminRedeemCodePlaintext>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DisableAdminRedeemCodeBatchInput {
    pub batch_id: String,
    pub operator_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DeleteAdminRedeemCodeBatchInput {
    pub batch_id: String,
    pub operator_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DisableAdminRedeemCodeInput {
    pub code_id: String,
    pub operator_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RedeemWalletCodeInput {
    pub code: String,
    pub user_id: String,
    pub order_no: String,
}

pub fn redeem_code_credits_recharge_balance(balance_bucket: &str) -> bool {
    balance_bucket.trim().eq_ignore_ascii_case("recharge")
}

pub fn redeem_code_payment_method(balance_bucket: &str) -> &'static str {
    if redeem_code_credits_recharge_balance(balance_bucket) {
        "card_code"
    } else {
        "gift_code"
    }
}

/// Canonicalize the payment-method namespace before it reaches storage.
///
/// Payment methods are identifiers, not display labels. Keeping one lowercase
/// representation prevents case-only aliases from bypassing gateway-order
/// uniqueness and refund-routing rules across database backends.
pub fn canonicalize_payment_method(payment_method: &str) -> Result<String, String> {
    let normalized = payment_method.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err("payment method is required".to_string());
    }
    if normalized.chars().count() > 64 {
        return Err("payment method exceeds 64 characters".to_string());
    }
    if !normalized
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
    {
        return Err("payment method contains invalid characters".to_string());
    }
    Ok(normalized)
}

/// Validate the relationship between a stored payment method, provider, and
/// channel.  Provider integrations use an explicit namespace so a callback
/// cannot be routed to a different gateway than the one that created the
/// order.  EPay is the one compatibility exception: older orders stored the
/// selected `alipay`/`wxpay` channel as the method while using `epay` as the
/// provider.
pub fn validate_payment_provider_channel_binding(
    payment_method: &str,
    payment_provider: Option<&str>,
    payment_channel: Option<&str>,
) -> Result<(), String> {
    let payment_method = canonicalize_payment_method(payment_method)?;
    let payment_provider = payment_provider
        .map(canonicalize_payment_method)
        .transpose()?;
    let payment_channel = payment_channel
        .map(canonicalize_payment_method)
        .transpose()?;

    let Some(payment_provider) = payment_provider else {
        return Ok(());
    };
    let Some(payment_channel) = payment_channel else {
        return Err("payment provider requires a payment channel".to_string());
    };

    let valid = match payment_provider.as_str() {
        "epay" => {
            matches!(payment_method.as_str(), "epay" | "alipay" | "wxpay")
                && (payment_method == "epay" || payment_channel == payment_method)
        }
        "alipay" => payment_method == "alipay" && payment_channel == "alipay",
        // Keep this list aligned with the direct gateway implementations and
        // the public channel resolver.  Accepting an arbitrary value here
        // would let a repository caller persist a channel that no callback
        // verifier or checkout implementation can ever produce.
        "wxpay" => {
            payment_method == "wxpay"
                && matches!(payment_channel.as_str(), "native" | "h5" | "jsapi")
        }
        "stripe" => {
            payment_method == "stripe"
                && matches!(
                    payment_channel.as_str(),
                    "card" | "alipay" | "wechat_pay" | "link"
                )
        }
        "admin" => payment_method == "admin_grant" && payment_channel == "manual",
        _ => payment_method == payment_provider,
    };
    if valid {
        Ok(())
    } else {
        Err("payment method, provider, and channel binding mismatch".to_string())
    }
}

/// Validate the wallet-credit entries embedded in a plan entitlement snapshot.
///
/// A malformed `wallet_credit` must fail the whole fulfillment transaction.
/// Silently filtering it out would leave the entitlement active while the
/// balance promised by the plan was never delivered.  Entries for other
/// entitlement kinds are intentionally left to their existing validators.
pub fn validate_plan_wallet_credit_entitlements(
    entitlements: &serde_json::Value,
) -> Result<(), String> {
    let Some(items) = entitlements.as_array() else {
        return Err("plan entitlements must be an array".to_string());
    };
    for (index, item) in items.iter().enumerate() {
        let Some(object) = item.as_object() else {
            return Err(format!(
                "plan entitlement at index {index} must be an object"
            ));
        };
        let Some(kind) = object.get("type").and_then(serde_json::Value::as_str) else {
            return Err(format!("plan entitlement at index {index} is missing type"));
        };
        if !kind.eq_ignore_ascii_case("wallet_credit") {
            continue;
        }
        let Some(amount) = object.get("amount_usd").and_then(serde_json::Value::as_f64) else {
            return Err(format!(
                "wallet_credit.amount_usd is missing at entitlement index {index}"
            ));
        };
        if !amount.is_finite() || amount <= 0.0 {
            return Err(format!(
                "wallet_credit.amount_usd is invalid at entitlement index {index}"
            ));
        }
        if let Some(bucket) = object.get("balance_bucket") {
            let Some(bucket) = bucket.as_str() else {
                return Err(format!(
                    "wallet_credit.balance_bucket is invalid at entitlement index {index}"
                ));
            };
            if !matches!(
                bucket.trim().to_ascii_lowercase().as_str(),
                "recharge" | "gift"
            ) {
                return Err(format!(
                    "wallet_credit.balance_bucket is invalid at entitlement index {index}"
                ));
            }
        }
    }
    Ok(())
}

/// Validate the immutable fields of a plan-purchase order before any wallet
/// row or payment order is created.
///
/// The public handlers already perform most of these checks, but repository
/// methods are also called by administrative workflows and import/recovery
/// code. Keeping the checks here prevents a backend-specific caller from
/// persisting values that another adapter cannot represent (or that later
/// fulfillment would interpret differently).
pub fn validate_plan_purchase_order_input(
    input: &CreatePlanPurchaseOrderInput,
) -> Result<(), String> {
    fn required_identifier(value: &str, field: &str, max_len: usize) -> Result<(), String> {
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("{field} is required"));
        }
        if value.chars().count() > max_len {
            return Err(format!("{field} exceeds {max_len} characters"));
        }
        if value.chars().any(char::is_control) {
            return Err(format!("{field} contains control characters"));
        }
        Ok(())
    }

    fn optional_identifier(value: Option<&str>, field: &str, max_len: usize) -> Result<(), String> {
        let Some(value) = value else {
            return Ok(());
        };
        required_identifier(value, field, max_len)
    }

    required_identifier(&input.user_id, "user_id", 128)?;
    required_identifier(&input.order_no, "order_no", 64)?;
    required_identifier(&input.gateway_order_id, "gateway_order_id", 128)?;
    required_identifier(&input.product_id, "product_id", 64)?;
    required_identifier(&input.pay_currency, "pay_currency", 3)?;
    if input.pay_currency.trim().chars().count() != 3
        || !input
            .pay_currency
            .trim()
            .chars()
            .all(|character| character.is_ascii_alphabetic())
    {
        return Err("pay_currency must be a 3-letter currency code".to_string());
    }

    let payment_method = canonicalize_payment_method(&input.payment_method)?;
    optional_identifier(input.payment_provider.as_deref(), "payment_provider", 64)?;
    optional_identifier(input.payment_channel.as_deref(), "payment_channel", 64)?;
    validate_payment_provider_channel_binding(
        &payment_method,
        input.payment_provider.as_deref(),
        input.payment_channel.as_deref(),
    )?;

    let is_admin_grant = payment_method == "admin_grant"
        && input
            .payment_provider
            .as_deref()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("admin"))
        && input
            .payment_channel
            .as_deref()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("manual"));
    if !input.amount_usd.is_finite() || input.amount_usd < 0.0 {
        return Err("amount_usd must be finite and non-negative".to_string());
    }
    if !input.pay_amount.is_finite() || input.pay_amount < 0.0 {
        return Err("pay_amount must be finite and non-negative".to_string());
    }
    if is_admin_grant {
        if !input.pay_currency.trim().eq_ignore_ascii_case("USD") {
            return Err("admin_grant orders must use USD as the settlement currency".to_string());
        }
        if input.amount_usd != 0.0 || input.pay_amount != 0.0 {
            return Err("admin_grant orders must have zero amounts".to_string());
        }
    } else if input.amount_usd <= 0.0 || input.pay_amount <= 0.0 {
        return Err("paid plan orders must have positive amounts".to_string());
    }
    if !input.exchange_rate.is_finite() || input.exchange_rate <= 0.0 {
        return Err("exchange_rate must be finite and positive".to_string());
    }
    if input.expires_at_unix_secs == 0 || input.expires_at_unix_secs > i64::MAX as u64 {
        return Err("expires_at_unix_secs is outside the supported range".to_string());
    }
    if !input.gateway_response.is_object() {
        return Err("gateway_response must be an object".to_string());
    }

    let Some(snapshot) = input.product_snapshot.as_object() else {
        return Err("product_snapshot must be an object".to_string());
    };
    let Some(snapshot_id) = snapshot.get("id").and_then(serde_json::Value::as_str) else {
        return Err("product_snapshot.id is required".to_string());
    };
    if snapshot_id.trim() != input.product_id.trim() {
        return Err("product_snapshot.id must match product_id".to_string());
    }
    crate::repository::billing::checked_plan_duration_days_from_snapshot(&input.product_snapshot)?;
    if let Some(max_active) = snapshot.get("max_active_per_user") {
        if max_active.as_i64().is_none_or(|value| value <= 0) {
            return Err("product_snapshot.max_active_per_user must be positive".to_string());
        }
    }
    if let Some(scope) = snapshot
        .get("purchase_limit_scope")
        .and_then(serde_json::Value::as_str)
    {
        if !matches!(scope, "active_period" | "lifetime" | "unlimited") {
            return Err("product_snapshot.purchase_limit_scope is invalid".to_string());
        }
    }
    let entitlements = snapshot
        .get("entitlements")
        .or_else(|| snapshot.get("entitlements_json"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    validate_plan_wallet_credit_entitlements(&entitlements)
}

/// Validate a wallet-recharge order before a backend creates its wallet or
/// payment row.  Recharge orders predate the explicit provider/channel
/// columns, so a provider may be absent and a legacy provider-bound row may
/// also omit its channel.  When a channel is present, however, it must be a
/// channel that the corresponding official integration can verify.
pub fn validate_wallet_recharge_order_input(
    input: &CreateWalletRechargeOrderInput,
) -> Result<(), String> {
    fn required_identifier(value: &str, field: &str, max_len: usize) -> Result<(), String> {
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("{field} is required"));
        }
        if value.chars().count() > max_len {
            return Err(format!("{field} exceeds {max_len} characters"));
        }
        if value.chars().any(char::is_control) {
            return Err(format!("{field} contains control characters"));
        }
        Ok(())
    }

    fn optional_identifier(value: Option<&str>, field: &str, max_len: usize) -> Result<(), String> {
        let Some(value) = value else {
            return Ok(());
        };
        required_identifier(value, field, max_len)
    }

    required_identifier(&input.user_id, "user_id", 128)?;
    optional_identifier(
        input.preferred_wallet_id.as_deref(),
        "preferred_wallet_id",
        64,
    )?;
    required_identifier(&input.order_no, "order_no", 128)?;
    required_identifier(&input.gateway_order_id, "gateway_order_id", 128)?;
    let payment_method = canonicalize_payment_method(&input.payment_method)?;
    optional_identifier(input.payment_provider.as_deref(), "payment_provider", 64)?;
    optional_identifier(input.payment_channel.as_deref(), "payment_channel", 64)?;

    if !input.amount_usd.is_finite() || input.amount_usd <= 0.0 {
        return Err("amount_usd must be finite and positive".to_string());
    }
    if input
        .pay_amount
        .is_some_and(|value| !value.is_finite() || value <= 0.0)
    {
        return Err("pay_amount must be finite and positive".to_string());
    }
    if input
        .exchange_rate
        .is_some_and(|value| !value.is_finite() || value <= 0.0)
    {
        return Err("exchange_rate must be finite and positive".to_string());
    }
    if let Some(currency) = input.pay_currency.as_deref() {
        let currency = currency.trim();
        if currency.len() != 3 || !currency.bytes().all(|byte| byte.is_ascii_alphabetic()) {
            return Err("pay_currency must be a 3-letter currency code".to_string());
        }
    }
    if input.expires_at_unix_secs == 0 || input.expires_at_unix_secs > i64::MAX as u64 {
        return Err("expires_at_unix_secs is outside the supported range".to_string());
    }
    if !input.gateway_response.is_object() {
        return Err("gateway_response must be an object".to_string());
    }

    let provider = input
        .payment_provider
        .as_deref()
        .map(canonicalize_payment_method)
        .transpose()?;
    let channel = input
        .payment_channel
        .as_deref()
        .map(canonicalize_payment_method)
        .transpose()?;
    if let Some(provider) = provider.as_deref() {
        // Keep legacy rows that have a provider but no channel readable.  A
        // present channel still goes through the strict official-provider
        // allowlist above, preventing unsupported channels from being stored.
        if let Some(channel) = channel.as_deref() {
            validate_payment_provider_channel_binding(
                &payment_method,
                Some(provider),
                Some(channel),
            )?;
        } else {
            let method_matches = match provider {
                "epay" => matches!(payment_method.as_str(), "epay" | "alipay" | "wxpay"),
                "alipay" | "wxpay" | "stripe" => payment_method == provider,
                "admin" => payment_method == "admin_grant",
                _ => payment_method == provider,
            };
            if !method_matches {
                return Err("payment method and provider binding mismatch".to_string());
            }
        }
    } else if channel.is_some() {
        // Provider-less rows are a legacy shape from before the explicit
        // provider/channel columns existed. Such rows never had a channel;
        // accepting one now would persist an identity that no official
        // callback verifier can bind to a gateway.
        return Err("payment channel requires a payment provider".to_string());
    }
    Ok(())
}

fn wallet_recharge_gateway_identity(
    payment_method: &str,
    payment_provider: Option<&str>,
    payment_channel: Option<&str>,
) -> Option<(String, Option<String>)> {
    let method = canonicalize_payment_method(payment_method).ok()?;
    let provider = payment_provider
        .map(canonicalize_payment_method)
        .transpose()
        .ok()?;
    let channel = payment_channel
        .map(canonicalize_payment_method)
        .transpose()
        .ok()?;

    // Historical EPay orders used alipay/wxpay as the method and either left
    // provider/channel empty or stored provider=epay. Normalize those rows to
    // the same identity as today's method=epay representation.
    if provider.as_deref() == Some("epay")
        || (provider.is_none() && matches!(method.as_str(), "alipay" | "wxpay"))
    {
        let channel = channel
            .or_else(|| matches!(method.as_str(), "alipay" | "wxpay").then(|| method.clone()));
        return Some(("epay".to_string(), channel));
    }

    Some((provider.unwrap_or(method), channel))
}

#[allow(clippy::too_many_arguments)]
pub fn wallet_recharge_replay_matches(
    existing_wallet_id: &str,
    existing_amount_usd: f64,
    existing_pay_amount: Option<f64>,
    existing_pay_currency: Option<&str>,
    existing_exchange_rate: Option<f64>,
    existing_payment_method: &str,
    existing_payment_provider: Option<&str>,
    existing_payment_channel: Option<&str>,
    wallet_id: &str,
    input: &CreateWalletRechargeOrderInput,
) -> bool {
    fn same_number(left: f64, right: f64) -> bool {
        left.is_finite() && right.is_finite() && (left - right).abs() <= 0.00000001
    }
    fn same_optional_number(left: Option<f64>, right: Option<f64>) -> bool {
        match (left, right) {
            (Some(left), Some(right)) => same_number(left, right),
            (None, None) => true,
            _ => false,
        }
    }
    fn same_optional_text(left: Option<&str>, right: Option<&str>) -> bool {
        match (left, right) {
            (Some(left), Some(right)) => left.trim().eq_ignore_ascii_case(right.trim()),
            (None, None) => true,
            _ => false,
        }
    }

    existing_wallet_id == wallet_id
        && same_number(existing_amount_usd, input.amount_usd)
        && same_optional_number(existing_pay_amount, input.pay_amount)
        && same_optional_text(existing_pay_currency, input.pay_currency.as_deref())
        && same_optional_number(existing_exchange_rate, input.exchange_rate)
        && wallet_recharge_gateway_identity(
            existing_payment_method,
            existing_payment_provider,
            existing_payment_channel,
        ) == wallet_recharge_gateway_identity(
            &input.payment_method,
            input.payment_provider.as_deref(),
            input.payment_channel.as_deref(),
        )
}

/// Validate amounts read from a pending payment order before crediting it.
/// A zero-value order is valid only for the server-controlled admin grant
/// namespace; ordinary payment orders must always carry positive amounts.
pub fn validate_payment_order_credit_amounts(
    order_kind: &str,
    payment_method: &str,
    payment_provider: Option<&str>,
    payment_channel: Option<&str>,
    amount_usd: f64,
    pay_amount: Option<f64>,
) -> Result<(), String> {
    let is_admin_grant = order_kind.eq_ignore_ascii_case("plan_purchase")
        && payment_method.eq_ignore_ascii_case("admin_grant")
        && payment_provider.is_some_and(|value| value.trim().eq_ignore_ascii_case("admin"))
        && payment_channel.is_some_and(|value| value.trim().eq_ignore_ascii_case("manual"));
    if !amount_usd.is_finite() || amount_usd < 0.0 {
        return Err("payment order amount is invalid".to_string());
    }
    if pay_amount.is_some_and(|value| !value.is_finite() || value < 0.0) {
        return Err("payment order pay amount is invalid".to_string());
    }
    if is_admin_grant {
        if amount_usd != 0.0 || pay_amount.is_some_and(|value| value != 0.0) {
            return Err("admin_grant payment order amounts are invalid".to_string());
        }
    } else if amount_usd <= 0.0 || pay_amount.is_some_and(|value| value <= 0.0) {
        return Err("payment order amount is invalid".to_string());
    }
    Ok(())
}

/// Match the provider settlement amount against a payment order.
///
/// `pay_amount` was nullable in the original payment-order schema.  New
/// orders carry the provider amount and must compare it exactly (within the
/// storage precision).  For a legacy row that has no provider amount, only a
/// deterministic amount reconstructed from the order's own USD amount,
/// currency, and exchange rate is accepted.  In particular, a callback's
/// self-reported `amount_usd` is not sufficient for an official callback:
/// gateway handlers may intentionally project that field from the order
/// snapshot before reaching the repository.
pub fn payment_callback_amount_matches_order(
    order_amount_usd: f64,
    order_pay_amount: Option<f64>,
    order_pay_currency: Option<&str>,
    order_exchange_rate: Option<f64>,
    callback_amount_usd: f64,
    callback_pay_amount: Option<f64>,
) -> bool {
    const EPSILON: f64 = 0.000001;

    fn valid_positive(value: f64) -> bool {
        value.is_finite() && value > 0.0
    }

    fn rounded_major(value: f64) -> Option<f64> {
        if !value.is_finite() {
            return None;
        }
        let rounded = (value * 100.0).round() / 100.0;
        valid_positive(rounded).then_some(rounded)
    }

    if !valid_positive(order_amount_usd) || !valid_positive(callback_amount_usd) {
        return false;
    }

    match (callback_pay_amount, order_pay_amount) {
        (Some(callback), Some(order)) => {
            valid_positive(callback) && valid_positive(order) && (callback - order).abs() <= EPSILON
        }
        // A provider amount on the callback cannot be accepted against a
        // legacy row unless the expected settlement can be reconstructed from
        // values that were already persisted with that row.
        (Some(callback), None) if valid_positive(callback) => {
            let Some(currency) = order_pay_currency.map(str::trim) else {
                return false;
            };
            let currency = currency.to_ascii_uppercase();
            if currency.len() != 3 || !currency.bytes().all(|byte| byte.is_ascii_alphabetic()) {
                return false;
            }
            // USD is the accounting currency.  Historical rows sometimes
            // stored the old default CNY rate even when the currency was USD;
            // never turn $10 into 72 CNY during compatibility matching.
            let exchange_rate = if currency == "USD" {
                1.0
            } else {
                let Some(rate) = order_exchange_rate else {
                    return false;
                };
                if !valid_positive(rate) {
                    return false;
                }
                rate
            };
            let expected = rounded_major(order_amount_usd * exchange_rate);
            expected.is_some_and(|expected| (callback - expected).abs() <= EPSILON)
        }
        // Keep malformed provider amounts out of the compatibility path.
        (Some(_), None) => false,
        // Callbacks without a provider amount are legacy/non-official input.
        // Keep the old USD fallback, but never use it when the order had a
        // provider amount that the callback omitted.
        (None, None) => (callback_amount_usd - order_amount_usd).abs() <= EPSILON,
        (None, Some(_)) => false,
    }
}

const WALLET_CHECKOUT_SAFE_KEYS: &[&str] = &[
    "gateway",
    "display_name",
    "gateway_order_id",
    "payment_url",
    "payment_params",
    "submit_method",
    "qr_code",
    "expires_at",
    "pay_amount",
    "base_pay_amount",
    "fee_rate",
    "fee_amount",
    "pay_currency",
    "payment_channel",
    "payment_provider",
    "code_url",
    "h5_url",
    "jsapi",
    "publishable_key",
    "intent_id",
    "payment_method_types",
    "provider_label",
    "subject",
    "instructions",
    "callback_url",
    "return_url",
    "integration_status",
    "manual_credit",
    // Internal, server-generated checkout-claim metadata. These fields are
    // deliberately excluded by the public response projection.
    "checkout_claim_token",
    "checkout_claimed_at_unix_secs",
    "failed_at_unix_secs",
    "failure_reason",
];
const WALLET_CHECKOUT_SAFE_EPAY_PARAM_KEYS: &[&str] = &[
    "pid",
    "type",
    "out_trade_no",
    "notify_url",
    "return_url",
    "name",
    "money",
    "sign_type",
    "sign",
];
const STRIPE_CLIENT_SECRET_ENCRYPTED_KEY: &str = "_stripe_client_secret_encrypted";
const PAYMENT_ORDER_STRIPE_CLIENT_SECRET_V2_PREFIX: &str =
    "aether-payment-order-stripe-client-secret-v2:aether-runtime-secret-v1:";
pub const WALLET_RECHARGE_CHECKOUT_CLAIM_LEASE_SECS: u64 = 120;

/// Project a provider checkout response before it is persisted in
/// `payment_orders.gateway_response`.
///
/// Provider responses are untrusted input. Keep this projection in the data
/// contract so callers that bypass the HTTP handler cannot persist credentials,
/// customer data, or arbitrary nested JSON. The raw Stripe `client_secret` is
/// deliberately excluded; the gateway may pass only its encrypted form.
///
/// This generic projection intentionally does not assign an order kind. Plan
/// purchases and wallet recharges share the same gateway response shape, and
/// stamping a wallet discriminator here would misclassify plan orders.
pub fn project_wallet_gateway_response(
    value: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let Some(object) = value.as_object() else {
        return Err("wallet recharge gateway response must be an object".to_string());
    };
    let mut projected = serde_json::Map::new();
    for key in WALLET_CHECKOUT_SAFE_KEYS {
        let Some(item) = object.get(*key) else {
            continue;
        };
        let item = match *key {
            "payment_method_types" => {
                let Some(values) = item.as_array() else {
                    continue;
                };
                let values = values
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| serde_json::Value::String(value.to_string()))
                    .collect::<Vec<_>>();
                if values.is_empty() {
                    continue;
                }
                serde_json::Value::Array(values)
            }
            _ if *key == "payment_params" => {
                let Some(params) = item.as_object() else {
                    continue;
                };
                let mut safe_params = serde_json::Map::new();
                for param_key in WALLET_CHECKOUT_SAFE_EPAY_PARAM_KEYS {
                    if let Some(param_value) = params.get(*param_key) {
                        if param_value.is_string() {
                            safe_params.insert((*param_key).to_string(), param_value.clone());
                        }
                    }
                }
                if safe_params.is_empty() {
                    continue;
                }
                serde_json::Value::Object(safe_params)
            }
            _ if item.is_object() || item.is_array() => continue,
            _ => item.clone(),
        };
        projected.insert((*key).to_string(), item);
    }

    if let Some(encrypted) = object
        .get(STRIPE_CLIENT_SECRET_ENCRYPTED_KEY)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 8192)
    {
        projected.insert(
            STRIPE_CLIENT_SECRET_ENCRYPTED_KEY.to_string(),
            serde_json::Value::String(encrypted.to_string()),
        );
    }
    Ok(serde_json::Value::Object(projected))
}

/// Project a wallet-recharge checkout response and attach its server-controlled
/// discriminator. The in-memory repository uses this marker because it has no
/// dedicated `order_kind` column.
pub fn project_wallet_recharge_gateway_response(
    value: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let mut projected = project_wallet_gateway_response(value)?;
    let Some(object) = projected.as_object_mut() else {
        return Err("wallet recharge gateway response must be an object".to_string());
    };
    object.insert(
        "order_kind".to_string(),
        serde_json::Value::String("wallet_recharge".to_string()),
    );
    Ok(projected)
}

pub fn wallet_recharge_checkout_claim_token(value: &serde_json::Value) -> Option<&str> {
    value
        .as_object()
        .and_then(|object| object.get("checkout_claim_token"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty() && token.len() <= 128)
}

pub fn wallet_recharge_checkout_claimed_at(value: &serde_json::Value) -> Option<u64> {
    value
        .as_object()
        .and_then(|object| object.get("checkout_claimed_at_unix_secs"))
        .and_then(serde_json::Value::as_u64)
}

/// Build the server-controlled placeholder used while an external checkout
/// request is in flight. The response is projected again at this boundary so
/// callers cannot smuggle additional fields into the persisted claim.
pub fn wallet_recharge_checkout_claim_response(
    value: &serde_json::Value,
    claim_token: &str,
    claimed_at_unix_secs: u64,
) -> Result<serde_json::Value, String> {
    let token = claim_token.trim();
    if token.is_empty() || token.len() > 128 || token.chars().any(char::is_control) {
        return Err("wallet recharge checkout claim token is invalid".to_string());
    }
    let mut projected = project_wallet_recharge_gateway_response(value)?;
    let Some(object) = projected.as_object_mut() else {
        return Err("wallet recharge gateway response must be an object".to_string());
    };
    object.insert(
        "order_kind".to_string(),
        serde_json::Value::String("wallet_recharge".to_string()),
    );
    object.insert(
        "integration_status".to_string(),
        serde_json::Value::String("checkout_pending".to_string()),
    );
    object.insert(
        "checkout_claim_token".to_string(),
        serde_json::Value::String(token.to_string()),
    );
    object.insert(
        "checkout_claimed_at_unix_secs".to_string(),
        serde_json::Value::Number(serde_json::Number::from(claimed_at_unix_secs)),
    );
    Ok(projected)
}

pub fn wallet_recharge_checkout_failed_response(
    value: Option<&serde_json::Value>,
    reason: &str,
    failed_at_unix_secs: u64,
) -> serde_json::Value {
    wallet_recharge_checkout_failure_response(value, reason, failed_at_unix_secs, false)
}

/// Record a checkout failure whose provider-side outcome cannot be proven.
///
/// A transport timeout, truncated response, or persistence error can happen
/// after a gateway has accepted the payment request. Such an order remains
/// settleable by a verified callback, but must not be reclaimed for a second
/// checkout because replacing its gateway identifier could strand the first
/// payment.
pub fn wallet_recharge_checkout_uncertain_response(
    value: Option<&serde_json::Value>,
    reason: &str,
    failed_at_unix_secs: u64,
) -> serde_json::Value {
    wallet_recharge_checkout_failure_response(value, reason, failed_at_unix_secs, true)
}

fn wallet_recharge_checkout_failure_response(
    value: Option<&serde_json::Value>,
    reason: &str,
    failed_at_unix_secs: u64,
    provider_request_may_have_succeeded: bool,
) -> serde_json::Value {
    let integration_status = if provider_request_may_have_succeeded {
        "checkout_uncertain"
    } else {
        "checkout_failed"
    };
    let mut projected = value
        .and_then(|value| project_wallet_recharge_gateway_response(value).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let Some(object) = projected.as_object_mut() else {
        return serde_json::json!({
            "order_kind": "wallet_recharge",
            "integration_status": integration_status,
        });
    };
    object.remove("checkout_claim_token");
    object.remove("checkout_claimed_at_unix_secs");
    object.insert(
        "order_kind".to_string(),
        serde_json::Value::String("wallet_recharge".to_string()),
    );
    object.insert(
        "integration_status".to_string(),
        serde_json::Value::String(integration_status.to_string()),
    );
    let reason = reason.trim();
    if !reason.is_empty() {
        let bounded = reason.chars().take(512).collect::<String>();
        object.insert(
            "failure_reason".to_string(),
            serde_json::Value::String(bounded),
        );
    }
    object.insert(
        "failed_at_unix_secs".to_string(),
        serde_json::Value::Number(serde_json::Number::from(failed_at_unix_secs)),
    );
    projected
}

const WALLET_RECHARGE_CHECKOUT_EVIDENCE_KEYS: &[&str] = &[
    "payment_url",
    "payment_params",
    "qr_code",
    "code_url",
    "h5_url",
    "jsapi",
    "client_secret",
    STRIPE_CLIENT_SECRET_ENCRYPTED_KEY,
    "intent_id",
];

/// Whether an order still contains only the server-created checkout claim and
/// has no provider checkout evidence.  This marker is intentionally strict:
/// an order with any provider URL, token, or intent must never be reclaimed.
pub fn wallet_recharge_response_is_checkout_placeholder(value: &serde_json::Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let integration_status = object
        .get("integration_status")
        .and_then(serde_json::Value::as_str);
    if !matches!(
        integration_status,
        Some("checkout_pending" | "checkout_failed" | "checkout_uncertain")
    ) {
        return false;
    }
    if object.get("order_kind").and_then(serde_json::Value::as_str) != Some("wallet_recharge") {
        return false;
    }
    !WALLET_RECHARGE_CHECKOUT_EVIDENCE_KEYS
        .iter()
        .any(|key| object.contains_key(*key))
}

/// Whether a failed payment order is the server-created checkout placeholder
/// that may still be settled by a verified provider callback.
///
/// Checkout failures are persisted before the provider's eventual callback
/// can arrive (for example when the checkout response is lost after the
/// provider accepted it).  Keep this exception narrowly scoped: ordinary
/// failed orders and placeholders that contain any provider checkout evidence
/// must remain non-creditable.
pub fn payment_order_is_failed_wallet_checkout_placeholder(
    order_status: &str,
    order_kind: &str,
    gateway_response: Option<&serde_json::Value>,
) -> bool {
    if !order_status.trim().eq_ignore_ascii_case("failed")
        || !order_kind.trim().eq_ignore_ascii_case("wallet_recharge")
    {
        return false;
    }
    let Some(gateway_response) = gateway_response else {
        return false;
    };
    if !wallet_recharge_response_is_checkout_placeholder(gateway_response) {
        return false;
    }
    gateway_response
        .get("integration_status")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|status| {
            matches!(
                status.trim().to_ascii_lowercase().as_str(),
                "checkout_failed" | "checkout_uncertain"
            )
        })
}

/// Whether a failed wallet checkout was marked as provider-outcome-uncertain.
/// Such orders may be settled by a verified callback even after the local
/// checkout expiry, because the provider may have accepted the request before
/// the response was lost.
pub fn payment_order_is_uncertain_wallet_checkout_placeholder(
    order_status: &str,
    order_kind: &str,
    gateway_response: Option<&serde_json::Value>,
) -> bool {
    if !payment_order_is_failed_wallet_checkout_placeholder(
        order_status,
        order_kind,
        gateway_response,
    ) {
        return false;
    }
    gateway_response
        .and_then(|value| value.get("integration_status"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|status| status.trim().eq_ignore_ascii_case("checkout_uncertain"))
}

pub fn wallet_recharge_order_is_checkout_placeholder(order: &StoredAdminPaymentOrder) -> bool {
    order
        .gateway_response
        .as_ref()
        .is_some_and(wallet_recharge_response_is_checkout_placeholder)
}

/// Convert a persisted timestamp that may use either seconds or milliseconds
/// to epoch seconds.
///
/// The `*_unix_ms` field names predate the current adapters. The in-memory
/// repository stores milliseconds, while the SQL adapters historically
/// selected epoch seconds under the same aliases. Keep this compatibility
/// conversion at the contract boundary so public payloads and lease decisions
/// agree across backends.
pub fn stored_timestamp_unix_secs(value: u64) -> u64 {
    const MILLIS_THRESHOLD: u64 = 10_000_000_000;
    if value >= MILLIS_THRESHOLD {
        value / 1000
    } else {
        value
    }
}

/// Return the creation time in epoch seconds for checkout lease decisions.
pub fn wallet_recharge_order_created_at_unix_secs(order: &StoredAdminPaymentOrder) -> u64 {
    stored_timestamp_unix_secs(order.created_at_unix_ms)
}

/// A failed or expired placeholder may be claimed by one subsequent checkout
/// attempt.  Live pending placeholders remain owned by the request that first
/// created them, which prevents concurrent provider-side duplicate orders.
pub fn wallet_recharge_order_is_reclaimable_placeholder(
    order: &StoredAdminPaymentOrder,
    now_unix_secs: u64,
) -> bool {
    if !wallet_recharge_order_is_checkout_placeholder(order) {
        return false;
    }
    // An uncertain provider result may still arrive as a signed callback.
    // Never replace its gateway identity, even after the original claim lease
    // has elapsed.
    let integration_status = order
        .gateway_response
        .as_ref()
        .and_then(|value| value.get("integration_status"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if integration_status.eq_ignore_ascii_case("checkout_uncertain") {
        return false;
    }
    match order.status.trim().to_ascii_lowercase().as_str() {
        "failed" | "expired" => true,
        "pending" => {
            let claimed_at = order
                .gateway_response
                .as_ref()
                .and_then(wallet_recharge_checkout_claimed_at)
                .unwrap_or_else(|| wallet_recharge_order_created_at_unix_secs(order));
            now_unix_secs.saturating_sub(claimed_at) >= WALLET_RECHARGE_CHECKOUT_CLAIM_LEASE_SECS
        }
        _ => false,
    }
}

/// Enforce the provider namespace for callbacks verified by an official
/// gateway integration. This is a repository boundary check: callers cannot
/// relabel an official payment method as a provider-less generic callback.
pub fn validate_payment_callback_provider_binding(
    payment_method: &str,
    payment_provider: Option<&str>,
) -> Result<(), String> {
    let payment_method = canonicalize_payment_method(payment_method)?;
    let payment_provider = payment_provider
        .map(canonicalize_payment_method)
        .transpose()?;
    let provider_matches = match payment_method.as_str() {
        // Legacy EPay orders stored the selected channel as the method. Keep
        // that explicit, signed aggregator binding valid while rejecting a
        // provider-less generic callback.
        "alipay" | "wxpay" => matches!(
            payment_provider.as_deref(),
            Some(provider) if provider == payment_method || provider == "epay"
        ),
        "stripe" => payment_provider.as_deref() == Some("stripe"),
        "epay" => payment_provider.as_deref() == Some("epay"),
        _ => true,
    };
    if !provider_matches {
        return Err("official payment callback provider binding mismatch".to_string());
    }
    Ok(())
}

/// Match a verified callback's method against the method stored on an order.
///
/// EPay historically stored the selected channel (`alipay` or `wxpay`) in
/// `payment_method`, while the notification endpoint identifies itself as the
/// `epay` provider.  Keep that legacy representation compatible, but only when
/// both sides are in the EPay namespace; the adapter still verifies the
/// explicit payment channel immediately afterwards.
pub fn payment_callback_method_matches_order(
    order_method: &str,
    order_provider: Option<&str>,
    callback_method: &str,
    callback_provider: Option<&str>,
) -> bool {
    if order_method.eq_ignore_ascii_case(callback_method) {
        return true;
    }
    let order_method_is_epay_alias = ["epay", "alipay", "wxpay"]
        .iter()
        .any(|method| method.eq_ignore_ascii_case(order_method));
    let callback_method_is_epay_alias = ["epay", "alipay", "wxpay"]
        .iter()
        .any(|method| method.eq_ignore_ascii_case(callback_method));
    let order_is_epay_namespace = order_provider
        .map(|provider| provider.eq_ignore_ascii_case("epay"))
        .unwrap_or_else(|| {
            // Before payment_provider was added, EPay direct-channel orders
            // persisted alipay/wxpay as payment_method and left the provider
            // column NULL. Keep only those legacy channel rows compatible.
            ["alipay", "wxpay"]
                .iter()
                .any(|method| method.eq_ignore_ascii_case(order_method))
        });
    order_is_epay_namespace
        && callback_provider.is_some_and(|provider| provider.eq_ignore_ascii_case("epay"))
        && order_method_is_epay_alias
        && callback_method_is_epay_alias
}

/// Match the provider namespace stored on a payment order against a verified
/// callback. Historical EPay channel orders predate `payment_provider` and
/// therefore have a NULL provider; only their explicit alipay/wxpay method
/// aliases may be upgraded by an EPay callback.
pub fn payment_callback_provider_matches_order(
    order_method: &str,
    order_provider: Option<&str>,
    callback_method: &str,
    callback_provider: Option<&str>,
) -> bool {
    match (order_provider, callback_provider) {
        (None, None) => true,
        (Some(order), Some(callback)) => order.eq_ignore_ascii_case(callback),
        (None, Some(callback)) => {
            callback.eq_ignore_ascii_case("epay")
                && ["alipay", "wxpay"]
                    .iter()
                    .any(|method| method.eq_ignore_ascii_case(order_method))
                && ["epay", "alipay", "wxpay"]
                    .iter()
                    .any(|method| method.eq_ignore_ascii_case(callback_method))
        }
        _ => false,
    }
}

impl ProcessPaymentCallbackInput {
    pub fn canonicalize_and_validate(&mut self) -> Result<(), String> {
        self.payment_method = canonicalize_payment_method(&self.payment_method)?;
        self.payment_provider = self
            .payment_provider
            .as_deref()
            .map(canonicalize_payment_method)
            .transpose()?;
        self.payment_channel = self
            .payment_channel
            .as_deref()
            .map(canonicalize_payment_method)
            .transpose()?;
        validate_payment_callback_provider_binding(
            &self.payment_method,
            self.payment_provider.as_deref(),
        )?;
        validate_payment_provider_channel_binding(
            &self.payment_method,
            self.payment_provider.as_deref(),
            self.payment_channel.as_deref(),
        )?;

        if self.payment_provider.is_some()
            && (self.payment_channel.is_none()
                || self
                    .order_no
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
                || self
                    .gateway_order_id
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
                || self.pay_amount.is_none()
                || self
                    .pay_currency
                    .as_deref()
                    .is_none_or(|value| value.trim().len() != 3))
        {
            return Err(
                "official payment callback is missing settlement binding fields".to_string(),
            );
        }
        Ok(())
    }

    /// Return the only callback data that may be copied to a payment order.
    ///
    /// `payload` is provider-controlled (and may contain credentials, payment
    /// capabilities, or customer data), so adapters must never persist it as
    /// `payment_orders.gateway_response`. Keep this projection at the shared
    /// contract boundary so every database backend applies the same allowlist,
    /// including callers that bypass the HTTP handler's projection.
    pub fn gateway_response_projection(
        &self,
        order_no: &str,
        gateway_order_id: Option<&str>,
    ) -> serde_json::Value {
        serde_json::json!({
            "gateway": self.payment_method,
            "payment_provider": self.payment_provider,
            "payment_channel": self.payment_channel,
            "order_no": order_no,
            "gateway_order_id": gateway_order_id,
            "amount_usd": self.amount_usd,
            "pay_amount": self.pay_amount,
            "pay_currency": self.pay_currency,
            "exchange_rate": self.exchange_rate,
            "signature_valid": self.signature_valid,
        })
    }
}

pub fn redeem_code_refundable_amount(balance_bucket: &str, amount_usd: f64) -> f64 {
    if redeem_code_credits_recharge_balance(balance_bucket) {
        amount_usd
    } else {
        0.0
    }
}

pub fn validate_admin_redeem_code_batch_input(
    input: &CreateAdminRedeemCodeBatchInput,
) -> Result<(), String> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err("redeem code batch name is invalid".to_string());
    }
    if !input.amount_usd.is_finite() || input.amount_usd <= 0.0 {
        return Err("redeem code batch amount_usd must be finite and positive".to_string());
    }
    if !input.currency.trim().eq_ignore_ascii_case("USD") {
        return Err("redeem code batch currency must be USD".to_string());
    }
    if !matches!(input.balance_bucket.trim(), "gift" | "recharge") {
        return Err("redeem code batch balance_bucket must be gift or recharge".to_string());
    }
    if input.total_count == 0 || input.total_count > 5_000 {
        return Err("redeem code batch total_count must be between 1 and 5000".to_string());
    }
    if input
        .expires_at_unix_secs
        .is_some_and(|value| value == 0 || value > i64::MAX as u64)
    {
        return Err(
            "redeem code batch expires_at_unix_secs is outside the supported range".to_string(),
        );
    }
    Ok(())
}

/// Validate a persisted redeem batch and the wallet arithmetic before any
/// credit is applied. Imported rows can bypass repository input validation,
/// so redemption must fail closed when either side contains invalid amounts.
pub fn validate_redeem_wallet_credit(
    balance_bucket: &str,
    amount_usd: f64,
    before_recharge: f64,
    before_gift: f64,
    before_total_recharged: f64,
) -> Result<(f64, f64, f64), String> {
    let balance_bucket = balance_bucket.trim();
    if !matches!(balance_bucket, "gift" | "recharge") {
        return Err("redeem code batch balance bucket is invalid".to_string());
    }
    if !amount_usd.is_finite() || amount_usd <= 0.0 {
        return Err("redeem code batch amount is invalid".to_string());
    }
    if !before_recharge.is_finite()
        || !before_gift.is_finite()
        || !before_total_recharged.is_finite()
        || before_gift < 0.0
        || before_total_recharged < 0.0
        || !(before_recharge + before_gift).is_finite()
    {
        return Err("wallet amount is invalid".to_string());
    }

    let after_recharge = if balance_bucket == "recharge" {
        before_recharge + amount_usd
    } else {
        before_recharge
    };
    let after_gift = if balance_bucket == "recharge" {
        before_gift
    } else {
        before_gift + amount_usd
    };
    let after_total_recharged = before_total_recharged + amount_usd;
    if !after_recharge.is_finite()
        || !after_gift.is_finite()
        || !after_total_recharged.is_finite()
        || !(after_recharge + after_gift).is_finite()
    {
        return Err("redeem wallet credit would overflow".to_string());
    }
    Ok((after_recharge, after_gift, after_total_recharged))
}

/// Validate a manual recharge and calculate the exact wallet values that may
/// be persisted. Manual recharges are also invoked by import and recovery
/// workflows, so the repository cannot rely on the admin HTTP validator.
pub fn validate_manual_wallet_recharge(
    amount_usd: f64,
    before_recharge: f64,
    before_gift: f64,
    before_total_recharged: f64,
) -> Result<(f64, f64), String> {
    if !amount_usd.is_finite() || amount_usd <= 0.0 {
        return Err("manual recharge amount must be finite and positive".to_string());
    }
    if !before_recharge.is_finite()
        || !before_gift.is_finite()
        || before_gift < 0.0
        || !before_total_recharged.is_finite()
        || before_total_recharged < 0.0
        || !(before_recharge + before_gift).is_finite()
    {
        return Err("wallet amount is invalid for manual recharge".to_string());
    }

    let after_recharge = before_recharge + amount_usd;
    let after_total_recharged = before_total_recharged + amount_usd;
    if !after_recharge.is_finite()
        || !after_total_recharged.is_finite()
        || !(after_recharge + before_gift).is_finite()
    {
        return Err("manual recharge would overflow wallet amounts".to_string());
    }
    Ok((after_recharge, after_total_recharged))
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum RedeemWalletCodeOutcome {
    Redeemed {
        wallet: StoredWalletSnapshot,
        order: StoredAdminPaymentOrder,
        amount_usd: f64,
        batch_name: String,
    },
    InvalidCode,
    CodeNotFound,
    CodeDisabled,
    BatchDisabled,
    CodeExpired,
    CodeRedeemed,
    WalletInactive,
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateWalletRechargeOrderInput {
    pub preferred_wallet_id: Option<String>,
    pub user_id: String,
    pub amount_usd: f64,
    pub pay_amount: Option<f64>,
    pub pay_currency: Option<String>,
    pub exchange_rate: Option<f64>,
    pub payment_method: String,
    pub payment_provider: Option<String>,
    pub payment_channel: Option<String>,
    pub gateway_order_id: String,
    pub gateway_response: serde_json::Value,
    pub order_no: String,
    pub expires_at_unix_secs: u64,
}

impl std::fmt::Debug for CreateWalletRechargeOrderInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CreateWalletRechargeOrderInput")
            .field("preferred_wallet_id", &self.preferred_wallet_id)
            .field("user_id", &self.user_id)
            .field("amount_usd", &self.amount_usd)
            .field("payment_method", &self.payment_method)
            .field("payment_provider", &self.payment_provider)
            .field("payment_channel", &self.payment_channel)
            .field("gateway_order_id", &self.gateway_order_id)
            .field("gateway_response", &WALLET_REDACTED_DEBUG_VALUE)
            .field("order_no", &self.order_no)
            .field("expires_at_unix_secs", &self.expires_at_unix_secs)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum CreateWalletRechargeOrderOutcome {
    Created(StoredAdminPaymentOrder),
    Existing(StoredAdminPaymentOrder),
    WalletInactive,
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpdateWalletRechargeCheckoutInput {
    pub order_id: String,
    pub gateway_order_id: String,
    pub gateway_response: serde_json::Value,
}

impl std::fmt::Debug for UpdateWalletRechargeCheckoutInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UpdateWalletRechargeCheckoutInput")
            .field("order_id", &self.order_id)
            .field("gateway_order_id", &self.gateway_order_id)
            .field("gateway_response", &WALLET_REDACTED_DEBUG_VALUE)
            .finish()
    }
}

/// Exact compare-and-swap for lazily migrating one Stripe client-secret field.
///
/// Every immutable identity component and the complete observed gateway
/// response are included so a stale reader cannot overwrite another checkout
/// update or move a capability between payment orders.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CompareAndSwapPaymentOrderStripeClientSecretInput {
    pub order_id: String,
    pub order_no: String,
    pub wallet_id: String,
    pub user_id: Option<String>,
    pub payment_method: String,
    pub payment_provider: Option<String>,
    pub order_kind: String,
    pub gateway_order_id: Option<String>,
    pub expected_status: String,
    pub expected_expires_at_unix_secs: Option<u64>,
    pub expected_gateway_response: serde_json::Value,
    pub expected_client_secret_encrypted: String,
    pub replacement_client_secret_encrypted: String,
}

impl std::fmt::Debug for CompareAndSwapPaymentOrderStripeClientSecretInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompareAndSwapPaymentOrderStripeClientSecretInput")
            .field("order_id", &self.order_id)
            .field("order_no", &self.order_no)
            .field("wallet_id", &self.wallet_id)
            .field("user_id", &self.user_id)
            .field("payment_method", &self.payment_method)
            .field("payment_provider", &self.payment_provider)
            .field("order_kind", &self.order_kind)
            .field("gateway_order_id", &self.gateway_order_id)
            .field("expected_status", &self.expected_status)
            .field(
                "expected_expires_at_unix_secs",
                &self.expected_expires_at_unix_secs,
            )
            .field("expected_gateway_response", &WALLET_REDACTED_DEBUG_VALUE)
            .field(
                "expected_client_secret_encrypted",
                &WALLET_REDACTED_DEBUG_VALUE,
            )
            .field(
                "replacement_client_secret_encrypted",
                &WALLET_REDACTED_DEBUG_VALUE,
            )
            .finish()
    }
}

/// Validate a Stripe-secret migration against a row locked by a repository
/// implementation and build the exact replacement response.
///
/// `Ok(None)` is a normal CAS miss. Invalid replacement values are rejected as
/// input errors; all unrelated JSON fields are preserved byte-for-value.
pub fn payment_order_stripe_client_secret_cas_replacement(
    current: &StoredAdminPaymentOrder,
    input: &CompareAndSwapPaymentOrderStripeClientSecretInput,
) -> Result<Option<serde_json::Value>, String> {
    let replacement_ciphertext = input
        .replacement_client_secret_encrypted
        .strip_prefix(PAYMENT_ORDER_STRIPE_CLIENT_SECRET_V2_PREFIX);
    if input.replacement_client_secret_encrypted.len() > 8192
        || replacement_ciphertext.is_none_or(|ciphertext| ciphertext.is_empty())
        || input
            .replacement_client_secret_encrypted
            .chars()
            .any(char::is_control)
    {
        return Err("replacement Stripe client-secret envelope is invalid".to_string());
    }
    if current.id != input.order_id
        || current.order_no != input.order_no
        || current.wallet_id != input.wallet_id
        || current.user_id != input.user_id
        || current.payment_method != input.payment_method
        || current.payment_provider != input.payment_provider
        || current.order_kind != input.order_kind
        || current.gateway_order_id != input.gateway_order_id
        || current.status != input.expected_status
        || current.expires_at_unix_secs != input.expected_expires_at_unix_secs
        || current.gateway_response.as_ref() != Some(&input.expected_gateway_response)
    {
        return Ok(None);
    }
    let Some(object) = input.expected_gateway_response.as_object() else {
        return Ok(None);
    };
    if object
        .get(STRIPE_CLIENT_SECRET_ENCRYPTED_KEY)
        .and_then(serde_json::Value::as_str)
        != Some(input.expected_client_secret_encrypted.as_str())
    {
        return Ok(None);
    }
    let mut replacement = input.expected_gateway_response.clone();
    let Some(object) = replacement.as_object_mut() else {
        return Ok(None);
    };
    object.insert(
        STRIPE_CLIENT_SECRET_ENCRYPTED_KEY.to_string(),
        serde_json::Value::String(input.replacement_client_secret_encrypted.clone()),
    );
    Ok(Some(replacement))
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FailWalletRechargeCheckoutInput {
    pub order_id: String,
    /// The claim token assigned to the request that created/reclaimed the
    /// placeholder. Requiring it prevents a slow error from invalidating a
    /// newer retry that has already taken over the order.
    pub claim_token: String,
    pub reason: String,
    /// True when the provider request may have been accepted even though the
    /// gateway response was not durably observed. Such failures are marked
    /// `checkout_uncertain` and cannot be reclaimed for another checkout.
    #[serde(default)]
    pub provider_request_may_have_succeeded: bool,
}

impl std::fmt::Debug for FailWalletRechargeCheckoutInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FailWalletRechargeCheckoutInput")
            .field("order_id", &self.order_id)
            .field("claim_token", &WALLET_REDACTED_DEBUG_VALUE)
            .field("reason", &WALLET_REDACTED_DEBUG_VALUE)
            .field(
                "provider_request_may_have_succeeded",
                &self.provider_request_may_have_succeeded,
            )
            .finish()
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReclaimWalletRechargeCheckoutInput {
    pub order_id: String,
    pub claim_token: String,
    pub gateway_response: serde_json::Value,
    pub expires_at_unix_secs: u64,
}

impl std::fmt::Debug for ReclaimWalletRechargeCheckoutInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReclaimWalletRechargeCheckoutInput")
            .field("order_id", &self.order_id)
            .field("claim_token", &WALLET_REDACTED_DEBUG_VALUE)
            .field("gateway_response", &WALLET_REDACTED_DEBUG_VALUE)
            .field("expires_at_unix_secs", &self.expires_at_unix_secs)
            .finish()
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreatePlanPurchaseOrderInput {
    pub preferred_wallet_id: Option<String>,
    pub user_id: String,
    pub amount_usd: f64,
    pub pay_amount: f64,
    pub pay_currency: String,
    pub exchange_rate: f64,
    pub payment_method: String,
    pub payment_provider: Option<String>,
    pub payment_channel: Option<String>,
    pub gateway_order_id: String,
    pub gateway_response: serde_json::Value,
    pub order_no: String,
    pub product_id: String,
    pub product_snapshot: serde_json::Value,
    pub expires_at_unix_secs: u64,
}

impl std::fmt::Debug for CreatePlanPurchaseOrderInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CreatePlanPurchaseOrderInput")
            .field("preferred_wallet_id", &self.preferred_wallet_id)
            .field("user_id", &self.user_id)
            .field("amount_usd", &self.amount_usd)
            .field("payment_method", &self.payment_method)
            .field("payment_provider", &self.payment_provider)
            .field("payment_channel", &self.payment_channel)
            .field("gateway_order_id", &self.gateway_order_id)
            .field("gateway_response", &WALLET_REDACTED_DEBUG_VALUE)
            .field("order_no", &self.order_no)
            .field("product_id", &self.product_id)
            .field("expires_at_unix_secs", &self.expires_at_unix_secs)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum CreatePlanPurchaseOrderOutcome {
    Created(StoredAdminPaymentOrder),
    WalletInactive,
    ActivePlanLimitReached,
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CanonicalWalletRefundFields {
    pub source_type: String,
    pub source_id: Option<String>,
    pub refund_mode: String,
}

/// Validate and canonicalize refund provenance fields at the data boundary.
/// The client may omit fields, but it must not be able to select a different
/// refund route or claim a different source than the server-resolved order.
pub fn canonicalize_wallet_refund_fields(
    payment_order_id: Option<&str>,
    source_type: Option<&str>,
    source_id: Option<&str>,
    refund_mode: Option<&str>,
    payment_method: Option<&str>,
) -> Result<CanonicalWalletRefundFields, String> {
    fn normalize(value: Option<&str>) -> Option<&str> {
        value.map(str::trim).filter(|value| !value.is_empty())
    }

    if let Some(order_id) = normalize(payment_order_id) {
        let Some(payment_method) = normalize(payment_method) else {
            return Err("payment method is required for an order refund".to_string());
        };
        let expected_mode = default_refund_mode_for_payment_method(payment_method);
        if let Some(value) = normalize(source_type) {
            if !value.eq_ignore_ascii_case("payment_order") {
                return Err("source_type does not match payment_order".to_string());
            }
        }
        if let Some(value) = normalize(source_id) {
            if value != order_id {
                return Err("source_id does not match payment_order_id".to_string());
            }
        }
        if let Some(value) = normalize(refund_mode) {
            if !value.eq_ignore_ascii_case(expected_mode) {
                return Err("refund_mode does not match the payment method".to_string());
            }
        }
        return Ok(CanonicalWalletRefundFields {
            source_type: "payment_order".to_string(),
            source_id: Some(order_id.to_string()),
            refund_mode: expected_mode.to_string(),
        });
    }

    if normalize(payment_method).is_some() {
        return Err("payment method is only valid for an order refund".to_string());
    }
    if let Some(value) = normalize(source_type) {
        if !value.eq_ignore_ascii_case("wallet_balance") {
            return Err("source_type must be wallet_balance without an order".to_string());
        }
    }
    if normalize(source_id).is_some() {
        return Err("source_id is not valid without a payment order".to_string());
    }
    if let Some(value) = normalize(refund_mode) {
        if !value.eq_ignore_ascii_case("offline_payout") {
            return Err("refund_mode must be offline_payout without an order".to_string());
        }
    }
    Ok(CanonicalWalletRefundFields {
        source_type: "wallet_balance".to_string(),
        source_id: None,
        refund_mode: "offline_payout".to_string(),
    })
}

fn default_refund_mode_for_payment_method(payment_method: &str) -> &'static str {
    if matches!(
        payment_method.trim().to_ascii_lowercase().as_str(),
        "admin_manual" | "card_recharge" | "card_code" | "gift_code"
    ) {
        return "offline_payout";
    }
    "original_channel"
}

/// Validate the durable accounting split for a refundable payment order.
///
/// Database numeric conversions and repeated partial refunds can introduce a
/// very small rounding delta, so compare the invariant with a sub-cent
/// tolerance while still rejecting malformed or out-of-range components.
pub fn payment_order_refund_amounts_are_consistent(
    amount_usd: f64,
    refunded_amount_usd: f64,
    refundable_amount_usd: f64,
) -> bool {
    const REFUND_AMOUNT_EPSILON_USD: f64 = 0.000_001;

    amount_usd.is_finite()
        && amount_usd > 0.0
        && refunded_amount_usd.is_finite()
        && refunded_amount_usd >= 0.0
        && refunded_amount_usd <= amount_usd + REFUND_AMOUNT_EPSILON_USD
        && refundable_amount_usd.is_finite()
        && refundable_amount_usd >= 0.0
        && refundable_amount_usd <= amount_usd + REFUND_AMOUNT_EPSILON_USD
        && (refunded_amount_usd + refundable_amount_usd - amount_usd).abs()
            <= REFUND_AMOUNT_EPSILON_USD
}

/// Return whether a persisted refund proof represents a terminal successful
/// gateway settlement. Gateway evidence is stored either directly (the
/// pending response) or under the `gateway_refund` projection (the successful
/// completion response), so accept both shapes while ignoring arbitrary
/// nested payloads.
pub fn wallet_refund_proof_is_success(value: &serde_json::Value) -> bool {
    fn status_is_success(value: Option<&serde_json::Value>) -> bool {
        value
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .is_some_and(|status| {
                status.eq_ignore_ascii_case("success") || status.eq_ignore_ascii_case("succeeded")
            })
    }

    let Some(object) = value.as_object() else {
        return false;
    };
    if let Some(gateway_refund) = object.get("gateway_refund") {
        return gateway_refund
            .as_object()
            .and_then(|gateway_refund| gateway_refund.get("status"))
            .is_some_and(|status| status_is_success(Some(status)));
    }
    status_is_success(object.get("status"))
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum CreateWalletRefundRequestOutcome {
    Created(StoredAdminWalletRefund),
    Duplicate(StoredAdminWalletRefund),
    InvalidInput(String),
    WalletMissing,
    RefundAmountExceedsAvailableBalance,
    PaymentOrderNotFound,
    PaymentOrderNotRefundable,
    RefundAmountExceedsAvailableOrderAmount,
    DuplicateRejected,
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProcessPaymentCallbackInput {
    pub payment_method: String,
    pub payment_provider: Option<String>,
    pub payment_channel: Option<String>,
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

impl std::fmt::Debug for ProcessPaymentCallbackInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProcessPaymentCallbackInput")
            .field("payment_method", &self.payment_method)
            .field("payment_provider", &self.payment_provider)
            .field("payment_channel", &self.payment_channel)
            .field("callback_key", &WALLET_REDACTED_DEBUG_VALUE)
            .field("order_no", &self.order_no)
            .field("gateway_order_id", &self.gateway_order_id)
            .field("amount_usd", &self.amount_usd)
            .field("payload_hash", &WALLET_REDACTED_DEBUG_VALUE)
            .field("payload", &WALLET_REDACTED_DEBUG_VALUE)
            .field("signature_valid", &self.signature_valid)
            .finish_non_exhaustive()
    }
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

/// Records a provider response while the local refund remains processing.
/// This persists asynchronous gateway evidence without releasing the wallet
/// reservation or changing the refund state.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpdateAdminWalletRefundGatewayInput {
    pub wallet_id: String,
    pub refund_id: String,
    pub gateway_refund_id: String,
    pub payout_proof: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FailAdminWalletRefundInput {
    pub wallet_id: String,
    pub refund_id: String,
    pub reason: String,
    pub operator_id: Option<String>,
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreditAdminPaymentOrderInput {
    pub order_id: String,
    pub gateway_order_id: Option<String>,
    pub pay_amount: Option<f64>,
    pub pay_currency: Option<String>,
    pub exchange_rate: Option<f64>,
    pub gateway_response_patch: Option<serde_json::Value>,
    pub operator_id: Option<String>,
}

impl std::fmt::Debug for CreditAdminPaymentOrderInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CreditAdminPaymentOrderInput")
            .field("order_id", &self.order_id)
            .field("gateway_order_id", &self.gateway_order_id)
            .field("pay_amount", &self.pay_amount)
            .field("pay_currency", &self.pay_currency)
            .field("exchange_rate", &self.exchange_rate)
            .field(
                "gateway_response_patch",
                &wallet_redacted_debug_option(&self.gateway_response_patch),
            )
            .field("operator_id", &self.operator_id)
            .finish()
    }
}

#[async_trait]
pub trait WalletReadRepository: Send + Sync {
    async fn find(
        &self,
        key: WalletLookupKey<'_>,
    ) -> Result<Option<StoredWalletSnapshot>, crate::DataLayerError>;

    async fn update_auth_user_wallet_limit_mode(
        &self,
        user_id: &str,
        limit_mode: &str,
    ) -> Result<Option<StoredWalletSnapshot>, crate::DataLayerError>;

    async fn update_auth_api_key_wallet_limit_mode(
        &self,
        api_key_id: &str,
        limit_mode: &str,
    ) -> Result<Option<StoredWalletSnapshot>, crate::DataLayerError>;

    async fn initialize_auth_user_wallet(
        &self,
        user_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<StoredWalletSnapshot>, crate::DataLayerError>;

    /// Atomically initialize a user wallet and report whether this call won
    /// the create race. Implementations that cannot provide the creation bit
    /// must fail explicitly; callers use it to decide whether compensation is
    /// allowed during an aggregate import rollback.
    async fn initialize_auth_user_wallet_with_outcome(
        &self,
        user_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<InitializeAuthWalletOutcome>, crate::DataLayerError> {
        let _ = (user_id, initial_gift_usd, unlimited);
        Err(crate::DataLayerError::InvalidInput(
            "atomic user wallet initialization is not available".to_string(),
        ))
    }

    async fn initialize_auth_api_key_wallet(
        &self,
        api_key_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<StoredWalletSnapshot>, crate::DataLayerError>;

    /// Atomically initialize an API-key wallet and report whether this call
    /// created the row. See the user-wallet variant for the rollback contract.
    async fn initialize_auth_api_key_wallet_with_outcome(
        &self,
        api_key_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<InitializeAuthWalletOutcome>, crate::DataLayerError> {
        let _ = (api_key_id, initial_gift_usd, unlimited);
        Err(crate::DataLayerError::InvalidInput(
            "atomic API-key wallet initialization is not available".to_string(),
        ))
    }

    #[allow(clippy::too_many_arguments)]
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
    ) -> Result<Option<StoredWalletSnapshot>, crate::DataLayerError>;

    #[allow(clippy::too_many_arguments)]
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

    async fn count_pending_refunds_by_user_id(
        &self,
        user_id: &str,
    ) -> Result<u64, crate::DataLayerError>;

    async fn count_pending_payment_orders_by_user_id(
        &self,
        user_id: &str,
    ) -> Result<u64, crate::DataLayerError>;

    async fn find_wallet_payment_order_by_user_id(
        &self,
        user_id: &str,
        order_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, crate::DataLayerError>;

    /// Finds a wallet-recharge order by the stable merchant order number.
    ///
    /// Public recharge retries use a deterministic order number derived from
    /// their idempotency key.  Keeping this lookup on the repository boundary
    /// lets the gateway replay the original checkout without contacting the
    /// payment provider again.
    async fn find_wallet_recharge_order_by_order_no(
        &self,
        user_id: &str,
        order_no: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, crate::DataLayerError> {
        let _ = (user_id, order_no);
        Ok(None)
    }

    async fn find_pending_plan_purchase_order_by_user_id(
        &self,
        user_id: &str,
        product_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, crate::DataLayerError>;

    /// Finds any payment order by its globally unique merchant order number.
    /// Checkout uses this narrow lookup before contacting a provider so a
    /// deterministic retry key cannot collide with an older terminal order.
    async fn find_payment_order_by_order_no(
        &self,
        order_no: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, crate::DataLayerError> {
        let _ = order_no;
        Ok(None)
    }

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

    async fn list_admin_redeem_code_batches(
        &self,
        query: &AdminRedeemCodeBatchListQuery,
    ) -> Result<StoredAdminRedeemCodeBatchPage, crate::DataLayerError>;

    async fn find_admin_redeem_code_batch(
        &self,
        batch_id: &str,
    ) -> Result<Option<StoredAdminRedeemCodeBatch>, crate::DataLayerError>;

    async fn list_admin_redeem_codes(
        &self,
        query: &AdminRedeemCodeListQuery,
    ) -> Result<StoredAdminRedeemCodePage, crate::DataLayerError>;
}

#[async_trait]
pub trait WalletWriteRepository: Send + Sync {
    /// Delete one wallet identified by its exact id and owner, but only when it has no
    /// financial or usage references. This is reserved for compensating a wallet created by
    /// an operation that has not completed; it must never be used to erase an established
    /// wallet by owner lookup alone.
    async fn delete_wallet_if_unreferenced(
        &self,
        wallet_id: &str,
        owner: WalletLookupKey<'_>,
    ) -> Result<bool, crate::DataLayerError>;

    /// Delete a wallet only when its complete persisted snapshot still matches
    /// the snapshot captured by the compensating operation and it has no
    /// financial, usage, or redemption references.  Unlike the zero-balance
    /// helper above, this is intentionally suitable for rolling back an
    /// imported wallet whose snapshot contains a non-zero balance.
    async fn delete_wallet_if_snapshot_matches_and_unreferenced(
        &self,
        expected: &StoredWalletSnapshot,
        owner: WalletLookupKey<'_>,
    ) -> Result<bool, crate::DataLayerError>;

    /// Restore an existing wallet to its pre-import snapshot only when the
    /// persisted row still exactly matches the post-import snapshot captured
    /// by the same operation.  Implementations must perform the compare and
    /// update while holding the wallet row/lifecycle lock; a mismatch, missing
    /// row, or owner mismatch returns `false` without changing the wallet.
    async fn restore_wallet_if_snapshot_matches(
        &self,
        before: &StoredWalletSnapshot,
        after: &StoredWalletSnapshot,
        owner: WalletLookupKey<'_>,
    ) -> Result<bool, crate::DataLayerError>;

    /// Remove the exact user wallet created during an incomplete authentication provisioning
    /// flow, but only when it is still an untouched initial wallet. Requiring both the wallet id
    /// and owner prevents a failed initializer from deleting a wallet created concurrently by a
    /// different operation.
    async fn delete_provisional_auth_user_wallet(
        &self,
        wallet_id: &str,
        user_id: &str,
    ) -> Result<bool, crate::DataLayerError>;

    async fn create_wallet_recharge_order(
        &self,
        input: CreateWalletRechargeOrderInput,
    ) -> Result<CreateWalletRechargeOrderOutcome, crate::DataLayerError>;

    async fn update_wallet_recharge_checkout(
        &self,
        input: UpdateWalletRechargeCheckoutInput,
    ) -> Result<WalletMutationOutcome<StoredAdminPaymentOrder>, crate::DataLayerError>;

    /// Replace only the Stripe client-secret envelope when the complete
    /// payment-order identity and observed gateway response still match.
    async fn compare_and_swap_payment_order_stripe_client_secret(
        &self,
        input: CompareAndSwapPaymentOrderStripeClientSecretInput,
    ) -> Result<bool, crate::DataLayerError> {
        let _ = input;
        Ok(false)
    }

    /// Mark the currently claimed checkout placeholder as failed.  The
    /// expected expiry makes this conditional on the caller's claim, so a
    /// delayed error cannot invalidate a newer retry.
    async fn fail_wallet_recharge_checkout(
        &self,
        input: FailWalletRechargeCheckoutInput,
    ) -> Result<WalletMutationOutcome<StoredAdminPaymentOrder>, crate::DataLayerError> {
        let _ = input;
        Ok(WalletMutationOutcome::Invalid(
            "wallet recharge checkout failure handling is not available".to_string(),
        ))
    }

    /// Atomically take over a failed, expired, or timed-out checkout
    /// placeholder. Implementations must lock/compare the current claim before
    /// writing the new token, so at most one caller proceeds to the provider.
    async fn reclaim_wallet_recharge_checkout(
        &self,
        input: ReclaimWalletRechargeCheckoutInput,
    ) -> Result<WalletMutationOutcome<StoredAdminPaymentOrder>, crate::DataLayerError> {
        let _ = input;
        Ok(WalletMutationOutcome::Invalid(
            "wallet recharge checkout reclaim is not available".to_string(),
        ))
    }

    async fn create_plan_purchase_order(
        &self,
        input: CreatePlanPurchaseOrderInput,
    ) -> Result<CreatePlanPurchaseOrderOutcome, crate::DataLayerError> {
        let _ = input;
        Err(crate::DataLayerError::InvalidInput(
            "plan purchase order creation is not available".to_string(),
        ))
    }

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

    async fn update_admin_wallet_refund_gateway(
        &self,
        input: UpdateAdminWalletRefundGatewayInput,
    ) -> Result<WalletMutationOutcome<StoredAdminWalletRefund>, crate::DataLayerError>;

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

    async fn create_admin_redeem_code_batch(
        &self,
        input: CreateAdminRedeemCodeBatchInput,
    ) -> Result<CreateAdminRedeemCodeBatchResult, crate::DataLayerError>;

    async fn disable_admin_redeem_code_batch(
        &self,
        input: DisableAdminRedeemCodeBatchInput,
    ) -> Result<WalletMutationOutcome<StoredAdminRedeemCodeBatch>, crate::DataLayerError>;

    async fn delete_admin_redeem_code_batch(
        &self,
        input: DeleteAdminRedeemCodeBatchInput,
    ) -> Result<WalletMutationOutcome<StoredAdminRedeemCodeBatch>, crate::DataLayerError>;

    async fn disable_admin_redeem_code(
        &self,
        input: DisableAdminRedeemCodeInput,
    ) -> Result<WalletMutationOutcome<StoredAdminRedeemCode>, crate::DataLayerError>;

    async fn redeem_wallet_code(
        &self,
        input: RedeemWalletCodeInput,
    ) -> Result<RedeemWalletCodeOutcome, crate::DataLayerError>;
}

pub trait WalletRepository: WalletReadRepository + WalletWriteRepository + Send + Sync {}

impl<T> WalletRepository for T where T: WalletReadRepository + WalletWriteRepository + Send + Sync {}

#[cfg(test)]
mod tests {
    use super::{
        canonicalize_payment_method, payment_callback_amount_matches_order,
        payment_callback_method_matches_order, payment_callback_provider_matches_order,
        payment_order_is_failed_wallet_checkout_placeholder,
        payment_order_refund_amounts_are_consistent,
        payment_order_stripe_client_secret_cas_replacement, project_wallet_gateway_response,
        project_wallet_recharge_gateway_response, redeem_code_credits_recharge_balance,
        redeem_code_payment_method, redeem_code_refundable_amount, validate_manual_wallet_recharge,
        validate_payment_callback_provider_binding, validate_payment_order_credit_amounts,
        validate_payment_provider_channel_binding, validate_plan_purchase_order_input,
        validate_plan_wallet_credit_entitlements, validate_wallet_recharge_order_input,
        wallet_recharge_order_created_at_unix_secs, wallet_refund_proof_is_success,
        CompareAndSwapPaymentOrderStripeClientSecretInput, CreatePlanPurchaseOrderInput,
        CreateWalletRechargeOrderInput, ProcessPaymentCallbackInput, StoredAdminPaymentOrder,
        StoredWalletSnapshot,
    };
    use crate::repository::settlement::UsageSettlementInput;

    fn stripe_secret_cas_fixture() -> (
        StoredAdminPaymentOrder,
        CompareAndSwapPaymentOrderStripeClientSecretInput,
    ) {
        let legacy = "gAAAAABlegacy-ciphertext";
        let gateway_response = serde_json::json!({
            "gateway": "stripe",
            "publishable_key": "pk_test_public",
            "provider_label": "Stripe",
            "nested_unrelated": {"keep": true},
            "_stripe_client_secret_encrypted": legacy,
        });
        let order = StoredAdminPaymentOrder {
            id: "order-cas".to_string(),
            order_no: "po-cas".to_string(),
            wallet_id: "wallet-cas".to_string(),
            user_id: Some("user-cas".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(10.0),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 10.0,
            payment_method: "stripe".to_string(),
            payment_provider: Some("stripe".to_string()),
            order_kind: "wallet_recharge".to_string(),
            gateway_order_id: Some("pi-cas".to_string()),
            gateway_response: Some(gateway_response.clone()),
            status: "pending".to_string(),
            created_at_unix_ms: 1,
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: Some(4_102_444_800),
        };
        let input = CompareAndSwapPaymentOrderStripeClientSecretInput {
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
            expected_gateway_response: gateway_response,
            expected_client_secret_encrypted: legacy.to_string(),
            replacement_client_secret_encrypted: concat!(
                "aether-payment-order-stripe-client-secret-v2:",
                "aether-runtime-secret-v1:gAAAAABreplacement"
            )
            .to_string(),
        };
        (order, input)
    }

    #[test]
    fn payment_order_debug_output_redacts_gateway_and_stripe_secret_material() {
        let (order, input) = stripe_secret_cas_fixture();
        let order_debug = format!("{order:?}");
        let input_debug = format!("{input:?}");

        assert!(order_debug.contains("[REDACTED]"));
        assert!(input_debug.contains("[REDACTED]"));
        for secret in [
            "gAAAAABlegacy-ciphertext",
            "gAAAAABreplacement",
            "nested_unrelated",
        ] {
            assert!(!order_debug.contains(secret), "order debug leaked {secret}");
            assert!(!input_debug.contains(secret), "CAS debug leaked {secret}");
        }
    }

    #[test]
    fn stripe_secret_cas_replaces_only_the_exact_observed_field() {
        let (order, input) = stripe_secret_cas_fixture();
        let replacement = payment_order_stripe_client_secret_cas_replacement(&order, &input)
            .expect("valid replacement should be accepted")
            .expect("complete observed row should match");

        assert_eq!(
            replacement["_stripe_client_secret_encrypted"].as_str(),
            Some(input.replacement_client_secret_encrypted.as_str())
        );
        assert_eq!(replacement["publishable_key"], "pk_test_public");
        assert_eq!(replacement["provider_label"], "Stripe");
        assert_eq!(
            replacement["nested_unrelated"],
            serde_json::json!({"keep": true})
        );
        assert_eq!(replacement.as_object().map(|object| object.len()), Some(5));
    }

    #[test]
    fn stripe_secret_cas_rejects_stale_json_ciphertext_and_identity() {
        let (order, input) = stripe_secret_cas_fixture();

        let mut stale_json = input.clone();
        stale_json.expected_gateway_response["provider_label"] = serde_json::json!("changed");
        assert_eq!(
            payment_order_stripe_client_secret_cas_replacement(&order, &stale_json)
                .expect("stale JSON is a normal miss"),
            None
        );

        let mut stale_ciphertext = input.clone();
        stale_ciphertext.expected_client_secret_encrypted = "gAAAAABstale".to_string();
        assert_eq!(
            payment_order_stripe_client_secret_cas_replacement(&order, &stale_ciphertext)
                .expect("stale ciphertext is a normal miss"),
            None
        );

        for foreign in [
            CompareAndSwapPaymentOrderStripeClientSecretInput {
                order_no: "po-foreign".to_string(),
                ..input.clone()
            },
            CompareAndSwapPaymentOrderStripeClientSecretInput {
                user_id: Some("user-foreign".to_string()),
                ..input.clone()
            },
            CompareAndSwapPaymentOrderStripeClientSecretInput {
                order_kind: "plan_purchase".to_string(),
                ..input.clone()
            },
            CompareAndSwapPaymentOrderStripeClientSecretInput {
                payment_provider: Some("STRIPE".to_string()),
                ..input.clone()
            },
        ] {
            assert_eq!(
                payment_order_stripe_client_secret_cas_replacement(&order, &foreign)
                    .expect("identity mismatch is a normal miss"),
                None
            );
        }
    }

    #[test]
    fn stripe_secret_cas_rejects_unknown_or_malformed_replacement_envelopes() {
        let (order, input) = stripe_secret_cas_fixture();
        for replacement in [
            "aether-payment-order-stripe-client-secret-v3:unknown",
            "aether-payment-order-stripe-client-secret-v2:",
            "aether-payment-order-stripe-client-secret-v2:aether-runtime-secret-v2:unknown",
            "aether-payment-order-stripe-client-secret-v2:aether-runtime-secret-v1:\n",
        ] {
            let invalid = CompareAndSwapPaymentOrderStripeClientSecretInput {
                replacement_client_secret_encrypted: replacement.to_string(),
                ..input.clone()
            };
            assert!(payment_order_stripe_client_secret_cas_replacement(&order, &invalid).is_err());
        }
    }

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
            api_key_is_standalone: false,
            provider_id: None,
            status: "completed".to_string(),
            billing_status: "pending".to_string(),
            total_cost_usd: 0.1,
            actual_total_cost_usd: 0.1,
            finalized_at_unix_secs: None,
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn redeem_code_bucket_defaults_to_non_refundable_gift_semantics() {
        assert!(!redeem_code_credits_recharge_balance("gift"));
        assert_eq!(redeem_code_payment_method("gift"), "gift_code");
        assert_eq!(redeem_code_refundable_amount("gift", 8.5), 0.0);
        assert_eq!(redeem_code_payment_method("mystery"), "gift_code");
    }

    #[test]
    fn recharge_bucket_preserves_refundable_recharge_semantics() {
        assert!(redeem_code_credits_recharge_balance("recharge"));
        assert!(redeem_code_credits_recharge_balance(" Recharge "));
        assert_eq!(redeem_code_payment_method("recharge"), "card_code");
        assert_eq!(redeem_code_refundable_amount("recharge", 8.5), 8.5);
    }

    #[test]
    fn plan_wallet_credit_entitlements_fail_closed_on_malformed_entries() {
        assert!(
            validate_plan_wallet_credit_entitlements(&serde_json::json!([
                {"type": "wallet_credit", "amount_usd": 5.0, "balance_bucket": "gift"}
            ]))
            .is_ok()
        );
        for invalid in [
            serde_json::json!([{"type": "wallet_credit"}]),
            serde_json::json!([{"type": "wallet_credit", "amount_usd": 0.0}]),
            serde_json::json!([{"type": "wallet_credit", "amount_usd": -1.0}]),
            serde_json::json!([{"type": "wallet_credit", "amount_usd": 5.0, "balance_bucket": "unknown"}]),
            serde_json::json!([{"type": "wallet_credit", "amount_usd": 5.0, "balance_bucket": 7}]),
            serde_json::json!(["wallet_credit"]),
            serde_json::json!({"type": "wallet_credit", "amount_usd": 5.0}),
        ] {
            assert!(
                validate_plan_wallet_credit_entitlements(&invalid).is_err(),
                "malformed wallet credit should be rejected: {invalid}"
            );
        }
    }

    #[test]
    fn plan_purchase_input_rejects_invalid_money_identity_and_snapshot_fields() {
        let valid = CreatePlanPurchaseOrderInput {
            preferred_wallet_id: None,
            user_id: "user-1".to_string(),
            amount_usd: 1.0,
            pay_amount: 7.2,
            pay_currency: "CNY".to_string(),
            exchange_rate: 7.2,
            payment_method: "alipay".to_string(),
            payment_provider: Some("epay".to_string()),
            payment_channel: Some("alipay".to_string()),
            gateway_order_id: "gateway-1".to_string(),
            gateway_response: serde_json::json!({}),
            order_no: "order-1".to_string(),
            product_id: "plan-1".to_string(),
            product_snapshot: serde_json::json!({
                "id": "plan-1",
                "duration_unit": "month",
                "duration_value": 1,
                "purchase_limit_scope": "active_period",
                "entitlements": [{"type": "daily_quota", "daily_quota_usd": 1.0}]
            }),
            expires_at_unix_secs: 4_102_444_800,
        };
        assert!(validate_plan_purchase_order_input(&valid).is_ok());

        let mut invalid = valid.clone();
        invalid.amount_usd = f64::NAN;
        assert!(validate_plan_purchase_order_input(&invalid).is_err());
        invalid = valid.clone();
        invalid.product_snapshot["id"] = serde_json::json!("another-plan");
        assert!(validate_plan_purchase_order_input(&invalid).is_err());
        invalid = valid.clone();
        invalid.gateway_response = serde_json::json!("provider payload");
        assert!(validate_plan_purchase_order_input(&invalid).is_err());

        for (duration_unit, duration_value) in
            [("day", i64::MAX), ("month", i64::MAX), ("year", i64::MAX)]
        {
            invalid = valid.clone();
            invalid.product_snapshot["duration_unit"] = serde_json::json!(duration_unit);
            invalid.product_snapshot["duration_value"] = serde_json::json!(duration_value);
            assert!(
                validate_plan_purchase_order_input(&invalid).is_err(),
                "overflowing {duration_unit} duration must be rejected"
            );
        }
    }

    #[test]
    fn wallet_recharge_input_rejects_invalid_channels_and_numbers() {
        let valid = CreateWalletRechargeOrderInput {
            preferred_wallet_id: None,
            user_id: "user-1".to_string(),
            amount_usd: 1.0,
            pay_amount: Some(7.2),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.2),
            payment_method: "wxpay".to_string(),
            payment_provider: Some("wxpay".to_string()),
            payment_channel: Some("native".to_string()),
            gateway_order_id: "gateway-1".to_string(),
            gateway_response: serde_json::json!({}),
            order_no: "order-1".to_string(),
            expires_at_unix_secs: 4_102_444_800,
        };
        assert!(validate_wallet_recharge_order_input(&valid).is_ok());

        let mut invalid = valid.clone();
        invalid.payment_channel = Some("app".to_string());
        assert!(validate_wallet_recharge_order_input(&invalid).is_err());
        invalid = valid.clone();
        invalid.pay_currency = Some("C1Y".to_string());
        assert!(validate_wallet_recharge_order_input(&invalid).is_err());
        invalid = valid.clone();
        invalid.amount_usd = f64::INFINITY;
        assert!(validate_wallet_recharge_order_input(&invalid).is_err());

        // Legacy checkout placeholders may omit the explicit channel; keep
        // those rows readable while still checking the provider/method pair.
        invalid = valid;
        invalid.payment_channel = None;
        assert!(validate_wallet_recharge_order_input(&invalid).is_ok());

        invalid.payment_provider = None;
        invalid.payment_channel = Some("native".to_string());
        assert!(validate_wallet_recharge_order_input(&invalid).is_err());

        invalid.payment_channel = None;
        assert!(validate_wallet_recharge_order_input(&invalid).is_ok());
    }

    #[test]
    fn zero_value_credit_is_limited_to_admin_grants() {
        assert!(validate_payment_order_credit_amounts(
            "plan_purchase",
            "admin_grant",
            Some("admin"),
            Some("manual"),
            0.0,
            Some(0.0),
        )
        .is_ok());
        assert!(validate_payment_order_credit_amounts(
            "wallet_recharge",
            "admin_grant",
            Some("admin"),
            Some("manual"),
            0.0,
            Some(0.0),
        )
        .is_err());
        assert!(validate_payment_order_credit_amounts(
            "plan_purchase",
            "stripe",
            Some("stripe"),
            Some("card"),
            0.0,
            Some(0.0),
        )
        .is_err());
    }

    #[test]
    fn payment_method_namespace_is_trimmed_lowercase_and_bounded() {
        assert_eq!(
            canonicalize_payment_method("  Admin_Manual  "),
            Ok("admin_manual".to_string())
        );
        assert!(canonicalize_payment_method("   ").is_err());
        assert!(canonicalize_payment_method(&"X".repeat(65)).is_err());
        assert!(canonicalize_payment_method("epay/card").is_err());
    }

    #[test]
    fn official_payment_callbacks_require_an_explicit_matching_provider() {
        for provider in ["alipay", "wxpay", "stripe", "epay"] {
            assert!(validate_payment_callback_provider_binding(provider, Some(provider)).is_ok());
            assert!(validate_payment_callback_provider_binding(provider, None).is_err());
            assert!(validate_payment_callback_provider_binding(provider, Some("manual")).is_err());
        }
        assert!(validate_payment_callback_provider_binding("alipay", Some("epay")).is_ok());
        assert!(validate_payment_callback_provider_binding("wxpay", Some("epay")).is_ok());
        assert!(validate_payment_callback_provider_binding("manual", None).is_ok());
    }

    #[test]
    fn payment_provider_channel_binding_rejects_cross_gateway_orders() {
        assert!(
            validate_payment_provider_channel_binding("stripe", Some("stripe"), Some("card"))
                .is_ok()
        );
        assert!(
            validate_payment_provider_channel_binding("wxpay", Some("wxpay"), Some("native"))
                .is_ok()
        );
        assert!(validate_payment_provider_channel_binding(
            "stripe",
            Some("stripe"),
            Some("wechat_pay")
        )
        .is_ok());
        assert!(
            validate_payment_provider_channel_binding("alipay", Some("epay"), Some("alipay"))
                .is_ok()
        );
        assert!(
            validate_payment_provider_channel_binding("epay", Some("epay"), Some("wxpay")).is_ok()
        );
        assert!(
            validate_payment_provider_channel_binding("epay", Some("epay"), Some("qqpay")).is_ok()
        );
        assert!(validate_payment_provider_channel_binding(
            "admin_grant",
            Some("admin"),
            Some("manual")
        )
        .is_ok());
        for invalid in [
            ("stripe", Some("epay"), Some("card")),
            ("alipay", Some("epay"), Some("wxpay")),
            ("alipay", Some("alipay"), Some("native")),
            ("stripe", Some("stripe"), None),
            ("wxpay", Some("wxpay"), Some("app")),
            ("stripe", Some("stripe"), Some("bank_transfer")),
            ("admin_grant", Some("admin"), Some("card")),
        ] {
            assert!(
                validate_payment_provider_channel_binding(invalid.0, invalid.1, invalid.2).is_err(),
                "invalid payment binding should be rejected: {:?}",
                invalid
            );
        }
    }

    #[test]
    fn epay_callback_accepts_legacy_method_aliases_only_with_epay_binding() {
        assert!(payment_callback_method_matches_order(
            "alipay",
            Some("epay"),
            "epay",
            Some("epay")
        ));
        assert!(payment_callback_method_matches_order(
            "epay",
            Some("epay"),
            "wxpay",
            Some("epay")
        ));
        assert!(!payment_callback_method_matches_order(
            "alipay",
            Some("alipay"),
            "epay",
            Some("epay")
        ));
        assert!(!payment_callback_method_matches_order(
            "manual",
            Some("epay"),
            "epay",
            Some("epay")
        ));
    }

    #[test]
    fn legacy_epay_channel_orders_accept_only_epay_provider_callbacks() {
        assert!(payment_callback_provider_matches_order(
            "alipay",
            None,
            "epay",
            Some("epay")
        ));
        assert!(payment_callback_provider_matches_order(
            "wxpay",
            None,
            "alipay",
            Some("epay")
        ));
        assert!(!payment_callback_provider_matches_order(
            "alipay",
            None,
            "alipay",
            Some("stripe")
        ));
        assert!(!payment_callback_provider_matches_order(
            "manual",
            None,
            "epay",
            Some("epay")
        ));
        assert!(!payment_callback_provider_matches_order(
            "alipay",
            None,
            "alipay",
            Some("alipay")
        ));
    }

    #[test]
    fn official_payment_callbacks_require_complete_settlement_bindings() {
        let valid = ProcessPaymentCallbackInput {
            payment_method: " Stripe ".to_string(),
            payment_provider: Some("STRIPE".to_string()),
            payment_channel: Some("CARD".to_string()),
            callback_key: "stripe:event-1".to_string(),
            order_no: Some("po_1".to_string()),
            gateway_order_id: Some("pi_1".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(10.0),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            payload_hash: "hash-1".to_string(),
            payload: serde_json::json!({}),
            signature_valid: true,
        };

        let mut canonical = valid.clone();
        canonical
            .canonicalize_and_validate()
            .expect("complete official callback should validate");
        assert_eq!(canonical.payment_method, "stripe");
        assert_eq!(canonical.payment_provider.as_deref(), Some("stripe"));
        assert_eq!(canonical.payment_channel.as_deref(), Some("card"));

        for mut incomplete in [
            {
                let mut value = valid.clone();
                value.payment_channel = None;
                value
            },
            {
                let mut value = valid.clone();
                value.order_no = None;
                value
            },
            {
                let mut value = valid.clone();
                value.gateway_order_id = None;
                value
            },
            {
                let mut value = valid.clone();
                value.pay_amount = None;
                value
            },
            {
                let mut value = valid.clone();
                value.pay_currency = None;
                value
            },
        ] {
            assert!(incomplete.canonicalize_and_validate().is_err());
        }
    }

    #[test]
    fn callback_amounts_require_provider_settlement_to_match_order() {
        assert!(payment_callback_amount_matches_order(
            10.0,
            Some(72.0),
            Some("CNY"),
            Some(7.2),
            10.0,
            Some(72.0),
        ));
        assert!(payment_callback_amount_matches_order(
            10.0,
            Some(72.0),
            Some("CNY"),
            Some(7.2),
            10.0000005,
            Some(72.0000005),
        ));
        // The gateway's USD presentation may include a fee or use a
        // provider-side conversion. The repository credits the order's
        // locked USD amount and therefore only binds the signed settlement.
        assert!(payment_callback_amount_matches_order(
            10.0,
            Some(72.0),
            Some("CNY"),
            Some(7.2),
            11.0,
            Some(72.0),
        ));
        assert!(!payment_callback_amount_matches_order(
            10.0,
            Some(72.0),
            Some("CNY"),
            Some(7.2),
            10.0,
            Some(71.0),
        ));
    }

    #[test]
    fn legacy_callback_amounts_are_reconstructed_only_from_order_terms() {
        assert!(payment_callback_amount_matches_order(
            10.0,
            None,
            Some("CNY"),
            Some(7.2),
            10.0,
            Some(72.0),
        ));
        assert!(!payment_callback_amount_matches_order(
            10.0,
            None,
            Some("CNY"),
            Some(7.2),
            10.0,
            Some(72.01),
        ));

        // A stale historical USD rate must not reinterpret a USD order as
        // CNY.  The currency itself determines the effective rate.
        assert!(payment_callback_amount_matches_order(
            10.0,
            None,
            Some("USD"),
            Some(7.2),
            10.0,
            Some(10.0),
        ));
        assert!(!payment_callback_amount_matches_order(
            10.0,
            None,
            Some("USD"),
            Some(7.2),
            10.0,
            Some(72.0),
        ));

        // An unknown currency/rate cannot be completed from provider data.
        assert!(!payment_callback_amount_matches_order(
            10.0,
            None,
            None,
            Some(7.2),
            10.0,
            Some(72.0),
        ));
        assert!(!payment_callback_amount_matches_order(
            10.0,
            None,
            Some("CNY"),
            None,
            10.0,
            Some(72.0),
        ));
    }

    #[test]
    fn payment_callback_gateway_response_projection_drops_provider_payload() {
        let input = ProcessPaymentCallbackInput {
            payment_method: "stripe".to_string(),
            payment_provider: Some("stripe".to_string()),
            payment_channel: Some("card".to_string()),
            callback_key: "evt_1".to_string(),
            order_no: Some("po_1".to_string()),
            gateway_order_id: Some("pi_1".to_string()),
            amount_usd: 10.0,
            pay_amount: Some(10.0),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            payload_hash: "hash-1".to_string(),
            payload: serde_json::json!({
                "client_secret": "pi_1_secret_replayable",
                "customer": {"email": "payer@example.com"},
                "authorization": "Bearer upstream-secret",
            }),
            signature_valid: true,
        };

        let projected = input.gateway_response_projection("po_1", Some("pi_1"));
        assert_eq!(
            projected,
            serde_json::json!({
                "gateway": "stripe",
                "payment_provider": "stripe",
                "payment_channel": "card",
                "order_no": "po_1",
                "gateway_order_id": "pi_1",
                "amount_usd": 10.0,
                "pay_amount": 10.0,
                "pay_currency": "USD",
                "exchange_rate": 1.0,
                "signature_valid": true,
            })
        );
        let encoded = projected.to_string();
        for forbidden in [
            "client_secret",
            "replayable",
            "payer@example.com",
            "authorization",
            "upstream-secret",
        ] {
            assert!(!encoded.contains(forbidden), "persisted {forbidden}");
        }
    }

    #[test]
    fn wallet_recharge_gateway_projection_is_an_allowlist() {
        let projected = project_wallet_recharge_gateway_response(&serde_json::json!({
            "gateway": "stripe",
            "instructions": "confirm payment",
            "payment_method_types": ["card", {"secret": "drop"}, ""],
            "payment_params": {
                "pid": "merchant",
                "sign": "signed",
                "nested": {"credential": "drop"}
            },
            "_stripe_client_secret_encrypted": "enc:v1:secret",
            "client_secret": "pi_1_secret_raw",
            "customer": {"email": "payer@example.com"},
            "unknown": "drop",
        }))
        .expect("checkout object should project");

        assert_eq!(
            projected,
            serde_json::json!({
                "gateway": "stripe",
                "instructions": "confirm payment",
                "payment_method_types": ["card"],
                "payment_params": {"pid": "merchant", "sign": "signed"},
                "_stripe_client_secret_encrypted": "enc:v1:secret",
                "order_kind": "wallet_recharge",
            })
        );
        let encoded = projected.to_string();
        for forbidden in [
            "pi_1_secret_raw",
            "payer@example.com",
            "credential",
            "unknown",
        ] {
            assert!(!encoded.contains(forbidden), "projected {forbidden}");
        }
        assert!(project_wallet_recharge_gateway_response(&serde_json::json!(null)).is_err());
    }

    #[test]
    fn plan_gateway_projection_is_an_allowlist_without_wallet_marker() {
        let projected = project_wallet_gateway_response(&serde_json::json!({
            "gateway": "stripe",
            "gateway_order_id": "pi_1",
            "intent_id": "pi_1",
            "publishable_key": "pk_test_public",
            "client_secret": "pi_1_secret_raw",
            "_stripe_client_secret_encrypted": "enc:v1:secret",
            "order_kind": "wallet_recharge",
            "product_id": "plan-secret-overwrite",
            "customer": {"email": "payer@example.com"},
            "provider_private_token": "drop",
        }))
        .expect("plan checkout object should project");

        assert_eq!(
            projected,
            serde_json::json!({
                "gateway": "stripe",
                "gateway_order_id": "pi_1",
                "intent_id": "pi_1",
                "publishable_key": "pk_test_public",
                "_stripe_client_secret_encrypted": "enc:v1:secret",
            })
        );
        assert_ne!(
            projected
                .get("order_kind")
                .and_then(serde_json::Value::as_str),
            Some("wallet_recharge")
        );
        let encoded = projected.to_string();
        for forbidden in [
            "pi_1_secret_raw",
            "payer@example.com",
            "provider_private_token",
            "plan-secret-overwrite",
        ] {
            assert!(!encoded.contains(forbidden), "projected {forbidden}");
        }
    }

    #[test]
    fn failed_wallet_checkout_placeholder_is_the_only_recoverable_failed_order() {
        let placeholder = serde_json::json!({
            "order_kind": "wallet_recharge",
            "integration_status": "checkout_failed",
            "gateway": "stripe",
            "gateway_order_id": "order-1",
            "failure_reason": "provider response lost",
        });
        assert!(payment_order_is_failed_wallet_checkout_placeholder(
            "failed",
            "wallet_recharge",
            Some(&placeholder),
        ));
        assert!(!payment_order_is_failed_wallet_checkout_placeholder(
            "pending",
            "wallet_recharge",
            Some(&placeholder),
        ));
        assert!(!payment_order_is_failed_wallet_checkout_placeholder(
            "failed",
            "plan_purchase",
            Some(&placeholder),
        ));
        assert!(!payment_order_is_failed_wallet_checkout_placeholder(
            "failed",
            "wallet_recharge",
            None,
        ));

        for evidence_key in [
            "payment_url",
            "qr_code",
            "code_url",
            "h5_url",
            "jsapi",
            "client_secret",
            "_stripe_client_secret_encrypted",
            "intent_id",
        ] {
            let mut with_evidence = placeholder.clone();
            with_evidence[evidence_key] = serde_json::json!("provider-evidence");
            assert!(
                !payment_order_is_failed_wallet_checkout_placeholder(
                    "failed",
                    "wallet_recharge",
                    Some(&with_evidence),
                ),
                "provider evidence {evidence_key} must keep a failed order non-creditable"
            );
        }
    }

    #[test]
    fn uncertain_wallet_checkout_is_callback_settleable_but_not_reclaimable() {
        let uncertain = super::wallet_recharge_checkout_uncertain_response(
            Some(&serde_json::json!({
                "gateway": "stripe",
                "gateway_order_id": "po_1",
                "order_kind": "wallet_recharge",
                "integration_status": "checkout_pending",
                "checkout_claim_token": "claim-1",
                "checkout_claimed_at_unix_secs": 1,
            })),
            "response lost after provider acceptance",
            2,
        );
        assert_eq!(uncertain["integration_status"], "checkout_uncertain");
        assert!(super::payment_order_is_failed_wallet_checkout_placeholder(
            "failed",
            "wallet_recharge",
            Some(&uncertain),
        ));
        let order = StoredAdminPaymentOrder {
            id: "order-uncertain".to_string(),
            order_no: "po_1".to_string(),
            wallet_id: "wallet-1".to_string(),
            user_id: Some("user-1".to_string()),
            amount_usd: 1.0,
            pay_amount: Some(1.0),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            payment_method: "stripe".to_string(),
            payment_provider: Some("stripe".to_string()),
            order_kind: "wallet_recharge".to_string(),
            gateway_order_id: Some("po_1".to_string()),
            gateway_response: Some(uncertain),
            status: "failed".to_string(),
            created_at_unix_ms: 1,
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: Some(u64::MAX),
        };
        assert!(!super::wallet_recharge_order_is_reclaimable_placeholder(
            &order, 10_000,
        ));
    }

    #[test]
    fn checkout_lease_fallback_accepts_legacy_seconds_and_milliseconds() {
        let base = StoredAdminPaymentOrder {
            id: "order-1".to_string(),
            order_no: "order-no-1".to_string(),
            wallet_id: "wallet-1".to_string(),
            user_id: Some("user-1".to_string()),
            amount_usd: 1.0,
            pay_amount: Some(1.0),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            refunded_amount_usd: 0.0,
            refundable_amount_usd: 0.0,
            payment_method: "stripe".to_string(),
            payment_provider: Some("stripe".to_string()),
            order_kind: "wallet_recharge".to_string(),
            gateway_order_id: Some("order-no-1".to_string()),
            gateway_response: None,
            status: "pending".to_string(),
            created_at_unix_ms: 1_700_000_000,
            paid_at_unix_secs: None,
            credited_at_unix_secs: None,
            expires_at_unix_secs: None,
        };
        assert_eq!(
            wallet_recharge_order_created_at_unix_secs(&base),
            1_700_000_000
        );

        let mut millis = base.clone();
        millis.created_at_unix_ms = 1_700_000_000_000;
        assert_eq!(
            wallet_recharge_order_created_at_unix_secs(&millis),
            1_700_000_000
        );
    }

    #[test]
    fn canonicalizes_and_rejects_refund_provenance_tampering() {
        let fields = super::canonicalize_wallet_refund_fields(
            Some("order-1"),
            None,
            None,
            None,
            Some("stripe"),
        )
        .expect("order fields should canonicalize");
        assert_eq!(fields.source_type, "payment_order");
        assert_eq!(fields.source_id.as_deref(), Some("order-1"));
        assert_eq!(fields.refund_mode, "original_channel");

        assert!(super::canonicalize_wallet_refund_fields(
            Some("order-1"),
            Some("wallet_balance"),
            None,
            None,
            Some("stripe"),
        )
        .is_err());
        assert!(super::canonicalize_wallet_refund_fields(
            None,
            None,
            Some("forged-source"),
            None,
            None,
        )
        .is_err());
    }

    #[test]
    fn manual_wallet_recharge_requires_safe_finite_arithmetic() {
        assert_eq!(
            validate_manual_wallet_recharge(5.0, 10.0, 2.0, 20.0),
            Ok((15.0, 25.0))
        );
        assert_eq!(
            validate_manual_wallet_recharge(5.0, -10.0, 2.0, 20.0),
            Ok((-5.0, 25.0))
        );

        for amount_usd in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                validate_manual_wallet_recharge(amount_usd, 10.0, 2.0, 20.0).is_err(),
                "invalid recharge amount should be rejected: {amount_usd:?}"
            );
        }
        assert!(validate_manual_wallet_recharge(f64::MAX, f64::MAX, 0.0, f64::MAX).is_err());
    }

    #[test]
    fn payment_order_refund_amounts_require_a_consistent_split() {
        assert!(payment_order_refund_amounts_are_consistent(10.0, 0.0, 10.0));
        assert!(payment_order_refund_amounts_are_consistent(10.0, 2.5, 7.5));
        assert!(payment_order_refund_amounts_are_consistent(
            10.0, 2.5, 7.5000005
        ));
        for values in [
            (10.0, 0.0, 5.0),
            (10.0, -1.0, 11.0),
            (10.0, 11.0, 0.0),
            (f64::NAN, 0.0, 0.0),
            (10.0, f64::INFINITY, 0.0),
        ] {
            assert!(
                !payment_order_refund_amounts_are_consistent(values.0, values.1, values.2),
                "malformed refund split should be rejected: {values:?}"
            );
        }
    }

    #[test]
    fn refund_proof_success_detection_only_accepts_terminal_status() {
        assert!(wallet_refund_proof_is_success(&serde_json::json!({
            "status": "success"
        })));
        assert!(wallet_refund_proof_is_success(&serde_json::json!({
            "gateway_refund": { "status": "succeeded" }
        })));
        assert!(!wallet_refund_proof_is_success(&serde_json::json!({
            "status": "processing"
        })));
        assert!(!wallet_refund_proof_is_success(&serde_json::json!({
            "gateway_refund": { "status": "processing" }
        })));
        assert!(!wallet_refund_proof_is_success(&serde_json::json!({
            "status": "success",
            "gateway_refund": { "status": "processing" }
        })));
    }
}
