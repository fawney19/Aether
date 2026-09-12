use super::oauth_config::{
    admin_oauth_builtin_allowed_domains, admin_oauth_custom_allowed_domains,
    admin_oauth_is_supported_provider, admin_oauth_provider_type_from_path,
    admin_oauth_test_provider_type_from_path, build_admin_oauth_provider_payload,
    build_admin_oauth_supported_types_payload, build_admin_oauth_upsert_record,
    validate_admin_oauth_url_override, AdminOAuthProviderUpsertRequest,
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
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

const ADMIN_OAUTH_TEST_TIMEOUT_SECS: u64 = 10;
const ADMIN_OAUTH_TEST_MAX_REDIRECTS: usize = 3;
const LINUXDO_AUTHORIZATION_URL: &str = "https://connect.linux.do/oauth2/authorize";
const LINUXDO_TOKEN_URL: &str = "https://connect.linux.do/oauth2/token";

fn admin_oauth_payload_string<'a>(payload: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    payload
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn admin_oauth_secret_status(has_secret: bool) -> &'static str {
    if has_secret {
        "configured"
    } else {
        "not_provided"
    }
}

async fn admin_oauth_endpoint_reachable(
    url: &str,
    allowed_domains: &[&str],
    allow_benchmarking_ip: bool,
) -> bool {
    let Ok(mut current) = reqwest::Url::parse(url) else {
        return false;
    };
    for redirects in 0..=ADMIN_OAUTH_TEST_MAX_REDIRECTS {
        if validate_admin_oauth_url_override(current.as_str(), allowed_domains).is_err() {
            return false;
        }
        let Ok((host, addrs)) =
            resolve_public_admin_oauth_endpoint_with_policy(&current, allow_benchmarking_ip).await
        else {
            return false;
        };
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(ADMIN_OAUTH_TEST_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy();
        if host.parse::<IpAddr>().is_err() {
            builder = builder.resolve_to_addrs(host.as_str(), &addrs);
        }
        let Ok(client) = builder.build() else {
            return false;
        };
        let Ok(response) = client
            .get(current.clone())
            .header(reqwest::header::ACCEPT, "*/*")
            .header(
                reqwest::header::USER_AGENT,
                "Aether OAuth configuration tester",
            )
            .send()
            .await
        else {
            return false;
        };
        if !response.status().is_redirection() {
            return response.status().as_u16() < 500;
        }
        if redirects == ADMIN_OAUTH_TEST_MAX_REDIRECTS {
            return false;
        }
        let Some(location) = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
        else {
            return false;
        };
        let Ok(next) = current.join(location) else {
            return false;
        };
        current = next;
    }
    false
}

async fn resolve_public_admin_oauth_endpoint(
    url: &reqwest::Url,
) -> Result<(String, Vec<SocketAddr>), ()> {
    resolve_public_admin_oauth_endpoint_with_policy(url, false).await
}

async fn resolve_public_admin_oauth_endpoint_with_policy(
    url: &reqwest::Url,
    allow_benchmarking_ip: bool,
) -> Result<(String, Vec<SocketAddr>), ()> {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
    {
        return Err(());
    }
    let host = url.host_str().ok_or(())?;
    let port = url.port_or_known_default().ok_or(())?;
    let addrs = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        aether_http::lookup_host_with_limits(host, port, aether_http::DEFAULT_DNS_LOOKUP_TIMEOUT)
            .await
            .map_err(|_| ())?
    };
    if validate_public_admin_oauth_resolved_addrs(url, &addrs, allow_benchmarking_ip).is_err() {
        return Err(());
    }
    Ok((host.to_string(), addrs))
}

fn validate_public_admin_oauth_resolved_addrs(
    url: &reqwest::Url,
    addrs: &[SocketAddr],
    allow_benchmarking_ip: bool,
) -> Result<(), ()> {
    if addrs.is_empty()
        || addrs.iter().any(|addr| {
            aether_http::is_private_or_reserved_ip(addr.ip())
                && !(allow_benchmarking_ip
                    && is_fixed_linuxdo_oauth_origin(url)
                    && aether_http::is_ipv4_benchmarking_fake_ip(addr.ip()))
        })
    {
        return Err(());
    }
    Ok(())
}

