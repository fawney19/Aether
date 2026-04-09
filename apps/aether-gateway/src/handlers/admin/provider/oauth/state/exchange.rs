use super::super::errors::{
    build_internal_control_error_response, normalize_provider_oauth_refresh_error_message,
};
use super::json_non_empty_string;
use crate::handlers::admin::request::{AdminAppState, AdminProviderOAuthTemplate};
use axum::{body::Body, http, response::Response};

pub(crate) async fn exchange_admin_provider_oauth_code(
    state: &AdminAppState<'_>,
    template: AdminProviderOAuthTemplate,
    code: &str,
    state_nonce: &str,
    pkce_verifier: Option<&str>,
) -> Result<serde_json::Value, Response<Body>> {
    let token_url = state.provider_oauth_token_url(template.provider_type, template.token_url);
    let request = state.http_client().post(token_url);
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
        request
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .json(&serde_json::Value::Object(body))
            .send()
            .await
    } else {
        let mut form = vec![
            ("grant_type", "authorization_code".to_string()),
            ("client_id", template.client_id.to_string()),
            ("redirect_uri", template.redirect_uri.to_string()),
            ("code", code.to_string()),
        ];
        if !template.client_secret.trim().is_empty() {
            form.push(("client_secret", template.client_secret.to_string()));
        }
        if let Some(verifier) = pkce_verifier {
            form.push(("code_verifier", verifier.to_string()));
        }
        request
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .form(&form)
            .send()
            .await
    }
    .map_err(|_| {
        build_internal_control_error_response(http::StatusCode::BAD_REQUEST, "token exchange 失败")
    })?;

    if !response.status().is_success() {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "token exchange 失败",
        ));
    }

    let payload = response.json::<serde_json::Value>().await.map_err(|_| {
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
) -> Result<serde_json::Value, Response<Body>> {
    let token_url = state.provider_oauth_token_url(template.provider_type, template.token_url);
    let request = state.http_client().post(token_url);
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
        request
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .json(&serde_json::Value::Object(body))
            .send()
            .await
    } else {
        let mut form = vec![
            ("grant_type", "refresh_token".to_string()),
            ("client_id", template.client_id.to_string()),
            ("refresh_token", refresh_token.to_string()),
        ];
        if !scope.trim().is_empty() {
            form.push(("scope", scope));
        }
        if !template.client_secret.trim().is_empty() {
            form.push(("client_secret", template.client_secret.to_string()));
        }
        request
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .form(&form)
            .send()
            .await
    }
    .map_err(|_| {
        build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Refresh Token 验证失败: token exchange 失败",
        )
    })?;

    let status = response.status();
    let body = response.text().await.map_err(|_| {
        build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Refresh Token 验证失败: token exchange 失败",
        )
    })?;
    if !status.is_success() {
        let reason =
            normalize_provider_oauth_refresh_error_message(Some(status.as_u16()), Some(&body));
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            format!("Refresh Token 验证失败: {reason}"),
        ));
    }

    let payload = serde_json::from_str::<serde_json::Value>(&body).map_err(|_| {
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
