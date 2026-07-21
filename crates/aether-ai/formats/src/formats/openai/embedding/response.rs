use serde_json::Value;
use serde_json::{json, Map};

use crate::formats::openai::embedding::request::namespace_extensions;
use crate::protocol::canonical::{
    canonical_usage_to_openai, namespace_extension_object, openai_usage_to_canonical,
    CanonicalEmbedding, CanonicalEmbeddingResponse,
};

pub fn from(body: &Value) -> Option<CanonicalEmbeddingResponse> {
    from_namespace(body, "openai")
}

pub fn to(response: &CanonicalEmbeddingResponse) -> Option<Value> {
    Some(to_openai_like(response, "openai"))
}

pub(crate) fn from_namespace(
    body_json: &Value,
    namespace: &str,
) -> Option<CanonicalEmbeddingResponse> {
    let body = body_json.as_object()?;
    if body.contains_key("error") {
        return None;
    }
    let data = body.get("data")?.as_array()?;
    let mut embeddings = Vec::new();
    for (fallback_index, item) in data.iter().enumerate() {
        let item_object = item.as_object()?;
        let values = item_object.get("embedding")?.as_array()?;
        let embedding = values
            .iter()
            .map(Value::as_f64)
            .collect::<Option<Vec<_>>>()?;
        embeddings.push(CanonicalEmbedding {
            index: item_object
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(fallback_index),
            embedding,
            extensions: namespace_extensions(
                namespace,
                item_object,
                &["object", "index", "embedding"],
            ),
        });
    }
    Some(CanonicalEmbeddingResponse {
        id: body
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("embd-unknown")
            .to_string(),
        model: body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        embeddings,
        usage: openai_usage_to_canonical(body.get("usage")),
        extensions: namespace_extensions(
            namespace,
            body,
            &["id", "object", "model", "data", "usage"],
        ),
    })
}

pub(crate) fn to_openai_like(canonical: &CanonicalEmbeddingResponse, namespace: &str) -> Value {
    let mut response = Map::new();
    response.insert("object".to_string(), Value::String("list".to_string()));
    if !canonical.model.trim().is_empty() && canonical.model != "unknown" {
        response.insert("model".to_string(), Value::String(canonical.model.clone()));
    }
    response.insert(
        "data".to_string(),
        Value::Array(
            canonical
                .embeddings
                .iter()
                .map(|embedding| {
                    let mut item = Map::new();
                    item.insert("object".to_string(), Value::String("embedding".to_string()));
                    item.insert("index".to_string(), Value::from(embedding.index as u64));
                    item.insert("embedding".to_string(), json!(embedding.embedding));
                    item.extend(namespace_extension_object(
                        &embedding.extensions,
                        namespace,
                        &item,
                    ));
                    Value::Object(item)
                })
                .collect(),
        ),
    );
    if let Some(usage) = &canonical.usage {
        response.insert("usage".to_string(), canonical_usage_to_openai(usage));
    }
    response.extend(namespace_extension_object(
        &canonical.extensions,
        namespace,
        &response,
    ));
    Value::Object(response)
}
