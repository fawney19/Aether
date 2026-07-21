use serde_json::Map;
use serde_json::Value;

use crate::formats::context::FormatContext;
use crate::formats::openai::embedding::request::mapped_embedding_model;
use crate::protocol::canonical::{namespace_extension_object, CanonicalRequest};

pub fn from(body: &Value, _ctx: &FormatContext) -> Option<CanonicalRequest> {
    crate::formats::openai::embedding::request::from_namespace(body, "doubao")
}

pub fn to(request: &CanonicalRequest, ctx: &FormatContext) -> Option<Value> {
    let embedding = request.embedding.as_ref()?;
    let items = embedding.input.as_string_items()?;
    if items.is_empty() || items.iter().any(|value| value.trim().is_empty()) {
        return None;
    }
    let mut output = Map::new();
    output.insert(
        "model".to_string(),
        Value::String(mapped_embedding_model(
            request,
            ctx.mapped_model_or(request.model.as_str()),
        )),
    );
    output.insert(
        "input".to_string(),
        Value::Array(
            items
                .into_iter()
                .map(|text| Value::String(text.to_string()))
                .collect(),
        ),
    );
    if let Some(dimensions) = embedding.dimensions {
        output.insert("dimensions".to_string(), Value::from(dimensions));
    }
    output.extend(namespace_extension_object(
        &embedding.extensions,
        "doubao",
        &output,
    ));
    Some(Value::Object(output))
}
