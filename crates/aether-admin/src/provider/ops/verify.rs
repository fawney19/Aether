use super::architectures::normalize_architecture_id;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use http::StatusCode;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{json, Map, Value};

const ADMIN_PROVIDER_OPS_ANYROUTER_XOR_KEY: &str = "3000176000856006061501533003690027800375";
const ADMIN_PROVIDER_OPS_ANYROUTER_UNSBOX_TABLE: [usize; 40] = [
    0xF, 0x23, 0x1D, 0x18, 0x21, 0x10, 0x1, 0x26, 0xA, 0x9, 0x13, 0x1F, 0x28, 0x1B, 0x16, 0x17,
    0x19, 0xD, 0x6, 0xB, 0x27, 0x12, 0x14, 0x8, 0xE, 0x15, 0x20, 0x1A, 0x2, 0x1E, 0x7, 0x4, 0x11,
    0x5, 0x3, 0x1C, 0x22, 0x25, 0xC, 0x24,
];
pub const ADMIN_PROVIDER_OPS_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.7339.249 Electron/38.7.0 Safari/537.36";

pub fn build_headers(
    architecture_id: &str,
    config: &Map<String, Value>,
    credentials: &Map<String, Value>,
) -> Result<HeaderMap, String> {
    admin_provider_ops_verify_headers(architecture_id, config, credentials)
}

pub fn parse_verify_payload(
    architecture_id: &str,
    status: StatusCode,
    response_json: &Value,
    updated_credentials: Option<Map<String, Value>>,
) -> Value {
    match normalize_architecture_id(architecture_id) {
        "anyrouter" => admin_provider_ops_anyrouter_verify_payload(status, response_json),
        "cubence" => admin_provider_ops_cubence_verify_payload(status, response_json),
        "done_hub" => admin_provider_ops_anyrouter_verify_payload(status, response_json),
        "yescode" => admin_provider_ops_yescode_verify_payload(status, response_json),
        "nekocode" => admin_provider_ops_nekocode_verify_payload(status, response_json),
        "sub2api" => {
            admin_provider_ops_sub2api_verify_payload(status, response_json, updated_credentials)
        }
        _ => admin_provider_ops_generic_verify_payload(status, response_json),
    }
}

pub fn admin_provider_ops_extract_cookie_value(cookie_input: &str, key: &str) -> String {
    if cookie_input.contains(&format!("{key}=")) {
        for part in cookie_input.split(';') {
            let trimmed = part.trim();
            if let Some(value) = trimmed.strip_prefix(&format!("{key}=")) {
                return value.trim().to_string();
            }
        }
    }
    cookie_input.trim().to_string()
}

fn admin_provider_ops_strip_cookie_header_prefix(cookie_input: &str) -> &str {
    let trimmed = cookie_input.trim();
    if trimmed
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cookie:"))
    {
        return trimmed[7..].trim();
    }
    trimmed
}

fn admin_provider_ops_is_set_cookie_attribute(name: &str) -> bool {
    matches!(
        name,
        "path"
            | "domain"
            | "expires"
            | "max-age"
            | "secure"
            | "httponly"
            | "samesite"
            | "partitioned"
            | "priority"
    )
}

fn admin_provider_ops_cubence_cookie_header(cookie_input: &str) -> String {
    let trimmed = admin_provider_ops_strip_cookie_header_prefix(cookie_input);
    if trimmed.is_empty() {
        return String::new();
    }
    if !trimmed.contains('=') {
        return format!("token={trimmed}");
    }

    let cookies = trimmed
        .split(';')
        .filter_map(|part| {
            let part = part.trim();
            let (name, value) = part.split_once('=')?;
            let name = name.trim();
            let value = value.trim();
            if name.is_empty() || value.is_empty() {
                return None;
            }
            let lower = name.to_ascii_lowercase();
            if admin_provider_ops_is_set_cookie_attribute(&lower) {
                return None;
            }
            Some(format!("{name}={value}"))
        })
        .collect::<Vec<_>>();

    if cookies.is_empty() {
        let token = admin_provider_ops_extract_cookie_value(trimmed, "token");
        return format!("token={token}");
    }

    cookies.join("; ")
}

fn admin_provider_ops_session_cookie_header(cookie_input: &str) -> String {
    let trimmed = admin_provider_ops_strip_cookie_header_prefix(cookie_input);
    let session = admin_provider_ops_extract_cookie_value(trimmed, "session");
    format!("session={session}")
}

