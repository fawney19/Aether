use std::collections::BTreeMap;

use aether_contracts::ResponseBody;
use base64::Engine as _;

use crate::GatewayError;

type DecodedBody = (Vec<u8>, Option<serde_json::Value>, Option<String>);

pub(super) fn decode_execution_result_body(
    body: Option<ResponseBody>,
    headers: &mut BTreeMap<String, String>,
) -> Result<DecodedBody, GatewayError> {
    let Some(body) = body else {
        return Ok((Vec::new(), None, None));
    };

    if let Some(json_body) = body.json_body {
        remove_header_case_insensitive(headers, "content-encoding");
        remove_header_case_insensitive(headers, "content-length");
        headers
            .entry("content-type".to_string())
            .or_insert_with(|| "application/json".to_string());
        let bytes = serde_json::to_vec(&json_body)
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        headers.insert("content-length".to_string(), bytes.len().to_string());
        return Ok((bytes, Some(json_body), None));
    }

    if let Some(body_bytes_b64) = body.body_bytes_b64 {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&body_bytes_b64)
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok((bytes, None, Some(body_bytes_b64)));
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
}
