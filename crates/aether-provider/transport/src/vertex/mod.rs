mod auth;
mod context;
mod policy;
mod url;

pub use auth::{
    parse_vertex_service_account_auth_config, resolve_local_vertex_api_key_query_auth,
    resolve_local_vertex_service_account_auth_config,
    supports_local_vertex_service_account_auth_resolution, VertexApiKeyQueryAuth,
    VertexServiceAccountAuthConfig, VertexServiceAccountRefreshAdapter, VERTEX_API_KEY_QUERY_PARAM,
    VERTEX_SERVICE_ACCOUNT_AUTH_HEADER, VERTEX_SERVICE_ACCOUNT_PROVIDER_TYPE,
};
pub use context::{
    is_vertex_api_key_transport_context, is_vertex_service_account_transport_context,
    is_vertex_transport_context, looks_like_vertex_ai_host, uses_vertex_api_key_query_auth,
};
pub use policy::{
    local_vertex_api_key_gemini_transport_unsupported_reason_with_network,
    local_vertex_gemini_transport_unsupported_reason_with_network,
    supports_local_vertex_api_key_gemini_transport,
    supports_local_vertex_api_key_gemini_transport_with_network,
    supports_local_vertex_api_key_imagen_transport,
    supports_local_vertex_api_key_imagen_transport_with_network,
    supports_local_vertex_gemini_transport_with_network,
};
pub use url::{
    build_vertex_api_key_gemini_content_url, build_vertex_api_key_gemini_embedding_url,
    build_vertex_api_key_imagen_content_url, build_vertex_service_account_gemini_content_url,
    build_vertex_service_account_gemini_embedding_url, resolve_vertex_service_account_region,
    VERTEX_API_KEY_BASE_URL,
};

pub const PROVIDER_TYPE: &str = "vertex_ai";