pub fn admin_provider_ops_yescode_cookie_header(cookie_input: &str) -> String {
    if cookie_input.contains("yescode_auth=") {
        let mut parts = Vec::new();
        for part in cookie_input.split(';') {
            let trimmed = part.trim();
            if let Some(value) = trimmed.strip_prefix("yescode_auth=") {
                parts.push(format!("yescode_auth={}", value.trim()));
            } else if let Some(value) = trimmed.strip_prefix("yescode_csrf=") {
                parts.push(format!("yescode_csrf={}", value.trim()));
            }
        }
        return parts.join("; ");
    }
    format!("yescode_auth={}", cookie_input.trim())
}

pub fn admin_provider_ops_anyrouter_compute_acw_sc_v2(arg1: &str) -> Option<String> {
    if arg1.len() != 40 || !arg1.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let chars = arg1.chars().collect::<Vec<_>>();
    let unsboxed = ADMIN_PROVIDER_OPS_ANYROUTER_UNSBOX_TABLE
        .iter()
        .map(|index| chars.get(index.saturating_sub(1)).copied())
        .collect::<Option<String>>()?;

    let mut result = String::with_capacity(40);
    for i in (0..40).step_by(2) {
        let a = u8::from_str_radix(&unsboxed[i..i + 2], 16).ok()?;
        let b = u8::from_str_radix(&ADMIN_PROVIDER_OPS_ANYROUTER_XOR_KEY[i..i + 2], 16).ok()?;
        result.push_str(&format!("{:02x}", a ^ b));
    }
    Some(result)
}

pub fn admin_provider_ops_anyrouter_parse_session_user_id(cookie_input: &str) -> Option<String> {
    let session_cookie = admin_provider_ops_extract_cookie_value(cookie_input, "session");
    let decoded = decode_python_urlsafe_b64(&session_cookie)?;
    let text = String::from_utf8_lossy(&decoded);
    let mut parts = text.split('|');
    let _timestamp = parts.next()?;
    let gob_b64 = parts.next()?;
    let gob_data = decode_python_urlsafe_b64(gob_b64)?;

    let id_pattern = b"\x02id\x03int";
    let id_idx = gob_data
        .windows(id_pattern.len())
        .position(|window| window == id_pattern)?;
    let value_start = id_idx + id_pattern.len() + 2;
    let first_byte = *gob_data.get(value_start)?;
    if first_byte != 0 {
        return None;
    }
    let marker = *gob_data.get(value_start + 1)?;
    if marker < 0x80 {
        return None;
    }
    let length = 256usize.saturating_sub(marker as usize);
    let end = value_start + 2 + length;
    let bytes = gob_data.get(value_start + 2..end)?;
    let val = bytes
        .iter()
        .fold(0u64, |acc, byte| (acc << 8) | (*byte as u64));
    Some((val >> 1).to_string())
}

fn decode_python_urlsafe_b64(input: &str) -> Option<Vec<u8>> {
    let normalized = input.trim().replace('-', "+").replace('_', "/");
    if normalized.is_empty() {
        return None;
    }
    let remainder = normalized.len() % 4;
    let mut padded = normalized;
    if remainder != 0 {
        padded.push_str(&"=".repeat(4 - remainder));
    }
    STANDARD.decode(padded.as_bytes()).ok()
}

pub fn admin_provider_ops_verify_failure(message: impl Into<String>) -> Value {
    json!({
        "success": false,
        "message": message.into(),
    })
}

pub fn admin_provider_ops_verify_success(
    data: Value,
    updated_credentials: Option<Map<String, Value>>,
) -> Value {
    let mut payload = Map::from_iter([
        ("success".to_string(), Value::Bool(true)),
        ("message".to_string(), Value::Null),
        ("data".to_string(), data),
        (
            "updated_credentials".to_string(),
            updated_credentials
                .clone()
                .map(Value::Object)
                .unwrap_or(Value::Null),
        ),
    ]);
    if updated_credentials
        .as_ref()
        .is_some_and(|value| value.is_empty())
    {
        payload.insert("updated_credentials".to_string(), Value::Null);
    }
    Value::Object(payload)
}

