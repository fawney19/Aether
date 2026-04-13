use super::super::errors::{
    build_internal_control_error_response, normalize_provider_oauth_refresh_error_message,
};
use super::json_non_empty_string;
use crate::handlers::admin::request::{AdminAppState, AdminProviderOAuthTemplate};
use axum::{body::Body, http, response::Response};
use url::form_urlencoded;

pub(crate) async fn exchange_admin_provider_oauth_code(
    state: &AdminAppState<'_>,
    template: AdminProviderOAuthTemplate,
    code: &str,
    state_nonce: &str,
    pkce_verifier: Option<&str>,
    proxy_node_id: Option<&str>,
) -> Result<serde_json::Value, Response<Body>> {
    let token_url = state.provider_oauth_token_url(template.provider_type, template.token_url);
    let response = if template.provider_type == "claude_code" {
        let mut body = serde_json::Map::from_iter([
            (
                "grant_type".to_string(),
                serde_json::Value::String("authorization_code".to_string()),
            ),
            (
                "client_id".to_string(),
                serde_json::Value::String(template.client_id.to_string()),
            ),
            (
                "redirect_uri".to_string(),
                serde_json::Value::String(template.redirect_uri.to_string()),
            ),
            (
                "code".to_string(),
                serde_json::Value::String(code.to_string()),
            ),
            (
                "state".to_string(),
                serde_json::Value::String(state_nonce.to_string()),
            ),
        ]);
        if let Some(verifier) = pkce_verifier {
            body.insert(
                "code_verifier".to_string(),
                serde_json::Value::String(verifier.to_string()),
            );
        }
        let headers = reqwest::header::HeaderMap::from_iter([
            (
                reqwest::header::CONTENT_TYPE,
                reqwest::header::HeaderValue::from_static("application/json"),
            ),
            (
                reqwest::header::ACCEPT,
                reqwest::header::HeaderValue::from_static("application/json"),
            ),
        ]);
        state
            .execute_admin_provider_oauth_http_request(
                "provider-oauth:exchange-code",
                reqwest::Method::POST,
                &token_url,
                &headers,
                Some("application/json"),
                Some(serde_json::Value::Object(body)),
                None,
                proxy_node_id,
            )
            .await
    } else {
        let form_body = {
            let mut form = form_urlencoded::Serializer::new(String::new());
            form.append_pair("grant_type", "authorization_code");
            form.append_pair("client_id", template.client_id);
            form.append_pair("redirect_uri", template.redirect_uri);
            form.append_pair("code", code);
            if !template.client_secret.trim().is_empty() {
                form.append_pair("client_secret", template.client_secret);
            }
            if let Some(verifier) = pkce_verifier {
                form.append_pair("code_verifier", verifier);
            }
            form.finish().into_bytes()
        };
        let headers = reqwest::header::HeaderMap::from_iter([
            (
                reqwest::header::CONTENT_TYPE,
                reqwest::header::HeaderValue::from_static("application/x-www-form-urlencoded"),
            ),
            (
                reqwest::header::ACCEPT,
                reqwest::header::HeaderValue::from_static("application/json"),
            ),
        ]);
        state
            .execute_admin_provider_oauth_http_request(
                "provider-oauth:exchange-code",
                reqwest::Method::POST,
                &token_url,
                &headers,
                Some("application/x-www-form-urlencoded"),
                None,
                Some(form_body),
                proxy_node_id,
            )
            .await
    }
    .map_err(|_| {
        build_internal_control_error_response(http::StatusCode::BAD_REQUEST, "token exchange 失败")
    })?;

    if !response.status.is_success() {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "token exchange 失败",
        ));
    }

    let payload = response.json_body.ok_or_else(|| {
        build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "token exchange 返回缺少 access_token",
        )
    })?;
    if json_non_empty_string(payload.get("access_token")).is_none() {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "token exchange 返回缺少 access_token",
        ));
    }
    Ok(payload)
}

pub(crate) async fn exchange_admin_provider_oauth_refresh_token(
    state: &AdminAppState<'_>,
    template: AdminProviderOAuthTemplate,
    refresh_token: &str,
    proxy_node_id: Option<&str>,
) -> Result<serde_json::Value, Response<Body>> {
    let token_url = state.provider_oauth_token_url(template.provider_type, template.token_url);
    let scope = template.scopes.join(" ");
    let response = if template.provider_type == "claude_code" {
        let mut body = serde_json::Map::from_iter([
            (
                "grant_type".to_string(),
                serde_json::Value::String("refresh_token".to_string()),
            ),
            (
                "client_id".to_string(),
                serde_json::Value::String(template.client_id.to_string()),
            ),
            (
                "refresh_token".to_string(),
                serde_json::Value::String(refresh_token.to_string()),
            ),
        ]);
        if !scope.trim().is_empty() {
            body.insert("scope".to_string(), serde_json::Value::String(scope));
        }
        let headers = reqwest::header::HeaderMap::from_iter([
            (
                reqwest::header::CONTENT_TYPE,
                reqwest::header::HeaderValue::from_static("application/json"),
            ),
            (
                reqwest::header::ACCEPT,
                reqwest::header::HeaderValue::from_static("application/json"),
            ),
        ]);
        state
            .execute_admin_provider_oauth_http_request(
                "provider-oauth:refresh-token",
                reqwest::Method::POST,
                &token_url,
                &headers,
                Some("application/json"),
                Some(serde_json::Value::Object(body)),
                None,
                proxy_node_id,
            )
            .await
    } else {
        let form_body = {
            let mut form = form_urlencoded::Serializer::new(String::new());
            form.append_pair("grant_type", "refresh_token");
            form.append_pair("client_id", template.client_id);
            form.append_pair("refresh_token", refresh_token);
            if !scope.trim().is_empty() {
                form.append_pair("scope", &scope);
            }
            if !template.client_secret.trim().is_empty() {
                form.append_pair("client_secret", template.client_secret);
            }
            form.finish().into_bytes()
        };
        let headers = reqwest::header::HeaderMap::from_iter([
            (
                reqwest::header::CONTENT_TYPE,
                reqwest::header::HeaderValue::from_static("application/x-www-form-urlencoded"),
            ),
            (
                reqwest::header::ACCEPT,
                reqwest::header::HeaderValue::from_static("application/json"),
            ),
        ]);
        state
            .execute_admin_provider_oauth_http_request(
                "provider-oauth:refresh-token",
                reqwest::Method::POST,
                &token_url,
                &headers,
                Some("application/x-www-form-urlencoded"),
                None,
                Some(form_body),
                proxy_node_id,
            )
            .await
    }
    .map_err(|_| {
        build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Refresh Token 验证失败: token exchange 失败",
        )
    })?;

    let status = response.status;
    let body = response.body_text;
    if !status.is_success() {
        let reason =
            normalize_provider_oauth_refresh_error_message(Some(status.as_u16()), Some(&body));
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            format!("Refresh Token 验证失败: {reason}"),
        ));
    }

    let payload = response
        .json_body
        .or_else(|| serde_json::from_str::<serde_json::Value>(&body).ok())
        .ok_or_else(|| {
            build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "token refresh 返回缺少 access_token",
            )
        })?;
    if json_non_empty_string(payload.get("access_token")).is_none() {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "token refresh 返回缺少 access_token",
        ));
    }
    Ok(payload)
}
