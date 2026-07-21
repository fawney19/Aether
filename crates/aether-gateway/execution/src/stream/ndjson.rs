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

    use aether_contracts::{StreamFramePayload, StreamFrameType};

    use super::{decode_stream_frame_ndjson, encode_stream_frame_ndjson};

    #[test]
    fn ndjson_round_trip_preserves_frame() {
        let frame = aether_contracts::StreamFrame {
            frame_type: StreamFrameType::Headers,
            payload: StreamFramePayload::Headers {
                status_code: 200,
                headers: BTreeMap::from([("content-type".into(), "text/event-stream".into())]),
            },
        };

        let raw = encode_stream_frame_ndjson(&frame).expect("frame should encode");
        let decoded =
            decode_stream_frame_ndjson(raw.trim_ascii_end()).expect("frame should decode");
        assert_eq!(decoded, frame);
    }
}
