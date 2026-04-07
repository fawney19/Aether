use super::refresh::{
    build_internal_control_error_response, normalize_provider_oauth_refresh_error_message,
};
use crate::handlers::admin::provider::shared::support::ADMIN_PROVIDER_OAUTH_DATA_UNAVAILABLE_DETAIL;
use crate::provider_transport::provider_types::{
    provider_type_admin_oauth_template, provider_type_is_fixed_for_admin_oauth,
    ProviderOAuthTemplate, ADMIN_PROVIDER_OAUTH_TEMPLATE_TYPES,
};
use crate::{AppState, GatewayError};
use aether_data::repository::provider_oauth::{
    build_provider_oauth_batch_task_status_payload, provider_oauth_batch_task_storage_key,
    provider_oauth_device_session_storage_key, provider_oauth_state_storage_key,
    StoredAdminProviderOAuthDeviceSession, StoredAdminProviderOAuthState,
    PROVIDER_OAUTH_BATCH_TASK_TTL_SECS, PROVIDER_OAUTH_STATE_TTL_SECS,
};
use axum::{body::Body, http, response::Response};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use url::{form_urlencoded, Url};
use uuid::Uuid;

pub(crate) fn is_fixed_provider_type_for_provider_oauth(provider_type: &str) -> bool {
    provider_type_is_fixed_for_admin_oauth(provider_type)
}

pub(crate) fn admin_provider_oauth_template(provider_type: &str) -> Option<ProviderOAuthTemplate> {
    provider_type_admin_oauth_template(provider_type)
}

pub(crate) fn build_admin_provider_oauth_supported_types_payload() -> Vec<serde_json::Value> {
    ADMIN_PROVIDER_OAUTH_TEMPLATE_TYPES
        .iter()
        .filter_map(|provider_type| admin_provider_oauth_template(provider_type))
        .map(|template| {
            json!({
                "provider_type": template.provider_type,
                "display_name": template.display_name,
                "scopes": template.scopes,
                "redirect_uri": template.redirect_uri,
                "authorize_url": template.authorize_url,
                "token_url": template.token_url,
                "use_pkce": template.use_pkce,
            })
        })
        .collect()
}

pub(crate) fn build_admin_provider_oauth_backend_unavailable_response() -> Response<Body> {
    build_internal_control_error_response(
        http::StatusCode::SERVICE_UNAVAILABLE,
        ADMIN_PROVIDER_OAUTH_DATA_UNAVAILABLE_DETAIL,
    )
}

const KIRO_DEVICE_DEFAULT_START_URL: &str = "https://view.awsapps.com/start";
const KIRO_DEVICE_DEFAULT_REGION: &str = "us-east-1";
const KIRO_IDC_AMZ_USER_AGENT: &str =
    "aws-sdk-js/3.738.0 ua/2.1 os/other lang/js md/browser#unknown_unknown api/sso-oidc#3.738.0 m/E KiroIDE";

pub(crate) fn default_kiro_device_start_url() -> String {
    KIRO_DEVICE_DEFAULT_START_URL.to_string()
}

pub(crate) fn default_kiro_device_region() -> String {
    KIRO_DEVICE_DEFAULT_REGION.to_string()
}

pub(crate) fn normalize_kiro_device_region(value: Option<&str>) -> Option<String> {
    let value = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(KIRO_DEVICE_DEFAULT_REGION);
    value
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        .then(|| value.to_string())
}

pub(crate) fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub(crate) async fn save_provider_oauth_device_session(
    state: &AppState,
    session_id: &str,
    session: &StoredAdminProviderOAuthDeviceSession,
    ttl_seconds: u64,
) -> Result<(), Response<Body>> {
    let key = provider_oauth_device_session_storage_key(session_id);
    let value = serde_json::to_string(session).map_err(|_| {
        build_internal_control_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "provider oauth redis unavailable",
        )
    })?;
    if let Some(runner) = state.redis_kv_runner() {
        runner
            .setex(&key, &value, Some(ttl_seconds))
            .await
            .map_err(|_| {
                build_internal_control_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "provider oauth redis unavailable",
                )
            })?;
        return Ok(());
    }
    if state.save_provider_oauth_device_session_for_tests(&key, &value) {
        return Ok(());
    }
    Err(build_internal_control_error_response(
        http::StatusCode::SERVICE_UNAVAILABLE,
        "provider oauth redis unavailable",
    ))
}

