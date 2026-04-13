use super::parse::{AdminProviderOAuthBatchImportEntry, AdminProviderOAuthBatchImportOutcome};
use crate::handlers::admin::provider::oauth::duplicates::find_duplicate_provider_oauth_key;
use crate::handlers::admin::provider::oauth::provisioning::{
    create_provider_oauth_catalog_key, provider_oauth_active_api_formats,
    provider_oauth_key_proxy_value, update_existing_provider_oauth_catalog_key,
};
use crate::handlers::admin::provider::oauth::runtime::refresh_provider_oauth_account_state_after_update;
use crate::handlers::admin::provider::oauth::state::decode_jwt_claims;
use crate::handlers::admin::provider::shared::support::ADMIN_PROVIDER_OAUTH_DATA_UNAVAILABLE_DETAIL;
use crate::handlers::admin::request::{AdminAppState, AdminKiroAuthConfig};
use crate::provider_transport::kiro::generate_machine_id;
use crate::GatewayError;
use aether_admin::provider::oauth::{
    build_kiro_batch_import_key_name, coerce_admin_provider_oauth_import_str,
    parse_admin_provider_oauth_kiro_batch_import_entries,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

const KIRO_IDC_AMZ_USER_AGENT: &str =
    "aws-sdk-js/3.738.0 ua/2.1 os/other lang/js md/browser#unknown_unknown api/sso-oidc#3.738.0 m/E KiroIDE";

fn admin_provider_oauth_kiro_refresh_base_url_override(
    state: &AdminAppState<'_>,
    override_key: &str,
) -> Option<String> {
    let override_url = state.provider_oauth_token_url(override_key, "");
    let normalized = override_url.trim();
    (!normalized.is_empty()).then(|| normalized.to_string())
}

fn admin_provider_oauth_kiro_build_refresh_url(
    auth_config: &AdminKiroAuthConfig,
    override_base_url: Option<&str>,
    path: &str,
    default_host: impl FnOnce(&str) -> String,
) -> String {
    if let Some(base_url) = override_base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return format!("{}/{}", base_url.trim_end_matches('/'), path);
    }
    let region = auth_config.effective_auth_region();
    default_host(region)
}

fn admin_provider_oauth_kiro_effective_host(url: &str, fallback_host: String) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|value| value.host_str().map(ToOwned::to_owned))
        .unwrap_or(fallback_host)
}

fn admin_provider_oauth_kiro_ide_tag(kiro_version: &str, machine_id: &str) -> String {
    if machine_id.trim().is_empty() {
        format!("KiroIDE-{kiro_version}")
    } else {
        format!("KiroIDE-{kiro_version}-{machine_id}")
    }
}

fn admin_provider_oauth_kiro_refresh_expires_at(payload: &Value) -> u64 {
    let expires_in = payload
        .get("expiresIn")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str()?.parse::<u64>().ok())
        })
        .unwrap_or(3600);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
        .unwrap_or_default()
        .saturating_add(expires_in)
}

fn admin_provider_oauth_kiro_refresh_response_json(
    body_text: &str,
    json_body: Option<Value>,
) -> Result<Value, String> {
    json_body
        .or_else(|| serde_json::from_str::<Value>(body_text).ok())
        .ok_or_else(|| "refresh 接口返回了非 JSON 响应".to_string())
}

fn admin_provider_oauth_kiro_refresh_error_detail(
    status: http::StatusCode,
    body_text: &str,
) -> String {
    let detail = body_text.trim();
    if detail.is_empty() {
        format!("HTTP {}", status.as_u16())
    } else {
        detail.to_string()
    }
}

