mod memory;
mod mysql;
mod postgres;
mod sqlite;

const SETTLEMENT_EPSILON_USD: f64 = 0.000_000_01;

#[derive(Debug, Clone, Copy)]
struct WalletDebitPlan {
    recharge_deduction: f64,
    gift_deduction: f64,
    recharge_overdraft: f64,
}

impl WalletDebitPlan {
    fn after_balances(self, recharge_balance: f64, gift_balance: f64) -> (f64, f64) {
        (
            recharge_balance - self.recharge_deduction - self.recharge_overdraft,
            gift_balance - self.gift_deduction,
        )
    }
}

fn finite_wallet_available_usd(recharge_balance: f64, gift_balance: f64) -> f64 {
    recharge_balance.max(0.0) + gift_balance.max(0.0)
}

fn plan_finite_wallet_debit(
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

fn settlement_billing_status_for_usage_status(status: &str) -> &'static str {
    match status {
        "completed" | "cancelled" => "settled",
        _ => "void",
    }
}

fn settlement_billable_cost_usd(input: &UsageSettlementInput) -> f64 {
    input.actual_total_cost_usd.max(0.0)
}

#[allow(unused_imports)]
pub(crate) use aether_data_contracts::repository::settlement::{
    SettlementRepository, SettlementWriteRepository, StoredUsageSettlement, UsageSettlementInput,
};
pub use memory::InMemorySettlementRepository;
pub use mysql::MysqlSettlementRepository;
pub use postgres::SqlxSettlementRepository;
pub use sqlite::SqliteSettlementRepository;

#[cfg(test)]
mod tests {
    use super::settlement_billing_status_for_usage_status;

    #[test]
    fn cancelled_usage_status_is_billable() {
        assert_eq!(
            settlement_billing_status_for_usage_status("completed"),
            "settled"
        );
        assert_eq!(
            settlement_billing_status_for_usage_status("cancelled"),
            "settled"
        );
        assert_eq!(settlement_billing_status_for_usage_status("failed"), "void");
    }
}
