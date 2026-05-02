use super::super::token_import::{import_tokens_from_raw_token, normalize_single_import_tokens};
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
    pub refresh_token: Option<String>,
    pub access_token: Option<String>,
    pub expires_at: Option<u64>,
    pub account_id: Option<String>,
    pub account_user_id: Option<String>,
    pub plan_type: Option<String>,
    pub user_id: Option<String>,
    pub email: Option<String>,
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

fn extract_admin_provider_oauth_batch_import_entry(
    item: &serde_json::Value,
) -> Option<AdminProviderOAuthBatchImportEntry> {
    match item {
        serde_json::Value::String(value) => {
            let refresh_token = value.trim();
            if refresh_token.is_empty() {
                None
            } else {
                let (refresh_token, access_token) = import_tokens_from_raw_token(refresh_token);
                Some(AdminProviderOAuthBatchImportEntry {
                    refresh_token,
                    access_token,
                    expires_at: None,
                    account_id: None,
                    account_user_id: None,
                    plan_type: None,
                    user_id: None,
                    email: None,
                })
            }
        }
        serde_json::Value::Object(object) => {
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
            let (refresh_token, access_token) =
                normalize_single_import_tokens(refresh_token.as_deref(), access_token.as_deref());
            if refresh_token.is_none() && access_token.is_none() {
                return None;
            }
            let expires_at =
                json_u64_value(object.get("expires_at").or_else(|| object.get("expiresAt")));
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
            let user_id = coerce_admin_provider_oauth_import_str(
                object
                    .get("user_id")
                    .or_else(|| object.get("userId"))
                    .or_else(|| object.get("chatgpt_user_id"))
                    .or_else(|| object.get("chatgptUserId")),
            );
            let email = coerce_admin_provider_oauth_import_str(object.get("email"));
            Some(AdminProviderOAuthBatchImportEntry {
                refresh_token,
                access_token,
                expires_at,
                account_id,
                account_user_id,
                plan_type,
                user_id,
                email,
            })
        }
        _ => None,
    }
}

pub(super) fn parse_admin_provider_oauth_batch_import_entries(
    raw_credentials: &str,
) -> Vec<AdminProviderOAuthBatchImportEntry> {
    let raw = raw_credentials.trim();
    if raw.is_empty() {
        return Vec::new();
    }

    if raw.starts_with('[') {
        if let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(raw)
        {
            return items
                .iter()
                .filter_map(extract_admin_provider_oauth_batch_import_entry)
                .collect();
        }
    }

    if raw.starts_with('{') {
        if let Ok(value @ serde_json::Value::Object(_)) =
            serde_json::from_str::<serde_json::Value>(raw)
        {
            return extract_admin_provider_oauth_batch_import_entry(&value)
                .into_iter()
                .collect();
        }
    }

    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|token| {
            let (refresh_token, access_token) = import_tokens_from_raw_token(token);
            AdminProviderOAuthBatchImportEntry {
                refresh_token,
                access_token,
                expires_at: None,
                account_id: None,
                account_user_id: None,
                plan_type: None,
                user_id: None,
                email: None,
            }
        })
        .collect()
}

pub(super) fn apply_admin_provider_oauth_batch_import_hints(
    provider_type: &str,
    entry: &AdminProviderOAuthBatchImportEntry,
    auth_config: &mut serde_json::Map<String, serde_json::Value>,
) {
    if !provider_type.eq_ignore_ascii_case("codex") {
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
    use super::parse_admin_provider_oauth_batch_import_entries;
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
    fn parses_plain_jwt_line_as_access_token() {
        let token = unsigned_jwt(json!({
            "iss": "https://auth.openai.com",
            "aud": ["https://api.openai.com/v1"],
            "exp": 2_000_000_000u64,
        }));

        let entries = parse_admin_provider_oauth_batch_import_entries(&token);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].refresh_token, None);
        assert_eq!(entries[0].access_token.as_deref(), Some(token.as_str()));
    }
}
