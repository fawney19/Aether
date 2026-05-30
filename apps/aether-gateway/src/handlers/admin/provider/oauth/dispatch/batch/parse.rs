use super::super::token_import::{import_tokens_from_raw_token, normalize_provider_import_tokens};
use crate::handlers::admin::provider::oauth::errors::build_internal_control_error_response;
use crate::handlers::admin::provider::oauth::state::{current_unix_secs, json_u64_value};
use axum::{
    body::{to_bytes, Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Deserialize)]
pub(super) struct AdminProviderOAuthBatchImportRequest {
    pub credentials: String,
    pub proxy_node_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct AdminProviderOAuthBatchImportEntry {
    pub parse_error: Option<String>,
    pub refresh_token: Option<String>,
    pub access_token: Option<String>,
    pub raw_credentials: Option<serde_json::Value>,
    pub expires_at: Option<u64>,
    pub account_id: Option<String>,
    pub account_user_id: Option<String>,
    pub plan_type: Option<String>,
    pub pool_tier: Option<String>,
    pub user_id: Option<String>,
    pub email: Option<String>,
    pub account_name: Option<String>,
    pub project_id: Option<String>,
    pub sso_rw_token: Option<String>,
    pub cf_cookies: Option<String>,
    pub cf_clearance: Option<String>,
    pub user_agent: Option<String>,
    pub browser_profile: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct AdminProviderOAuthBatchImportOutcome {
    pub total: usize,
    pub success: usize,
    pub failed: usize,
    pub results: Vec<serde_json::Value>,
}

pub(super) fn parse_admin_provider_oauth_batch_import_request(
    request_body: Option<&Bytes>,
) -> Result<AdminProviderOAuthBatchImportRequest, Response<Body>> {
    let Some(request_body) = request_body else {
        return Err(
            crate::handlers::admin::provider::oauth::errors::build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求体必须是合法的 JSON 对象",
            ),
        );
    };
    match serde_json::from_slice::<AdminProviderOAuthBatchImportRequest>(request_body) {
        Ok(payload) if !payload.credentials.trim().is_empty() => Ok(payload),
        _ => Err(
            crate::handlers::admin::provider::oauth::errors::build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求体必须是合法的 JSON 对象",
            ),
        ),
    }
}

