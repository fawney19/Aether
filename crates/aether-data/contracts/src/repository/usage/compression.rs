use std::io::Read;

use crate::DataLayerError;

/// Hard ceiling for usage JSON after decompression.
///
/// Usage bodies may contain large model responses, so this stays aligned with the gateway's
/// largest routinely buffered response while still bounding gzip expansion from stored data.
pub const MAX_DECOMPRESSED_USAGE_JSON_BYTES: usize = 64 * 1024 * 1024;

pub fn read_decompressed_usage_json(reader: impl Read) -> Result<Vec<u8>, DataLayerError> {
    read_decompressed_usage_json_with_limit(reader, MAX_DECOMPRESSED_USAGE_JSON_BYTES)
}

fn read_decompressed_usage_json_with_limit(
    reader: impl Read,
    limit_bytes: usize,
) -> Result<Vec<u8>, DataLayerError> {
    let read_limit = u64::try_from(limit_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut limited = reader.take(read_limit);
    let mut decoded = Vec::new();
    limited.read_to_end(&mut decoded).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to decompress usage json: {err}"))
    })?;
    if decoded.len() > limit_bytes {
        return Err(DataLayerError::UnexpectedValue(format!(
            "decompressed usage json exceeds {limit_bytes} bytes"
        )));
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::read_decompressed_usage_json_with_limit;

    #[test]
    fn decompressed_usage_json_reader_accepts_exact_limit() {
        let decoded = read_decompressed_usage_json_with_limit(Cursor::new(b"1234"), 4)
            .expect("payload at the hard limit should decode");

        assert_eq!(decoded, b"1234");
    }

    #[test]
    fn decompressed_usage_json_reader_rejects_limit_plus_one() {
        let error = read_decompressed_usage_json_with_limit(Cursor::new(b"12345"), 4)
            .expect_err("payload over the hard limit should fail");

        assert!(error
            .to_string()
            .contains("decompressed usage json exceeds 4 bytes"));
    }
}