pub fn admin_provider_ops_verify_user_payload(
    username: Option<String>,
    display_name: Option<String>,
    email: Option<String>,
    quota: Option<f64>,
    extra: Option<Map<String, Value>>,
) -> Value {
    let resolved_username = username.filter(|value| !value.trim().is_empty());
    let resolved_display_name = display_name
        .filter(|value| !value.trim().is_empty())
        .or_else(|| resolved_username.clone());
    let mut payload = Map::new();
    payload.insert(
        "username".to_string(),
        resolved_username.map(Value::String).unwrap_or(Value::Null),
    );
    payload.insert(
        "display_name".to_string(),
        resolved_display_name
            .map(Value::String)
            .unwrap_or(Value::Null),
    );
    payload.insert(
        "email".to_string(),
        email.map(Value::String).unwrap_or(Value::Null),
    );
    payload.insert(
        "quota".to_string(),
        quota
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or(Value::Null),
    );
    payload.insert(
        "extra".to_string(),
        Value::Object(extra.unwrap_or_default()),
    );
    Value::Object(payload)
}

pub fn admin_provider_ops_verify_user_payload_with_usage(
    username: Option<String>,
    display_name: Option<String>,
    email: Option<String>,
    quota: Option<f64>,
    used_quota: Option<f64>,
    request_count: Option<u64>,
    extra: Option<Map<String, Value>>,
) -> Value {
    let mut payload =
        admin_provider_ops_verify_user_payload(username, display_name, email, quota, extra)
            .as_object()
            .cloned()
            .unwrap_or_default();
    payload.insert(
        "used_quota".to_string(),
        used_quota
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or(Value::Null),
    );
    payload.insert(
        "request_count".to_string(),
        request_count
            .map(serde_json::Number::from)
            .map(Value::Number)
            .unwrap_or(Value::Null),
    );
    Value::Object(payload)
}

pub fn admin_provider_ops_value_as_f64(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(raw)) => raw.trim().parse::<f64>().ok(),
        _ => None,
    }
}

pub fn admin_provider_ops_value_as_u64(value: Option<&Value>) -> Option<u64> {
    match value {
        Some(Value::Number(number)) => number.as_u64().or_else(|| {
            number
                .as_i64()
                .filter(|value| *value >= 0)
                .map(|value| value as u64)
        }),
        Some(Value::String(raw)) => raw.trim().parse::<u64>().ok(),
        _ => None,
    }
}