fn is_fixed_linuxdo_oauth_origin(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|host| {
            host.trim_end_matches('.')
                .eq_ignore_ascii_case("connect.linux.do")
        })
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn admin_oauth_test_allowed_domains(
    provider_type: &str,
    payload: &serde_json::Value,
    persisted_config: Option<&aether_data::repository::oauth_providers::StoredOAuthProviderConfig>,
) -> Vec<String> {
    if let Some(domains) = admin_oauth_builtin_allowed_domains(provider_type) {
        return domains.iter().map(|domain| (*domain).to_string()).collect();
    }
    let payload_extra = payload.get("extra_config");
    let domains = admin_oauth_custom_allowed_domains(payload_extra);
    if domains.is_empty() {
        admin_oauth_custom_allowed_domains(
            persisted_config.and_then(|provider| provider.extra_config.as_ref()),
        )
    } else {
        domains
    }
}

fn management_token_may_configure_frontend_callback(
    request_context: &AdminRequestContext<'_>,
    existing: Option<&aether_data::repository::oauth_providers::StoredOAuthProviderConfig>,
    requested_callback: &str,
) -> bool {
    let Some(principal) = request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
    else {
        return false;
    };
    if principal.management_token_id.is_none() {
        return true;
    }
    let callback_changed =
        existing.is_none_or(|provider| provider.frontend_callback_url != requested_callback.trim());
    if !callback_changed {
        return true;
    }

    // A missing permission list is the legacy full-access token representation.
    principal
        .management_token_permissions
        .as_ref()
        .is_none_or(|permissions| {
            permissions
                .iter()
                .any(|permission| permission == "admin:oauth:admin")
        })
}

fn oauth_frontend_callback_permission_denied_response(
    request_context: &AdminRequestContext<'_>,
) -> Response<Body> {
    let actor_id = request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .and_then(|principal| principal.management_token_id.as_deref())
        .unwrap_or("unknown");
    attach_admin_audit_response(
        (
            http::StatusCode::FORBIDDEN,
            Json(json!({
                "detail": "management token permission denied",
                "required_permission": "admin:oauth:admin",
                "route_family": "oauth_manage",
                "route_kind": "upsert_provider",
                "request_path": request_context.path(),
            })),
        )
            .into_response(),
        "admin_oauth_frontend_callback_permission_denied",
        "permission_denied",
        "oauth_frontend_callback",
        actor_id,
    )
}