async fn refresh_admin_provider_oauth_kiro_auth_config(
    state: &AdminAppState<'_>,
    auth_config: &AdminKiroAuthConfig,
    proxy_node_id: Option<&str>,
    social_refresh_base_url: Option<&str>,
    idc_refresh_base_url: Option<&str>,
) -> Result<AdminKiroAuthConfig, String> {
    if auth_config.is_idc_auth() {
        let fallback_host = format!("oidc.{}.amazonaws.com", auth_config.effective_auth_region());
        let url = admin_provider_oauth_kiro_build_refresh_url(
            auth_config,
            idc_refresh_base_url,
            "token",
            |region| format!("https://oidc.{region}.amazonaws.com/token"),
        );
        let host = admin_provider_oauth_kiro_effective_host(&url, fallback_host);
        let headers = reqwest::header::HeaderMap::from_iter([
            (
                reqwest::header::CONTENT_TYPE,
                reqwest::header::HeaderValue::from_static("application/json"),
            ),
            (
                reqwest::header::HOST,
                reqwest::header::HeaderValue::from_str(&host)
                    .map_err(|_| "IDC host 无效".to_string())?,
            ),
            (
                reqwest::header::HeaderName::from_static("x-amz-user-agent"),
                reqwest::header::HeaderValue::from_static(KIRO_IDC_AMZ_USER_AGENT),
            ),
            (
                reqwest::header::USER_AGENT,
                reqwest::header::HeaderValue::from_static("node"),
            ),
            (
                reqwest::header::ACCEPT,
                reqwest::header::HeaderValue::from_static("*/*"),
            ),
        ]);
        let response = state
            .execute_admin_provider_oauth_http_request(
                "kiro_batch_refresh:idc",
                reqwest::Method::POST,
                &url,
                &headers,
                Some("application/json"),
                Some(json!({
                    "clientId": auth_config
                        .client_id
                        .as_deref()
                        .map(str::trim)
                        .unwrap_or_default(),
                    "clientSecret": auth_config
                        .client_secret
                        .as_deref()
                        .map(str::trim)
                        .unwrap_or_default(),
                    "refreshToken": auth_config
                        .refresh_token
                        .as_deref()
                        .map(str::trim)
                        .unwrap_or_default(),
                    "grantType": "refresh_token",
                })),
                None,
                proxy_node_id,
            )
            .await
            .map_err(|err| format!("IDC refresh 请求失败: {err}"))?;
        if !response.status.is_success() {
            return Err(format!(
                "IDC refresh 失败: {}",
                admin_provider_oauth_kiro_refresh_error_detail(
                    response.status,
                    &response.body_text
                )
            ));
        }
        let payload = admin_provider_oauth_kiro_refresh_response_json(
            &response.body_text,
            response.json_body,
        )?;
        let access_token = payload
            .get("accessToken")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "IDC refresh 返回了空 accessToken".to_string())?;

        let mut refreshed = auth_config.clone();
        refreshed.access_token = Some(access_token.to_string());
        refreshed.expires_at = Some(admin_provider_oauth_kiro_refresh_expires_at(&payload));
        if refreshed
            .machine_id
            .as_deref()
            .map(str::trim)
            .is_none_or(|value| value.is_empty())
        {
            refreshed.machine_id = generate_machine_id(auth_config, None);
        }
        if let Some(refresh_token) = payload
            .get("refreshToken")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            refreshed.refresh_token = Some(refresh_token.to_string());
        }
        return Ok(refreshed);
    }

    let machine_id = generate_machine_id(auth_config, None)
        .ok_or_else(|| "缺少 machine_id 种子，无法刷新 social token".to_string())?;
    let fallback_host = format!(
        "prod.{}.auth.desktop.kiro.dev",
        auth_config.effective_auth_region()
    );
    let url = admin_provider_oauth_kiro_build_refresh_url(
        auth_config,
        social_refresh_base_url,
        "refreshToken",
        |region| format!("https://prod.{region}.auth.desktop.kiro.dev/refreshToken"),
    );
    let host = admin_provider_oauth_kiro_effective_host(&url, fallback_host);
    let user_agent =
        admin_provider_oauth_kiro_ide_tag(auth_config.effective_kiro_version(), &machine_id);
    let headers = reqwest::header::HeaderMap::from_iter([
        (
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_str(&user_agent)
                .map_err(|_| "Kiro User-Agent 无效".to_string())?,
        ),
        (
            reqwest::header::HOST,
            reqwest::header::HeaderValue::from_str(&host)
                .map_err(|_| "Kiro host 无效".to_string())?,
        ),
        (
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/json, text/plain, */*"),
        ),
        (
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        ),
        (
            reqwest::header::CONNECTION,
            reqwest::header::HeaderValue::from_static("close"),
        ),
        (
            reqwest::header::ACCEPT_ENCODING,
            reqwest::header::HeaderValue::from_static("gzip, compress, deflate, br"),
        ),
    ]);
    let response = state
        .execute_admin_provider_oauth_http_request(
            "kiro_batch_refresh:social",
            reqwest::Method::POST,
            &url,
            &headers,
            Some("application/json"),
            Some(json!({
                "refreshToken": auth_config
                    .refresh_token
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default(),
            })),
            None,
            proxy_node_id,
        )
        .await
        .map_err(|err| format!("social refresh 请求失败: {err}"))?;
    if !response.status.is_success() {
        return Err(format!(
            "social refresh 失败: {}",
            admin_provider_oauth_kiro_refresh_error_detail(response.status, &response.body_text)
        ));
    }
    let payload =
        admin_provider_oauth_kiro_refresh_response_json(&response.body_text, response.json_body)?;
    let access_token = payload
        .get("accessToken")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "social refresh 返回了空 accessToken".to_string())?;

    let mut refreshed = auth_config.clone();
    refreshed.access_token = Some(access_token.to_string());
    refreshed.expires_at = Some(admin_provider_oauth_kiro_refresh_expires_at(&payload));
    if refreshed
        .machine_id
        .as_deref()
        .map(str::trim)
        .is_none_or(|value| value.is_empty())
    {
        refreshed.machine_id = Some(machine_id);
    }
    if let Some(refresh_token) = payload
        .get("refreshToken")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        refreshed.refresh_token = Some(refresh_token.to_string());
    }
    if let Some(profile_arn) = payload
        .get("profileArn")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        refreshed.profile_arn = Some(profile_arn.to_string());
    }
    Ok(refreshed)
}

pub(super) async fn execute_admin_provider_oauth_kiro_batch_import(
    state: &AdminAppState<'_>,
    provider_id: &str,
    raw_credentials: &str,
    proxy_node_id: Option<&str>,
) -> Result<AdminProviderOAuthBatchImportOutcome, GatewayError> {
    let entries = parse_admin_provider_oauth_kiro_batch_import_entries(raw_credentials);
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(&[provider_id.to_string()])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(AdminProviderOAuthBatchImportOutcome {
            total: entries.len(),
            success: 0,
            failed: entries.len(),
            results: entries
                .iter()
                .enumerate()
                .map(|(index, _)| {
                    json!({
                        "index": index,
                        "status": "error",
                        "error": "Provider 不存在",
                        "replaced": false,
                    })
                })
                .collect(),
        });
    };

    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(&[provider_id.to_string()])
        .await?;
    let key_proxy = provider_oauth_key_proxy_value(proxy_node_id);
    let social_refresh_base_url =
        admin_provider_oauth_kiro_refresh_base_url_override(state, "kiro_social_refresh");
    let idc_refresh_base_url =
        admin_provider_oauth_kiro_refresh_base_url_override(state, "kiro_idc_refresh");
    let mut results = Vec::with_capacity(entries.len());
    let mut success = 0usize;
    let mut failed = 0usize;

    for (index, entry) in entries.iter().enumerate() {
        let Some(mut refreshed_auth_config) = AdminKiroAuthConfig::from_json_value(entry) else {
            failed += 1;
            results.push(json!({
                "index": index,
                "status": "error",
                "error": "未找到有效的凭据数据",
                "replaced": false,
            }));
            continue;
        };

        let has_refresh_token = refreshed_auth_config
            .refresh_token
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());
        if !has_refresh_token {
            failed += 1;
            results.push(json!({
                "index": index,
                "status": "error",
                "error": "缺少可用的 Kiro refresh 凭据",
                "replaced": false,
            }));
            continue;
        }

        refreshed_auth_config = match refresh_admin_provider_oauth_kiro_auth_config(
            state,
            &refreshed_auth_config,
            proxy_node_id,
            social_refresh_base_url.as_deref(),
            idc_refresh_base_url.as_deref(),
        )
        .await
        {
            Ok(config) => config,
            Err(err) => {
                failed += 1;
                results.push(json!({
                    "index": index,
                    "status": "error",
                    "error": format!("Token 验证失败: {err}"),
                    "replaced": false,
                }));
                continue;
            }
        };

        if refreshed_auth_config.auth_method.is_none() {
            refreshed_auth_config.auth_method = Some(if refreshed_auth_config.is_idc_auth() {
                "idc".to_string()
            } else {
                "social".to_string()
            });
        }

        let mut auth_config = refreshed_auth_config
            .to_json_value()
            .as_object()
            .cloned()
            .unwrap_or_default();
        auth_config.insert("provider_type".to_string(), json!("kiro"));
        let email = decode_jwt_claims(
            refreshed_auth_config
                .access_token
                .as_deref()
                .unwrap_or_default(),
        )
        .and_then(|claims: Map<String, Value>| claims.get("email").cloned())
        .and_then(|value: Value| value.as_str().map(ToOwned::to_owned))
        .or_else(|| coerce_admin_provider_oauth_import_str(entry.get("email")));
        if let Some(email) = email.as_ref() {
            auth_config.insert("email".to_string(), json!(email));
        }

        let duplicate =
            match find_duplicate_provider_oauth_key(state, provider_id, &auth_config, None).await {
                Ok(value) => value,
                Err(detail) => {
                    failed += 1;
                    results.push(json!({
                        "index": index,
                        "status": "error",
                        "error": detail,
                        "replaced": false,
                    }));
                    continue;
                }
            };

        let access_token = refreshed_auth_config
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let Some(access_token) = access_token else {
            failed += 1;
            results.push(json!({
                "index": index,
                "status": "error",
                "error": "Token 验证失败: accessToken 为空",
                "replaced": false,
            }));
            continue;
        };

        let replaced = duplicate.is_some();
        let (persisted_key, key_name) = if let Some(existing_key) = duplicate {
            match update_existing_provider_oauth_catalog_key(
                state,
                &existing_key,
                &access_token,
                &auth_config,
                key_proxy.clone(),
                refreshed_auth_config.expires_at,
            )
            .await?
            {
                Some(key) => (key, existing_key.name.clone()),
                None => {
                    failed += 1;
                    results.push(json!({
                        "index": index,
                        "status": "error",
                        "error": "provider oauth write unavailable",
                        "replaced": true,
                    }));
                    continue;
                }
            }
        } else {
            let key_name = build_kiro_batch_import_key_name(
                auth_config.get("email").and_then(serde_json::Value::as_str),
                auth_config
                    .get("auth_method")
                    .and_then(serde_json::Value::as_str),
                auth_config
                    .get("refresh_token")
                    .and_then(serde_json::Value::as_str),
            );
            match create_provider_oauth_catalog_key(
                state,
                provider_id,
                &key_name,
                &access_token,
                &auth_config,
                &provider_oauth_active_api_formats(&endpoints),
                key_proxy.clone(),
                refreshed_auth_config.expires_at,
            )
            .await?
            {
                Some(key) => (key, key_name),
                None => {
                    failed += 1;
                    results.push(json!({
                        "index": index,
                        "status": "error",
                        "error": "provider oauth write unavailable",
                        "replaced": false,
                    }));
                    continue;
                }
            }
        };

        let auth_method = auth_config
            .get("auth_method")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let _ =
            refresh_provider_oauth_account_state_after_update(state, &provider, &persisted_key.id)
                .await;

        success += 1;
        results.push(json!({
            "index": index,
            "status": "success",
            "key_id": persisted_key.id,
            "key_name": key_name,
            "auth_method": auth_method,
            "error": serde_json::Value::Null,
            "replaced": replaced,
        }));
    }

    Ok(AdminProviderOAuthBatchImportOutcome {
        total: entries.len(),
        success,
        failed,
        results,
    })
}
