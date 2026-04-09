use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use http::StatusCode;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{json, Map, Value};

const ADMIN_PROVIDER_OPS_ANYROUTER_XOR_KEY: &str = "3000176000856006061501533003690027800375";
const ADMIN_PROVIDER_OPS_ANYROUTER_UNSBOX_TABLE: [usize; 40] = [
    0xF, 0x23, 0x1D, 0x18, 0x21, 0x10, 0x1, 0x26, 0xA, 0x9, 0x13, 0x1F, 0x28, 0x1B, 0x16, 0x17,
    0x19, 0xD, 0x6, 0xB, 0x27, 0x12, 0x14, 0x8, 0xE, 0x15, 0x20, 0x1A, 0x2, 0x1E, 0x7, 0x4, 0x11,
    0x5, 0x3, 0x1C, 0x22, 0x25, 0xC, 0x24,
];

pub fn admin_provider_ops_normalized_verify_architecture_id(architecture_id: &str) -> &str {
    match architecture_id.trim() {
        "" => "generic_api",
        "generic_api" | "new_api" | "cubence" | "yescode" | "nekocode" | "anyrouter"
        | "sub2api" => architecture_id.trim(),
        _ => "generic_api",
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
    let decoded = URL_SAFE_NO_PAD.decode(session_cookie.as_bytes()).ok()?;
    let text = String::from_utf8_lossy(&decoded);
    let mut parts = text.split('|');
    let _timestamp = parts.next()?;
    let gob_b64 = parts.next()?;
    let gob_data = URL_SAFE_NO_PAD.decode(gob_b64.as_bytes()).ok()?;

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
        ("data".to_string(), data),
    ]);
    if let Some(credentials) = updated_credentials.filter(|value| !value.is_empty()) {
        payload.insert(
            "updated_credentials".to_string(),
            Value::Object(credentials),
        );
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
    if let Some(extra) = extra.filter(|value| !value.is_empty()) {
        payload.insert("extra".to_string(), Value::Object(extra));
    }
    Value::Object(payload)
}

pub fn admin_provider_ops_value_as_f64(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(raw)) => raw.trim().parse::<f64>().ok(),
        _ => None,
    }
}

pub fn admin_provider_ops_json_object(value: &Value) -> Option<&serde_json::Map<String, Value>> {
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

fn admin_provider_ops_insert_header(
    headers: &mut HeaderMap,
    name: &str,
    value: &str,
) -> Result<(), String> {
    let header_name =
        HeaderName::from_bytes(name.as_bytes()).map_err(|_| format!("无效的请求头: {name}"))?;
    let header_value =
        HeaderValue::from_str(value).map_err(|_| format!("无效的请求头值: {name}"))?;
    headers.insert(header_name, header_value);
    Ok(())
}

pub fn admin_provider_ops_verify_headers(
    architecture_id: &str,
    config: &serde_json::Map<String, Value>,
    credentials: &serde_json::Map<String, Value>,
) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    match architecture_id {
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
                    admin_provider_ops_insert_header(&mut headers, header_name, api_key)?;
                } else {
                    admin_provider_ops_insert_header(
                        &mut headers,
                        "Authorization",
                        &format!("Bearer {api_key}"),
                    )?;
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
                ("Accept-Encoding", "gzip, deflate, br"),
                ("Accept-Language", "zh-CN"),
                ("sec-ch-ua", "\"Not=A?Brand\";v=\"24\", \"Chromium\";v=\"140\""),
                ("sec-ch-ua-mobile", "?0"),
                ("sec-ch-ua-platform", "\"macOS\""),
                ("Sec-Fetch-Site", "cross-site"),
                ("Sec-Fetch-Mode", "cors"),
                ("Sec-Fetch-Dest", "empty"),
            ] {
                admin_provider_ops_insert_header(&mut headers, name, value)?;
            }
            if let Some(api_key) = credentials.get("api_key").and_then(Value::as_str) {
                if !api_key.trim().is_empty() {
                    admin_provider_ops_insert_header(
                        &mut headers,
                        "Authorization",
                        &format!("Bearer {}", api_key.trim()),
                    )?;
                }
            }
            if let Some(user_id) = credentials.get("user_id").and_then(Value::as_str) {
                if !user_id.trim().is_empty() {
                    admin_provider_ops_insert_header(&mut headers, "New-Api-User", user_id.trim())?;
                }
            }
            if let Some(cookie) = credentials.get("cookie").and_then(Value::as_str) {
                if !cookie.trim().is_empty() {
                    admin_provider_ops_insert_header(&mut headers, "Cookie", cookie.trim())?;
                }
            }
        }
        "cubence" => {
            if let Some(token_cookie) = credentials
                .get("token_cookie")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                let token = admin_provider_ops_extract_cookie_value(token_cookie, "token");
                admin_provider_ops_insert_header(
                    &mut headers,
                    "Cookie",
                    &format!("token={token}"),
                )?;
            }
        }
        "yescode" => {
            if let Some(auth_cookie) = credentials
                .get("auth_cookie")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                admin_provider_ops_insert_header(
                    &mut headers,
                    "Cookie",
                    &admin_provider_ops_yescode_cookie_header(auth_cookie),
                )?;
            }
        }
        "nekocode" => {
            if let Some(session_cookie) = credentials
                .get("session_cookie")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                let session = admin_provider_ops_extract_cookie_value(session_cookie, "session");
                admin_provider_ops_insert_header(
                    &mut headers,
                    "Cookie",
                    &format!("session={session}"),
                )?;
            }
        }
        "anyrouter" => {
            let mut cookies = Vec::new();
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
                    admin_provider_ops_insert_header(&mut headers, "New-Api-User", user_id.trim())?;
                }
            }
            if !cookies.is_empty() {
                admin_provider_ops_insert_header(&mut headers, "Cookie", &cookies.join("; "))?;
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
    if status == StatusCode::UNAUTHORIZED {
        return admin_provider_ops_verify_failure("认证失败：无效的凭据");
    }
    if status == StatusCode::FORBIDDEN {
        return admin_provider_ops_verify_failure("认证失败：权限不足");
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
            admin_provider_ops_value_as_f64(user_data.get("quota")),
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

    let Some(payload) = admin_provider_ops_json_object(response_json) else {
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

    admin_provider_ops_verify_success(
        admin_provider_ops_verify_user_payload(
            user_data
                .get("username")
                .or_else(|| user_data.get("email"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            user_data
                .get("username")
                .or_else(|| user_data.get("email"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            user_data
                .get("email")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            Some(balance + points),
            Some(extra),
        ),
        updated_credentials,
    )
}
