use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UsageSettlementInput {
    pub request_id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    #[serde(default)]
    pub api_key_is_standalone: bool,
    pub provider_id: Option<String>,
    pub status: String,
    pub billing_status: String,
    pub total_cost_usd: f64,
    pub actual_total_cost_usd: f64,
    pub finalized_at_unix_secs: Option<u64>,
}

impl UsageSettlementInput {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.request_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "settlement request_id cannot be empty".to_string(),
            ));
        }
        if self.status.trim().is_empty() || self.billing_status.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "settlement status cannot be empty".to_string(),
            ));
        }
        if !self.total_cost_usd.is_finite() || !self.actual_total_cost_usd.is_finite() {
            return Err(crate::DataLayerError::InvalidInput(
                "settlement cost must be finite".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredUsageSettlement {
    pub request_id: String,
    pub wallet_id: Option<String>,
    pub billing_status: String,
    pub wallet_balance_before: Option<f64>,
    pub wallet_balance_after: Option<f64>,
    pub wallet_recharge_balance_before: Option<f64>,
    pub wallet_recharge_balance_after: Option<f64>,
    pub wallet_gift_balance_before: Option<f64>,
    pub wallet_gift_balance_after: Option<f64>,
    pub provider_monthly_used_usd: Option<f64>,
    pub finalized_at_unix_secs: Option<u64>,
}

#[async_trait]
pub trait SettlementWriteRepository: Send + Sync {
    async fn settle_usage(
        &self,
        input: UsageSettlementInput,
    ) -> Result<Option<StoredUsageSettlement>, crate::DataLayerError>;
}

pub trait SettlementRepository: SettlementWriteRepository + Send + Sync {}

impl<T> SettlementRepository for T where T: SettlementWriteRepository + Send + Sync {}

pub const SETTLEMENT_EPSILON_USD: f64 = 0.000_000_01;

#[derive(Debug, Clone, Copy)]
pub struct WalletDebitPlan {
    pub recharge_deduction: f64,
    pub gift_deduction: f64,
    pub recharge_overdraft: f64,
}

impl WalletDebitPlan {
    pub fn after_balances(self, recharge_balance: f64, gift_balance: f64) -> (f64, f64) {
        (
            recharge_balance - self.recharge_deduction - self.recharge_overdraft,
            gift_balance - self.gift_deduction,
        )
    }
}

pub fn finite_wallet_available_usd(recharge_balance: f64, gift_balance: f64) -> f64 {
    recharge_balance.max(0.0) + gift_balance.max(0.0)
}

pub fn plan_finite_wallet_debit(
    recharge_balance: f64,
    gift_balance: f64,
    requested_usd: f64,
) -> WalletDebitPlan {
    let requested_usd = requested_usd.max(0.0);
    let recharge_deduction = recharge_balance.max(0.0).min(requested_usd);
    let after_recharge_remaining = (requested_usd - recharge_deduction).max(0.0);
    let gift_deduction = gift_balance.max(0.0).min(after_recharge_remaining);
    let recharge_overdraft = (after_recharge_remaining - gift_deduction).max(0.0);
    WalletDebitPlan {
        recharge_deduction,
        gift_deduction,
        recharge_overdraft,
    }
}

pub fn settlement_billing_status_for_usage_status(status: &str) -> &'static str {
    match status {
        "completed" | "cancelled" => "settled",
        _ => "void",
    }
}

pub fn settlement_billable_cost_usd(input: &UsageSettlementInput) -> f64 {
    input.actual_total_cost_usd.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::UsageSettlementInput;

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
}
