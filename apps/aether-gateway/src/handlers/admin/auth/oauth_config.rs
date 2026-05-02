use crate::handlers::admin::request::AdminAppState;
use aether_data::repository::oauth_providers::{
    EncryptedSecretUpdate, UpsertOAuthProviderConfigRecord,
};
use axum::http;
use serde::Deserialize;
use serde_json::json;
use url::Url;

#[derive(Debug, Deserialize)]
pub(crate) struct AdminOAuthProviderUpsertRequest {
    pub(super) display_name: String,
    pub(super) client_id: String,
    #[serde(default)]
    pub(super) client_secret: Option<String>,
    #[serde(default)]
    pub(super) authorization_url_override: Option<String>,
    #[serde(default)]
    pub(super) token_url_override: Option<String>,
    #[serde(default)]
    pub(super) userinfo_url_override: Option<String>,
    #[serde(default)]
    pub(super) scopes: Option<Vec<String>>,
    pub(super) redirect_uri: String,
    pub(super) frontend_callback_url: String,
    #[serde(default)]
    pub(super) attribute_mapping: Option<serde_json::Value>,
    #[serde(default)]
    pub(super) extra_config: Option<serde_json::Value>,
    #[serde(default)]
    pub(super) is_enabled: bool,
    #[serde(default)]
    pub(super) force: bool,
}

pub(super) fn build_admin_oauth_supported_types_payload() -> Vec<serde_json::Value> {
    vec![
        json!({
            "provider_type": "linuxdo",
            "display_name": "Linux Do",
            "default_authorization_url": "https://connect.linux.do/oauth2/authorize",
            "default_token_url": "https://connect.linux.do/oauth2/token",
            "default_userinfo_url": "https://connect.linux.do/api/user",
            "default_scopes": [],
        }),
        json!({
            "provider_type": "custom_oidc",
            "display_name": "Custom OIDC",
            "default_authorization_url": "",
            "default_token_url": "",
            "default_userinfo_url": "",
            "default_scopes": ["openid", "profile", "email"],
        }),
    ]
}

pub(super) fn build_admin_oauth_provider_payload(
    provider: &aether_data::repository::oauth_providers::StoredOAuthProviderConfig,
) -> serde_json::Value {
    json!({
        "provider_type": provider.provider_type,
        "display_name": provider.display_name,
        "client_id": provider.client_id,
        "has_secret": provider.client_secret_encrypted.as_ref().is_some(),
        "authorization_url_override": provider.authorization_url_override,
        "token_url_override": provider.token_url_override,
        "userinfo_url_override": provider.userinfo_url_override,
        "scopes": provider.scopes,
        "redirect_uri": provider.redirect_uri,
        "frontend_callback_url": provider.frontend_callback_url,
        "attribute_mapping": provider.attribute_mapping,
        "extra_config": provider.extra_config,
        "is_enabled": provider.is_enabled,
    })
}

pub(crate) fn admin_oauth_provider_type_from_path(request_path: &str) -> Option<String> {
    let provider_type = request_path.strip_prefix("/api/admin/oauth/providers/")?;
    (!provider_type.is_empty() && !provider_type.contains('/')).then_some(provider_type.to_string())
}

pub(crate) fn admin_oauth_test_provider_type_from_path(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/oauth/providers/")?
        .strip_suffix("/test")
        .filter(|provider_type| !provider_type.is_empty() && !provider_type.contains('/'))
        .map(ToOwned::to_owned)
}

fn admin_oauth_is_supported_provider(provider_type: &str) -> bool {
    matches!(
        provider_type.to_ascii_lowercase().as_str(),
        "linuxdo" | "custom_oidc"
    )
}

fn admin_oauth_builtin_allowed_domains(provider_type: &str) -> Option<&'static [&'static str]> {
    if provider_type.eq_ignore_ascii_case("linuxdo") {
        Some(&["linux.do", "connect.linux.do", "connect.linuxdo.org"])
    } else {
        None
    }
}

