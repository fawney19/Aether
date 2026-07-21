use serde_json::Value;

use crate::formats::context::FormatContext;
use crate::protocol::canonical::CanonicalRequest;

pub fn from(body: &Value, _ctx: &FormatContext) -> Option<CanonicalRequest> {
    crate::formats::openai::embedding::request::from_namespace(body, "jina")
}

pub fn to(request: &CanonicalRequest, ctx: &FormatContext) -> Option<Value> {
    crate::formats::openai::embedding::request::to_openai_like(
        request,
        ctx.mapped_model_or(request.model.as_str()),
        "jina",
        true,
    )
}
