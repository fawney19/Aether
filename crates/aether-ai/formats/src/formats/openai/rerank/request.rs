use serde_json::Map;
use serde_json::Value;

use crate::formats::context::FormatContext;
use crate::formats::openai::embedding::request::namespace_extensions;
use crate::protocol::canonical::{
    namespace_extension_object, CanonicalRequest, CanonicalRerankRequest,
};

pub fn from(body: &Value, _ctx: &FormatContext) -> Option<CanonicalRequest> {
    from_namespace(body, "openai")
}

pub fn to(request: &CanonicalRequest, ctx: &FormatContext) -> Option<Value> {
    to_openai_like(
        request,
        ctx.mapped_model_or(request.model.as_str()),
        "openai",
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
    let query = request
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let documents = request.get("documents").and_then(Value::as_array)?.to_vec();
    let rerank = CanonicalRerankRequest {
        query,
        documents,
        top_n: request
            .get("top_n")
            .or_else(|| request.get("topN"))
            .and_then(Value::as_u64),
        return_documents: request
            .get("return_documents")
            .or_else(|| request.get("returnDocuments"))
            .and_then(Value::as_bool),
        extensions: namespace_extensions(
            namespace,
            request,
            &[
                "model",
                "query",
                "documents",
                "top_n",
                "topN",
                "return_documents",
                "returnDocuments",
            ],
        ),
    };
    if rerank.is_empty() || rerank.top_n == Some(0) {
        return None;
    }

    Some(CanonicalRequest {
        model,
        rerank: Some(rerank),
        ..CanonicalRequest::default()
    })
}

pub(crate) fn to_openai_like(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    namespace: &str,
) -> Option<Value> {
    let rerank = canonical.rerank.as_ref()?;
    if rerank.is_empty() || rerank.top_n == Some(0) {
        return None;
    }
    let mut output = Map::new();
    output.insert(
        "model".to_string(),
        Value::String(mapped_rerank_model(canonical, mapped_model)),
    );
    output.insert("query".to_string(), Value::String(rerank.query.clone()));
    output.insert(
        "documents".to_string(),
        Value::Array(rerank.documents.clone()),
    );
    if let Some(value) = rerank.top_n {
        output.insert("top_n".to_string(), Value::from(value));
    }
    if let Some(value) = rerank.return_documents {
        output.insert("return_documents".to_string(), Value::Bool(value));
    }
    output.extend(namespace_extension_object(
        &rerank.extensions,
        namespace,
        &output,
    ));
    Some(Value::Object(output))
}

fn mapped_rerank_model(canonical: &CanonicalRequest, mapped_model: &str) -> String {
    let mapped_model = mapped_model.trim();
    if mapped_model.is_empty() {
        canonical.model.clone()
    } else {
        mapped_model.to_string()
    }
}
