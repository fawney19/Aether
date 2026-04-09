use super::oauth_config::{
    admin_oauth_provider_type_from_path, admin_oauth_test_provider_type_from_path,
    build_admin_oauth_provider_payload, build_admin_oauth_supported_types_payload,
    build_admin_oauth_upsert_record, AdminOAuthProviderUpsertRequest,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{attach_admin_audit_response, build_proxy_error_response};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(crate) async fn maybe_build_local_admin_oauth_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("oauth_manage") {
        return Ok(None);
    }

    if decision.route_kind.as_deref() == Some("supported_types")
        && request_context.method() == http::Method::GET
        && request_context.path() == "/api/admin/oauth/supported-types"
    {
        return Ok(Some(
            Json(build_admin_oauth_supported_types_payload()).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("list_providers")
        && request_context.method() == http::Method::GET
        && matches!(
            request_context.path(),
            "/api/admin/oauth/providers" | "/api/admin/oauth/providers/"
        )
    {
        let providers = state.list_oauth_provider_configs().await?;
        return Ok(Some(attach_admin_audit_response(
            Json(
                providers
                    .iter()
                    .map(build_admin_oauth_provider_payload)
                    .collect::<Vec<_>>(),
            )
            .into_response(),
            "admin_oauth_provider_configs_viewed",
            "list_oauth_provider_configs",
            "oauth_provider",
            "all",
        )));
    }

    if decision.route_kind.as_deref() == Some("get_provider")
        && request_context.method() == http::Method::GET
    {
        let Some(provider_type) = admin_oauth_provider_type_from_path(request_context.path())
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 配置不存在" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match state.get_oauth_provider_config(&provider_type).await? {
                Some(provider) => attach_admin_audit_response(
                    Json(build_admin_oauth_provider_payload(&provider)).into_response(),
                    "admin_oauth_provider_config_viewed",
                    "view_oauth_provider_config",
                    "oauth_provider",
                    &provider_type,
                ),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 配置不存在" })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("upsert_provider")
        && request_context.method() == http::Method::PUT
    {
        let Some(provider_type) = admin_oauth_provider_type_from_path(request_context.path())
        else {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::BAD_REQUEST,
                "invalid_request",
                "Provider 配置不存在",
                None,
            )));
        };
        let Some(request_body) = request_body else {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::BAD_REQUEST,
                "invalid_request",
                "请求数据验证失败",
                None,
            )));
        };
        let payload = match serde_json::from_slice::<AdminOAuthProviderUpsertRequest>(request_body)
        {
            Ok(payload) => payload,
            Err(_) => {
                return Ok(Some(build_proxy_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "请求数据验证失败",
                    None,
                )));
            }
        };
        let existing = state.get_oauth_provider_config(&provider_type).await?;
        let ldap_exclusive = state.get_ldap_module_config().await?.is_some_and(|config| {
            config.is_enabled
                && config.is_exclusive
                && config
                    .bind_password_encrypted
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|value| !value.is_empty())
        });
        if existing
            .as_ref()
            .is_some_and(|provider| provider.is_enabled && !payload.is_enabled)
        {
            let affected_count = state
                .count_locked_users_if_oauth_provider_disabled(&provider_type, ldap_exclusive)
                .await?;
            if affected_count > 0 && !payload.force {
                return Ok(Some(build_proxy_error_response(
                    http::StatusCode::CONFLICT,
                    "confirmation_required",
                    format!("禁用该 Provider 会导致 {affected_count} 个用户无法登录"),
                    Some(json!({
                        "affected_count": affected_count,
                        "action": "disable_oauth_provider",
                    })),
                )));
            }
        }
        let record = match build_admin_oauth_upsert_record(state, &provider_type, payload) {
            Ok(record) => record,
            Err(message) => {
                return Ok(Some(build_proxy_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "invalid_request",
                    message,
                    None,
                )));
            }
        };
        let Some(provider) = state.upsert_oauth_provider_config(&record).await? else {
            return Ok(None);
        };
        return Ok(Some(
            Json(build_admin_oauth_provider_payload(&provider)).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("delete_provider")
        && request_context.method() == http::Method::DELETE
    {
        let Some(provider_type) = admin_oauth_provider_type_from_path(request_context.path())
        else {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::BAD_REQUEST,
                "invalid_request",
                "Provider 配置不存在",
                None,
            )));
        };
        let Some(existing) = state.get_oauth_provider_config(&provider_type).await? else {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::BAD_REQUEST,
                "invalid_request",
                "Provider 配置不存在",
                None,
            )));
        };
        if existing.is_enabled {
            let ldap_exclusive = state.get_ldap_module_config().await?.is_some_and(|config| {
                config.is_enabled
                    && config.is_exclusive
                    && config
                        .bind_password_encrypted
                        .as_deref()
                        .map(str::trim)
                        .is_some_and(|value| !value.is_empty())
            });
            let affected_count = state
                .count_locked_users_if_oauth_provider_disabled(&provider_type, ldap_exclusive)
                .await?;
            if affected_count > 0 {
                return Ok(Some(build_proxy_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "invalid_request",
                    format!(
                        "删除该 Provider 会导致部分用户无法登录（数量: {affected_count}），已阻止操作"
                    ),
                    None,
                )));
            }
        }
        let deleted = state.delete_oauth_provider_config(&provider_type).await?;
        if !deleted {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::BAD_REQUEST,
                "invalid_request",
                "Provider 配置不存在",
                None,
            )));
        }
        return Ok(Some(Json(json!({ "message": "删除成功" })).into_response()));
    }

    if decision.route_kind.as_deref() == Some("test_provider")
        && request_context.method() == http::Method::POST
    {
        let Some(provider_type) = admin_oauth_test_provider_type_from_path(request_context.path())
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 配置不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求数据验证失败" })),
                )
                    .into_response(),
            ));
        };
        let payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
            Ok(payload) => payload,
            Err(_) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": "请求数据验证失败" })),
                    )
                        .into_response(),
                ));
            }
        };
        let client_id = payload
            .get("client_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let redirect_uri = payload
            .get("redirect_uri")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if client_id.is_none() || redirect_uri.is_none() {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求数据验证失败" })),
                )
                    .into_response(),
            ));
        }
        let provided_secret = payload
            .get("client_secret")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let persisted_secret = state
            .get_oauth_provider_config(&provider_type)
            .await?
            .and_then(|provider| provider.client_secret_encrypted);
        let supported_provider = provider_type.eq_ignore_ascii_case("linuxdo");
        let secret_status = if supported_provider {
            if provided_secret.is_some() || persisted_secret.is_some() {
                "unsupported"
            } else {
                "not_provided"
            }
        } else {
            "unknown"
        };
        let details = if supported_provider {
            "OAuth 配置测试仅支持 Rust execution runtime"
        } else {
            "provider 未安装/不可用"
        };
        return Ok(Some(attach_admin_audit_response(
            Json(json!({
                "authorization_url_reachable": false,
                "token_url_reachable": false,
                "secret_status": secret_status,
                "details": details,
            }))
            .into_response(),
            "admin_oauth_provider_tested",
            "test_oauth_provider_config",
            "oauth_provider",
            &provider_type,
        )));
    }

    Ok(None)
}