fn coerce_admin_provider_oauth_import_str(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn coerce_admin_provider_oauth_import_project_id(
    value: Option<&serde_json::Value>,
) -> Option<String> {
    match value {
        Some(serde_json::Value::Object(object)) => coerce_admin_provider_oauth_import_str(
            object
                .get("id")
                .or_else(|| object.get("project_id"))
                .or_else(|| object.get("projectId")),
        ),
        other => coerce_admin_provider_oauth_import_str(other),
    }
}

fn json_import_expiry_value(value: Option<&serde_json::Value>) -> Option<u64> {
    let value = value?;
    json_u64_value(Some(value)).or_else(|| {
        value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .and_then(|value| u64::try_from(value.timestamp()).ok())
    })
}

fn json_import_expiry_from_keys(
    object: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<u64> {
    keys.iter()
        .find_map(|key| json_import_expiry_value(object.get(*key)))
}

fn grok_cookie_value(raw: &str, name: &str) -> Option<String> {
    raw.trim()
        .strip_prefix("Cookie:")
        .unwrap_or_else(|| raw.trim())
        .split(';')
        .filter_map(|segment| segment.trim().split_once('='))
        .find_map(|(cookie_name, cookie_value)| {
            cookie_name
                .trim()
                .eq_ignore_ascii_case(name)
                .then(|| cookie_value.trim())
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
}

fn grok_cookie_profile(raw: &str) -> Option<String> {
    let raw = raw
        .trim()
        .strip_prefix("Cookie:")
        .unwrap_or_else(|| raw.trim());
    let parts = raw
        .split(';')
        .filter_map(|segment| {
            let (cookie_name, cookie_value) = segment.trim().split_once('=')?;
            let cookie_name = cookie_name.trim();
            let cookie_value = cookie_value.trim();
            if cookie_name.is_empty()
                || cookie_value.is_empty()
                || cookie_name.eq_ignore_ascii_case("sso")
                || cookie_name.eq_ignore_ascii_case("sso-rw")
            {
                return None;
            }
            Some(format!("{cookie_name}={cookie_value}"))
        })
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join("; "))
}

fn grok_cookie_session_token(provider_type: &str, raw: &str) -> Option<String> {
    provider_type
        .trim()
        .eq_ignore_ascii_case("grok")
        .then(|| grok_cookie_value(raw, "sso"))
        .flatten()
}

fn extract_admin_provider_oauth_batch_import_entry(
    provider_type: &str,
    item: &serde_json::Value,
) -> Option<AdminProviderOAuthBatchImportEntry> {
    match item {
        serde_json::Value::String(value) => {
            let raw_token = value.trim();
            if raw_token.is_empty() {
                None
            } else {
                let sso_from_cookie = grok_cookie_session_token(provider_type, raw_token);
                let token_input = sso_from_cookie.as_deref().unwrap_or(raw_token);
                let (refresh_token, access_token) = import_tokens_from_raw_token(token_input);
                let (refresh_token, access_token) = normalize_provider_import_tokens(
                    provider_type,
                    refresh_token.as_deref(),
                    access_token.as_deref(),
                );
                Some(AdminProviderOAuthBatchImportEntry {
                    parse_error: None,
                    refresh_token,
                    access_token,
                    raw_credentials: None,
                    expires_at: None,
                    account_id: None,
                    account_user_id: None,
                    plan_type: None,
                    pool_tier: None,
                    user_id: grok_cookie_value(raw_token, "x-userid"),
                    email: None,
                    account_name: None,
                    project_id: None,
                    sso_rw_token: grok_cookie_value(raw_token, "sso-rw"),
                    cf_cookies: grok_cookie_profile(raw_token),
                    cf_clearance: grok_cookie_value(raw_token, "cf_clearance"),
                    user_agent: None,
                    browser_profile: None,
                })
            }
        }
        serde_json::Value::Object(object) => {
            let is_grok = provider_type.trim().eq_ignore_ascii_case("grok");
            let is_windsurf = provider_type.trim().eq_ignore_ascii_case("windsurf");
            let refresh_token = coerce_admin_provider_oauth_import_str(
                object
                    .get("refresh_token")
                    .or_else(|| object.get("refreshToken")),
            );
            let access_token = coerce_admin_provider_oauth_import_str(
                object
                    .get("access_token")
                    .or_else(|| object.get("accessToken")),
            );
            let grok_token_alias = if is_grok { object.get("token") } else { None };
            let grok_cookie = if is_grok {
                coerce_admin_provider_oauth_import_str(
                    object.get("cookie").or_else(|| object.get("cookieHeader")),
                )
            } else {
                None
            };
            let session_token = coerce_admin_provider_oauth_import_str(
                object
                    .get("sso_token")
                    .or_else(|| object.get("ssoToken"))
                    .or_else(|| object.get("session_token"))
                    .or_else(|| object.get("sessionToken"))
                    .or(grok_token_alias),
            )
            .or_else(|| {
                grok_cookie
                    .as_deref()
                    .and_then(|cookie| grok_cookie_value(cookie, "sso"))
            });
            let (refresh_token, access_token) = normalize_provider_import_tokens(
                provider_type,
                refresh_token.as_deref(),
                access_token.as_deref().or(session_token.as_deref()),
            );
            let windsurf_api_key = is_windsurf
                .then(|| {
                    coerce_admin_provider_oauth_import_str(
                        object.get("api_key").or_else(|| object.get("apiKey")),
                    )
                })
                .flatten();
            let windsurf_token = is_windsurf
                .then(|| {
                    coerce_admin_provider_oauth_import_str(
                        object
                            .get("token")
                            .or_else(|| object.get("auth_token"))
                            .or_else(|| object.get("authToken")),
                    )
                })
                .flatten();
            let windsurf_password = is_windsurf
                .then(|| coerce_admin_provider_oauth_import_str(object.get("password")))
                .flatten();
            let raw_credentials = if is_windsurf
                && (windsurf_api_key.is_some()
                    || windsurf_token.is_some()
                    || windsurf_password.is_some())
            {
                Some(item.clone())
            } else {
                None
            };
            if refresh_token.is_none() && access_token.is_none() && raw_credentials.is_none() {
                return None;
            }
            let refresh_token = if is_windsurf {
                refresh_token.or(windsurf_api_key).or(windsurf_token)
            } else {
                refresh_token
            };
            let expires_at =
                json_import_expiry_from_keys(object, &["expires_at", "expiresAt", "expired"]);
            let account_id = coerce_admin_provider_oauth_import_str(
                object
                    .get("account_id")
                    .or_else(|| object.get("accountId"))
                    .or_else(|| object.get("chatgpt_account_id"))
                    .or_else(|| object.get("chatgptAccountId")),
            );
            let account_user_id = coerce_admin_provider_oauth_import_str(
                object
                    .get("account_user_id")
                    .or_else(|| object.get("accountUserId"))
                    .or_else(|| object.get("chatgpt_account_user_id"))
                    .or_else(|| object.get("chatgptAccountUserId")),
            );
            let plan_type = coerce_admin_provider_oauth_import_str(
                object
                    .get("plan_type")
                    .or_else(|| object.get("planType"))
                    .or_else(|| object.get("chatgpt_plan_type"))
                    .or_else(|| object.get("chatgptPlanType")),
            )
            .map(|value| value.to_ascii_lowercase());
            let pool_tier = coerce_admin_provider_oauth_import_str(
                object
                    .get("pool_tier")
                    .or_else(|| object.get("poolTier"))
                    .or_else(|| object.get("tier")),
            )
            .map(|value| value.to_ascii_lowercase());
            let user_id = coerce_admin_provider_oauth_import_str(
                object
                    .get("user_id")
                    .or_else(|| object.get("userId"))
                    .or_else(|| object.get("chatgpt_user_id"))
                    .or_else(|| object.get("chatgptUserId")),
            )
            .or_else(|| {
                grok_cookie
                    .as_deref()
                    .and_then(|cookie| grok_cookie_value(cookie, "x-userid"))
            });
            let email = coerce_admin_provider_oauth_import_str(object.get("email"));
            let account_name = coerce_admin_provider_oauth_import_str(
                object
                    .get("account_name")
                    .or_else(|| object.get("accountName")),
            );
            let project_id = coerce_admin_provider_oauth_import_project_id(
                object
                    .get("project_id")
                    .or_else(|| object.get("projectId"))
                    .or_else(|| object.get("cloudaicompanionProject"))
                    .or_else(|| object.get("cloudAiCompanionProject")),
            );
            let sso_rw_token = coerce_admin_provider_oauth_import_str(
                object
                    .get("sso_rw_token")
                    .or_else(|| object.get("ssoRwToken")),
            )
            .or_else(|| {
                grok_cookie
                    .as_deref()
                    .and_then(|cookie| grok_cookie_value(cookie, "sso-rw"))
            });
            let cf_clearance = coerce_admin_provider_oauth_import_str(
                object
                    .get("cf_clearance")
                    .or_else(|| object.get("cfClearance")),
            )
            .or_else(|| {
                grok_cookie
                    .as_deref()
                    .and_then(|cookie| grok_cookie_value(cookie, "cf_clearance"))
            });
            let cf_cookies = coerce_admin_provider_oauth_import_str(
                object.get("cf_cookies").or_else(|| object.get("cfCookies")),
            )
            .or_else(|| grok_cookie.as_deref().and_then(grok_cookie_profile));
            let user_agent = coerce_admin_provider_oauth_import_str(
                object.get("user_agent").or_else(|| object.get("userAgent")),
            );
            let browser_profile = coerce_admin_provider_oauth_import_str(
                object
                    .get("browser_profile")
                    .or_else(|| object.get("browserProfile"))
                    .or_else(|| object.get("browser"))
                    .or_else(|| object.get("impersonate")),
            );
            Some(AdminProviderOAuthBatchImportEntry {
                parse_error: None,
                refresh_token,
                access_token,
                raw_credentials,
                expires_at,
                account_id,
                account_user_id,
                plan_type,
                pool_tier,
                user_id,
                email,
                account_name,
                project_id,
                sso_rw_token,
                cf_cookies,
                cf_clearance,
                user_agent,
                browser_profile,
            })
        }
        _ => None,
    }
}

pub(super) fn parse_admin_provider_oauth_batch_import_entries(
    provider_type: &str,
    raw_credentials: &str,
) -> Vec<AdminProviderOAuthBatchImportEntry> {
    let raw = raw_credentials.trim();
    if raw.is_empty() {
        return Vec::new();
    }

    if raw.starts_with('[') {
        match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(serde_json::Value::Array(items)) => {
                return items
                    .iter()
                    .filter_map(|item| {
                        extract_admin_provider_oauth_batch_import_entry(provider_type, item)
                    })
                    .collect();
            }
            Ok(_) => {}
            Err(error) => return vec![parse_error_entry(format!("JSON 数组解析失败: {error}"))],
        }
    }

    if raw.starts_with('{') {
        if let Ok(value @ serde_json::Value::Object(_)) =
            serde_json::from_str::<serde_json::Value>(raw)
        {
            return extract_admin_provider_oauth_batch_import_entry(provider_type, &value)
                .into_iter()
                .collect();
        }
    }

    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|token| {
            if is_json_like_batch_line(token) {
                match serde_json::from_str::<serde_json::Value>(token) {
                    Ok(value @ serde_json::Value::Object(_)) => {
                        return extract_admin_provider_oauth_batch_import_entry(
                            provider_type,
                            &value,
                        );
                    }
                    Ok(_) => {
                        return Some(parse_error_entry(
                            "JSON 行必须是账号对象，不能作为 raw token 导入".to_string(),
                        ));
                    }
                    Err(error) => {
                        return Some(parse_error_entry(format!("JSON 行解析失败: {error}")));
                    }
                }
            }

            extract_admin_provider_oauth_batch_import_entry(
                provider_type,
                &serde_json::Value::String(token.to_string()),
            )
        })
        .collect()
}

fn parse_error_entry(error: String) -> AdminProviderOAuthBatchImportEntry {
    AdminProviderOAuthBatchImportEntry {
        parse_error: Some(error),
        refresh_token: None,
        access_token: None,
        raw_credentials: None,
        expires_at: None,
        account_id: None,
        account_user_id: None,
        plan_type: None,
        pool_tier: None,
        user_id: None,
        email: None,
        account_name: None,
        project_id: None,
        sso_rw_token: None,
        cf_cookies: None,
        cf_clearance: None,
        user_agent: None,
        browser_profile: None,
    }
}

fn is_json_like_batch_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('{') || line.starts_with('[')
}