pub fn admin_provider_ops_json_object(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

pub fn admin_provider_ops_frontend_updated_credentials(
    credentials: Map<String, Value>,
) -> Option<Map<String, Value>> {
    let filtered = credentials
        .into_iter()
        .filter(|(key, value)| {
            !key.starts_with('_')
                && !matches!(value, Value::Null)
                && !value.as_str().is_some_and(|raw| raw.trim().is_empty())
        })
        .collect::<Map<String, Value>>();
    (!filtered.is_empty()).then_some(filtered)
}

fn insert_header(headers: &mut HeaderMap, name: &str, value: &str) -> Result<(), String> {
    let header_name =
        HeaderName::from_bytes(name.as_bytes()).map_err(|_| format!("无效的请求头: {name}"))?;
    let header_value =
        HeaderValue::from_str(value).map_err(|_| format!("无效的请求头值: {name}"))?;
    headers.insert(header_name, header_value);
    Ok(())
}

pub fn admin_provider_ops_verify_headers(
    architecture_id: &str,
    config: &Map<String, Value>,
    credentials: &Map<String, Value>,
) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    match normalize_architecture_id(architecture_id) {
        "generic_api" => {
            let api_key = credentials
                .get("api_key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if !api_key.is_empty() {
                let auth_method = config
                    .get("auth_method")
                    .and_then(Value::as_str)
                    .unwrap_or("bearer");
                if auth_method == "header" {
                    let header_name = config
                        .get("header_name")
                        .and_then(Value::as_str)
                        .unwrap_or("X-API-Key");
                    insert_header(&mut headers, header_name, api_key)?;
                } else {
                    insert_header(&mut headers, "Authorization", &format!("Bearer {api_key}"))?;
                }
            }
        }
        "new_api" => {
            for (name, value) in [
                (
                    "User-Agent",
                    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.7339.249 Electron/38.7.0 Safari/537.36",
                ),
                ("Accept", "application/json"),
                ("Accept-Language", "zh-CN"),
                ("sec-ch-ua", "\"Not=A?Brand\";v=\"24\", \"Chromium\";v=\"140\""),
                ("sec-ch-ua-mobile", "?0"),
                ("sec-ch-ua-platform", "\"macOS\""),
                ("Sec-Fetch-Site", "cross-site"),
                ("Sec-Fetch-Mode", "cors"),
                ("Sec-Fetch-Dest", "empty"),
            ] {
                insert_header(&mut headers, name, value)?;
            }
            if let Some(api_key) = credentials.get("api_key").and_then(Value::as_str) {
                if !api_key.trim().is_empty() {
                    insert_header(
                        &mut headers,
                        "Authorization",
                        &format!("Bearer {}", api_key.trim()),
                    )?;
                }
            }
            if let Some(user_id) = credentials.get("user_id").and_then(Value::as_str) {
                if !user_id.trim().is_empty() {
                    insert_header(&mut headers, "New-Api-User", user_id.trim())?;
                }
            }
            if let Some(cookie) = credentials.get("cookie").and_then(Value::as_str) {
                if !cookie.trim().is_empty() {
                    insert_header(&mut headers, "Cookie", cookie.trim())?;
                }
            }
        }
        "cubence" => {
            insert_header(&mut headers, "User-Agent", ADMIN_PROVIDER_OPS_USER_AGENT)?;
            if let Some(token_cookie) = credentials
                .get("token_cookie")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                let cookie_header = admin_provider_ops_cubence_cookie_header(token_cookie);
                if !cookie_header.is_empty() {
                    insert_header(&mut headers, "Cookie", &cookie_header)?;
                }
            }
        }
        "yescode" => {
            if let Some(auth_cookie) = credentials
                .get("auth_cookie")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                insert_header(
                    &mut headers,
                    "Cookie",
                    &admin_provider_ops_yescode_cookie_header(auth_cookie),
                )?;
            }
        }
        "nekocode" | "done_hub" => {
            if let Some(session_cookie) = credentials
                .get("session_cookie")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                insert_header(
                    &mut headers,
                    "Cookie",
                    &admin_provider_ops_session_cookie_header(session_cookie),
                )?;
            }
        }
        "anyrouter" => {
            let mut cookies = Vec::new();
            insert_header(&mut headers, "User-Agent", ADMIN_PROVIDER_OPS_USER_AGENT)?;
            if let Some(acw_cookie) = config
                .get("acw_cookie")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                cookies.push(acw_cookie.to_string());
            }
            if let Some(session_cookie) = credentials
                .get("session_cookie")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                let session = admin_provider_ops_extract_cookie_value(session_cookie, "session");
                cookies.push(format!("session={session}"));
                if let Some(user_id) =
                    admin_provider_ops_anyrouter_parse_session_user_id(session_cookie)
                {
                    insert_header(&mut headers, "New-Api-User", user_id.trim())?;
                }
            }
            if !cookies.is_empty() {
                insert_header(&mut headers, "Cookie", &cookies.join("; "))?;
            }
        }
        _ => {}
    }
    Ok(headers)
}

pub fn admin_provider_ops_generic_verify_payload(
    status: StatusCode,
    response_json: &Value,
) -> Value {
    verify_payload_with_auth_messages(
        status,
        response_json,
        "认证失败：无效的凭据",
        "认证失败：权限不足",
    )
}

pub fn admin_provider_ops_anyrouter_verify_payload(
    status: StatusCode,
    response_json: &Value,
) -> Value {
    verify_payload_with_auth_messages(
        status,
        response_json,
        "Cookie 已失效，请重新配置",
        "Cookie 已失效或无权限",
    )
}

