//! Driver-specific wallet usage aggregation adapters.

#[cfg(feature = "mysql")]
mod mysql;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "sqlite")]
mod sqlite;

#[cfg(any(feature = "mysql", feature = "sqlite"))]
use sha2::{Digest, Sha256};

use crate::DataLayerError;

pub(super) fn u64_to_i64(value: u64, field_name: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::InvalidInput(format!("invalid {field_name}: {value}")))
}

#[cfg(feature = "postgres")]
pub(super) fn unix_secs_to_utc(
    value: u64,
    field_name: &str,
) -> Result<chrono::DateTime<chrono::Utc>, DataLayerError> {
    let value = u64_to_i64(value, field_name)?;
    chrono::DateTime::<chrono::Utc>::from_timestamp(value, 0)
        .ok_or_else(|| DataLayerError::InvalidInput(format!("invalid {field_name}: {value}")))
}

#[cfg(any(feature = "mysql", feature = "sqlite"))]
pub(super) fn wallet_daily_usage_id(
    wallet_id: &str,
    billing_date: &str,
    billing_timezone: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"wallet-daily-usage:");
    hasher.update(wallet_id.as_bytes());
    hasher.update(b":");
    hasher.update(billing_date.as_bytes());
    hasher.update(b":");
    hasher.update(billing_timezone.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::u64_to_i64;
    #[cfg(any(feature = "mysql", feature = "sqlite"))]
    use super::wallet_daily_usage_id;

    #[cfg(any(feature = "mysql", feature = "sqlite"))]
    #[test]
    fn wallet_daily_usage_ids_are_stable_and_partition_specific() {
        let first = wallet_daily_usage_id("wallet-1", "2026-07-13", "UTC");
        let same = wallet_daily_usage_id("wallet-1", "2026-07-13", "UTC");
        let other_day = wallet_daily_usage_id("wallet-1", "2026-07-14", "UTC");

        assert_eq!(first, same);
        assert_ne!(first, other_day);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn rejects_timestamps_outside_i64_range() {
        if usize::BITS >= 64 {
            assert!(u64_to_i64(u64::MAX, "window_start").is_err());
        }
    }
}
