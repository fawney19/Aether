mod memory;
pub use aether_data_contracts::repository::settlement::*;
#[cfg(feature = "mysql")]
pub use aether_data_mysql::MysqlSettlementRepository;
#[cfg(feature = "postgres")]
pub use aether_data_postgres::SqlxSettlementRepository;
#[cfg(feature = "sqlite")]
pub use aether_data_sqlite::SqliteSettlementRepository;
pub use memory::InMemorySettlementRepository;

#[cfg(test)]
mod tests {
    use aether_data_contracts::repository::settlement::settlement_billing_status_for_usage_status;

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