fn verify_payload_with_auth_messages(
    status: StatusCode,
    response_json: &Value,
    unauthorized_message: &str,
    forbidden_message: &str,
) -> Value {
    if status == StatusCode::UNAUTHORIZED {
        return admin_provider_ops_verify_failure(unauthorized_message);
    }
    if status == StatusCode::FORBIDDEN {
        return admin_provider_ops_verify_failure(forbidden_message);
    }
    if status != StatusCode::OK {
        return admin_provider_ops_verify_failure(format!("验证失败：HTTP {}", status.as_u16()));
    }

    let user_data = if response_json.get("success").and_then(Value::as_bool) == Some(true)
        && response_json.get("data").is_some_and(Value::is_object)
    {
        response_json.get("data")
    } else if response_json.get("success").and_then(Value::as_bool) == Some(false) {
        return admin_provider_ops_verify_failure(
            response_json
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("验证失败"),
        );
    } else {
        Some(response_json)
    };

    let Some(user_data) = user_data.and_then(admin_provider_ops_json_object) else {
        return admin_provider_ops_verify_failure("响应格式无效");
    };

    let mut extra = Map::new();
    for (key, value) in user_data {
        if matches!(
            key.as_str(),
            "username" | "display_name" | "email" | "quota" | "used_quota" | "request_count"
        ) {
            continue;
        }
        extra.insert(key.clone(), value.clone());
    }

    admin_provider_ops_verify_success(
        admin_provider_ops_verify_user_payload_with_usage(
            user_data
                .get("username")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            user_data
                .get("display_name")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            user_data
                .get("email")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            admin_provider_ops_value_as_f64(user_data.get("quota")),
            admin_provider_ops_value_as_f64(user_data.get("used_quota")),
            admin_provider_ops_value_as_u64(user_data.get("request_count")),
            Some(extra),
        ),
        None,
    )
}

pub fn admin_provider_ops_cubence_verify_payload(
    status: StatusCode,
    response_json: &Value,
) -> Value {
    if status == StatusCode::UNAUTHORIZED {
        return admin_provider_ops_verify_failure("Cookie 已失效，请重新配置");
    }
    if status == StatusCode::FORBIDDEN {
        return admin_provider_ops_verify_failure("Cookie 已失效或无权限");
    }
    if status != StatusCode::OK {
        return admin_provider_ops_verify_failure(format!("验证失败：HTTP {}", status.as_u16()));
    }

    let payload = if response_json.get("success").and_then(Value::as_bool) == Some(true)
        && response_json.get("data").is_some_and(Value::is_object)
    {
        response_json.get("data")
    } else if response_json.get("success").and_then(Value::as_bool) == Some(false) {
        return admin_provider_ops_verify_failure(
            response_json
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("验证失败"),
        );
    } else {
        Some(response_json)
    };

    let Some(payload) = payload.and_then(admin_provider_ops_json_object) else {
        return admin_provider_ops_verify_failure("响应格式无效");
    };
    let user_info = payload
        .get("user")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let balance_info = payload
        .get("balance")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut extra = Map::new();
    if let Some(role) = user_info.get("role") {
        extra.insert("role".to_string(), role.clone());
    }
    if let Some(invite_code) = user_info.get("invite_code") {
        extra.insert("invite_code".to_string(), invite_code.clone());
    }

    admin_provider_ops_verify_success(
        admin_provider_ops_verify_user_payload(
            user_info
                .get("username")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            user_info
                .get("username")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            None,
            admin_provider_ops_value_as_f64(balance_info.get("total_balance_dollar")),
            Some(extra),
        ),
        None,
    )
}

pub fn admin_provider_ops_yescode_verify_payload(
    status: StatusCode,
    response_json: &Value,
) -> Value {
    if status == StatusCode::UNAUTHORIZED {
        return admin_provider_ops_verify_failure("Cookie 已失效，请重新配置");
    }
    if status == StatusCode::FORBIDDEN {
        return admin_provider_ops_verify_failure("Cookie 已失效或无权限");
    }
    if status != StatusCode::OK {
        return admin_provider_ops_verify_failure(format!("验证失败：HTTP {}", status.as_u16()));
    }

    let Some(payload) = admin_provider_ops_json_object(response_json) else {
        return admin_provider_ops_verify_failure("响应格式无效");
    };
    let Some(username) = payload
        .get("username")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return admin_provider_ops_verify_failure("响应格式无效");
    };

    let pay_as_you_go =
        admin_provider_ops_value_as_f64(payload.get("pay_as_you_go_balance")).unwrap_or(0.0);
    let subscription =
        admin_provider_ops_value_as_f64(payload.get("subscription_balance")).unwrap_or(0.0);
    let plan = payload
        .get("subscription_plan")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let weekly_limit = admin_provider_ops_value_as_f64(
        payload
            .get("weekly_limit")
            .or_else(|| plan.get("weekly_limit")),
    );
    let weekly_spent = admin_provider_ops_value_as_f64(
        payload
            .get("weekly_spent_balance")
            .or_else(|| payload.get("current_week_spend")),
    )
    .unwrap_or(0.0);
    let subscription_available = weekly_limit
        .map(|limit| (limit - weekly_spent).max(0.0).min(subscription))
        .unwrap_or(subscription);

    admin_provider_ops_verify_success(
        admin_provider_ops_verify_user_payload(
            Some(username.clone()),
            Some(username),
            payload
                .get("email")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            Some(pay_as_you_go + subscription_available),
            None,
        ),
        None,
    )
}

