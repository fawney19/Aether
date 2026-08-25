use crate::handlers::public::support::build_unhandled_public_support_response;
use axum::{body::Body, http, response::Response};

use super::{
    handle_auth_me, handle_users_me_api_key_capabilities_put, handle_users_me_api_key_create,
    handle_users_me_api_key_delete, handle_users_me_api_key_detail_get,
    handle_users_me_api_key_install_session_create, handle_users_me_api_key_patch,
    handle_users_me_api_key_providers_put, handle_users_me_api_key_update,
    handle_users_me_api_keys_get, handle_users_me_available_models,
    handle_users_me_client_config_get, handle_users_me_delete_other_sessions,
    handle_users_me_delete_session, handle_users_me_detail_put,
    handle_users_me_endpoint_status_get, handle_users_me_groups_get,
    handle_users_me_management_token_create, handle_users_me_management_token_delete,
    handle_users_me_management_token_detail_get, handle_users_me_management_token_regenerate,
    handle_users_me_management_token_toggle, handle_users_me_management_token_update,
    handle_users_me_management_tokens_list, handle_users_me_model_capabilities_get,
    handle_users_me_model_capabilities_put, handle_users_me_password_patch,
    handle_users_me_preferences_get, handle_users_me_preferences_put,
    handle_users_me_providers_get, handle_users_me_referral_get, handle_users_me_sessions_get,
    handle_users_me_update_session, handle_users_me_usage_active_get, handle_users_me_usage_get,
    handle_users_me_usage_heatmap_get, handle_users_me_usage_interval_timeline_get,
    users_me_api_key_capabilities_path_matches, users_me_api_key_detail_path_matches,
    users_me_api_key_install_sessions_path_matches, users_me_api_key_providers_path_matches,
    users_me_management_token_detail_path_matches,
    users_me_management_token_regenerate_path_matches,
    users_me_management_token_toggle_path_matches, users_me_management_tokens_root,
    users_me_session_detail_path_matches, AppState, GatewayPublicRequestContext,
};

