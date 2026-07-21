//! Compatibility helpers used by the embedded gateway tunnel.

use bytes::Bytes;

pub use aether_contracts::tunnel::{
    decode_payload, encode_connection_close, encode_frame, encode_goaway, encode_goaway_v3,
    encode_hello, encode_load_report, encode_ping, encode_pong, encode_reset_stream,
    encode_settings, encode_stream_error, encode_window_update, frame_payload_by_header,
    ConnectionClosePayload, FrameHeader, GoAwayPayload, HelloPayload, LoadReportPayload,
    RequestMeta, ResetStreamPayload, ResponseMeta, SettingsPayload, WindowUpdatePayload,
    CONNECTION_CLOSE, FLAG_END_STREAM, FLAG_GZIP_COMPRESSED, GOAWAY, HEADER_SIZE, HEARTBEAT_ACK,
    HEARTBEAT_DATA, HELLO, LOAD_REPORT, PING, PONG, REQUEST_BODY, REQUEST_HEADERS, RESET_STREAM,
    RESPONSE_BODY, RESPONSE_HEADERS, SETTINGS, STREAM_END, STREAM_ERROR, WINDOW_UPDATE,
};

pub fn compress_payload(payload: &[u8]) -> Result<(Vec<u8>, u8), std::io::Error> {
    let (compressed, flags) =
        aether_contracts::tunnel::compress_payload(Bytes::copy_from_slice(payload));
    Ok((compressed.to_vec(), flags))
}

pub fn raw_payload(payload: &[u8]) -> (Vec<u8>, u8) {
    let (payload, flags) = aether_contracts::tunnel::raw_payload(Bytes::copy_from_slice(payload));
    (payload.to_vec(), flags)
}

#[cfg(test)]
mod tests {
    use super::{
        compress_payload, decode_payload, encode_frame, raw_payload, FrameHeader,
        FLAG_GZIP_COMPRESSED, RESPONSE_BODY,
    };

    #[test]
    fn vec_compatibility_helpers_round_trip_payloads() {
        let input = vec![b'a'; 4_096];
        let (compressed, flags) = compress_payload(&input).expect("payload should compress");
        assert_eq!(flags & FLAG_GZIP_COMPRESSED, FLAG_GZIP_COMPRESSED);
        let frame = encode_frame(1, RESPONSE_BODY, flags, &compressed);
        let header = FrameHeader::parse(&frame).expect("frame header should parse");
        let decoded = decode_payload(&frame, &header).expect("payload should decode");
        assert_eq!(decoded, input);

        let (raw, raw_flags) = raw_payload(&input);
        assert_eq!(raw, input);
        assert_eq!(raw_flags, 0);
    }
}
