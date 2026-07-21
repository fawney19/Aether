use serde_json::Value;

use crate::protocol::canonical::CanonicalEmbeddingResponse;

pub fn from(body: &Value) -> Option<CanonicalEmbeddingResponse> {
    crate::formats::openai::embedding::response::from_namespace(body, "jina")
}

pub fn to(response: &CanonicalEmbeddingResponse) -> Option<Value> {
    Some(crate::formats::openai::embedding::response::to_openai_like(
        response, "jina",
    ))
}
