use std::io::Error as IoError;

use aether_contracts::StreamFrame;
use bytes::Bytes;

pub fn encode_stream_frame_ndjson(frame: &StreamFrame) -> Result<Bytes, IoError> {
    let mut raw = serde_json::to_vec(frame).map_err(|err| IoError::other(err.to_string()))?;
    raw.push(b'\n');
    Ok(Bytes::from(raw))
}

pub fn decode_stream_frame_ndjson(line: &[u8]) -> Result<StreamFrame, IoError> {
    serde_json::from_slice(line).map_err(|err| IoError::other(err.to_string()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aether_contracts::{
        ExecutionStreamTerminalSummary, StandardizedUsage, StreamFramePayload, StreamFrameType,
    };

    use super::{decode_stream_frame_ndjson, encode_stream_frame_ndjson};

    #[test]
    fn ndjson_round_trip_preserves_frame() {
        let frame = aether_contracts::StreamFrame {
            frame_type: StreamFrameType::Headers,
            payload: StreamFramePayload::Headers {
                status_code: 200,
                headers: BTreeMap::from([("content-type".into(), "text/event-stream".into())]),
                response_observation: Some(aether_contracts::ExecutionResponseObservation {
                    request_started_at_unix_ms: 100,
                    response_headers_observed_at_unix_ms: 125,
                    request_order_id: "0198-order-1".to_string(),
                }),
            },
        };

        let raw = encode_stream_frame_ndjson(&frame).expect("frame should encode");
        let decoded =
            decode_stream_frame_ndjson(raw.trim_ascii_end()).expect("frame should decode");
        assert_eq!(decoded, frame);
    }

    #[test]
    fn ndjson_round_trip_preserves_terminal_usage_with_fractional_fields() {
        let mut usage = StandardizedUsage::new();
        usage.cache_storage_token_hours = 0.125;
        let frame =
            aether_contracts::StreamFrame::eof_with_summary(Some(ExecutionStreamTerminalSummary {
                standardized_usage: Some(usage),
                observed_finish: true,
                ..ExecutionStreamTerminalSummary::default()
            }));

        let raw = encode_stream_frame_ndjson(&frame).expect("frame should encode");
        let decoded = decode_stream_frame_ndjson(raw.trim_ascii_end())
            .expect("terminal usage frame should decode");

        assert_eq!(decoded, frame);
    }
}
