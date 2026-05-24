use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use url::{form_urlencoded, Url};
use uuid::Uuid;

const KIRO_DEVICE_DEFAULT_START_URL: &str = "https://view.awsapps.com/start";
const KIRO_DEVICE_DEFAULT_REGION: &str = "us-east-1";

pub fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub fn generate_provider_oauth_nonce() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

pub fn generate_provider_oauth_pkce_verifier() -> String {
    format!(
        "{}{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

pub fn provider_oauth_pkce_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

pub fn parse_provider_oauth_callback_params(callback_url: &str) -> BTreeMap<String, String> {
    let mut merged = BTreeMap::new();
    let raw_callback_url = callback_url.trim();
    let parsed_url = Url::parse(raw_callback_url).or_else(|_| {
        Url::parse(&format!(
            "https://aether.local/{}",
            raw_callback_url.trim_start_matches('/')
        ))
    });
    let Ok(url) = parsed_url else {
        return merged;
    };
    if url.query().is_none()
        && url.fragment().is_none()
        && raw_callback_url.contains('=')
        && !raw_callback_url.contains("://")
    {
        for (key, value) in
            form_urlencoded::parse(raw_callback_url.trim_start_matches('?').as_bytes())
        {
            merged.insert(key.into_owned(), value.into_owned());
        }
    }
    for (key, value) in form_urlencoded::parse(url.query().unwrap_or_default().as_bytes()) {
        merged.insert(key.into_owned(), value.into_owned());
    }
    if let Some(fragment) = url.fragment() {
        for (key, value) in form_urlencoded::parse(fragment.trim_start_matches('#').as_bytes()) {
            merged.insert(key.into_owned(), value.into_owned());
        }
    }
    if let Some(code) = merged.get("code").cloned() {
        if let Some((code_part, state_part)) = code.split_once('#') {
            merged.insert("code".to_string(), code_part.to_string());
            if !merged.contains_key("state") && !state_part.is_empty() {
                let normalized_state = state_part
                    .strip_prefix("state=")
                    .unwrap_or(state_part)
                    .trim();
                if !normalized_state.is_empty() {
                    merged.insert("state".to_string(), normalized_state.to_string());
                }
            }
        }
    }
    merged
}

pub fn json_non_empty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub fn json_u64_value(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number.as_u64(),
        Value::String(value) => value.trim().parse::<u64>().ok(),
        _ => None,
    }
}

pub fn decode_jwt_claims(token: &str) -> Option<Map<String, Value>> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .as_object()
        .cloned()
}

