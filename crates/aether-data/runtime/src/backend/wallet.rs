//! Driver-specific wallet usage aggregation adapters.

#[cfg(feature = "postgres")]
mod postgres;

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

#[cfg(test)]
mod tests {
    use super::u64_to_i64;

    #[test]
    fn rejects_timestamps_outside_i64_range() {
        if usize::BITS >= 64 {
            assert!(u64_to_i64(u64::MAX, "window_start").is_err());
        }
    }
}
