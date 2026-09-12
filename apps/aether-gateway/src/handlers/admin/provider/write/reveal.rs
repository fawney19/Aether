fn normalize_reveal_auth_type(value: &str) -> &str {
    match value.trim().to_ascii_lowercase().as_str() {
        "service_account" | "vertex_ai" => "service_account",
        "oauth" => "oauth",
        "bearer" => "bearer",
        _ => "api_key",
    }
}

use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::parse_catalog_auth_config_json;
use crate::provider_key_auth::provider_key_auth_semantics;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use chrono::{SecondsFormat, Utc};
use serde_json::json;

fn reveal_provider_type_from_auth_config(
    auth_config: Option<&serde_json::Map<String, serde_json::Value>>,
) -> String {
    auth_config
        .and_then(|value| value.get("provider_type"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn build_admin_reveal_key_payload(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
) -> Result<serde_json::Value, String> {
    let parsed_auth_config = state.parse_catalog_auth_config_json(key);
    if parsed_auth_config.as_ref().is_some_and(|auth_config| {
        aether_provider_transport::is_codex_agent_identity_auth_config_value(
            &serde_json::Value::Object(auth_config.clone()),
        )
    }) {
        return Err(
            "Agent Identity 凭据不能通过通用 Key 查看接口读取，请使用专属 provider-oauth 管理面"
                .to_string(),
        );
    }
    let provider_type = reveal_provider_type_from_auth_config(parsed_auth_config.as_ref());
    let auth_semantics = provider_key_auth_semantics(key, provider_type.as_str());
    let auth_type = if auth_semantics.oauth_managed() {
        "oauth"
    } else {
        normalize_reveal_auth_type(&key.auth_type)
    };
    if matches!(auth_type, "service_account") {
        if let Some(auth_config) = parsed_auth_config {
            return Ok(json!({
                "auth_type": auth_type,
                "auth_config": auth_config,
            }));
        }
        let decrypted = state
            .app()
            .decrypt_provider_catalog_key_api_key(key)
            .ok()
            .flatten()
            .ok_or_else(|| {
                "无法解密认证配置，可能是加密密钥已更改。请重新添加该密钥。".to_string()
            })?;
        if decrypted == "__placeholder__" {
            return Err("认证配置丢失，请重新添加该密钥。".to_string());
        }
        return Ok(json!({
            "auth_type": auth_type,
            "auth_config": decrypted,
        }));
    }

    let decrypted = match key.encrypted_api_key.as_deref().map(str::trim) {
        Some(ciphertext) if !ciphertext.is_empty() => state
            .app()
            .decrypt_provider_catalog_key_api_key(key)
            .map_err(|_| "无法解密 API Key，可能是加密密钥已更改。请重新添加该密钥。".to_string())?
            .ok_or_else(|| {
                "无法解密 API Key，可能是加密密钥已更改。请重新添加该密钥。".to_string()
            })?,
        _ => String::new(),
    };
    Ok(json!({
        "auth_type": auth_type,
        "api_key": decrypted,
    }))
}

fn provider_oauth_export_payload(
    provider_type: &str,
    auth_config: &serde_json::Map<String, serde_json::Value>,
    upstream_metadata: Option<&serde_json::Value>,
    fallback_access_token: Option<&str>,
) -> serde_json::Map<String, serde_json::Value> {
    let normalized_provider_type = provider_type.trim().to_ascii_lowercase();
    let skip_keys = ["updated_at", "updatedAt"];
    let mut payload = serde_json::Map::new();
    for (key, value) in auth_config {
        if skip_keys.contains(&key.as_str()) {
            continue;
        }
        if value.is_null() || value.as_str().is_some_and(|inner| inner.trim().is_empty()) {
            continue;
        }
        payload.insert(key.clone(), value.clone());
    }
    if !json_map_has_non_empty_string(&payload, &["access_token", "accessToken"]) {
        if let Some(access_token) = fallback_access_token
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "__placeholder__")
            .filter(|value| !oauth_export_fallback_matches_authorization_header(&payload, value))
        {
            payload.insert("access_token".to_string(), json!(access_token));
        }
    }
    if normalized_provider_type == "kiro" && !payload.contains_key("email") {
        if let Some(email) = upstream_metadata
            .and_then(serde_json::Value::as_object)
            .and_then(|meta| meta.get("kiro"))
            .and_then(serde_json::Value::as_object)
            .and_then(|meta| meta.get("email"))
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            payload.insert("email".to_string(), json!(email));
        }
    }
    payload
}

fn oauth_export_fallback_matches_authorization_header(
    payload: &serde_json::Map<String, serde_json::Value>,
    fallback_access_token: &str,
) -> bool {
    let fallback_access_token = fallback_access_token.trim();
    if fallback_access_token.is_empty() {
        return false;
    }
    let Some(authorization) = payload
        .get("headers")
        .and_then(serde_json::Value::as_object)
        .and_then(|headers| {
            headers
                .iter()
                .find(|(key, _)| key.trim().eq_ignore_ascii_case("authorization"))
                .and_then(|(_, value)| value.as_str())
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };

    if authorization == fallback_access_token {
        return true;
    }

    let mut parts = authorization.splitn(2, char::is_whitespace);
    let Some(scheme) = parts.next() else {
        return false;
    };
    let Some(token) = parts.next() else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer") && token.trim() == fallback_access_token
}

fn json_map_has_non_empty_string(
    map: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> bool {
    keys.iter().any(|key| {
        map.get(*key)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    })
}

pub(crate) async fn build_admin_export_key_payload(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
) -> Result<serde_json::Value, String> {
    let _ciphertext = key
        .encrypted_auth_config
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "缺少认证配置，无法导出".to_string())?;
    let plaintext = state
        .app()
        .decrypt_provider_catalog_key_auth_config(key)
        .map_err(|_| "无法解密认证配置".to_string())?
        .ok_or_else(|| "无法解密认证配置".to_string())?;
    let auth_config = serde_json::from_str::<serde_json::Value>(&plaintext)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| "无法解密认证配置".to_string())?;

    if aether_provider_transport::is_codex_agent_identity_auth_config_value(
        &serde_json::Value::Object(auth_config.clone()),
    ) {
        return Err(
            "Agent Identity 凭据不能通过通用 Key 导出接口导出，请使用专属 provider-oauth 管理面"
                .to_string(),
        );
    }

    let provider_type_from_config = auth_config
        .get("provider_type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let provider_type = if let Some(provider_type) = provider_type_from_config {
        provider_type
    } else {
        state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&key.provider_id))
            .await
            .map_err(|err| format!("{err:?}"))?
            .into_iter()
            .next()
            .map(|provider| provider.provider_type)
            .unwrap_or_default()
    };
    if !provider_key_auth_semantics(key, provider_type.as_str()).can_export_oauth() {
        return Err("仅 OAuth 管理账号支持导出".to_string());
    }

    let fallback_access_token = key
        .encrypted_api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|_| {
            state
                .app()
                .decrypt_provider_catalog_key_api_key(key)
                .ok()
                .flatten()
        });
    let mut payload = provider_oauth_export_payload(
        &provider_type,
        &auth_config,
        key.upstream_metadata.as_ref(),
        fallback_access_token.as_deref(),
    );
    payload.insert("name".to_string(), json!(key.name));
    payload.insert(
        "exported_at".to_string(),
        json!(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)),
    );
    Ok(serde_json::Value::Object(payload))
}

