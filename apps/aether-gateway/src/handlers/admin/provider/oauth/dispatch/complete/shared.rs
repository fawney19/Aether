use super::super::super::errors::build_internal_control_error_response;
use super::super::super::state::parse_provider_oauth_callback_params;
use crate::control::GatewayAdminPrincipalContext;
use crate::handlers::admin::request::AdminRequestContext;
use aether_data::repository::provider_oauth::StoredAdminProviderOAuthState;
use axum::{
    body::{Body, Bytes},
    http,
    response::Response,
};

pub(super) struct AdminProviderOAuthCompleteRequest {
    pub(super) callback_url: String,
    pub(super) name: Option<String>,
    pub(super) proxy_node_id: Option<String>,
}

pub(super) struct AdminProviderOAuthCompleteCallback {
    pub(super) code: String,
    pub(super) state_nonce: String,
}

pub(super) fn admin_provider_oauth_state_matches_principal(
    state: &StoredAdminProviderOAuthState,
    request_context: &AdminRequestContext<'_>,
) -> bool {
    admin_provider_oauth_state_matches_resolved_principal(
        state,
        request_context
            .decision()
            .and_then(|decision| decision.admin_principal.as_ref()),
    )
}

fn admin_provider_oauth_state_matches_resolved_principal(
    state: &StoredAdminProviderOAuthState,
    principal: Option<&GatewayAdminPrincipalContext>,
) -> bool {
    let Some(principal) = principal else {
        return false;
    };
    state.initiated_by_user_id == principal.user_id
        && state.initiated_by_session_id == principal.session_id
        && state.initiated_by_management_token_id == principal.management_token_id
        && (principal.session_id.is_some() || principal.management_token_id.is_some())
}

pub(super) fn parse_admin_provider_oauth_callback_url(
    raw_payload: &serde_json::Map<String, serde_json::Value>,
) -> Result<String, Response<Body>> {
    raw_payload
        .get("callback_url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "callback_url 缺少 code/state",
            )
        })
}

pub(super) fn extract_admin_provider_oauth_code(
    params: &std::collections::BTreeMap<String, String>,
) -> Result<String, Response<Body>> {
    params
        .get("code")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "callback_url 缺少 code/state",
            )
        })
}

pub(super) fn extract_admin_provider_oauth_state(
    params: &std::collections::BTreeMap<String, String>,
) -> Result<String, Response<Body>> {
    params
        .get("state")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "callback_url 缺少 code/state",
            )
        })
}

pub(super) fn parse_admin_provider_oauth_complete_request_body(
    request_body: Option<&Bytes>,
) -> Result<AdminProviderOAuthCompleteRequest, Response<Body>> {
    let Some(request_body) = request_body else {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "请求体必须是合法的 JSON 对象",
        ));
    };
    let raw_payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => {
            return Err(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求体必须是合法的 JSON 对象",
            ));
        }
    };
    let callback_url = parse_admin_provider_oauth_callback_url(&raw_payload)?;

    Ok(AdminProviderOAuthCompleteRequest {
        callback_url,
        name: raw_payload
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        proxy_node_id: raw_payload
            .get("proxy_node_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
    })
}

pub(super) fn parse_admin_provider_oauth_complete_callback(
    callback_url: &str,
) -> Result<AdminProviderOAuthCompleteCallback, Response<Body>> {
    let params = parse_provider_oauth_callback_params(callback_url);
    let code = extract_admin_provider_oauth_code(&params)?;
    let state_nonce = extract_admin_provider_oauth_state(&params)?;

    Ok(AdminProviderOAuthCompleteCallback { code, state_nonce })
}

#[cfg(test)]
mod tests {
    use super::admin_provider_oauth_state_matches_resolved_principal;
    use crate::control::GatewayAdminPrincipalContext;
    use aether_data::repository::provider_oauth::StoredAdminProviderOAuthState;

    fn state() -> StoredAdminProviderOAuthState {
        StoredAdminProviderOAuthState {
            nonce: "a".repeat(64),
            key_id: "key-1".to_string(),
            provider_id: "provider-1".to_string(),
            provider_type: "codex".to_string(),
            pkce_verifier: Some("verifier".to_string()),
            expected_encrypted_auth_config: None,
            initiated_by_user_id: "admin-1".to_string(),
            initiated_by_session_id: Some("session-1".to_string()),
            initiated_by_management_token_id: None,
            created_at: 1,
        }
    }

    fn principal(user_id: &str, session_id: Option<&str>) -> GatewayAdminPrincipalContext {
        GatewayAdminPrincipalContext {
            user_id: user_id.to_string(),
            user_role: "admin".to_string(),
            session_id: session_id.map(ToOwned::to_owned),
            management_token_id: None,
            management_token_permissions: None,
        }
    }

    #[test]
    fn provider_oauth_state_is_bound_to_exact_admin_session() {
        let state = state();
        let matching = principal("admin-1", Some("session-1"));
        let wrong_user = principal("admin-2", Some("session-1"));
        let wrong_session = principal("admin-1", Some("session-2"));

        assert!(admin_provider_oauth_state_matches_resolved_principal(
            &state,
            Some(&matching)
        ));
        assert!(!admin_provider_oauth_state_matches_resolved_principal(
            &state,
            Some(&wrong_user)
        ));
        assert!(!admin_provider_oauth_state_matches_resolved_principal(
            &state,
            Some(&wrong_session)
        ));
        assert!(!admin_provider_oauth_state_matches_resolved_principal(
            &state, None
        ));
    }
}
