use super::super::users::{
    default_admin_user_api_key_name, format_optional_unix_secs_iso8601,
    generate_admin_user_api_key_plaintext, hash_admin_user_api_key, masked_user_api_key_display,
    normalize_admin_optional_api_key_name, normalize_admin_user_api_formats,
    normalize_admin_user_string_list,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{
    decrypt_catalog_secret_with_fallbacks, encrypt_catalog_secret_with_fallbacks, query_param_bool,
    query_param_optional_bool, query_param_value,
};
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

mod mutation_routes;
mod read_routes;
mod routes;
mod shared;

use self::mutation_routes::{
    build_admin_create_api_key_response, build_admin_delete_api_key_response,
    build_admin_toggle_api_key_response, build_admin_update_api_key_response,
};
use self::read_routes::{build_admin_api_key_detail_response, build_admin_list_api_keys_response};
use self::shared::{
    admin_api_keys_id_from_path, admin_api_keys_operator_id, admin_api_keys_parse_limit,
    admin_api_keys_parse_skip, build_admin_api_key_detail_payload,
    build_admin_api_key_list_item_payload, build_admin_api_keys_bad_request_response,
    build_admin_api_keys_data_unavailable_response, build_admin_api_keys_not_found_response,
    AdminStandaloneApiKeyCreateRequest, AdminStandaloneApiKeyToggleRequest,
};

pub(crate) async fn maybe_build_local_admin_api_keys_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    routes::maybe_build_local_admin_api_keys_routes_response(state, request_context, request_body)
        .await
}
