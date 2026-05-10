use super::shared::{
    build_quota_snapshot_payload, default_provider_quota_execution_timeouts,
    execute_provider_quota_plan, extract_execution_error_message,
    persist_provider_quota_refresh_state, public_refresh_quota_probe_timeout,
    quota_refresh_success_invalid_state, ProviderQuotaExecutionOutcome,
};
use crate::admin_api::{
    classify_oauth_key_probe_status, OauthKeyProbeClassification, OauthKeyProbeOutcome,
};
use crate::handlers::admin::provider::shared::payloads::{
    OAUTH_ACCOUNT_BLOCK_PREFIX, OAUTH_EXPIRED_PREFIX,
};
use crate::handlers::admin::request::{AdminAppState, AdminGatewayProviderTransportSnapshot};
use crate::GatewayError;
use aether_admin::provider::quota::parse_chatgpt_web_conversation_init_response;
use aether_contracts::{
    ExecutionPlan, ProxySnapshot, RequestBody, EXECUTION_REQUEST_ACCEPT_INVALID_CERTS_HEADER,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CHATGPT_WEB_DEFAULT_BASE_URL: &str = "https://chatgpt.com";
const CHATGPT_WEB_CONVERSATION_INIT_PATH: &str = "/backend-api/conversation/init";
const CHATGPT_WEB_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36 Edg/143.0.0.0";
const CHATGPT_WEB_CLIENT_VERSION: &str = "prod-be885abbfcfe7b1f511e88b3003d9ee44757fbad";
const CHATGPT_WEB_BUILD_NUMBER: &str = "5955942";
const CHATGPT_WEB_SEC_CH_UA: &str =
    r#""Microsoft Edge";v="143", "Chromium";v="143", "Not A(Brand";v="24""#;
const PLACEHOLDER_API_KEY: &str = "__placeholder__";
const CHATGPT_WEB_FREE_IMAGE_QUOTA_LIMIT: f64 = 25.0;

fn chatgpt_web_base_url(endpoint: &StoredProviderCatalogEndpoint) -> String {
    let base_url = endpoint.base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        CHATGPT_WEB_DEFAULT_BASE_URL.to_string()
    } else {
        base_url.to_string()
    }
}

