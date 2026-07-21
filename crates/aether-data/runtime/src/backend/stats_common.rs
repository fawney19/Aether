use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use crate::DataLayerError;

pub(crate) fn unix_secs(value: DateTime<Utc>) -> i64 {
    value.timestamp().max(0)
}

pub(crate) fn unix_ms(value: i64) -> Result<i64, DataLayerError> {
    value.checked_mul(1000).ok_or_else(|| {
        DataLayerError::InvalidInput(format!("timestamp overflow while converting {value} to ms"))
    })
}

pub(crate) fn utc_from_unix_secs(
    value: i64,
    field_name: &str,
) -> Result<DateTime<Utc>, DataLayerError> {
    DateTime::<Utc>::from_timestamp(value, 0).ok_or_else(|| {
        DataLayerError::UnexpectedValue(format!("{field_name} contains invalid timestamp {value}"))
    })
}

pub(crate) fn stats_id(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
