use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::formats::openai::embedding::request::namespace_extensions;
use crate::protocol::canonical::{CanonicalEmbedding, CanonicalEmbeddingResponse, CanonicalUsage};

pub fn from(body_json: &Value) -> Option<CanonicalEmbeddingResponse> {
    let body = body_json.as_object()?;
    if body.contains_key("error") || body.contains_key("code") && body.contains_key("message") {
        return None;
    }
    let data = body
        .get("output")?
        .as_object()?
        .get("embeddings")?
        .as_array()?;
    let mut embeddings = Vec::new();
    for (fallback_index, item) in data.iter().enumerate() {
        let item_object = item.as_object()?;
        let values = item_object.get("embedding")?.as_array()?;
        let embedding = values
            .iter()
            .map(Value::as_f64)
            .collect::<Option<Vec<_>>>()?;
        let mut extensions =
            namespace_extensions("aliyun", item_object, &["index", "embedding", "type"]);
        if let Some(value) = item_object.get("type").cloned() {
            extensions.insert(
                "openai".to_string(),
                Value::Object(Map::from_iter([("type".to_string(), value)])),
            );
        }
        embeddings.push(CanonicalEmbedding {
            index: item_object
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(fallback_index),
            embedding,
            extensions,
        });
    }

    let request_id = body.get("request_id").and_then(Value::as_str);
    let mut extensions =
        namespace_extensions("aliyun", body, &["output", "usage", "request_id", "model"]);
    if let Some(request_id) = request_id {
        extensions.insert(
            "openai".to_string(),
            Value::Object(Map::from_iter([(
                "request_id".to_string(),
                Value::String(request_id.to_string()),
            )])),
        );
    }

    Some(CanonicalEmbeddingResponse {
        id: request_id.unwrap_or("aliyun-request-unknown").to_string(),
        model: body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        embeddings,
        usage: aliyun_usage_to_canonical(body.get("usage")),
        extensions,
    })
}

fn aliyun_usage_to_canonical(value: Option<&Value>) -> Option<CanonicalUsage> {
    let usage = value?.as_object()?;
    let input_tokens = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Some(CanonicalUsage {
        input_tokens,
        output_tokens,
        total_tokens: usage
            .get("total_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(input_tokens.saturating_add(output_tokens)),
        extensions: BTreeMap::from([("aliyun".to_string(), Value::Object(usage.clone()))]),
        ..CanonicalUsage::default()
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::from;
    use crate::formats::openai::embedding::response::to as to_openai;

    #[test]
    fn parses_dashscope_embeddings_to_openai_compatible_shape() {
        let body = json!({
            "output": {
                "embeddings": [
                    {
                        "index": 0,
                        "embedding": [0.1, 0.2, 0.3],
                        "type": "fused"
                    }
                ]
            },
            "usage": {
                "input_tokens": 432,
                "input_tokens_details": {
                    "image_tokens": 402,
                    "text_tokens": 30
                },
                "output_tokens": 1,
                "total_tokens": 433
            },
            "request_id": "aliyun-request-1"
        });

        let canonical = from(&body).expect("aliyun response");
        let emitted = to_openai(&canonical).expect("openai response");

        assert_eq!(emitted["request_id"], "aliyun-request-1");
        assert_eq!(emitted["data"][0]["embedding"], json!([0.1, 0.2, 0.3]));
        assert_eq!(emitted["data"][0]["type"], "fused");
        assert_eq!(emitted["usage"]["prompt_tokens"], 432);
        assert_eq!(emitted["usage"]["completion_tokens"], 1);
        assert_eq!(emitted["usage"]["total_tokens"], 433);
    }
}