fn build_chatgpt_web_quota_headers(
    authorization: (String, String),
    base_url: &str,
) -> BTreeMap<String, String> {
    let device_id = uuid::Uuid::new_v4().to_string();
    let session_id = uuid::Uuid::new_v4().to_string();
    let mut headers = BTreeMap::from([
        ("accept".to_string(), "application/json".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
        ("user-agent".to_string(), CHATGPT_WEB_USER_AGENT.to_string()),
        ("origin".to_string(), base_url.to_string()),
        ("referer".to_string(), format!("{base_url}/")),
        (
            "accept-language".to_string(),
            "zh-CN,zh;q=0.9,en;q=0.8,en-US;q=0.7".to_string(),
        ),
        ("cache-control".to_string(), "no-cache".to_string()),
        ("pragma".to_string(), "no-cache".to_string()),
        ("priority".to_string(), "u=1, i".to_string()),
        ("sec-ch-ua".to_string(), CHATGPT_WEB_SEC_CH_UA.to_string()),
        ("sec-ch-ua-arch".to_string(), r#""x86""#.to_string()),
        ("sec-ch-ua-bitness".to_string(), r#""64""#.to_string()),
        ("sec-ch-ua-mobile".to_string(), "?0".to_string()),
        ("sec-ch-ua-model".to_string(), r#""""#.to_string()),
        ("sec-ch-ua-platform".to_string(), r#""Windows""#.to_string()),
        (
            "sec-ch-ua-platform-version".to_string(),
            r#""19.0.0""#.to_string(),
        ),
        ("sec-fetch-dest".to_string(), "empty".to_string()),
        ("sec-fetch-mode".to_string(), "cors".to_string()),
        ("sec-fetch-site".to_string(), "same-origin".to_string()),
        ("oai-device-id".to_string(), device_id),
        ("oai-session-id".to_string(), session_id),
        ("oai-language".to_string(), "zh-CN".to_string()),
        (
            "oai-client-version".to_string(),
            CHATGPT_WEB_CLIENT_VERSION.to_string(),
        ),
        (
            "oai-client-build-number".to_string(),
            CHATGPT_WEB_BUILD_NUMBER.to_string(),
        ),
        (
            "x-openai-target-path".to_string(),
            CHATGPT_WEB_CONVERSATION_INIT_PATH.to_string(),
        ),
        (
            "x-openai-target-route".to_string(),
            CHATGPT_WEB_CONVERSATION_INIT_PATH.to_string(),
        ),
        (
            EXECUTION_REQUEST_ACCEPT_INVALID_CERTS_HEADER.to_string(),
            "true".to_string(),
        ),
    ]);
    headers.insert(authorization.0.to_ascii_lowercase(), authorization.1);
    headers
}

fn chatgpt_web_auth_config(
    transport: &AdminGatewayProviderTransportSnapshot,
) -> Option<serde_json::Value> {
    transport
        .key
        .decrypted_auth_config
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
}

fn chatgpt_web_auth_config_string(
    auth_config: Option<&serde_json::Value>,
    fields: &[&str],
) -> Option<String> {
    let object = auth_config.and_then(serde_json::Value::as_object)?;
    fields.iter().find_map(|field| {
        object
            .get(*field)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn enrich_chatgpt_web_quota_metadata(
    metadata: &mut serde_json::Value,
    auth_config: Option<&serde_json::Value>,
) {
    let Some(object) = metadata.as_object_mut() else {
        return;
    };
    for (target, fields) in [
        ("plan_type", &["plan_type", "tier", "plan"][..]),
        ("email", &["email"][..]),
        ("account_id", &["account_id", "accountId"][..]),
        ("account_user_id", &["account_user_id", "accountUserId"][..]),
        ("user_id", &["user_id", "userId"][..]),
    ] {
        if object.contains_key(target) {
            continue;
        }
        if let Some(value) = chatgpt_web_auth_config_string(auth_config, fields) {
            object.insert(target.to_string(), json!(value));
        }
    }
}

fn chatgpt_web_json_number(value: Option<&serde_json::Value>) -> Option<f64> {
    let value = value?;
    if let Some(number) = value.as_f64() {
        return number.is_finite().then_some(number);
    }
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

fn chatgpt_web_json_string(value: Option<&serde_json::Value>) -> Option<&str> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn existing_chatgpt_web_image_quota_limit(
    upstream_metadata: Option<&serde_json::Value>,
) -> Option<f64> {
    upstream_metadata
        .and_then(serde_json::Value::as_object)
        .and_then(|metadata| metadata.get("chatgpt_web"))
        .and_then(serde_json::Value::as_object)
        .and_then(|bucket| chatgpt_web_json_number(bucket.get("image_quota_total")))
        .filter(|value| *value > 0.0)
}

fn infer_chatgpt_web_image_quota_limit(
    plan_type: Option<&str>,
    remaining: Option<f64>,
    existing_limit: Option<f64>,
) -> Option<f64> {
    let normalized_plan = plan_type.unwrap_or_default().trim().to_ascii_lowercase();
    if normalized_plan == "free" {
        return Some(CHATGPT_WEB_FREE_IMAGE_QUOTA_LIMIT);
    }

    if let Some(existing_limit) = existing_limit.filter(|value| *value > 0.0) {
        return Some(existing_limit);
    }

    remaining.filter(|value| *value > 0.0)
}

fn normalize_chatgpt_web_image_quota_limit(
    metadata: &mut serde_json::Value,
    upstream_metadata: Option<&serde_json::Value>,
) {
    let existing_limit = existing_chatgpt_web_image_quota_limit(upstream_metadata);
    let Some(object) = metadata.as_object_mut() else {
        return;
    };

    let remaining = chatgpt_web_json_number(object.get("image_quota_remaining"));
    let explicit_limit =
        chatgpt_web_json_number(object.get("image_quota_total")).filter(|value| *value > 0.0);
    let plan_type = chatgpt_web_json_string(object.get("plan_type"));
    let is_free_plan = plan_type.is_some_and(|value| value.trim().eq_ignore_ascii_case("free"));
    let limit = if is_free_plan {
        Some(CHATGPT_WEB_FREE_IMAGE_QUOTA_LIMIT)
    } else {
        explicit_limit
            .or_else(|| infer_chatgpt_web_image_quota_limit(plan_type, remaining, existing_limit))
    };

    if let Some(limit) = limit {
        object.insert("image_quota_total".to_string(), json!(limit));

        if !object.contains_key("image_quota_used") {
            if let Some(remaining) = remaining {
                object.insert(
                    "image_quota_used".to_string(),
                    json!((limit - remaining).max(0.0)),
                );
            } else if object
                .get("image_quota_blocked")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                object.insert("image_quota_used".to_string(), json!(limit));
            }
        }
    }
}

async fn resolve_chatgpt_web_quota_auth(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
) -> Result<Option<(String, String)>, GatewayError> {
    if let Some(auth) = state.resolve_local_oauth_header_auth(transport).await? {
        return Ok(Some(auth));
    }
    let decrypted_key = transport.key.decrypted_api_key.trim();
    if decrypted_key.is_empty() || decrypted_key == PLACEHOLDER_API_KEY {
        return Ok(None);
    }
    Ok(Some((
        "authorization".to_string(),
        format!("Bearer {decrypted_key}"),
    )))
}

async fn execute_chatgpt_web_quota_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    endpoint: &StoredProviderCatalogEndpoint,
    authorization: (String, String),
    proxy_override: Option<&ProxySnapshot>,
    probe_timeout: Duration,
) -> Result<ProviderQuotaExecutionOutcome, GatewayError> {
    let base_url = chatgpt_web_base_url(endpoint);
    let proxy = match proxy_override {
        Some(proxy) => Some(proxy.clone()),
        None => {
            state
                .resolve_transport_proxy_snapshot_with_tunnel_affinity(transport)
                .await
        }
    };
    let timeouts = state
        .resolve_transport_execution_timeouts(transport)
        .or(Some(default_provider_quota_execution_timeouts(
            proxy.as_ref(),
        )));
    let plan = ExecutionPlan {
        request_id: format!("chatgpt-web-quota:{}", transport.key.id),
        candidate_id: None,
        provider_name: Some("chatgpt_web".to_string()),
        provider_id: transport.provider.id.clone(),
        endpoint_id: transport.endpoint.id.clone(),
        key_id: transport.key.id.clone(),
        method: "POST".to_string(),
        url: format!("{base_url}{CHATGPT_WEB_CONVERSATION_INIT_PATH}"),
        headers: build_chatgpt_web_quota_headers(authorization, base_url.as_str()),
        content_type: Some("application/json".to_string()),
        content_encoding: None,
        body: RequestBody::from_json(json!({
            "gizmo_id": serde_json::Value::Null,
            "requested_default_model": serde_json::Value::Null,
            "conversation_id": serde_json::Value::Null,
            "timezone_offset_min": -480,
            "system_hints": ["picture_v2"],
        })),
        stream: false,
        client_api_format: "openai:image".to_string(),
        provider_api_format: "chatgpt_web:conversation_init".to_string(),
        model_name: Some("chatgpt-web-conversation-init".to_string()),
        proxy,
        transport_profile: state.resolve_transport_profile(transport),
        timeouts,
    };

    execute_provider_quota_plan(state, transport, plan, "chatgpt_web", probe_timeout).await
}

fn chatgpt_web_quota_invalid_reason(status_code: u16, upstream_message: Option<&str>) -> String {
    let message = upstream_message.unwrap_or_default().trim();
    let detail = if message.is_empty() {
        match status_code {
            401 => "ChatGPT Web Token 无效或已过期",
            403 => "ChatGPT Web 账户访问受限",
            _ => "ChatGPT Web 请求失败",
        }
    } else {
        message
    };
    match status_code {
        401 => format!("{OAUTH_EXPIRED_PREFIX}{detail}"),
        403 => format!("{OAUTH_ACCOUNT_BLOCK_PREFIX}{detail}"),
        _ => detail.to_string(),
    }
}

pub(crate) async fn probe_chatgpt_web_provider_key_with_classification(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    key: StoredProviderCatalogKey,
    proxy_override: Option<&ProxySnapshot>,
    probe_timeout: Duration,
    _allow_auto_remove: bool,
) -> Result<OauthKeyProbeOutcome, GatewayError> {
    let transport = match state
        .read_provider_transport_snapshot(&provider.id, &endpoint.id, &key.id)
        .await?
    {
        Some(transport) => transport,
        None => {
            return Ok(OauthKeyProbeOutcome::from_payload(
                OauthKeyProbeClassification::TransportError,
                false,
                false,
                false,
                json!({
                    "key_id": key.id.clone(),
                    "key_name": key.name.clone(),
                    "status": "error",
                    "message": "Provider transport snapshot unavailable",
                }),
            ));
        }
    };

    let authorization = match resolve_chatgpt_web_quota_auth(state, &transport).await? {
        Some(auth) => auth,
        None => {
            return Ok(OauthKeyProbeOutcome::from_payload(
                OauthKeyProbeClassification::OAuthInvalid,
                false,
                false,
                false,
                json!({
                    "key_id": key.id.clone(),
                    "key_name": key.name.clone(),
                    "status": "error",
                    "message": "缺少 ChatGPT Web OAuth 认证信息，请先导入/刷新 Token",
                }),
            ));
        }
    };

    let result = match execute_chatgpt_web_quota_plan(
        state,
        &transport,
        endpoint,
        authorization,
        proxy_override,
        probe_timeout,
    )
    .await?
    {
        ProviderQuotaExecutionOutcome::Response(result) => result,
        ProviderQuotaExecutionOutcome::Failure(detail) => {
            return Ok(OauthKeyProbeOutcome::from_payload(
                OauthKeyProbeClassification::TransportError,
                false,
                false,
                false,
                json!({
                    "key_id": key.id.clone(),
                    "key_name": key.name.clone(),
                    "status": "error",
                    "message": format!("conversation/init 请求执行失败: {detail}"),
                    "status_code": 502,
                }),
            ));
        }
    };

    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut metadata_update = None::<serde_json::Value>;
    let (mut oauth_invalid_at_unix_secs, mut oauth_invalid_reason) = (
        key.oauth_invalid_at_unix_secs,
        key.oauth_invalid_reason.clone(),
    );
    let mut status = "error".to_string();
    let mut message = None::<String>;

    if result.status_code == 200 {
        if let Some(body_json) = result
            .body
            .as_ref()
            .and_then(|body| body.json_body.as_ref())
        {
            if let Some(mut metadata) =
                parse_chatgpt_web_conversation_init_response(body_json, now_unix_secs)
            {
                let auth_config = chatgpt_web_auth_config(&transport);
                enrich_chatgpt_web_quota_metadata(&mut metadata, auth_config.as_ref());
                normalize_chatgpt_web_image_quota_limit(
                    &mut metadata,
                    key.upstream_metadata.as_ref(),
                );
                metadata_update = Some(json!({ "chatgpt_web": metadata }));
                (oauth_invalid_at_unix_secs, oauth_invalid_reason) =
                    quota_refresh_success_invalid_state(&key);
                status = "success".to_string();
            } else {
                status = "no_metadata".to_string();
                message = Some("响应中未包含 ChatGPT Web 生图限额信息".to_string());
            }
        } else {
            status = "no_metadata".to_string();
            message = Some("响应中未包含 ChatGPT Web 生图限额信息".to_string());
        }
    } else {
        let err_msg = extract_execution_error_message(&result);
        message = Some(match err_msg.as_deref() {
            Some(detail) if !detail.is_empty() => {
                format!(
                    "conversation/init 返回状态码 {}: {}",
                    result.status_code, detail
                )
            }
            _ => format!("conversation/init 返回状态码 {}", result.status_code),
        });

        if matches!(result.status_code, 401 | 403) {
            oauth_invalid_at_unix_secs = Some(now_unix_secs);
            oauth_invalid_reason = Some(chatgpt_web_quota_invalid_reason(
                result.status_code,
                err_msg.as_deref(),
            ));
            status = if result.status_code == 401 {
                "auth_invalid".to_string()
            } else {
                "forbidden".to_string()
            };
        }
    }

    if !persist_provider_quota_refresh_state(
        state,
        &key.id,
        metadata_update.as_ref(),
        oauth_invalid_at_unix_secs,
        oauth_invalid_reason.clone(),
        None,
    )
    .await?
    {
        return Ok(OauthKeyProbeOutcome::from_payload(
            OauthKeyProbeClassification::ServerError,
            false,
            false,
            false,
            json!({
                "key_id": key.id.clone(),
                "key_name": key.name.clone(),
                "status": "error",
                "message": "Key 状态写入失败",
            }),
        ));
    }

    let success = status == "success";
    let classification = classify_oauth_key_probe_status(
        provider.provider_type.as_str(),
        status.as_str(),
        Some(result.status_code),
        message.as_deref(),
        oauth_invalid_reason.as_deref(),
        success,
    );
    let mut payload = serde_json::Map::new();
    payload.insert("key_id".to_string(), json!(key.id.clone()));
    payload.insert("key_name".to_string(), json!(key.name.clone()));
    payload.insert("status".to_string(), json!(status));
    if let Some(message) = message {
        payload.insert("message".to_string(), json!(message));
    }
    if result.status_code != 200 {
        payload.insert("status_code".to_string(), json!(result.status_code));
    }
    if let Some(metadata) = metadata_update
        .as_ref()
        .and_then(|value| value.get("chatgpt_web"))
        .cloned()
    {
        payload.insert("metadata".to_string(), metadata);
    }
    if let Some(quota_snapshot) = build_quota_snapshot_payload(
        "chatgpt_web",
        key.status_snapshot.as_ref(),
        metadata_update.as_ref(),
    ) {
        payload.insert("quota_snapshot".to_string(), quota_snapshot);
    }

    Ok(OauthKeyProbeOutcome::from_payload(
        classification,
        success,
        false,
        false,
        serde_json::Value::Object(payload),
    ))
}

pub(crate) async fn refresh_chatgpt_web_provider_quota_locally(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoint: &StoredProviderCatalogEndpoint,
    keys: Vec<StoredProviderCatalogKey>,
    proxy_override: Option<ProxySnapshot>,
) -> Result<Option<serde_json::Value>, GatewayError> {
    let mut results = Vec::new();
    let mut success_count = 0usize;
    let mut failed_count = 0usize;
    let probe_timeout = public_refresh_quota_probe_timeout(provider, proxy_override.as_ref());

    for key in keys {
        let outcome = probe_chatgpt_web_provider_key_with_classification(
            state,
            provider,
            endpoint,
            key,
            proxy_override.as_ref(),
            probe_timeout,
            true,
        )
        .await?;
        if outcome.success() {
            success_count += 1;
        } else {
            failed_count += 1;
        }
        results.push(outcome.into_payload());
    }

    Ok(Some(json!({
        "success": success_count,
        "failed": failed_count,
        "total": success_count + failed_count,
        "results": results,
        "message": format!("已处理 {} 个 Key", success_count + failed_count),
        "auto_removed": 0,
    })))
}