fn admin_oauth_custom_allowed_domains(extra_config: Option<&serde_json::Value>) -> Vec<String> {
    extra_config
        .and_then(serde_json::Value::as_object)
        .and_then(|object| {
            object
                .get("allowed_domains")
                .or_else(|| object.get("oauth_allowed_domains"))
        })
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.trim_end_matches('.').to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn validate_admin_oauth_frontend_callback_url(url: &str) -> Result<(), String> {
    let parsed = Url::parse(url).map_err(|_| "frontend_callback_url 必须是绝对 URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("frontend_callback_url scheme 必须是 http/https".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("frontend_callback_url 必须是绝对 URL".to_string());
    }
    let path = parsed.path().trim_end_matches('/');
    if !path.ends_with("/auth/callback") {
        return Err("frontend_callback_url 路径必须以 /auth/callback 结尾".to_string());
    }
    Ok(())
}

fn validate_admin_oauth_redirect_uri(url: &str) -> Result<(), String> {
    let parsed = Url::parse(url).map_err(|_| "redirect_uri 必须是绝对 URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("redirect_uri scheme 必须是 http/https".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("redirect_uri 必须是绝对 URL".to_string());
    }
    Ok(())
}

fn validate_admin_oauth_url_override(url: &str, allowed_domains: &[&str]) -> Result<(), String> {
    let parsed = Url::parse(url).map_err(|_| "端点覆盖必须是 https 绝对 URL".to_string())?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err("端点覆盖必须是 https 绝对 URL".to_string());
    }
    let host = parsed
        .host_str()
        .map(|value| value.trim().trim_end_matches('.').to_ascii_lowercase())
        .unwrap_or_default();
    let allowed = allowed_domains.iter().any(|domain| {
        let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
        host == domain || host.ends_with(&format!(".{domain}"))
    });
    if !allowed {
        return Err("端点覆盖不在允许的域名白名单中".to_string());
    }
    Ok(())
}

fn validate_admin_oauth_url_override_for_domains(
    url: &str,
    allowed_domains: &[String],
) -> Result<(), String> {
    let allowed = allowed_domains
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    validate_admin_oauth_url_override(url, &allowed)
}

