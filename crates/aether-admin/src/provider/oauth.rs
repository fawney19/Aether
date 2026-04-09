use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub fn build_kiro_batch_import_key_name(
    email: Option<&str>,
    auth_method: Option<&str>,
    refresh_token: Option<&str>,
) -> String {
    let method = auth_method
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("social");
    let base = email
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            let hash = Sha256::digest(refresh_token.unwrap_or_default().as_bytes());
            let hex = hash
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            format!("kiro_{}", &hex[..6])
        });
    format!("{base} ({method})")
}

pub fn coerce_admin_provider_oauth_import_str(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_admin_provider_oauth_kiro_import_item(item: &Value) -> Option<Value> {
    match item {
        Value::String(value) => {
            let refresh_token = value.trim();
            if refresh_token.is_empty() {
                None
            } else {
                Some(json!({ "refresh_token": refresh_token }))
            }
        }
        Value::Object(object) => {
            if let Some(nested) = object
                .get("auth_config")
                .or_else(|| object.get("authConfig"))
                .and_then(Value::as_object)
            {
                let mut merged = nested.clone();
                for key in [
                    "provider_type",
                    "providerType",
                    "auth_method",
                    "authMethod",
                    "auth_type",
                    "authType",
                    "refresh_token",
                    "refreshToken",
                    "expires_at",
                    "expiresAt",
                    "profile_arn",
                    "profileArn",
                    "region",
                    "auth_region",
                    "authRegion",
                    "api_region",
                    "apiRegion",
                    "client_id",
                    "clientId",
                    "client_secret",
                    "clientSecret",
                    "machine_id",
                    "machineId",
                    "kiro_version",
                    "kiroVersion",
                    "system_version",
                    "systemVersion",
                    "node_version",
                    "nodeVersion",
                    "email",
                    "access_token",
                    "accessToken",
                ] {
                    if let Some(value) = object.get(key) {
                        if !value.is_null()
                            && !(value.is_string()
                                && value.as_str().is_some_and(|inner| inner.trim().is_empty()))
                        {
                            merged.insert(key.to_string(), value.clone());
                        }
                    }
                }
                return Some(Value::Object(merged));
            }
            Some(Value::Object(object.clone()))
        }
        _ => None,
    }
}

pub fn parse_admin_provider_oauth_kiro_batch_import_entries(raw_credentials: &str) -> Vec<Value> {
    let raw = raw_credentials.trim();
    if raw.is_empty() {
        return Vec::new();
    }

    if raw.starts_with('[') {
        if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(raw) {
            return items
                .iter()
                .filter_map(normalize_admin_provider_oauth_kiro_import_item)
                .collect();
        }
    }

    if raw.starts_with('{') {
        if let Ok(value) = serde_json::from_str::<Value>(raw) {
            return normalize_admin_provider_oauth_kiro_import_item(&value)
                .into_iter()
                .collect();
        }
    }

    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|refresh_token| json!({ "refreshToken": refresh_token }))
        .collect()
}