pub(crate) async fn read_provider_oauth_device_session(
    state: &AppState,
    session_id: &str,
) -> Result<Option<StoredAdminProviderOAuthDeviceSession>, GatewayError> {
    let key = provider_oauth_device_session_storage_key(session_id);
    let raw = if let Some(runner) = state.redis_kv_runner() {
        let mut connection = runner
            .client()
            .get_multiplexed_async_connection()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let namespaced_key = runner.keyspace().key(&key);
        redis::cmd("GET")
            .arg(&namespaced_key)
            .query_async::<Option<String>>(&mut connection)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
    } else {
        state.load_provider_oauth_device_session_for_tests(&key)
    };
    raw.map(|value| {
        serde_json::from_str::<StoredAdminProviderOAuthDeviceSession>(&value)
            .map_err(|err| GatewayError::Internal(err.to_string()))
    })
    .transpose()
}

async fn post_kiro_device_oidc_json(
    state: &AppState,
    endpoint_key: &str,
    default_url: String,
    body: serde_json::Value,
) -> Result<serde_json::Value, Response<Body>> {
    let url = state.provider_oauth_token_url(endpoint_key, &default_url);
    let host = Url::parse(&url)
        .ok()
        .and_then(|value| value.host_str().map(ToOwned::to_owned))
        .unwrap_or_default();
    let response = state
        .client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "*/*")
        .header("User-Agent", "node")
        .header("x-amz-user-agent", KIRO_IDC_AMZ_USER_AGENT)
        .header("Host", host)
        .json(&body)
        .send()
        .await
        .map_err(|_| {
            build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "发起设备授权失败: unknown",
            )
        })?;
    let status = response.status();
    let body_text = response.text().await.map_err(|_| {
        build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "发起设备授权失败: unknown",
        )
    })?;
    serde_json::from_str::<serde_json::Value>(&body_text).or_else(|_| {
        Ok(json!({
            "_error": !status.is_success(),
            "error": body_text.trim(),
        }))
    })
}

pub(crate) async fn register_admin_kiro_device_oidc_client(
    state: &AppState,
    region: &str,
    start_url: &str,
) -> Result<serde_json::Value, Response<Body>> {
    let payload = post_kiro_device_oidc_json(
        state,
        "kiro_device_register",
        format!("https://oidc.{region}.amazonaws.com/client/register"),
        json!({
            "clientName": "Aether Gateway",
            "clientType": "public",
            "scopes": [
                "codewhisperer:completions",
                "codewhisperer:analysis",
                "codewhisperer:conversations",
                "codewhisperer:transformations",
                "codewhisperer:taskassist"
            ],
            "grantTypes": [
                "urn:ietf:params:oauth:grant-type:device_code",
                "refresh_token"
            ],
            "issuerUrl": start_url,
        }),
    )
    .await?;
    if payload
        .get("_error")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        let error_desc = json_non_empty_string(payload.get("error_description"))
            .or_else(|| json_non_empty_string(payload.get("error")))
            .unwrap_or_else(|| "unknown".to_string());
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            format!("注册 OIDC 客户端失败: {error_desc}"),
        ));
    }
    Ok(payload)
}

pub(crate) async fn start_admin_kiro_device_authorization(
    state: &AppState,
    region: &str,
    client_id: &str,
    client_secret: &str,
    start_url: &str,
) -> Result<serde_json::Value, Response<Body>> {
    let payload = post_kiro_device_oidc_json(
        state,
        "kiro_device_authorize",
        format!("https://oidc.{region}.amazonaws.com/device_authorization"),
        json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "startUrl": start_url,
        }),
    )
    .await?;
    if payload
        .get("_error")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        let error_desc = json_non_empty_string(payload.get("error_description"))
            .or_else(|| json_non_empty_string(payload.get("error")))
            .unwrap_or_else(|| "unknown".to_string());
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            format!("发起设备授权失败: {error_desc}"),
        ));
    }
    Ok(payload)
}

