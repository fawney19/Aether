use serde_json::Map;
use serde_json::Value;
use std::collections::BTreeMap;

use crate::formats::context::FormatContext;
use crate::protocol::canonical::{
    namespace_extension_object, CanonicalEmbeddingInput, CanonicalEmbeddingRequest,
    CanonicalRequest,
};

pub fn from(body: &Value, _ctx: &FormatContext) -> Option<CanonicalRequest> {
    from_namespace(body, "openai")
}

pub fn to(request: &CanonicalRequest, ctx: &FormatContext) -> Option<Value> {
    to_openai_like(
        request,
        ctx.mapped_model_or(request.model.as_str()),
        "openai",
        false,
    )
}

pub(crate) fn from_namespace(body_json: &Value, namespace: &str) -> Option<CanonicalRequest> {
    let request = body_json.as_object()?;
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let input =
        serde_json::from_value::<CanonicalEmbeddingInput>(request.get("input")?.clone()).ok()?;
    if input.is_empty() {
        return None;
    }

    let embedding = CanonicalEmbeddingRequest {
        input,
        encoding_format: request
            .get("encoding_format")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        dimensions: request.get("dimensions").and_then(Value::as_u64),
        task: request
            .get("task")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        user: request
            .get("user")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        parameters: request
            .get("parameters")
            .and_then(Value::as_object)
            .cloned(),
        extensions: namespace_extensions(
            namespace,
            request,
            &[
                "model",
                "input",
                "encoding_format",
                "dimensions",
                "task",
                "user",
                "parameters",
            ],
        ),
    };

    Some(CanonicalRequest {
        model,
        embedding: Some(embedding),
        ..CanonicalRequest::default()
    })
}

pub(crate) fn to_openai_like(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    namespace: &str,
    default_task: bool,
) -> Option<Value> {
    let embedding = canonical.embedding.as_ref()?;
    if embedding.input.is_empty() {
        return None;
    }
    if matches!(&embedding.input, CanonicalEmbeddingInput::Multimodal(_)) {
        return None;
    }
    let mut output = Map::new();
    output.insert(
        "model".to_string(),
        Value::String(mapped_embedding_model(canonical, mapped_model)),
    );
    output.insert(
        "input".to_string(),
        serde_json::to_value(&embedding.input).ok()?,
    );
    if let Some(value) = &embedding.encoding_format {
        output.insert("encoding_format".to_string(), Value::String(value.clone()));
    }
    if let Some(value) = embedding.dimensions {
        output.insert("dimensions".to_string(), Value::from(value));
    }
    if let Some(value) = &embedding.user {
        output.insert("user".to_string(), Value::String(value.clone()));
    }
    if let Some(value) = &embedding.parameters {
        output.insert("parameters".to_string(), Value::Object(value.clone()));
    }
    if let Some(task) = embedding
        .task
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        output.insert("task".to_string(), Value::String(task.clone()));
    } else if default_task {
        output.insert(
            "task".to_string(),
            Value::String("text-matching".to_string()),
        );
    }
    output.extend(namespace_extension_object(
        &embedding.extensions,
        namespace,
        &output,
    ));
    Some(Value::Object(output))
}

pub(crate) fn mapped_embedding_model(canonical: &CanonicalRequest, mapped_model: &str) -> String {
    let mapped_model = mapped_model.trim();
    if mapped_model.is_empty() {
        canonical.model.clone()
    } else {
        mapped_model.to_string()
    }
}

pub(crate) fn namespace_extensions(
    namespace: &str,
    object: &Map<String, Value>,
    handled_keys: &[&str],
) -> BTreeMap<String, Value> {
    let handled = handled_keys
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let raw = object
        .iter()
        .filter(|(key, _)| !handled.contains(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<String, Value>>();
    if raw.is_empty() {
        BTreeMap::new()
    } else {
        BTreeMap::from([(namespace.to_string(), Value::Object(raw))])
    }
}
