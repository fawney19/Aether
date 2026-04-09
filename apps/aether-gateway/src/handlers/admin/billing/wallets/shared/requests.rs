pub(in super::super) const ADMIN_WALLETS_DATA_UNAVAILABLE_DETAIL: &str =
    "Admin wallets data unavailable";
pub(in super::super) const ADMIN_WALLETS_API_KEY_REFUND_DETAIL: &str = "独立密钥钱包不支持退款审批";
pub(in super::super) const ADMIN_WALLETS_API_KEY_RECHARGE_DETAIL: &str =
    "独立密钥钱包不支持充值，请使用调账";
pub(in super::super) const ADMIN_WALLETS_API_KEY_GIFT_ADJUST_DETAIL: &str =
    "独立密钥钱包不支持赠款调账";

#[derive(Debug, serde::Deserialize)]
pub(in super::super) struct AdminWalletRechargeRequest {
    pub(in super::super) amount_usd: f64,
    #[serde(default = "default_admin_wallet_payment_method")]
    pub(in super::super) payment_method: String,
    #[serde(default)]
    pub(in super::super) description: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub(in super::super) struct AdminWalletAdjustRequest {
    pub(in super::super) amount_usd: f64,
    #[serde(default = "default_admin_wallet_balance_type")]
    pub(in super::super) balance_type: String,
    #[serde(default)]
    pub(in super::super) description: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub(in super::super) struct AdminWalletRefundFailRequest {
    pub(in super::super) reason: String,
}

#[derive(Debug, serde::Deserialize)]
pub(in super::super) struct AdminWalletRefundCompleteRequest {
    #[serde(default)]
    pub(in super::super) gateway_refund_id: Option<String>,
    #[serde(default)]
    pub(in super::super) payout_reference: Option<String>,
    #[serde(default)]
    pub(in super::super) payout_proof: Option<serde_json::Value>,
}

fn default_admin_wallet_payment_method() -> String {
    "admin_manual".to_string()
}

fn default_admin_wallet_balance_type() -> String {
    "recharge".to_string()
}