async fn build_admin_oauth_test_payload(
    state: &AdminAppState<'_>,
    provider_type: &str,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, GatewayError> {
    if !admin_oauth_is_supported_provider(provider_type) {
        return Ok(json!({
            "authorization_url_reachable": false,
            "token_url_reachable": false,
            "secret_status": "unknown",
            "details": "provider 未安装/不可用",
        }));
    }

    let provided_secret = admin_oauth_payload_string(payload, "client_secret");
    let persisted_config = state.get_oauth_provider_config(provider_type).await?;
    let persisted_secret_configured = persisted_config
        .as_ref()
        .and_then(|provider| provider.client_secret_encrypted.as_ref())
        .is_some();
    let has_secret = provided_secret.is_some() || persisted_secret_configured;

    let builtin_defaults = provider_type
        .eq_ignore_ascii_case("linuxdo")
        .then_some((LINUXDO_AUTHORIZATION_URL, LINUXDO_TOKEN_URL));
    let authorization_url = admin_oauth_payload_string(payload, "authorization_url_override")
        .map(ToOwned::to_owned)
        .or_else(|| {
            persisted_config
                .as_ref()
                .and_then(|provider| provider.authorization_url_override.clone())
        })
        .or_else(|| builtin_defaults.map(|defaults| defaults.0.to_string()));
    let token_url = admin_oauth_payload_string(payload, "token_url_override")
        .map(ToOwned::to_owned)
        .or_else(|| {
            persisted_config
                .as_ref()
                .and_then(|provider| provider.token_url_override.clone())
        })
        .or_else(|| builtin_defaults.map(|defaults| defaults.1.to_string()));
    let Some(authorization_url) = authorization_url else {
        return Ok(json!({
            "authorization_url_reachable": false,
            "token_url_reachable": false,
            "secret_status": admin_oauth_secret_status(has_secret),
            "details": "Authorization URL 未配置",
        }));
    };
    let Some(token_url) = token_url else {
        return Ok(json!({
            "authorization_url_reachable": false,
            "token_url_reachable": false,
            "secret_status": admin_oauth_secret_status(has_secret),
            "details": "Token URL 未配置",
        }));
    };

    let allowed_domains =
        admin_oauth_test_allowed_domains(provider_type, payload, persisted_config.as_ref());
    let allowed_domain_refs = allowed_domains
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    if allowed_domain_refs.is_empty()
        || validate_admin_oauth_url_override(&authorization_url, &allowed_domain_refs).is_err()
        || validate_admin_oauth_url_override(&token_url, &allowed_domain_refs).is_err()
    {
        return Ok(json!({
            "authorization_url_reachable": false,
            "token_url_reachable": false,
            "secret_status": admin_oauth_secret_status(has_secret),
            "details": "OAuth 端点必须使用 https 且位于 provider 域名白名单中",
        }));
    }

    let allow_benchmarking_ip = provider_type.eq_ignore_ascii_case("linuxdo");
    let (authorization_url_reachable, token_url_reachable) = tokio::join!(
        admin_oauth_endpoint_reachable(
            &authorization_url,
            &allowed_domain_refs,
            allow_benchmarking_ip,
        ),
        admin_oauth_endpoint_reachable(&token_url, &allowed_domain_refs, allow_benchmarking_ip),
    );

    let details = if authorization_url_reachable && token_url_reachable {
        "OAuth 端点可达；client_secret 仅在授权回调兑换 code 时校验"
    } else {
        "OAuth 端点不可达或返回不可用状态；请检查端点 URL 和网络配置"
    };

    Ok(json!({
        "authorization_url_reachable": authorization_url_reachable,
        "token_url_reachable": token_url_reachable,
        "secret_status": admin_oauth_secret_status(has_secret),
        "details": details,
    }))
}

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
        if !management_token_may_configure_frontend_callback(
            request_context,
            existing.as_ref(),
            &payload.frontend_callback_url,
        ) {
            return Ok(Some(oauth_frontend_callback_permission_denied_response(
                request_context,
            )));
        }
        let force_disable = payload.force;
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
        if let Some(existing) = existing.as_ref() {
            if existing
                .client_secret_encrypted
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                && matches!(
                    record.client_secret_encrypted,
                    aether_data::repository::oauth_providers::EncryptedSecretUpdate::Preserve
                )
            {
                match crate::handlers::shared::identity_oauth_provider_secret_binding_matches(
                    existing, &record,
                ) {
                    Ok(true) => {}
                    Ok(false) => {
                        return Ok(Some(build_proxy_error_response(
                            http::StatusCode::BAD_REQUEST,
                            "invalid_request",
                            "修改 OAuth Provider 的 Client ID、端点或 redirect_uri 时必须重新提供 client_secret",
                            None,
                        )));
                    }
                    Err(_) => {
                        return Ok(Some(build_proxy_error_response(
                            http::StatusCode::BAD_REQUEST,
                            "invalid_request",
                            "OAuth Provider 密钥绑定校验失败，请重新提供 client_secret",
                            None,
                        )));
                    }
                }
            }
        }
        let Some(outcome) = state
            .upsert_oauth_provider_config_with_force_disable(&record, force_disable)
            .await?
        else {
            return Ok(None);
        };
        let provider = match outcome {
            aether_data::repository::oauth_providers::UpsertOAuthProviderConfigOutcome::Upserted(
                provider,
            ) => provider,
            aether_data::repository::oauth_providers::UpsertOAuthProviderConfigOutcome::DisableRequiresConfirmation {
                affected_count,
            } => {
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
        let Some(_existing) = state.get_oauth_provider_config(&provider_type).await? else {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::BAD_REQUEST,
                "invalid_request",
                "Provider 配置不存在",
                None,
            )));
        };
        let _mutation_guard = crate::oauth::lock_identity_oauth_mutation().await;
        if state.has_oauth_links_for_provider(&provider_type).await? {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::CONFLICT,
                "provider_has_bindings",
                "Provider 仍有用户绑定，必须先解除全部绑定",
                None,
            )));
        }
        let deleted = state
            .delete_oauth_provider_config_if_unlinked(&provider_type)
            .await?;
        if !deleted {
            if state.has_oauth_links_for_provider(&provider_type).await? {
                return Ok(Some(build_proxy_error_response(
                    http::StatusCode::CONFLICT,
                    "provider_has_bindings",
                    "Provider 仍有用户绑定，必须先解除全部绑定",
                    None,
                )));
            }
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
        let client_id = admin_oauth_payload_string(&payload, "client_id");
        let redirect_uri = admin_oauth_payload_string(&payload, "redirect_uri");
        if client_id.is_none() || redirect_uri.is_none() {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求数据验证失败" })),
                )
                    .into_response(),
            ));
        }
        let test_payload = build_admin_oauth_test_payload(state, &provider_type, &payload).await?;
        return Ok(Some(attach_admin_audit_response(
            Json(test_payload).into_response(),
            "admin_oauth_provider_tested",
            "test_oauth_provider_config",
            "oauth_provider",
            &provider_type,
        )));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::{
        is_fixed_linuxdo_oauth_origin, resolve_public_admin_oauth_endpoint,
        validate_public_admin_oauth_resolved_addrs,
    };
    use std::net::SocketAddr;

    #[tokio::test]
    async fn oauth_test_endpoint_rejects_loopback_https_targets_before_connecting() {
        let url = reqwest::Url::parse("https://127.0.0.1/oauth/token").expect("URL");

        assert!(resolve_public_admin_oauth_endpoint(&url).await.is_err());
    }

    #[test]
    fn linuxdo_builtin_origin_allows_only_benchmarking_addresses() {
        let fixed = reqwest::Url::parse("https://connect.linux.do/oauth2/token")
            .expect("LinuxDo URL should parse");
        let fake = SocketAddr::from(([198, 18, 75, 234], 443));
        assert!(is_fixed_linuxdo_oauth_origin(&fixed));
        assert!(validate_public_admin_oauth_resolved_addrs(&fixed, &[fake], true).is_ok());
        assert!(validate_public_admin_oauth_resolved_addrs(&fixed, &[fake], false).is_err());
        assert!(validate_public_admin_oauth_resolved_addrs(
            &fixed,
            &[fake, SocketAddr::from(([127, 0, 0, 1], 443))],
            true,
        )
        .is_err());
    }

    #[test]
    fn custom_or_non_default_oauth_origins_reject_benchmarking_addresses() {
        let fake = SocketAddr::from(([198, 18, 75, 234], 443));
        for raw_url in [
            "https://oauth.example.test/token",
            "https://connect.linux.do:8443/oauth2/token",
            "https://connect.linuxdo.org/oauth2/token",
            "https://connect.linux.do.evil.test/oauth2/token",
            "https://connect.linux.do/oauth2/token?tenant=unexpected",
        ] {
            let url = reqwest::Url::parse(raw_url).expect("test URL should parse");
            assert!(
                !is_fixed_linuxdo_oauth_origin(&url),
                "must not trust {raw_url}"
            );
            assert!(validate_public_admin_oauth_resolved_addrs(&url, &[fake], true).is_err());
        }
    }
}
