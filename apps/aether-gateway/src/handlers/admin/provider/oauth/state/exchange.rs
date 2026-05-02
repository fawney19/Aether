use super::super::errors::{
    build_internal_control_error_response, normalize_provider_oauth_refresh_error_message,
};
use crate::handlers::admin::request::{AdminAppState, AdminProviderOAuthTemplate};
use aether_contracts::ProxySnapshot;
use aether_oauth::provider::providers::GenericProviderOAuthAdapter;
use aether_oauth::provider::{ProviderOAuthService, ProviderOAuthTransportContext};
use axum::{body::Body, http, response::Response};
use std::sync::Arc;

fn provider_oauth_transport_error_detail(prefix: &str, error: &str) -> String {
    let error = error.trim();
    if error.is_empty() {
        return prefix.to_string();
    }
    format!("{prefix}: {error}")
}

fn provider_oauth_exchange_context(
    provider_type: &str,
    proxy: Option<ProxySnapshot>,
) -> ProviderOAuthTransportContext {
    ProviderOAuthTransportContext {
        provider_id: String::new(),
        provider_type: provider_type.to_string(),
        endpoint_id: None,
        key_id: None,
        auth_type: Some("oauth".to_string()),
        decrypted_api_key: None,
        decrypted_auth_config: None,
        provider_config: None,
        endpoint_config: None,
        key_config: None,
        network: aether_oauth::network::OAuthNetworkContext::provider_operation(proxy),
    }
}

fn provider_oauth_service_for_template(
    template: AdminProviderOAuthTemplate,
    token_url: String,
) -> Result<ProviderOAuthService, Response<Body>> {
    GenericProviderOAuthAdapter::for_provider_type(template.provider_type)
        .map(|adapter| adapter.with_token_url_override(token_url))
        .map(|adapter| ProviderOAuthService::new().with_adapter(Arc::new(adapter)))
        .ok_or_else(|| {
            build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "该 Provider 不支持 OAuth 授权",
            )
        })
}

fn token_payload_from_provider_oauth_result(
    result: aether_oauth::provider::ProviderOAuthTokenSet,
) -> Result<serde_json::Value, Response<Body>> {
    result.token_set.raw_payload.ok_or_else(|| {
        build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "token exchange 返回缺少 access_token",
        )
    })
}

pub(crate) async fn exchange_admin_provider_oauth_code(
    state: &AdminAppState<'_>,
    template: AdminProviderOAuthTemplate,
    code: &str,
    state_nonce: &str,
    pkce_verifier: Option<&str>,
    proxy: Option<ProxySnapshot>,
) -> Result<serde_json::Value, Response<Body>> {
    let token_url = state.provider_oauth_token_url(template.provider_type, template.token_url);
    let service = provider_oauth_service_for_template(template, token_url)?;
    let ctx = provider_oauth_exchange_context(template.provider_type, proxy);
    let executor = crate::oauth::GatewayOAuthHttpExecutor::new(*state);
    let result = service
        .exchange_code(&executor, &ctx, code, state_nonce, pkce_verifier)
        .await
        .map_err(|error| match error {
            aether_oauth::core::OAuthError::HttpStatus { .. } => {
                build_internal_control_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "token exchange 失败",
                )
            }
            error => build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                provider_oauth_transport_error_detail("token exchange 失败", &error.to_string()),
            ),
        })?;
    token_payload_from_provider_oauth_result(result)
}

pub(crate) async fn exchange_admin_provider_oauth_refresh_token(
    state: &AdminAppState<'_>,
    template: AdminProviderOAuthTemplate,
    refresh_token: &str,
    proxy: Option<ProxySnapshot>,
) -> Result<serde_json::Value, Response<Body>> {
    let token_url = state.provider_oauth_token_url(template.provider_type, template.token_url);
    let service = provider_oauth_service_for_template(template, token_url)?;
    let ctx = provider_oauth_exchange_context(template.provider_type, proxy);
    let executor = crate::oauth::GatewayOAuthHttpExecutor::new(*state);
    let input = aether_oauth::provider::ProviderOAuthImportInput {
        provider_type: template.provider_type.to_string(),
        name: None,
        refresh_token: Some(refresh_token.to_string()),
        raw_credentials: None,
        network: ctx.network.clone(),
    };
    let result = service
        .import_credentials(&executor, &ctx, input)
        .await
        .map_err(|error| match error {
            aether_oauth::core::OAuthError::HttpStatus {
                status_code,
                body_excerpt,
            } => {
                let reason = normalize_provider_oauth_refresh_error_message(
                    Some(status_code),
                    Some(&body_excerpt),
                );
                build_internal_control_error_response(
                    http::StatusCode::BAD_REQUEST,
                    format!("Refresh Token 验证失败: {reason}"),
                )
            }
            error => build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                provider_oauth_transport_error_detail(
                    "Refresh Token 验证失败: token exchange 失败",
                    &error.to_string(),
                ),
            ),
        })?;
    token_payload_from_provider_oauth_result(result).map_err(|_| {
        build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "token refresh 返回缺少 access_token",
        )
    })
}