pub(super) fn apply_admin_provider_oauth_batch_import_hints(
    provider_type: &str,
    entry: &AdminProviderOAuthBatchImportEntry,
    auth_config: &mut serde_json::Map<String, serde_json::Value>,
) {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    if provider_type == "gemini_cli" {
        if let Some(project_id) = entry.project_id.as_ref() {
            auth_config
                .entry("project_id".to_string())
                .or_insert_with(|| json!(project_id));
        }
        if let Some(plan_type) = entry.plan_type.as_ref() {
            auth_config
                .entry("plan_type".to_string())
                .or_insert_with(|| json!(plan_type));
        }
        return;
    }
    if !matches!(provider_type.as_str(), "codex" | "chatgpt_web" | "grok") {
        return;
    }
    if let Some(account_id) = entry.account_id.as_ref() {
        auth_config
            .entry("account_id".to_string())
            .or_insert_with(|| json!(account_id));
    }
    if let Some(account_user_id) = entry.account_user_id.as_ref() {
        auth_config
            .entry("account_user_id".to_string())
            .or_insert_with(|| json!(account_user_id));
    }
    if let Some(plan_type) = entry.plan_type.as_ref() {
        auth_config
            .entry("plan_type".to_string())
            .or_insert_with(|| json!(plan_type));
    }
    if let Some(pool_tier) = entry.pool_tier.as_ref() {
        auth_config
            .entry("pool_tier".to_string())
            .or_insert_with(|| json!(pool_tier));
    }
    if let Some(user_id) = entry.user_id.as_ref() {
        auth_config
            .entry("user_id".to_string())
            .or_insert_with(|| json!(user_id));
    }
    if let Some(email) = entry.email.as_ref() {
        auth_config
            .entry("email".to_string())
            .or_insert_with(|| json!(email));
    }
    if let Some(account_name) = entry.account_name.as_ref() {
        auth_config
            .entry("account_name".to_string())
            .or_insert_with(|| json!(account_name));
    }
    if let Some(sso_rw_token) = entry.sso_rw_token.as_ref() {
        auth_config
            .entry("sso_rw_token".to_string())
            .or_insert_with(|| json!(sso_rw_token));
    }
    if let Some(cf_cookies) = entry.cf_cookies.as_ref() {
        auth_config
            .entry("cf_cookies".to_string())
            .or_insert_with(|| json!(cf_cookies));
    }
    if let Some(cf_clearance) = entry.cf_clearance.as_ref() {
        auth_config
            .entry("cf_clearance".to_string())
            .or_insert_with(|| json!(cf_clearance));
    }
    if let Some(user_agent) = entry.user_agent.as_ref() {
        auth_config
            .entry("user_agent".to_string())
            .or_insert_with(|| json!(user_agent));
    }
    if let Some(browser_profile) = entry.browser_profile.as_ref() {
        auth_config
            .entry("browser_profile".to_string())
            .or_insert_with(|| json!(browser_profile));
    }
}