pub(super) fn build_admin_oauth_upsert_record(
    state: &AdminAppState<'_>,
    provider_type: &str,
    payload: AdminOAuthProviderUpsertRequest,
) -> Result<UpsertOAuthProviderConfigRecord, String> {
    if !admin_oauth_is_supported_provider(provider_type) {
        return Err("不支持的 provider_type".to_string());
    }

    let display_name = payload.display_name.trim();
    if display_name.is_empty() {
        return Err("显示名称不能为空".to_string());
    }
    let client_id = payload.client_id.trim();
    if client_id.is_empty() {
        return Err("Client ID 不能为空".to_string());
    }
    let redirect_uri = payload.redirect_uri.trim();
    if redirect_uri.is_empty() {
        return Err("redirect_uri 不能为空".to_string());
    }
    let frontend_callback_url = payload.frontend_callback_url.trim();
    if frontend_callback_url.is_empty() {
        return Err("frontend_callback_url 不能为空".to_string());
    }

    validate_admin_oauth_frontend_callback_url(frontend_callback_url)?;
    validate_admin_oauth_redirect_uri(redirect_uri)?;

    let is_custom_oidc = provider_type.eq_ignore_ascii_case("custom_oidc");
    let custom_allowed_domains = if is_custom_oidc {
        let domains = admin_oauth_custom_allowed_domains(payload.extra_config.as_ref());
        if domains.is_empty() {
            return Err(
                "custom_oidc 必须在 extra_config.allowed_domains 配置域名白名单".to_string(),
            );
        }
        domains
    } else {
        Vec::new()
    };
    let builtin_allowed_domains = admin_oauth_builtin_allowed_domains(provider_type);

    if is_custom_oidc {
        for (field_name, value) in [
            (
                "authorization_url_override",
                payload.authorization_url_override.as_deref(),
            ),
            ("token_url_override", payload.token_url_override.as_deref()),
            (
                "userinfo_url_override",
                payload.userinfo_url_override.as_deref(),
            ),
        ] {
            let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
                return Err(format!("custom_oidc 必须配置 {field_name}"));
            };
            validate_admin_oauth_url_override_for_domains(value, &custom_allowed_domains)?;
        }
    }

    if let Some(value) = payload.authorization_url_override.as_deref().map(str::trim) {
        if !value.is_empty() {
            if let Some(allowed_domains) = builtin_allowed_domains {
                validate_admin_oauth_url_override(value, allowed_domains)?;
            } else {
                validate_admin_oauth_url_override_for_domains(value, &custom_allowed_domains)?;
            }
        }
    }
    if let Some(value) = payload.token_url_override.as_deref().map(str::trim) {
        if !value.is_empty() {
            if let Some(allowed_domains) = builtin_allowed_domains {
                validate_admin_oauth_url_override(value, allowed_domains)?;
            } else {
                validate_admin_oauth_url_override_for_domains(value, &custom_allowed_domains)?;
            }
        }
    }
    if let Some(value) = payload.userinfo_url_override.as_deref().map(str::trim) {
        if !value.is_empty() {
            if let Some(allowed_domains) = builtin_allowed_domains {
                validate_admin_oauth_url_override(value, allowed_domains)?;
            } else {
                validate_admin_oauth_url_override_for_domains(value, &custom_allowed_domains)?;
            }
        }
    }

    if payload
        .attribute_mapping
        .as_ref()
        .is_some_and(|value| !value.is_object())
    {
        return Err("attribute_mapping 必须是对象".to_string());
    }
    if payload
        .extra_config
        .as_ref()
        .is_some_and(|value| !value.is_object())
    {
        return Err("extra_config 必须是对象".to_string());
    }
    if payload
        .scopes
        .as_ref()
        .is_some_and(|items| items.iter().any(|value| value.trim().is_empty()))
    {
        return Err("scopes 不能为空".to_string());
    }

    let client_secret_encrypted = match payload.client_secret.as_deref() {
        None => EncryptedSecretUpdate::Preserve,
        Some(raw) => {
            let secret = raw.trim();
            if secret == "__CLEAR__" {
                EncryptedSecretUpdate::Clear
            } else if secret.is_empty() {
                EncryptedSecretUpdate::Preserve
            } else {
                let encrypted = state
                    .encrypt_catalog_secret_with_fallbacks(secret)
                    .ok_or_else(|| "gateway 未配置 OAuth provider 加密密钥".to_string())?;
                EncryptedSecretUpdate::Set(encrypted)
            }
        }
    };

    Ok(UpsertOAuthProviderConfigRecord {
        provider_type: provider_type.to_string(),
        display_name: display_name.to_string(),
        client_id: client_id.to_string(),
        client_secret_encrypted,
        authorization_url_override: payload.authorization_url_override.and_then(|value| {
            let value = value.trim().to_string();
            (!value.is_empty()).then_some(value)
        }),
        token_url_override: payload.token_url_override.and_then(|value| {
            let value = value.trim().to_string();
            (!value.is_empty()).then_some(value)
        }),
        userinfo_url_override: payload.userinfo_url_override.and_then(|value| {
            let value = value.trim().to_string();
            (!value.is_empty()).then_some(value)
        }),
        scopes: payload.scopes.map(|items| {
            items
                .into_iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect()
        }),
        redirect_uri: redirect_uri.to_string(),
        frontend_callback_url: frontend_callback_url.to_string(),
        attribute_mapping: payload.attribute_mapping,
        extra_config: payload.extra_config,
        is_enabled: payload.is_enabled,
    })
}