pub fn admin_provider_ops_nekocode_verify_payload(
    status: StatusCode,
    response_json: &Value,
) -> Value {
    if status == StatusCode::UNAUTHORIZED {
        return admin_provider_ops_verify_failure("Cookie 已失效，请重新配置");
    }
    if status == StatusCode::FORBIDDEN {
        return admin_provider_ops_verify_failure("Cookie 已失效或无权限");
    }
    if status != StatusCode::OK {
        return admin_provider_ops_verify_failure(format!("验证失败：HTTP {}", status.as_u16()));
    }

    let user_data = if response_json.get("success").and_then(Value::as_bool) == Some(true)
        && response_json.get("data").is_some_and(Value::is_object)
    {
        response_json.get("data")
    } else {
        Some(response_json)
    };
    let Some(user_data) = user_data.and_then(admin_provider_ops_json_object) else {
        return admin_provider_ops_verify_failure("响应格式无效");
    };

    admin_provider_ops_verify_success(
        admin_provider_ops_verify_user_payload(
            user_data
                .get("username")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            user_data
                .get("display_name")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            user_data
                .get("email")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            admin_provider_ops_value_as_f64(user_data.get("balance")),
            None,
        ),
        None,
    )
}

pub fn admin_provider_ops_sub2api_verify_payload(
    status: StatusCode,
    response_json: &Value,
    updated_credentials: Option<Map<String, Value>>,
) -> Value {
    if status == StatusCode::UNAUTHORIZED {
        return admin_provider_ops_verify_failure("认证失败：无效的凭据");
    }
    if status == StatusCode::FORBIDDEN {
        return admin_provider_ops_verify_failure("认证失败：权限不足");
    }
    if status != StatusCode::OK {
        return admin_provider_ops_verify_failure(format!("验证失败：HTTP {}", status.as_u16()));
    }

    let Some(payload) = admin_provider_ops_json_object(response_json) else {
        return admin_provider_ops_verify_failure("响应格式无效");
    };
    if payload.get("code").and_then(Value::as_i64).unwrap_or(-1) != 0 {
        return admin_provider_ops_verify_failure(
            payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("验证失败"),
        );
    }

    let Some(user_data) = payload.get("data").and_then(Value::as_object) else {
        return admin_provider_ops_verify_failure("响应格式无效");
    };
    let balance = admin_provider_ops_value_as_f64(user_data.get("balance")).unwrap_or(0.0);
    let points = admin_provider_ops_value_as_f64(user_data.get("points")).unwrap_or(0.0);
    let mut extra = Map::new();
    for key in ["balance", "points", "status", "concurrency"] {
        if let Some(value) = user_data.get(key) {
            extra.insert(key.to_string(), value.clone());
        }
    }

    let username_or_email = admin_provider_ops_sub2api_non_empty_string(user_data, "username")
        .or_else(|| admin_provider_ops_sub2api_non_empty_string(user_data, "email"));
    admin_provider_ops_verify_success(
        admin_provider_ops_verify_user_payload(
            username_or_email.clone(),
            username_or_email,
            admin_provider_ops_sub2api_non_empty_string(user_data, "email"),
            Some(balance + points),
            Some(extra),
        ),
        updated_credentials,
    )
}