pub(super) async fn extract_admin_provider_oauth_batch_error_detail(
    response: Response<Body>,
) -> String {
    let status = response.status();
    let raw_body = to_bytes(response.into_body(), usize::MAX).await.ok();
    if let Some(raw_body) = raw_body {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw_body) {
            if let Some(detail) = value.get("detail").and_then(serde_json::Value::as_str) {
                let normalized = detail.trim();
                if !normalized.is_empty() {
                    return normalized.to_string();
                }
            }
        }
        let normalized = String::from_utf8_lossy(&raw_body).trim().to_string();
        if !normalized.is_empty() {
            return normalized;
        }
    }
    format!("HTTP {}", status.as_u16())
}

pub(super) fn build_admin_provider_oauth_batch_import_response(
    outcome: &AdminProviderOAuthBatchImportOutcome,
) -> Json<serde_json::Value> {
    Json(json!({
        "total": outcome.total,
        "success": outcome.success,
        "failed": outcome.failed,
        "results": outcome.results,
    }))
}

pub(super) fn build_admin_provider_oauth_batch_task_state(
    task_id: &str,
    provider_id: &str,
    provider_type: &str,
    status: &str,
    total: usize,
    processed: usize,
    success: usize,
    failed: usize,
    created_count: usize,
    replaced_count: usize,
    message: Option<&str>,
    error: Option<&str>,
    error_samples: Vec<serde_json::Value>,
    created_at: u64,
    started_at: Option<u64>,
    finished_at: Option<u64>,
) -> serde_json::Value {
    let updated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(created_at);
    let progress_percent = processed
        .saturating_mul(100)
        .checked_div(total)
        .unwrap_or(0)
        .min(100) as u64;
    json!({
        "task_id": task_id,
        "provider_id": provider_id,
        "provider_type": provider_type,
        "status": status,
        "total": total,
        "processed": processed,
        "success": success,
        "failed": failed,
        "created_count": created_count,
        "replaced_count": replaced_count,
        "progress_percent": progress_percent,
        "message": message,
        "error": error,
        "error_samples": error_samples,
        "created_at": created_at,
        "started_at": started_at,
        "finished_at": finished_at,
        "updated_at": updated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        apply_admin_provider_oauth_batch_import_hints,
        parse_admin_provider_oauth_batch_import_entries,
    };
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::json;

    fn unsigned_jwt(payload: serde_json::Value) -> String {
        let header = json!({"alg": "none", "typ": "JWT"});
        let encode = |value: serde_json::Value| {
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).expect("jwt json should serialize"))
        };
        format!("{}.{}.signature", encode(header), encode(payload))
    }

    #[test]
    fn parses_access_token_only_entry() {
        let entries = parse_admin_provider_oauth_batch_import_entries(
            "codex",
            r#"[{"accessToken":"at_1","expiresAt":2100000000,"accountId":"acc-1","email":"u@example.com"}]"#,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].refresh_token, None);
        assert_eq!(entries[0].access_token.as_deref(), Some("at_1"));
        assert_eq!(entries[0].expires_at, Some(2_100_000_000));
        assert_eq!(entries[0].account_id.as_deref(), Some("acc-1"));
        assert_eq!(entries[0].email.as_deref(), Some("u@example.com"));
    }

    #[test]
    fn parses_common_chatgpt_web_json_aliases() {
        let entries = parse_admin_provider_oauth_batch_import_entries(
            "chatgpt_web",
            r#"[{"session_token":"session-1","expired":"2030-01-01T00:00:00Z","chatgpt_account_id":"acc-1","chatgpt_plan_type":"plus"}]"#,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].refresh_token, None);
        assert_eq!(entries[0].access_token.as_deref(), Some("session-1"));
        assert_eq!(entries[0].expires_at, Some(1_893_456_000));
        assert_eq!(entries[0].account_id.as_deref(), Some("acc-1"));
        assert_eq!(entries[0].plan_type.as_deref(), Some("plus"));
    }

    #[test]
    fn parses_plain_jwt_line_as_access_token() {
        let token = unsigned_jwt(json!({
            "iss": "https://auth.openai.com",
            "aud": ["https://api.openai.com/v1"],
            "exp": 2_000_000_000u64,
        }));

        let entries = parse_admin_provider_oauth_batch_import_entries("codex", &token);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].refresh_token, None);
        assert_eq!(entries[0].access_token.as_deref(), Some(token.as_str()));
    }

    #[test]
    fn parses_grok_jsonl_session_entries() {
        let entries = parse_admin_provider_oauth_batch_import_entries(
            "grok",
            r#"{"sso_token":"sso-1","cf_clearance":"cf-1","pool_tier":"heavy","email":"grok@example.com","browser_profile":"chrome136"}"#,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].refresh_token, None);
        assert_eq!(entries[0].access_token.as_deref(), Some("sso-1"));
        assert_eq!(entries[0].cf_clearance.as_deref(), Some("cf-1"));
        assert_eq!(entries[0].pool_tier.as_deref(), Some("heavy"));
        assert_eq!(entries[0].email.as_deref(), Some("grok@example.com"));
        assert_eq!(entries[0].browser_profile.as_deref(), Some("chrome136"));
    }

    #[test]
    fn parses_grok_token_alias_with_account_traits() {
        let entries = parse_admin_provider_oauth_batch_import_entries(
            "grok",
            r#"[{"token":"sso-1","planType":"super","tier":"heavy","accountName":"Grok Heavy"}]"#,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].refresh_token, None);
        assert_eq!(entries[0].access_token.as_deref(), Some("sso-1"));
        assert_eq!(entries[0].plan_type.as_deref(), Some("super"));
        assert_eq!(entries[0].pool_tier.as_deref(), Some("heavy"));
        assert_eq!(entries[0].account_name.as_deref(), Some("Grok Heavy"));
    }

    #[test]
    fn parses_grok_plain_line_as_session_token() {
        let entries = parse_admin_provider_oauth_batch_import_entries("grok", "opaque-sso-token");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].refresh_token, None);
        assert_eq!(entries[0].access_token.as_deref(), Some("opaque-sso-token"));
    }

    #[test]
    fn parses_grok_cookie_line_as_session_metadata() {
        let entries = parse_admin_provider_oauth_batch_import_entries(
            "grok",
            "i18nextLng=zh; cf_clearance=cf-1; sso-rw=rw-1; sso=sso-1; x-userid=user-1",
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].refresh_token, None);
        assert_eq!(entries[0].access_token.as_deref(), Some("sso-1"));
        assert_eq!(entries[0].sso_rw_token.as_deref(), Some("rw-1"));
        assert_eq!(
            entries[0].cf_cookies.as_deref(),
            Some("i18nextLng=zh; cf_clearance=cf-1; x-userid=user-1")
        );
        assert_eq!(entries[0].cf_clearance.as_deref(), Some("cf-1"));
        assert_eq!(entries[0].user_id.as_deref(), Some("user-1"));
    }

    #[test]
    fn parses_grok_cookie_object_as_session_metadata() {
        let entries = parse_admin_provider_oauth_batch_import_entries(
            "grok",
            r#"[{"cookie":"cf_clearance=cf-1; sso-rw=rw-1; sso=sso-1; x-userid=user-1","tier":"heavy"}]"#,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].refresh_token, None);
        assert_eq!(entries[0].access_token.as_deref(), Some("sso-1"));
        assert_eq!(entries[0].sso_rw_token.as_deref(), Some("rw-1"));
        assert_eq!(
            entries[0].cf_cookies.as_deref(),
            Some("cf_clearance=cf-1; x-userid=user-1")
        );
        assert_eq!(entries[0].cf_clearance.as_deref(), Some("cf-1"));
        assert_eq!(entries[0].user_id.as_deref(), Some("user-1"));
        assert_eq!(entries[0].pool_tier.as_deref(), Some("heavy"));
    }

    #[test]
    fn parses_gemini_cli_project_id_hint() {
        let entries = parse_admin_provider_oauth_batch_import_entries(
            "gemini_cli",
            r#"[{"refresh_token":"rt-1","projectId":"project-gemini-cli-1","planType":"free"}]"#,
        );

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].refresh_token.as_deref(), Some("rt-1"));
        assert_eq!(
            entries[0].project_id.as_deref(),
            Some("project-gemini-cli-1")
        );
        assert_eq!(entries[0].plan_type.as_deref(), Some("free"));
    }

    #[test]
    fn applies_gemini_cli_project_id_hint_to_auth_config() {
        let entries = parse_admin_provider_oauth_batch_import_entries(
            "gemini_cli",
            r#"{"refreshToken":"rt-1","cloudaicompanionProject":{"id":"project-gemini-cli-2"}}"#,
        );
        let mut auth_config = serde_json::Map::new();

        apply_admin_provider_oauth_batch_import_hints("gemini_cli", &entries[0], &mut auth_config);

        assert_eq!(
            auth_config.get("project_id"),
            Some(&json!("project-gemini-cli-2"))
        );
    }

    #[test]
    fn parses_windsurf_json_credentials_for_native_import() {
        let entries = parse_admin_provider_oauth_batch_import_entries(
            "windsurf",
            r#"[
                {"api_key":"devin-session-token$abc","email":"a@example.com"},
                {"token":"firebase-id-token","name":"Browser Login"},
                {"email":"b@example.com","password":"secret"},
                {"access_token":"devin-session-token$alias","email":"c@example.com"}
            ]"#,
        );

        assert_eq!(entries.len(), 4);
        assert_eq!(
            entries[0].refresh_token.as_deref(),
            Some("devin-session-token$abc")
        );
        assert_eq!(entries[0].email.as_deref(), Some("a@example.com"));
        assert_eq!(
            entries[0]
                .raw_credentials
                .as_ref()
                .and_then(|value| value.get("api_key")),
            Some(&json!("devin-session-token$abc"))
        );
        assert_eq!(
            entries[1]
                .raw_credentials
                .as_ref()
                .and_then(|value| value.get("token")),
            Some(&json!("firebase-id-token"))
        );
        assert_eq!(
            entries[2]
                .raw_credentials
                .as_ref()
                .and_then(|value| value.get("password")),
            Some(&json!("secret"))
        );
        assert_eq!(
            entries[3].access_token.as_deref(),
            Some("devin-session-token$alias")
        );
    }

    #[test]
    fn parses_windsurf_json_lines_credentials_for_native_import() {
        let entries = parse_admin_provider_oauth_batch_import_entries(
            "windsurf",
            r#"{"api_key":"devin-session-token$abc","email":"a@example.com"}
{"token":"firebase-id-token","name":"Browser Login"}
{"email":"b@example.com","password":"secret"}"#,
        );

        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[0]
                .raw_credentials
                .as_ref()
                .and_then(|value| value.get("api_key")),
            Some(&json!("devin-session-token$abc"))
        );
        assert_eq!(
            entries[1]
                .raw_credentials
                .as_ref()
                .and_then(|value| value.get("token")),
            Some(&json!("firebase-id-token"))
        );
        assert_eq!(
            entries[2]
                .raw_credentials
                .as_ref()
                .and_then(|value| value.get("password")),
            Some(&json!("secret"))
        );
    }

    #[test]
    fn invalid_json_line_is_parse_error_not_token() {
        let entries = parse_admin_provider_oauth_batch_import_entries(
            "windsurf",
            r#"{"email":"b@example.com","password":"secret""#,
        );

        assert_eq!(entries.len(), 1);
        assert!(entries[0].parse_error.is_some());
        assert!(entries[0].refresh_token.is_none());
        assert!(entries[0].access_token.is_none());
        assert!(entries[0].raw_credentials.is_none());
    }

    #[test]
    fn json_like_line_after_token_is_parse_error_not_token() {
        let entries = parse_admin_provider_oauth_batch_import_entries(
            "windsurf",
            "devin-session-token$abc\n[not-json",
        );

        assert_eq!(entries.len(), 2);
        assert!(entries[0].parse_error.is_none());
        assert_eq!(
            entries[0].refresh_token.as_deref(),
            Some("devin-session-token$abc")
        );
        assert!(entries[1].parse_error.is_some());
        assert!(entries[1].refresh_token.is_none());
        assert!(entries[1].access_token.is_none());
        assert!(entries[1].raw_credentials.is_none());
    }
}