pub(crate) async fn maybe_build_local_users_me_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&axum::body::Bytes>,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_family.as_deref() != Some("users_me") {
        return None;
    }

    match decision.route_kind.as_deref() {
        Some("detail") if request_context.request_path == "/api/users/me" => {
            Some(handle_auth_me(state, request_context, headers).await)
        }
        Some("update_detail") if request_context.request_path == "/api/users/me" => {
            Some(handle_users_me_detail_put(state, request_context, headers, request_body).await)
        }
        Some("password") if request_context.request_path == "/api/users/me/password" => Some(
            handle_users_me_password_patch(state, request_context, headers, request_body).await,
        ),
        Some("sessions") if request_context.request_path == "/api/users/me/sessions" => {
            Some(handle_users_me_sessions_get(state, request_context, headers).await)
        }
        Some("sessions_others_delete")
            if request_context.request_path == "/api/users/me/sessions/others" =>
        {
            Some(handle_users_me_delete_other_sessions(state, request_context, headers).await)
        }
        Some("session_delete")
            if users_me_session_detail_path_matches(&request_context.request_path) =>
        {
            Some(handle_users_me_delete_session(state, request_context, headers).await)
        }
        Some("session_update")
            if users_me_session_detail_path_matches(&request_context.request_path) =>
        {
            Some(
                handle_users_me_update_session(state, request_context, headers, request_body).await,
            )
        }
        Some("api_keys_list") if request_context.request_path == "/api/users/me/api-keys" => {
            Some(handle_users_me_api_keys_get(state, request_context, headers).await)
        }
        Some("groups") if request_context.request_path == "/api/users/me/groups" => {
            Some(handle_users_me_groups_get(state, request_context, headers).await)
        }
        Some("management_tokens_list")
            if users_me_management_tokens_root(&request_context.request_path) =>
        {
            Some(handle_users_me_management_tokens_list(state, request_context, headers).await)
        }
        Some("api_keys_create") if request_context.request_path == "/api/users/me/api-keys" => {
            Some(
                handle_users_me_api_key_create(state, request_context, headers, request_body).await,
            )
        }
        Some("management_tokens_create")
            if users_me_management_tokens_root(&request_context.request_path) =>
        {
            Some(
                handle_users_me_management_token_create(
                    state,
                    request_context,
                    headers,
                    request_body,
                )
                .await,
            )
        }
        Some("api_key_detail")
            if users_me_api_key_detail_path_matches(&request_context.request_path) =>
        {
            Some(handle_users_me_api_key_detail_get(state, request_context, headers).await)
        }
        Some("management_token_detail")
            if users_me_management_token_detail_path_matches(&request_context.request_path) =>
        {
            Some(handle_users_me_management_token_detail_get(state, request_context, headers).await)
        }
        Some("api_key_update")
            if users_me_api_key_detail_path_matches(&request_context.request_path) =>
        {
            Some(
                handle_users_me_api_key_update(state, request_context, headers, request_body).await,
            )
        }
        Some("management_token_update")
            if users_me_management_token_detail_path_matches(&request_context.request_path) =>
        {
            Some(
                handle_users_me_management_token_update(
                    state,
                    request_context,
                    headers,
                    request_body,
                )
                .await,
            )
        }
        Some("api_key_patch")
            if users_me_api_key_detail_path_matches(&request_context.request_path) =>
        {
            Some(handle_users_me_api_key_patch(state, request_context, headers, request_body).await)
        }
        Some("management_token_toggle")
            if users_me_management_token_toggle_path_matches(&request_context.request_path) =>
        {
            Some(handle_users_me_management_token_toggle(state, request_context, headers).await)
        }
        Some("api_key_delete")
            if users_me_api_key_detail_path_matches(&request_context.request_path) =>
        {
            Some(handle_users_me_api_key_delete(state, request_context, headers).await)
        }
        Some("management_token_delete")
            if users_me_management_token_detail_path_matches(&request_context.request_path) =>
        {
            Some(handle_users_me_management_token_delete(state, request_context, headers).await)
        }
        Some("api_key_providers_update")
            if users_me_api_key_providers_path_matches(&request_context.request_path) =>
        {
            Some(
                handle_users_me_api_key_providers_put(
                    state,
                    request_context,
                    headers,
                    request_body,
                )
                .await,
            )
        }
        Some("api_key_capabilities_update")
            if users_me_api_key_capabilities_path_matches(&request_context.request_path) =>
        {
            Some(
                handle_users_me_api_key_capabilities_put(
                    state,
                    request_context,
                    headers,
                    request_body,
                )
                .await,
            )
        }
        Some("api_key_install_session_create")
            if users_me_api_key_install_sessions_path_matches(&request_context.request_path) =>
        {
            Some(
                handle_users_me_api_key_install_session_create(
                    state,
                    request_context,
                    headers,
                    request_body,
                )
                .await,
            )
        }
        Some("management_token_regenerate")
            if users_me_management_token_regenerate_path_matches(&request_context.request_path) =>
        {
            Some(handle_users_me_management_token_regenerate(state, request_context, headers).await)
        }
        Some("usage") if request_context.request_path == "/api/users/me/usage" => {
            Some(handle_users_me_usage_get(state, request_context, headers).await)
        }
        Some("usage_active") if request_context.request_path == "/api/users/me/usage/active" => {
            Some(handle_users_me_usage_active_get(state, request_context, headers).await)
        }
        Some("usage_interval_timeline")
            if request_context.request_path == "/api/users/me/usage/interval-timeline" =>
        {
            Some(handle_users_me_usage_interval_timeline_get(state, request_context, headers).await)
        }
        Some("usage_heatmap") if request_context.request_path == "/api/users/me/usage/heatmap" => {
            Some(handle_users_me_usage_heatmap_get(state, request_context, headers).await)
        }
        Some("endpoint_status")
            if request_context.request_path == "/api/users/me/endpoint-status" =>
        {
            Some(handle_users_me_endpoint_status_get(state, request_context, headers).await)
        }
        Some("providers") if request_context.request_path == "/api/users/me/providers" => {
            Some(handle_users_me_providers_get(state, request_context, headers).await)
        }
        Some("preferences") if request_context.request_path == "/api/users/me/preferences" => {
            Some(handle_users_me_preferences_get(state, request_context, headers).await)
        }
        Some("referral") if request_context.request_path == "/api/users/me/referral" => {
            Some(handle_users_me_referral_get(state, request_context, headers).await)
        }
        Some("available_models")
            if request_context.request_path == "/api/users/me/available-models" =>
        {
            Some(handle_users_me_available_models(state, request_context, headers).await)
        }
        Some("client_config") if request_context.request_path == "/api/users/me/client-config" => {
            Some(handle_users_me_client_config_get(state, request_context, headers).await)
        }
        Some("model_capabilities")
            if request_context.request_path == "/api/users/me/model-capabilities" =>
        {
            Some(handle_users_me_model_capabilities_get(state, request_context, headers).await)
        }
        Some("model_capabilities_update")
            if request_context.request_path == "/api/users/me/model-capabilities" =>
        {
            Some(
                handle_users_me_model_capabilities_put(
                    state,
                    request_context,
                    headers,
                    request_body,
                )
                .await,
            )
        }
        Some("preferences_update")
            if request_context.request_path == "/api/users/me/preferences" =>
        {
            Some(
                handle_users_me_preferences_put(state, request_context, headers, request_body)
                    .await,
            )
        }
        _ => Some(build_unhandled_public_support_response(request_context)),
    }
}
