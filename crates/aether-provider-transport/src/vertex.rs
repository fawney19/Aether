mod auth;
mod context;
mod policy;
mod url;

pub use auth::{
    resolve_local_vertex_api_key_query_auth, VertexApiKeyQueryAuth, VERTEX_API_KEY_QUERY_PARAM,
};
pub use context::{
    is_vertex_api_key_transport_context, looks_like_vertex_ai_host, uses_vertex_api_key_query_auth,
};
pub use policy::{
    local_vertex_api_key_gemini_transport_unsupported_reason_with_network,
    supports_local_vertex_api_key_gemini_transport,
    supports_local_vertex_api_key_gemini_transport_with_network,
    supports_local_vertex_api_key_imagen_transport,
    supports_local_vertex_api_key_imagen_transport_with_network,
};
pub use url::{
    build_vertex_api_key_gemini_content_url, build_vertex_api_key_imagen_content_url,
    VERTEX_API_KEY_BASE_URL,
};

pub const PROVIDER_TYPE: &str = "vertex_ai";