pub(crate) async fn poll_admin_kiro_device_token(
    state: &AppState,
    region: &str,
    client_id: &str,
    client_secret: &str,
    device_code: &str,
) -> Result<serde_json::Value, Response<Body>> {
    post_kiro_device_oidc_json(
        state,
        "kiro_device_poll",
        format!("https://oidc.{region}.amazonaws.com/token"),
        json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "grantType": "urn:ietf:params:oauth:grant-type:device_code",
            "deviceCode": device_code,
        }),
    )
    .await
}

pub(crate) fn build_kiro_device_key_name(
    email: Option<&str>,
    refresh_token: Option<&str>,
) -> String {
    if let Some(email) = email.map(str::trim).filter(|value| !value.is_empty()) {
        return format!("{email} (idc)");
    }
    let fallback = refresh_token
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            let digest = Sha256::digest(value.as_bytes());
            digest[..3]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        })
        .unwrap_or_else(|| "unknown".to_string());
    format!("kiro_{fallback} (idc)")
}

pub(crate) fn generate_provider_oauth_nonce() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

pub(crate) fn generate_provider_oauth_pkce_verifier() -> String {
    format!(
        "{}{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

pub(crate) fn provider_oauth_pkce_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

pub(crate) async fn save_provider_oauth_state(
    state: &AppState,
    key_id: &str,
    provider_id: &str,
    provider_type: &str,
    pkce_verifier: Option<&str>,
) -> Result<String, GatewayError> {
    let nonce = generate_provider_oauth_nonce();
    let payload = json!({
        "nonce": nonce,
        "key_id": key_id,
        "provider_id": provider_id,
        "provider_type": provider_type,
        "pkce_verifier": pkce_verifier,
        "created_at": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0),
    });
    let key = provider_oauth_state_storage_key(&nonce);
    let value = payload.to_string();
    if let Some(runner) = state.redis_kv_runner() {
        runner
            .setex(&key, &value, Some(PROVIDER_OAUTH_STATE_TTL_SECS))
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok(nonce);
    }
    if state.save_provider_oauth_state_for_tests(&key, &value) {
        return Ok(nonce);
    }
    Err(GatewayError::Internal(
        "provider oauth redis unavailable".to_string(),
    ))
}

pub(crate) fn parse_provider_oauth_callback_params(callback_url: &str) -> BTreeMap<String, String> {
    let mut merged = BTreeMap::new();
    let Ok(url) = Url::parse(callback_url.trim()) else {
        return merged;
    };
    for (key, value) in url.query_pairs() {
        merged.insert(key.into_owned(), value.into_owned());
    }
    if let Some(fragment) = url.fragment() {
        for (key, value) in form_urlencoded::parse(fragment.as_bytes()) {
            merged
                .entry(key.into_owned())
                .or_insert_with(|| value.into_owned());
        }
    }
    if let Some(code) = merged.get("code").cloned() {
        if let Some((code_part, state_part)) = code.split_once("#state=") {
            merged.insert("code".to_string(), code_part.to_string());
            merged
                .entry("state".to_string())
                .or_insert_with(|| state_part.to_string());
        }
    }
    merged
}

pub(crate) async fn consume_provider_oauth_state(
    state: &AppState,
    nonce: &str,
) -> Result<Option<StoredAdminProviderOAuthState>, GatewayError> {
    let key = provider_oauth_state_storage_key(nonce);
    let raw = if let Some(runner) = state.redis_kv_runner() {
        let mut connection = runner
            .client()
            .get_multiplexed_async_connection()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        let namespaced_key = runner.keyspace().key(&key);
        redis::cmd("GETDEL")
            .arg(&namespaced_key)
            .query_async::<Option<String>>(&mut connection)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
    } else {
        state.take_provider_oauth_state_for_tests(&key)
    };
    raw.map(|value| {
        serde_json::from_str::<StoredAdminProviderOAuthState>(&value)
            .map_err(|err| GatewayError::Internal(err.to_string()))
    })
    .transpose()
}

pub(crate) fn json_non_empty_string(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn json_u64_value(value: Option<&serde_json::Value>) -> Option<u64> {
    match value? {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(value) => value.trim().parse::<u64>().ok(),
        _ => None,
    }
}

pub(crate) fn decode_jwt_claims(token: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()?
        .as_object()
        .cloned()
}

fn merge_missing_auth_config_fields(
    auth_config: &mut serde_json::Map<String, serde_json::Value>,
    source: &serde_json::Map<String, serde_json::Value>,
    fields: &[&str],
) {
    for field in fields {
        if auth_config.contains_key(*field) {
            continue;
        }
        if let Some(value) = source.get(*field).cloned() {
            auth_config.insert((*field).to_string(), value);
        }
    }
}

fn first_json_non_empty_string(
    values: impl IntoIterator<Item = Option<serde_json::Value>>,
) -> Option<String> {
    values.into_iter().find_map(|value| match value {
        Some(serde_json::Value::String(value)) => {
            let normalized = value.trim();
            (!normalized.is_empty()).then(|| normalized.to_string())
        }
        _ => None,
    })
}

fn extract_codex_auth_fields_from_object(
    source: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let auth = source
        .get("https://api.openai.com/auth")
        .and_then(serde_json::Value::as_object);
    let mut result = serde_json::Map::new();

    if let Some(email) = first_json_non_empty_string([
        source.get("email").cloned(),
        auth.and_then(|value| value.get("email")).cloned(),
    ]) {
        result.insert("email".to_string(), json!(email));
    }

    if let Some(account_id) = first_json_non_empty_string([
        auth.and_then(|value| value.get("chatgpt_account_id"))
            .cloned(),
        auth.and_then(|value| value.get("chatgptAccountId"))
            .cloned(),
        auth.and_then(|value| value.get("account_id")).cloned(),
        auth.and_then(|value| value.get("accountId")).cloned(),
        source.get("chatgpt_account_id").cloned(),
        source.get("chatgptAccountId").cloned(),
        source.get("account_id").cloned(),
        source.get("accountId").cloned(),
    ]) {
        result.insert("account_id".to_string(), json!(account_id));
    }

    if let Some(account_user_id) = first_json_non_empty_string([
        auth.and_then(|value| value.get("chatgpt_account_user_id"))
            .cloned(),
        auth.and_then(|value| value.get("chatgptAccountUserId"))
            .cloned(),
        auth.and_then(|value| value.get("account_user_id")).cloned(),
        auth.and_then(|value| value.get("accountUserId")).cloned(),
        source.get("chatgpt_account_user_id").cloned(),
        source.get("chatgptAccountUserId").cloned(),
        source.get("account_user_id").cloned(),
        source.get("accountUserId").cloned(),
    ]) {
        result.insert("account_user_id".to_string(), json!(account_user_id));
    }

    if let Some(plan_type) = first_json_non_empty_string([
        auth.and_then(|value| value.get("chatgpt_plan_type"))
            .cloned(),
        auth.and_then(|value| value.get("chatgptPlanType")).cloned(),
        auth.and_then(|value| value.get("plan_type")).cloned(),
        auth.and_then(|value| value.get("planType")).cloned(),
        source.get("chatgpt_plan_type").cloned(),
        source.get("chatgptPlanType").cloned(),
        source.get("plan_type").cloned(),
        source.get("planType").cloned(),
    ]) {
        result.insert("plan_type".to_string(), json!(plan_type));
    }

    if let Some(user_id) = first_json_non_empty_string([
        auth.and_then(|value| value.get("chatgpt_user_id")).cloned(),
        auth.and_then(|value| value.get("chatgptUserId")).cloned(),
        auth.and_then(|value| value.get("user_id")).cloned(),
        auth.and_then(|value| value.get("userId")).cloned(),
        source.get("chatgpt_user_id").cloned(),
        source.get("chatgptUserId").cloned(),
        source.get("user_id").cloned(),
        source.get("userId").cloned(),
        source.get("sub").cloned(),
    ]) {
        result.insert("user_id".to_string(), json!(user_id));
    }

    if let Some(organizations) = auth
        .and_then(|value| value.get("organizations"))
        .and_then(serde_json::Value::as_array)
        .filter(|value| !value.is_empty())
    {
        result.insert(
            "organizations".to_string(),
            serde_json::Value::Array(organizations.clone()),
        );
    }

    result
}

pub(crate) fn enrich_admin_provider_oauth_auth_config(
    provider_type: &str,
    auth_config: &mut serde_json::Map<String, serde_json::Value>,
    token_payload: &serde_json::Value,
) {
    let Some(token_payload_object) = token_payload.as_object() else {
        return;
    };

    merge_missing_auth_config_fields(
        auth_config,
        token_payload_object,
        &[
            "email",
            "account_id",
            "account_user_id",
            "plan_type",
            "user_id",
            "account_name",
        ],
    );

    if !provider_type.eq_ignore_ascii_case("codex") {
        return;
    }

    let codex_fields = extract_codex_auth_fields_from_object(token_payload_object);
    merge_missing_auth_config_fields(
        auth_config,
        &codex_fields,
        &[
            "email",
            "account_id",
            "account_user_id",
            "plan_type",
            "user_id",
            "organizations",
        ],
    );

    for token_field in ["id_token", "idToken", "access_token", "accessToken"] {
        let Some(token) = json_non_empty_string(token_payload.get(token_field)) else {
            continue;
        };
        let Some(claims) = decode_jwt_claims(&token) else {
            continue;
        };
        merge_missing_auth_config_fields(
            auth_config,
            &claims,
            &[
                "email",
                "account_id",
                "account_user_id",
                "plan_type",
                "user_id",
                "account_name",
            ],
        );
        let codex_claim_fields = extract_codex_auth_fields_from_object(&claims);
        merge_missing_auth_config_fields(
            auth_config,
            &codex_claim_fields,
            &[
                "email",
                "account_id",
                "account_user_id",
                "plan_type",
                "user_id",
                "organizations",
            ],
        );
    }
}

pub(crate) async fn exchange_admin_provider_oauth_code(
    state: &AppState,
    template: ProviderOAuthTemplate,
    code: &str,
    state_nonce: &str,
    pkce_verifier: Option<&str>,
) -> Result<serde_json::Value, Response<Body>> {
    let token_url = state.provider_oauth_token_url(template.provider_type, template.token_url);
    let request = state.client.post(token_url);
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
    state: &AppState,
    template: ProviderOAuthTemplate,
    refresh_token: &str,
) -> Result<serde_json::Value, Response<Body>> {
    let token_url = state.provider_oauth_token_url(template.provider_type, template.token_url);
    let request = state.client.post(token_url);
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

pub(crate) fn build_provider_oauth_start_response(
    template: ProviderOAuthTemplate,
    nonce: &str,
    code_challenge: Option<&str>,
) -> serde_json::Value {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("client_id", template.client_id);
    serializer.append_pair("response_type", "code");
    serializer.append_pair("redirect_uri", template.redirect_uri);
    serializer.append_pair("scope", &template.scopes.join(" "));
    serializer.append_pair("state", nonce);
    if template.provider_type == "codex" {
        serializer.append_pair("prompt", "login");
        serializer.append_pair("id_token_add_organizations", "true");
        serializer.append_pair("codex_cli_simplified_flow", "true");
    }
    if template.use_pkce {
        if let Some(code_challenge) = code_challenge {
            serializer.append_pair("code_challenge", code_challenge);
            serializer.append_pair("code_challenge_method", "S256");
        }
    }

    json!({
        "authorization_url": format!("{}?{}", template.authorize_url, serializer.finish()),
        "redirect_uri": template.redirect_uri,
        "provider_type": template.provider_type,
        "instructions": "1) 打开 authorization_url 完成授权\n2) 授权后会跳转到 redirect_uri（localhost）\n3) 复制浏览器地址栏完整 URL，调用 complete 接口粘贴 callback_url",
    })
}

pub(crate) async fn save_provider_oauth_batch_task_payload(
    state: &AppState,
    task_id: &str,
    task_state: &serde_json::Value,
) -> Result<(), GatewayError> {
    let key = provider_oauth_batch_task_storage_key(task_id);
    let serialized =
        serde_json::to_string(task_state).map_err(|err| GatewayError::Internal(err.to_string()))?;

    if let Some(runner) = state.redis_kv_runner() {
        let Ok(mut connection) = runner.client().get_multiplexed_async_connection().await else {
            return Err(GatewayError::Internal(
                "provider oauth batch task redis unavailable".to_string(),
            ));
        };
        let redis_key = runner.keyspace().key(&key);
        redis::cmd("SET")
            .arg(redis_key)
            .arg(&serialized)
            .arg("EX")
            .arg(PROVIDER_OAUTH_BATCH_TASK_TTL_SECS)
            .query_async::<()>(&mut connection)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok(());
    }

    if state.save_provider_oauth_batch_task_for_tests(&key, &serialized) {
        return Ok(());
    }

    Err(GatewayError::Internal(
        "provider oauth batch task redis unavailable".to_string(),
    ))
}

pub(crate) async fn read_provider_oauth_batch_task_payload(
    state: &AppState,
    provider_id: &str,
    task_id: &str,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let key = provider_oauth_batch_task_storage_key(task_id);
    let raw = if let Some(runner) = state.redis_kv_runner() {
        let Ok(mut connection) = runner.client().get_multiplexed_async_connection().await else {
            return Err(GatewayError::Internal(
                "provider oauth batch task redis unavailable".to_string(),
            ));
        };
        let redis_key = runner.keyspace().key(&key);
        redis::cmd("GET")
            .arg(redis_key)
            .query_async(&mut connection)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
    } else {
        state.load_provider_oauth_batch_task_for_tests(&key)
    };
    let Some(raw) = raw else {
        return Ok(None);
    };
    let parsed = match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let Some(state) = parsed.as_object() else {
        return Ok(None);
    };
    if state
        .get("provider_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        != provider_id
    {
        return Ok(None);
    }
    Ok(Some(build_provider_oauth_batch_task_status_payload(
        provider_id,
        state,
    )))
}

#[cfg(test)]
mod tests {
    use super::enrich_admin_provider_oauth_auth_config;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::json;

    fn sample_unsigned_jwt(payload: serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn codex_enrichment_extracts_identity_from_nested_auth_claims() {
        let access_token = sample_unsigned_jwt(json!({
            "email": "u@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc-1",
                "chatgpt_account_user_id": "user-1__acc-1",
                "chatgpt_plan_type": "team",
                "chatgpt_user_id": "user-1",
                "organizations": [
                    {"id": "org-1", "title": "Personal", "is_default": true}
                ],
            }
        }));
        let token_payload = json!({
            "access_token": access_token,
        });
        let mut auth_config = serde_json::Map::new();

        enrich_admin_provider_oauth_auth_config("codex", &mut auth_config, &token_payload);

        assert_eq!(auth_config.get("email"), Some(&json!("u@example.com")));
        assert_eq!(auth_config.get("account_id"), Some(&json!("acc-1")));
        assert_eq!(
            auth_config.get("account_user_id"),
            Some(&json!("user-1__acc-1"))
        );
        assert_eq!(auth_config.get("plan_type"), Some(&json!("team")));
        assert_eq!(auth_config.get("user_id"), Some(&json!("user-1")));
        assert_eq!(
            auth_config.get("organizations"),
            Some(&json!([
                {"id": "org-1", "title": "Personal", "is_default": true}
            ]))
        );
    }

    #[test]
    fn codex_enrichment_normalizes_direct_chatgpt_alias_fields() {
        let token_payload = json!({
            "email": "alias@example.com",
            "chatgpt_account_id": "acc-2",
            "chatgpt_account_user_id": "user-2__acc-2",
            "chatgpt_plan_type": "plus",
            "chatgpt_user_id": "user-2",
        });
        let mut auth_config = serde_json::Map::new();

        enrich_admin_provider_oauth_auth_config("codex", &mut auth_config, &token_payload);

        assert_eq!(auth_config.get("email"), Some(&json!("alias@example.com")));
        assert_eq!(auth_config.get("account_id"), Some(&json!("acc-2")));
        assert_eq!(
            auth_config.get("account_user_id"),
            Some(&json!("user-2__acc-2"))
        );
        assert_eq!(auth_config.get("plan_type"), Some(&json!("plus")));
        assert_eq!(auth_config.get("user_id"), Some(&json!("user-2")));
    }
}
