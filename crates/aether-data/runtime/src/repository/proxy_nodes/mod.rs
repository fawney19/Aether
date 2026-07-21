mod memory;

pub use aether_data_contracts::repository::proxy_nodes::*;
#[cfg(feature = "mysql")]
pub use aether_data_mysql::MysqlProxyNodeReadRepository;
#[cfg(feature = "postgres")]
pub use aether_data_postgres::SqlxProxyNodeRepository;
#[cfg(feature = "sqlite")]
pub use aether_data_sqlite::SqliteProxyNodeReadRepository;
pub use memory::InMemoryProxyNodeRepository;

pub fn log_reported_tunnel_error_event(
    node_id: &str,
    event: &TunnelErrorEventRecord,
    received_at_unix_secs: u64,
) {
    tracing::warn!(
        event_name = "proxy_tunnel_error_reported",
        source = "heartbeat",
        node_id = %node_id,
        category = %event.category,
        message = %event.message,
        severity = ?event.severity,
        component = ?event.component,
        summary = ?event.summary,
        operator_action = ?event.operator_action,
        error_reported_at_unix_secs = event.timestamp_unix_secs,
        error_reported_at_unix_ms = ?event.timestamp_unix_ms,
        report_received_at_unix_secs = received_at_unix_secs,
        "proxy reported tunnel error via heartbeat"
    );
}