#[cfg(test)]
mod tests {
    use super::provider_oauth_export_payload;
    use serde_json::json;

    #[test]
    fn oauth_export_preserves_imported_request_headers() {
        let auth_config = json!({
            "provider_type": "codex",
            "email": "user@example.com",
            "headers": {
                "authorization": "Bearer imported-session",
                "chatgpt-account-id": "acct-1"
            }
        })
        .as_object()
        .cloned()
        .expect("auth_config should be an object");

        let payload = provider_oauth_export_payload("codex", &auth_config, None, Some("fallback"));

        assert_eq!(
            payload.get("headers"),
            Some(&json!({
                "authorization": "Bearer imported-session",
                "chatgpt-account-id": "acct-1"
            }))
        );
        assert_eq!(payload.get("access_token"), Some(&json!("fallback")));
    }

    #[test]
    fn oauth_export_does_not_promote_imported_header_bearer_to_access_token() {
        let auth_config = json!({
            "provider_type": "codex",
            "email": "user@example.com",
            "headers": {
                "authorization": "Bearer imported-session"
            }
        })
        .as_object()
        .cloned()
        .expect("auth_config should be an object");

        let payload =
            provider_oauth_export_payload("codex", &auth_config, None, Some("imported-session"));

        assert_eq!(
            payload.get("headers"),
            Some(&json!({"authorization": "Bearer imported-session"}))
        );
        assert!(payload.get("access_token").is_none());
    }

    #[test]
    fn oauth_export_keeps_explicit_access_token_even_with_header_bearer() {
        let auth_config = json!({
            "provider_type": "codex",
            "access_token": "jwt-access-token",
            "headers": {
                "authorization": "Bearer imported-session"
            }
        })
        .as_object()
        .cloned()
        .expect("auth_config should be an object");

        let payload =
            provider_oauth_export_payload("codex", &auth_config, None, Some("imported-session"));

        assert_eq!(
            payload.get("access_token"),
            Some(&json!("jwt-access-token"))
        );
        assert_eq!(
            payload.get("headers"),
            Some(&json!({"authorization": "Bearer imported-session"}))
        );
    }
}