fn admin_provider_ops_sub2api_non_empty_string(
    map: &Map<String, Value>,
    key: &str,
) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{
        admin_provider_ops_anyrouter_compute_acw_sc_v2,
        admin_provider_ops_anyrouter_parse_session_user_id,
        admin_provider_ops_anyrouter_verify_payload, admin_provider_ops_cubence_verify_payload,
        admin_provider_ops_frontend_updated_credentials, admin_provider_ops_sub2api_verify_payload,
        admin_provider_ops_verify_headers, parse_verify_payload, ADMIN_PROVIDER_OPS_USER_AGENT,
    };
    use http::StatusCode;
    use reqwest::header::COOKIE;
    use reqwest::header::USER_AGENT;
    use serde_json::{json, Map, Value};

    #[test]
    fn anyrouter_compute_acw_sc_v2_matches_python_algorithm() {
        let actual = admin_provider_ops_anyrouter_compute_acw_sc_v2(
            "0123456789abcdef0123456789abcdef01234567",
        );
        assert_eq!(
            actual.as_deref(),
            Some("d2c7186598ab1a508a4f6064e4fa746323ab17c6")
        );
    }

    #[test]
    fn anyrouter_parse_session_user_id_extracts_numeric_id() {
        let actual = admin_provider_ops_anyrouter_parse_session_user_id(
            "session=MTIzfGVIaDRlQUpwWkFOcGJuU3F1d0RfVkhsNWVYa0lkWE5sY201aGJXVUdjM1J5YVc1bkRCQUFCV0ZzYVdObHxzaWc",
        );
        assert_eq!(actual.as_deref(), Some("42"));
    }

    #[test]
    fn anyrouter_parse_session_user_id_accepts_padded_urlsafe_base64() {
        let actual = admin_provider_ops_anyrouter_parse_session_user_id(
            "session=MTIzfGVIaDRlQUpwWkFOcGJuU3F1d0RfVkhsNWVYa0lkWE5sY201aGJXVUdjM1J5YVc1bkRCQUFCV0ZzYVdObHxzaWc=",
        );
        assert_eq!(actual.as_deref(), Some("42"));
    }

    #[test]
    fn frontend_updated_credentials_omits_internal_runtime_fields() {
        let filtered = admin_provider_ops_frontend_updated_credentials(Map::from_iter([
            ("refresh_token".to_string(), json!("refresh-token")),
            ("_cached_access_token".to_string(), json!("access-token")),
            ("_cached_token_expires_at".to_string(), json!(123456.0)),
            ("password".to_string(), Value::Null),
        ]));

        assert_eq!(
            filtered,
            Some(Map::from_iter([(
                "refresh_token".to_string(),
                json!("refresh-token")
            )]))
        );
    }

    #[test]
    fn sub2api_verify_payload_sums_balance_and_points() {
        let payload = admin_provider_ops_sub2api_verify_payload(
            StatusCode::OK,
            &json!({
                "code": 0,
                "data": {
                    "username": "sub2api-user",
                    "email": "sub2api@example.com",
                    "balance": 8.5,
                    "points": 1.5,
                    "status": "active",
                    "concurrency": 4
                }
            }),
            Some(Map::from_iter([(
                "refresh_token".to_string(),
                json!("refresh-token-new"),
            )])),
        );

        assert_eq!(payload["success"], json!(true));
        assert_eq!(payload["data"]["username"], json!("sub2api-user"));
        assert_eq!(payload["data"]["quota"], json!(10.0));
        assert_eq!(payload["data"]["extra"]["balance"], json!(8.5));
        assert_eq!(payload["data"]["extra"]["points"], json!(1.5));
        assert_eq!(payload["data"]["extra"]["status"], json!("active"));
        assert_eq!(payload["data"]["extra"]["concurrency"], json!(4));
        assert_eq!(
            payload["updated_credentials"],
            json!({ "refresh_token": "refresh-token-new" })
        );
    }

    #[test]
    fn sub2api_verify_payload_falls_back_to_email_when_username_is_null() {
        let payload = admin_provider_ops_sub2api_verify_payload(
            StatusCode::OK,
            &json!({
                "code": 0,
                "data": {
                    "username": null,
                    "email": "user@example.com",
                    "balance": 2.0,
                    "points": 0.0
                }
            }),
            None,
        );

        assert_eq!(payload["success"], json!(true));
        assert_eq!(payload["data"]["username"], json!("user@example.com"));
        assert_eq!(payload["data"]["display_name"], json!("user@example.com"));
        assert_eq!(payload["data"]["email"], json!("user@example.com"));
    }

    #[test]
    fn anyrouter_verify_payload_uses_cookie_auth_messages_and_usage_fields() {
        let payload = admin_provider_ops_anyrouter_verify_payload(
            StatusCode::OK,
            &json!({
                "id": 42,
                "username": "alice",
                "display_name": "Alice",
                "email": "alice@example.com",
                "quota": 7.5,
                "used_quota": 1.25,
                "request_count": 8
            }),
        );

        assert_eq!(payload["success"], json!(true));
        assert_eq!(payload["data"]["quota"], json!(7.5));
        assert_eq!(payload["data"]["used_quota"], json!(1.25));
        assert_eq!(payload["data"]["request_count"], json!(8));

        let auth_failed =
            admin_provider_ops_anyrouter_verify_payload(StatusCode::UNAUTHORIZED, &json!({}));
        assert_eq!(auth_failed["success"], json!(false));
        assert_eq!(auth_failed["message"], json!("Cookie 已失效，请重新配置"));
    }

    #[test]
    fn done_hub_verify_payload_reads_wrapped_profile() {
        let payload = parse_verify_payload(
            "done_hub",
            StatusCode::OK,
            &json!({
                "success": true,
                "data": {
                    "username": "linuxdo_850",
                    "display_name": "AAEE86",
                    "quota": 2_276_139_911_u64,
                    "used_quota": 13860089,
                    "request_count": 55
                }
            }),
            None,
        );

        assert_eq!(payload["success"], json!(true));
        assert_eq!(payload["data"]["username"], json!("linuxdo_850"));
        assert_eq!(payload["data"]["display_name"], json!("AAEE86"));
        assert_eq!(payload["data"]["quota"], json!(2276139911.0));
        assert_eq!(payload["data"]["used_quota"], json!(13860089.0));
        assert_eq!(payload["data"]["request_count"], json!(55));
    }

    #[test]
    fn done_hub_verify_headers_use_session_cookie_only() {
        let headers = admin_provider_ops_verify_headers(
            "done_hub",
            &Map::new(),
            &Map::from_iter([(
                "session_cookie".to_string(),
                json!("Cookie: session=abc; other=ignored"),
            )]),
        )
        .expect("headers should build");

        assert_eq!(
            headers.get(COOKIE).and_then(|value| value.to_str().ok()),
            Some("session=abc")
        );
    }

    #[test]
    fn anyrouter_verify_headers_include_shared_user_agent() {
        let headers = admin_provider_ops_verify_headers(
            "anyrouter",
            &Map::from_iter([("acw_cookie".to_string(), json!("acw_sc__v2=test"))]),
            &Map::from_iter([(
                "session_cookie".to_string(),
                json!("session=MTIzfGVIaDRlQUpwWkFOcGJuU3F1d0RfVkhsNWVYa0lkWE5sY201aGJXVUdjM1J5YVc1bkRCQUFCV0ZzYVdObHxzaWc="),
            )]),
        )
        .expect("headers should build");

        assert_eq!(
            headers
                .get(USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            Some(ADMIN_PROVIDER_OPS_USER_AGENT)
        );
        assert_eq!(
            headers
                .get("New-Api-User")
                .and_then(|value| value.to_str().ok()),
            Some("42")
        );
    }

    #[test]
    fn cubence_verify_headers_preserve_full_cookie_header() {
        let headers = admin_provider_ops_verify_headers(
            "cubence",
            &Map::new(),
            &Map::from_iter([(
                "token_cookie".to_string(),
                json!("Cookie: token=abc; cf_clearance=def; Path=/; HttpOnly"),
            )]),
        )
        .expect("headers should build");

        assert_eq!(
            headers.get(COOKIE).and_then(|value| value.to_str().ok()),
            Some("token=abc; cf_clearance=def")
        );
        assert_eq!(
            headers
                .get(USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            Some(ADMIN_PROVIDER_OPS_USER_AGENT)
        );
    }

    #[test]
    fn cubence_verify_payload_reads_wrapped_dashboard_overview() {
        let payload = admin_provider_ops_cubence_verify_payload(
            StatusCode::OK,
            &json!({
                "success": true,
                "data": {
                    "user": {
                        "username": "AAEE86",
                        "role": "user",
                        "invite_code": "SCFSJ5C5"
                    },
                    "balance": {
                        "total_balance_dollar": 0.6
                    }
                }
            }),
        );

        assert_eq!(payload["success"], json!(true));
        assert_eq!(payload["data"]["username"], json!("AAEE86"));
        assert_eq!(payload["data"]["quota"], json!(0.6));
        assert_eq!(payload["data"]["extra"]["role"], json!("user"));
        assert_eq!(payload["data"]["extra"]["invite_code"], json!("SCFSJ5C5"));
    }
}