fn merge_missing_auth_config_fields(
    auth_config: &mut Map<String, Value>,
    source: &Map<String, Value>,
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

fn first_json_non_empty_string(values: impl IntoIterator<Item = Option<Value>>) -> Option<String> {
    values.into_iter().find_map(|value| match value {
        Some(Value::String(value)) => {
            let normalized = value.trim();
            (!normalized.is_empty()).then(|| normalized.to_string())
        }
        _ => None,
    })
}

fn provider_type_uses_openai_chatgpt_identity(provider_type: &str) -> bool {
    matches!(
        provider_type.trim().to_ascii_lowercase().as_str(),
        "codex" | "chatgpt_web"
    )
}

fn extract_openai_chatgpt_auth_fields_from_object(
    source: &Map<String, Value>,
) -> Map<String, Value> {
    let auth = source
        .get("https://api.openai.com/auth")
        .and_then(Value::as_object);
    let profile = source
        .get("https://api.openai.com/profile")
        .and_then(Value::as_object);
    let mut result = Map::new();

    if let Some(email) = first_json_non_empty_string([
        source.get("email").cloned(),
        auth.and_then(|value| value.get("email")).cloned(),
        profile.and_then(|value| value.get("email")).cloned(),
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
        .and_then(Value::as_array)
        .filter(|value| !value.is_empty())
    {
        result.insert(
            "organizations".to_string(),
            Value::Array(organizations.clone()),
        );
    }

    result
}

pub fn enrich_admin_provider_oauth_auth_config(
    provider_type: &str,
    auth_config: &mut Map<String, Value>,
    token_payload: &Value,
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

    if !provider_type_uses_openai_chatgpt_identity(provider_type) {
        return;
    }

    let chatgpt_fields = extract_openai_chatgpt_auth_fields_from_object(token_payload_object);
    merge_missing_auth_config_fields(
        auth_config,
        &chatgpt_fields,
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
        let chatgpt_claim_fields = extract_openai_chatgpt_auth_fields_from_object(&claims);
        merge_missing_auth_config_fields(
            auth_config,
            &chatgpt_claim_fields,
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

pub fn default_kiro_device_start_url() -> String {
    KIRO_DEVICE_DEFAULT_START_URL.to_string()
}

pub fn default_kiro_device_region() -> String {
    KIRO_DEVICE_DEFAULT_REGION.to_string()
}

pub fn normalize_kiro_device_region(value: Option<&str>) -> Option<String> {
    let value = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(KIRO_DEVICE_DEFAULT_REGION);
    value
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        .then(|| value.to_string())
}

pub fn build_kiro_device_key_name(email: Option<&str>, refresh_token: Option<&str>) -> String {
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

#[cfg(test)]
mod tests {
    use super::{enrich_admin_provider_oauth_auth_config, parse_provider_oauth_callback_params};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::json;

    fn sample_unsigned_jwt(payload: serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn parse_provider_oauth_callback_params_reads_openai_query_state() {
        let params = parse_provider_oauth_callback_params(
            "http://localhost:1455/auth/callback?code=ac_test123&scope=openid+email+profile+offline_access&state=4a138f8c65814df691b1a567fd425fb3d7010e86df9b4eb48dcadd94de233d93",
        );

        assert_eq!(params.get("code").map(String::as_str), Some("ac_test123"));
        assert_eq!(
            params.get("scope").map(String::as_str),
            Some("openid email profile offline_access")
        );
        assert_eq!(
            params.get("state").map(String::as_str),
            Some("4a138f8c65814df691b1a567fd425fb3d7010e86df9b4eb48dcadd94de233d93")
        );
    }

    #[test]
    fn parse_provider_oauth_callback_params_prefers_fragment_values_like_python() {
        let params = parse_provider_oauth_callback_params(
            "http://localhost:1455/auth/callback?code=query-code&state=stale#code=fragment-code&state=fresh-state",
        );

        assert_eq!(
            params.get("code").map(String::as_str),
            Some("fragment-code")
        );
        assert_eq!(params.get("state").map(String::as_str), Some("fresh-state"));
    }

    #[test]
    fn parse_provider_oauth_callback_params_extracts_state_from_code_suffix() {
        let params = parse_provider_oauth_callback_params(
            "http://localhost:1455/auth/callback?code=code-value%23state%3Dnonce-value",
        );

        assert_eq!(params.get("code").map(String::as_str), Some("code-value"));
        assert_eq!(params.get("state").map(String::as_str), Some("nonce-value"));
    }

    #[test]
    fn parse_provider_oauth_callback_params_reads_relative_show_auth_token_url() {
        let params = parse_provider_oauth_callback_params(
            "show-auth-token?token=firebase-id-token&state=session-1&provider=google",
        );

        assert_eq!(
            params.get("token").map(String::as_str),
            Some("firebase-id-token")
        );
        assert_eq!(params.get("state").map(String::as_str), Some("session-1"));
        assert_eq!(params.get("provider").map(String::as_str), Some("google"));
    }

    #[test]
    fn parse_provider_oauth_callback_params_reads_raw_query_string() {
        let params = parse_provider_oauth_callback_params("token=raw-token&state=session-raw");

        assert_eq!(params.get("token").map(String::as_str), Some("raw-token"));
        assert_eq!(params.get("state").map(String::as_str), Some("session-raw"));
    }

    #[test]
    fn chatgpt_web_enrichment_extracts_identity_from_openai_claims() {
        let access_token = sample_unsigned_jwt(json!({
            "https://api.openai.com/profile": {
                "email": "image@example.com",
            },
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc-image",
                "chatgpt_account_user_id": "user-image__acc-image",
                "chatgpt_plan_type": "plus",
                "chatgpt_user_id": "user-image",
            },
        }));
        let token_payload = json!({
            "access_token": access_token,
        });
        let mut auth_config = serde_json::Map::new();

        enrich_admin_provider_oauth_auth_config("chatgpt_web", &mut auth_config, &token_payload);

        assert_eq!(auth_config.get("email"), Some(&json!("image@example.com")));
        assert_eq!(auth_config.get("account_id"), Some(&json!("acc-image")));
        assert_eq!(
            auth_config.get("account_user_id"),
            Some(&json!("user-image__acc-image"))
        );
        assert_eq!(auth_config.get("plan_type"), Some(&json!("plus")));
        assert_eq!(auth_config.get("user_id"), Some(&json!("user-image")));
    }
}
