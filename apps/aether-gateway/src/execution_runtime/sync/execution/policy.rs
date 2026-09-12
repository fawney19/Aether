use aether_contracts::ResponseBody;
use serde_json::Value;
use std::collections::BTreeMap;

use crate::execution_runtime::transport::{
    decode_base64_body_with_limit,
    serialize_json_body_with_limit as serialize_transport_json_body_with_limit,
};
use crate::GatewayError;

type DecodedBody = (Vec<u8>, Option<serde_json::Value>, Option<String>);

fn serialize_json_body_with_limit(body: &Value, limit: usize) -> Result<Vec<u8>, GatewayError> {
    serialize_transport_json_body_with_limit(body, limit)
        .map_err(|error| GatewayError::Internal(error.to_string()))
}

pub(super) fn decode_execution_result_body(
    body: Option<ResponseBody>,
    headers: &mut BTreeMap<String, String>,
) -> Result<DecodedBody, GatewayError> {
    decode_execution_result_body_with_limit(
        body,
        headers,
        crate::headers::max_internal_buffered_body_bytes(),
    )
}

pub(super) fn decode_execution_result_body_with_limit(
    body: Option<ResponseBody>,
    headers: &mut BTreeMap<String, String>,
    body_limit: usize,
) -> Result<DecodedBody, GatewayError> {
    let Some(body) = body else {
        return Ok((Vec::new(), None, None));
    };
    let ResponseBody {
        json_body,
        body_bytes_b64,
    } = body;
    if let Some(body_bytes_b64) = body_bytes_b64 {
        let bytes = decode_base64_body_with_limit(&body_bytes_b64, body_limit)
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok((bytes, json_body, Some(body_bytes_b64)));
    }

    if let Some(json_body) = json_body {
        remove_header_case_insensitive(headers, "content-encoding");
        remove_header_case_insensitive(headers, "content-length");
        headers
            .entry("content-type".to_string())
            .or_insert_with(|| "application/json".to_string());
        let bytes = serialize_json_body_with_limit(&json_body, body_limit)?;
        headers.insert("content-length".to_string(), bytes.len().to_string());
        return Ok((bytes, Some(json_body), None));
    }

    Ok((Vec::new(), None, None))
}

fn remove_header_case_insensitive(headers: &mut BTreeMap<String, String>, name: &str) {
    if let Some(existing_key) = headers
        .keys()
        .find(|key| key.eq_ignore_ascii_case(name))
        .cloned()
    {
        headers.remove(&existing_key);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aether_contracts::ResponseBody;
    use base64::Engine as _;
    use serde_json::json;

    use super::decode_execution_result_body;

    #[test]
    fn decoded_json_body_drops_stale_content_encoding_headers() {
        let mut headers = BTreeMap::from([
            ("content-encoding".to_string(), "gzip".to_string()),
            ("content-length".to_string(), "999".to_string()),
        ]);

        let (body_bytes, body_json, body_base64) = decode_execution_result_body(
            Some(ResponseBody {
                json_body: Some(json!({"ok": true})),
                body_bytes_b64: None,
            }),
            &mut headers,
        )
        .expect("body should decode");

        assert_eq!(body_json, Some(json!({"ok": true})));
        assert_eq!(body_base64, None);
        assert_eq!(body_bytes, br#"{"ok":true}"#);
        assert_eq!(headers.get("content-encoding"), None);
        assert_eq!(
            headers.get("content-length").cloned(),
            Some(body_bytes.len().to_string())
        );
    }

    #[test]
    fn dual_body_prefers_wire_bytes_and_retains_parsed_json() {
        let raw = br#"{ "unknown": true, "ok": true }"#;
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
        let raw_len = raw.len().to_string();
        let mut headers = BTreeMap::from([
            ("content-encoding".to_string(), "gzip".to_string()),
            ("content-length".to_string(), raw_len.clone()),
        ]);

        let (body_bytes, body_json, body_base64) = decode_execution_result_body(
            Some(ResponseBody {
                json_body: Some(json!({"unknown": true, "ok": true})),
                body_bytes_b64: Some(encoded.clone()),
            }),
            &mut headers,
        )
        .expect("body should decode");

        assert_eq!(body_bytes, raw);
        assert_eq!(body_json, Some(json!({"unknown": true, "ok": true})));
        assert_eq!(body_base64.as_deref(), Some(encoded.as_str()));
        assert_eq!(
            headers.get("content-encoding").map(String::as_str),
            Some("gzip")
        );
        assert_eq!(
            headers.get("content-length").map(String::as_str),
            Some(raw_len.as_str())
        );
    }

    #[test]
    fn scoped_decode_limit_can_cover_a_bounded_synthetic_envelope() {
        let raw = vec![b'x'; 65 * 1024];
        let encoded = base64::engine::general_purpose::STANDARD.encode(&raw);
        let mut headers = BTreeMap::new();

        let (decoded, json, retained) = super::decode_execution_result_body_with_limit(
            Some(ResponseBody {
                json_body: None,
                body_bytes_b64: Some(encoded.clone()),
            }),
            &mut headers,
            raw.len(),
        )
        .expect("body at the scoped limit should decode");
        assert_eq!(decoded, raw);
        assert_eq!(json, None);
        assert_eq!(retained.as_deref(), Some(encoded.as_str()));

        assert!(super::decode_execution_result_body_with_limit(
            Some(ResponseBody {
                json_body: None,
                body_bytes_b64: Some(encoded),
            }),
            &mut headers,
            raw.len() - 1,
        )
        .is_err());
    }
}
