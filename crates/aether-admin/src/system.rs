use aether_data::repository::{
    auth_modules::{StoredLdapModuleConfig, StoredOAuthProviderModuleConfig},
    proxy_nodes::{StoredProxyNode, StoredProxyNodeEvent},
    system::StoredSystemConfigEntry,
    wallet::StoredWalletSnapshot,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use axum::http;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct AdminSystemSettingsUpdate {
    pub default_provider: Option<Option<String>>,
    pub default_model: Option<Option<String>>,
    pub enable_usage_tracking: Option<bool>,
    pub password_policy_level: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AdminSystemConfigUpdate {
    pub normalized_key: String,
    pub value: serde_json::Value,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AdminEmailTemplateUpdate {
    pub subject: Option<String>,
    pub html: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct AdminApiFormatDefinition {
    value: &'static str,
    label: &'static str,
    default_path: &'static str,
    aliases: &'static [&'static str],
}

const REQUEST_RECORD_LEVEL_KEY: &str = "request_record_level";
const LEGACY_REQUEST_LOG_LEVEL_KEY: &str = "request_log_level";
const SENSITIVE_SYSTEM_CONFIG_KEYS: &[&str] = &["smtp_password"];
const ADMIN_API_FORMAT_DEFINITIONS: &[AdminApiFormatDefinition] = &[
    AdminApiFormatDefinition {
        value: "openai:chat",
        label: "OpenAI Chat",
        default_path: "/v1/chat/completions",
        aliases: &[
            "openai",
            "openai_compatible",
            "deepseek",
            "grok",
            "moonshot",
            "zhipu",
            "qwen",
            "baichuan",
            "minimax",
        ],
    },
    AdminApiFormatDefinition {
        value: "openai:cli",
        label: "OpenAI CLI",
        default_path: "/v1/responses",
        aliases: &["openai_cli", "responses"],
    },
    AdminApiFormatDefinition {
        value: "openai:compact",
        label: "OpenAI Compact",
        default_path: "/v1/responses/compact",
        aliases: &["openai_compact", "responses_compact"],
    },
    AdminApiFormatDefinition {
        value: "openai:video",
        label: "OpenAI Video",
        default_path: "/v1/videos",
        aliases: &["openai_video", "sora"],
    },
    AdminApiFormatDefinition {
        value: "claude:chat",
        label: "Claude Chat",
        default_path: "/v1/messages",
        aliases: &["claude", "anthropic", "claude_compatible"],
    },
    AdminApiFormatDefinition {
        value: "claude:cli",
        label: "Claude CLI",
        default_path: "/v1/messages",
        aliases: &["claude_cli", "claude-cli"],
    },
    AdminApiFormatDefinition {
        value: "gemini:chat",
        label: "Gemini Chat",
        default_path: "/v1beta/models/{model}:{action}",
        aliases: &["gemini", "google", "vertex"],
    },
    AdminApiFormatDefinition {
        value: "gemini:cli",
        label: "Gemini CLI",
        default_path: "/v1beta/models/{model}:{action}",
        aliases: &["gemini_cli", "gemini-cli"],
    },
    AdminApiFormatDefinition {
        value: "gemini:video",
        label: "Gemini Video",
        default_path: "/v1beta/models/{model}:predictLongRunning",
        aliases: &["gemini_video", "veo"],
    },
];

pub fn build_admin_system_check_update_payload(current_version: String) -> serde_json::Value {
    json!({
        "current_version": current_version,
        "latest_version": serde_json::Value::Null,
        "has_update": false,
        "release_url": serde_json::Value::Null,
        "release_notes": serde_json::Value::Null,
        "published_at": serde_json::Value::Null,
        "error": "检查更新需要 Rust 管理后端",
    })
}

pub fn build_admin_system_stats_payload(
    total_users: u64,
    active_users: u64,
    total_providers: u64,
    active_providers: u64,
    total_api_keys: u64,
    total_requests: u64,
) -> serde_json::Value {
    json!({
        "users": {
            "total": total_users,
            "active": active_users,
        },
        "providers": {
            "total": total_providers,
            "active": active_providers,
        },
        "api_keys": total_api_keys,
        "requests": total_requests,
    })
}

pub fn build_admin_system_settings_payload(
    default_provider: Option<String>,
    default_model: Option<String>,
    enable_usage_tracking: bool,
    password_policy_level: String,
) -> serde_json::Value {
    json!({
        "default_provider": default_provider,
        "default_model": default_model,
        "enable_usage_tracking": enable_usage_tracking,
        "password_policy_level": password_policy_level,
    })
}

pub fn parse_admin_system_settings_update(
    request_body: &[u8],
) -> Result<AdminSystemSettingsUpdate, (http::StatusCode, serde_json::Value)> {
    let payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(serde_json::Value::Object(payload)) => payload,
        Ok(_) | Err(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
    };

    let default_provider = match payload.get("default_provider") {
        Some(serde_json::Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                Some(None)
            } else {
                Some(Some(value.to_string()))
            }
        }
        Some(serde_json::Value::Null) => Some(None),
        Some(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
        None => None,
    };

    let default_model = match payload.get("default_model") {
        Some(serde_json::Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                Some(None)
            } else {
                Some(Some(value.to_string()))
            }
        }
        Some(serde_json::Value::Null) => Some(None),
        Some(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
        None => None,
    };

    let enable_usage_tracking = match payload.get("enable_usage_tracking") {
        Some(serde_json::Value::Bool(value)) => Some(*value),
        Some(serde_json::Value::Null) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
        Some(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
        None => None,
    };

    let password_policy_level = match payload.get("password_policy_level") {
        Some(serde_json::Value::String(value)) => {
            let value = value.trim();
            if matches!(value, "weak" | "medium" | "strong") {
                Some(value.to_string())
            } else {
                return Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                ));
            }
        }
        Some(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
        None => None,
    };

    Ok(AdminSystemSettingsUpdate {
        default_provider,
        default_model,
        enable_usage_tracking,
        password_policy_level,
    })
}

pub fn build_admin_system_settings_updated_payload() -> serde_json::Value {
    json!({ "message": "系统设置更新成功" })
}

pub fn build_admin_email_templates_payload(templates: Vec<serde_json::Value>) -> serde_json::Value {
    json!({ "templates": templates })
}

pub fn admin_email_template_not_found_error(
    template_type: &str,
) -> (http::StatusCode, serde_json::Value) {
    (
        http::StatusCode::NOT_FOUND,
        json!({ "detail": format!("模板类型 '{template_type}' 不存在") }),
    )
}

pub fn parse_admin_email_template_update(
    request_body: &[u8],
) -> Result<AdminEmailTemplateUpdate, (http::StatusCode, serde_json::Value)> {
    let payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(serde_json::Value::Object(payload)) => payload,
        _ => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
    };

    let subject = match payload.get("subject") {
        Some(serde_json::Value::String(value)) => Some(value.clone()),
        Some(serde_json::Value::Null) | None => None,
        Some(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
    };
    let html = match payload.get("html") {
        Some(serde_json::Value::String(value)) => Some(value.clone()),
        Some(serde_json::Value::Null) | None => None,
        Some(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
    };

    if subject.is_none() && html.is_none() {
        return Err((
            http::StatusCode::BAD_REQUEST,
            json!({ "detail": "请提供 subject 或 html" }),
        ));
    }

    Ok(AdminEmailTemplateUpdate { subject, html })
}

pub fn parse_admin_email_template_preview_payload(
    request_body: Option<&[u8]>,
) -> Result<serde_json::Map<String, serde_json::Value>, (http::StatusCode, serde_json::Value)> {
    match request_body {
        Some(bytes) => match serde_json::from_slice::<serde_json::Value>(bytes) {
            Ok(serde_json::Value::Object(payload)) => Ok(payload),
            Ok(serde_json::Value::Null) => Ok(serde_json::Map::new()),
            _ => Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            )),
        },
        None => Ok(serde_json::Map::new()),
    }
}

pub fn build_admin_email_template_saved_payload() -> serde_json::Value {
    json!({ "message": "模板保存成功" })
}

pub fn build_admin_email_template_preview_payload(
    rendered_html: String,
    preview_variables: std::collections::BTreeMap<String, String>,
) -> serde_json::Value {
    json!({
        "html": rendered_html,
        "variables": preview_variables,
    })
}

pub fn build_admin_email_template_reset_payload(
    template_type: &str,
    name: &str,
    default_subject: &str,
    default_html: &str,
) -> serde_json::Value {
    json!({
        "message": "模板已重置为默认值",
        "template": {
            "type": template_type,
            "name": name,
            "subject": default_subject,
            "html": default_html,
        }
    })
}

pub fn build_admin_api_formats_payload() -> serde_json::Value {
    json!({
        "formats": ADMIN_API_FORMAT_DEFINITIONS
            .iter()
            .map(|definition| json!({
                "value": definition.value,
                "label": definition.label,
                "default_path": definition.default_path,
                "aliases": definition.aliases,
            }))
            .collect::<Vec<_>>(),
    })
}

pub fn admin_module_name_from_status_path(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/modules/status/")
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .map(ToOwned::to_owned)
}

pub fn admin_module_name_from_enabled_path(request_path: &str) -> Option<String> {
    request_path
        .strip_prefix("/api/admin/modules/status/")
        .and_then(|value| value.strip_suffix("/enabled"))
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .map(ToOwned::to_owned)
}

pub fn oauth_module_config_is_valid(providers: &[StoredOAuthProviderModuleConfig]) -> bool {
    !providers.is_empty()
        && providers.iter().all(|provider| {
            !provider.client_id.trim().is_empty()
                && provider
                    .client_secret_encrypted
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_some()
                && !provider.redirect_uri.trim().is_empty()
        })
}

pub fn ldap_module_config_is_valid(config: Option<&StoredLdapModuleConfig>) -> bool {
    let Some(config) = config else {
        return false;
    };
    !config.server_url.trim().is_empty()
        && !config.bind_dn.trim().is_empty()
        && !config.base_dn.trim().is_empty()
        && config
            .bind_password_encrypted
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
}

pub fn build_admin_module_validation_result(
    module_name: &str,
    oauth_providers: &[StoredOAuthProviderModuleConfig],
    ldap_config: Option<&StoredLdapModuleConfig>,
    gemini_files_has_capable_key: bool,
    smtp_configured: bool,
) -> (bool, Option<String>) {
    match module_name {
        "oauth" => {
            if oauth_providers.is_empty() {
                return (
                    false,
                    Some("请先配置并启用至少一个 OAuth Provider".to_string()),
                );
            }
            for provider in oauth_providers {
                if provider.client_id.trim().is_empty() {
                    return (
                        false,
                        Some(format!(
                            "Provider [{}] 未配置 Client ID",
                            provider.display_name
                        )),
                    );
                }
                if provider
                    .client_secret_encrypted
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_none()
                {
                    return (
                        false,
                        Some(format!(
                            "Provider [{}] 未配置 Client Secret",
                            provider.display_name
                        )),
                    );
                }
                if provider.redirect_uri.trim().is_empty() {
                    return (
                        false,
                        Some(format!(
                            "Provider [{}] 未配置回调地址",
                            provider.display_name
                        )),
                    );
                }
            }
            (true, None)
        }
        "ldap" => {
            let Some(config) = ldap_config else {
                return (false, Some("请先配置 LDAP 连接信息".to_string()));
            };
            if config.server_url.trim().is_empty() {
                return (false, Some("请配置 LDAP 服务器地址".to_string()));
            }
            if config.bind_dn.trim().is_empty() {
                return (false, Some("请配置绑定 DN".to_string()));
            }
            if config.base_dn.trim().is_empty() {
                return (false, Some("请配置搜索基准 DN".to_string()));
            }
            if config
                .bind_password_encrypted
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                return (false, Some("请配置绑定密码".to_string()));
            }
            (true, None)
        }
        "notification_email" => {
            if smtp_configured {
                (true, None)
            } else {
                (false, Some("请先完成邮件配置（SMTP）".to_string()))
            }
        }
        "gemini_files" => {
            if gemini_files_has_capable_key {
                (true, None)
            } else {
                (
                    false,
                    Some("至少启用一个具有「Gemini 文件 API」能力的 Key".to_string()),
                )
            }
        }
        "management_tokens" | "proxy_nodes" => (true, None),
        _ => (true, None),
    }
}

pub fn build_admin_module_health(
    module_name: &str,
    gemini_files_has_capable_key: bool,
) -> &'static str {
    match module_name {
        "management_tokens" | "proxy_nodes" => "healthy",
        "gemini_files" => {
            if gemini_files_has_capable_key {
                "healthy"
            } else {
                "degraded"
            }
        }
        _ => "unknown",
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_admin_module_status_payload(
    name: &str,
    display_name: &str,
    description: &str,
    category: &str,
    admin_route: Option<&str>,
    admin_menu_icon: Option<&str>,
    admin_menu_group: Option<&str>,
    admin_menu_order: i32,
    available: bool,
    enabled: bool,
    config_validated: bool,
    config_error: Option<String>,
    health: &str,
) -> serde_json::Value {
    let active = available && enabled && config_validated;
    json!({
        "name": name,
        "available": available,
        "enabled": enabled,
        "active": active,
        "config_validated": config_validated,
        "config_error": if config_validated { serde_json::Value::Null } else { json!(config_error) },
        "display_name": display_name,
        "description": description,
        "category": category,
        "admin_route": if available { json!(admin_route) } else { serde_json::Value::Null },
        "admin_menu_icon": admin_menu_icon,
        "admin_menu_group": admin_menu_group,
        "admin_menu_order": admin_menu_order,
        "health": health,
    })
}

pub fn normalize_admin_system_export_api_formats(
    raw_formats: Option<&serde_json::Value>,
    mut signature_for: impl FnMut(&str) -> Option<String>,
) -> Vec<String> {
    let Some(raw_formats) = raw_formats.and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    let mut normalized = Vec::new();
    let mut seen = BTreeSet::new();
    for raw in raw_formats {
        let Some(value) = raw
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(signature) = signature_for(value) else {
            continue;
        };
        if seen.insert(signature.clone()) {
            normalized.push(signature);
        }
    }
    normalized
}

pub fn resolve_admin_system_export_key_api_formats(
    raw_formats: Option<&serde_json::Value>,
    provider_endpoint_formats: &[String],
    signature_for: impl FnMut(&str) -> Option<String>,
) -> Vec<String> {
    let normalized = normalize_admin_system_export_api_formats(raw_formats, signature_for);
    if !normalized.is_empty() {
        return normalized;
    }
    if raw_formats.is_none() {
        return provider_endpoint_formats.to_vec();
    }
    Vec::new()
}

pub fn collect_admin_system_export_provider_endpoint_formats(
    endpoints: &[StoredProviderCatalogEndpoint],
    mut signature_for: impl FnMut(&str) -> Option<String>,
) -> Vec<String> {
    endpoints
        .iter()
        .filter_map(|endpoint| signature_for(&endpoint.api_format))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn serialize_admin_system_users_export_wallet(
    wallet: Option<&StoredWalletSnapshot>,
) -> Option<serde_json::Value> {
    let wallet = wallet?;
    let recharge_balance = wallet.balance;
    let gift_balance = wallet.gift_balance;
    let spendable_balance = recharge_balance + gift_balance;
    let unlimited = wallet.limit_mode.eq_ignore_ascii_case("unlimited");

    Some(json!({
        "id": wallet.id.clone(),
        "balance": spendable_balance,
        "recharge_balance": recharge_balance,
        "gift_balance": gift_balance,
        "refundable_balance": recharge_balance,
        "currency": wallet.currency.clone(),
        "status": wallet.status.clone(),
        "limit_mode": wallet.limit_mode.clone(),
        "unlimited": unlimited,
        "total_recharged": wallet.total_recharged,
        "total_consumed": wallet.total_consumed,
        "total_refunded": wallet.total_refunded,
        "total_adjusted": wallet.total_adjusted,
        "updated_at": unix_secs_to_rfc3339(wallet.updated_at_unix_secs),
    }))
}

pub fn normalize_admin_system_config_key(requested_key: &str) -> String {
    let trimmed = requested_key.trim();
    if trimmed.eq_ignore_ascii_case(LEGACY_REQUEST_LOG_LEVEL_KEY) {
        REQUEST_RECORD_LEVEL_KEY.to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn admin_system_config_delete_keys(requested_key: &str) -> Vec<String> {
    let normalized = normalize_admin_system_config_key(requested_key);
    if normalized == REQUEST_RECORD_LEVEL_KEY {
        vec![
            REQUEST_RECORD_LEVEL_KEY.to_string(),
            LEGACY_REQUEST_LOG_LEVEL_KEY.to_string(),
        ]
    } else {
        vec![normalized]
    }
}

pub fn is_sensitive_admin_system_config_key(key: &str) -> bool {
    SENSITIVE_SYSTEM_CONFIG_KEYS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(key))
}

pub fn admin_system_config_default_value(key: &str) -> Option<serde_json::Value> {
    match key {
        "site_name" => Some(json!("Aether")),
        "site_subtitle" => Some(json!("AI Gateway")),
        "default_user_initial_gift_usd" => Some(json!(10.0)),
        "password_policy_level" => Some(json!("weak")),
        REQUEST_RECORD_LEVEL_KEY => Some(json!("basic")),
        "max_request_body_size" => Some(json!(5_242_880)),
        "max_response_body_size" => Some(json!(5_242_880)),
        "sensitive_headers" => Some(json!([
            "authorization",
            "x-api-key",
            "api-key",
            "cookie",
            "set-cookie"
        ])),
        "detail_log_retention_days" => Some(json!(7)),
        "compressed_log_retention_days" => Some(json!(30)),
        "header_retention_days" => Some(json!(90)),
        "log_retention_days" => Some(json!(365)),
        "enable_auto_cleanup" => Some(json!(true)),
        "cleanup_batch_size" => Some(json!(1000)),
        "request_candidates_retention_days" => Some(json!(30)),
        "request_candidates_cleanup_batch_size" => Some(json!(5000)),
        "enable_provider_checkin" => Some(json!(true)),
        "provider_checkin_time" => Some(json!("01:05")),
        "provider_priority_mode" => Some(json!("provider")),
        "scheduling_mode" => Some(json!("cache_affinity")),
        "auto_delete_expired_keys" => Some(json!(false)),
        "email_suffix_mode" => Some(json!("none")),
        "email_suffix_list" => Some(json!([])),
        "enable_format_conversion" => Some(json!(true)),
        "keep_priority_on_conversion" => Some(json!(false)),
        "audit_log_retention_days" => Some(json!(30)),
        "enable_db_maintenance" => Some(json!(true)),
        "system_proxy_node_id" => Some(serde_json::Value::Null),
        "smtp_host" => Some(serde_json::Value::Null),
        "smtp_port" => Some(json!(587)),
        "smtp_user" => Some(serde_json::Value::Null),
        "smtp_password" => Some(serde_json::Value::Null),
        "smtp_use_tls" => Some(json!(true)),
        "smtp_use_ssl" => Some(json!(false)),
        "smtp_from_email" => Some(serde_json::Value::Null),
        "smtp_from_name" => Some(json!("Aether")),
        "enable_oauth_token_refresh" => Some(json!(true)),
        _ => None,
    }
}

pub fn build_admin_system_configs_payload(
    entries: &[StoredSystemConfigEntry],
) -> serde_json::Value {
    let has_request_record_level = entries
        .iter()
        .any(|entry| entry.key == REQUEST_RECORD_LEVEL_KEY);
    json!(entries
        .iter()
        .filter_map(|entry| {
            if entry.key == LEGACY_REQUEST_LOG_LEVEL_KEY && has_request_record_level {
                return None;
            }
            let key = if entry.key == LEGACY_REQUEST_LOG_LEVEL_KEY {
                REQUEST_RECORD_LEVEL_KEY
            } else {
                entry.key.as_str()
            };
            Some(build_admin_system_config_list_item(
                key,
                &entry.value,
                entry.description.as_deref(),
                entry.updated_at_unix_secs,
            ))
        })
        .collect::<Vec<_>>())
}

pub fn build_admin_system_config_detail_payload(
    requested_key: &str,
    value: Option<serde_json::Value>,
) -> Result<serde_json::Value, (http::StatusCode, serde_json::Value)> {
    let normalized_key = normalize_admin_system_config_key(requested_key);
    let value = value.or_else(|| admin_system_config_default_value(&normalized_key));
    let Some(value) = value else {
        return Err((
            http::StatusCode::NOT_FOUND,
            json!({ "detail": format!("配置项 '{requested_key}' 不存在") }),
        ));
    };
    if is_sensitive_admin_system_config_key(&normalized_key) {
        return Ok(json!({
            "key": requested_key,
            "value": serde_json::Value::Null,
            "is_set": system_config_is_set(&value),
        }));
    }
    Ok(json!({
        "key": requested_key,
        "value": value,
    }))
}

pub fn parse_admin_system_config_update(
    requested_key: &str,
    request_body: &[u8],
) -> Result<AdminSystemConfigUpdate, (http::StatusCode, serde_json::Value)> {
    let payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(serde_json::Value::Object(payload)) => payload,
        _ => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
    };
    let normalized_key = normalize_admin_system_config_key(requested_key);
    let mut value = payload
        .get("value")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let description = match payload.get("description") {
        Some(serde_json::Value::String(value)) => Some(value.trim().to_string()),
        Some(serde_json::Value::Null) | None => None,
        Some(_) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                json!({ "detail": "请求数据验证失败" }),
            ));
        }
    };

    if normalized_key == "password_policy_level" {
        match value.as_str().map(str::trim) {
            Some("weak" | "medium" | "strong") => {
                value = json!(value.as_str().unwrap().trim());
            }
            Some(_) => {
                return Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                ));
            }
            None if value.is_null() => {
                value = json!("weak");
            }
            None => {
                return Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": "请求数据验证失败" }),
                ));
            }
        }
    }

    Ok(AdminSystemConfigUpdate {
        normalized_key,
        value,
        description,
    })
}

pub fn build_admin_system_config_updated_payload(
    key: String,
    value: serde_json::Value,
    description: Option<String>,
    updated_at_unix_secs: Option<u64>,
) -> serde_json::Value {
    json!({
        "key": key,
        "value": value,
        "description": description,
        "updated_at": updated_at_unix_secs.and_then(unix_secs_to_rfc3339),
    })
}

pub fn build_admin_system_config_deleted_payload(requested_key: &str) -> serde_json::Value {
    json!({
        "message": format!("配置项 '{}' 已删除", requested_key.trim()),
    })
}

pub fn is_admin_management_tokens_root(request_path: &str) -> bool {
    matches!(
        request_path,
        "/api/admin/management-tokens" | "/api/admin/management-tokens/"
    )
}

pub fn is_admin_system_configs_root(request_path: &str) -> bool {
    matches!(
        request_path,
        "/api/admin/system/configs" | "/api/admin/system/configs/"
    )
}

pub fn is_admin_system_email_templates_root(request_path: &str) -> bool {
    matches!(
        request_path,
        "/api/admin/system/email/templates" | "/api/admin/system/email/templates/"
    )
}

pub fn admin_system_config_key_from_path(request_path: &str) -> Option<String> {
    path_identifier_from_path(request_path, "/api/admin/system/configs/")
}

pub fn admin_system_email_template_type_from_path(request_path: &str) -> Option<String> {
    path_identifier_from_path(request_path, "/api/admin/system/email/templates/")
}

pub fn admin_system_email_template_preview_type_from_path(request_path: &str) -> Option<String> {
    suffixed_path_identifier_from_path(
        request_path,
        "/api/admin/system/email/templates/",
        "/preview",
    )
}

pub fn admin_system_email_template_reset_type_from_path(request_path: &str) -> Option<String> {
    suffixed_path_identifier_from_path(request_path, "/api/admin/system/email/templates/", "/reset")
}

pub fn admin_management_token_id_from_path(request_path: &str) -> Option<String> {
    path_identifier_from_path(request_path, "/api/admin/management-tokens/")
}

pub fn admin_management_token_status_id_from_path(request_path: &str) -> Option<String> {
    suffixed_path_identifier_from_path(request_path, "/api/admin/management-tokens/", "/status")
}

pub fn admin_adaptive_effective_limit(key: &StoredProviderCatalogKey) -> Option<u32> {
    if key.rpm_limit.is_none() {
        key.learned_rpm_limit
    } else {
        key.rpm_limit
    }
}

pub fn admin_adaptive_adjustment_items(
    value: Option<&serde_json::Value>,
) -> Vec<serde_json::Map<String, serde_json::Value>> {
    value
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_object)
        .cloned()
        .collect()
}

pub fn admin_adaptive_key_payload(key: &StoredProviderCatalogKey) -> serde_json::Value {
    json!({
        "id": key.id,
        "name": key.name,
        "provider_id": key.provider_id,
        "api_formats": key
            .api_formats
            .as_ref()
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        "is_adaptive": key.rpm_limit.is_none(),
        "rpm_limit": key.rpm_limit,
        "effective_limit": admin_adaptive_effective_limit(key),
        "learned_rpm_limit": key.learned_rpm_limit,
        "concurrent_429_count": key.concurrent_429_count.unwrap_or(0),
        "rpm_429_count": key.rpm_429_count.unwrap_or(0),
    })
}

pub fn build_admin_adaptive_summary_payload(
    keys: &[StoredProviderCatalogKey],
) -> serde_json::Value {
    let adaptive_keys = keys
        .iter()
        .filter(|key| key.rpm_limit.is_none())
        .collect::<Vec<_>>();

    let total_keys = adaptive_keys.len() as u64;
    let total_concurrent_429 = adaptive_keys
        .iter()
        .map(|key| u64::from(key.concurrent_429_count.unwrap_or(0)))
        .sum::<u64>();
    let total_rpm_429 = adaptive_keys
        .iter()
        .map(|key| u64::from(key.rpm_429_count.unwrap_or(0)))
        .sum::<u64>();

    let mut recent_adjustments = Vec::new();
    let mut total_adjustments = 0usize;
    for key in adaptive_keys {
        let history = admin_adaptive_adjustment_items(key.adjustment_history.as_ref());
        total_adjustments += history.len();
        for adjustment in history.into_iter().rev().take(3) {
            let mut payload = adjustment;
            payload.insert("key_id".to_string(), json!(key.id));
            payload.insert("key_name".to_string(), json!(key.name));
            recent_adjustments.push(serde_json::Value::Object(payload));
        }
    }

    recent_adjustments.sort_by(|left, right| {
        let lhs = left
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let rhs = right
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        rhs.cmp(lhs)
    });

    json!({
        "total_adaptive_keys": total_keys,
        "total_concurrent_429_errors": total_concurrent_429,
        "total_rpm_429_errors": total_rpm_429,
        "total_adjustments": total_adjustments,
        "recent_adjustments": recent_adjustments.into_iter().take(10).collect::<Vec<_>>(),
    })
}

pub fn build_admin_adaptive_stats_payload(key: &StoredProviderCatalogKey) -> serde_json::Value {
    let status_snapshot = key
        .status_snapshot
        .as_ref()
        .and_then(serde_json::Value::as_object);
    let adjustments = admin_adaptive_adjustment_items(key.adjustment_history.as_ref());
    let adjustment_count = adjustments.len();
    let recent_adjustments = adjustments
        .into_iter()
        .rev()
        .take(10)
        .map(serde_json::Value::Object)
        .collect::<Vec<_>>();

    json!({
        "adaptive_mode": key.rpm_limit.is_none(),
        "rpm_limit": key.rpm_limit,
        "effective_limit": admin_adaptive_effective_limit(key),
        "learned_limit": key.learned_rpm_limit,
        "concurrent_429_count": key.concurrent_429_count.unwrap_or(0),
        "rpm_429_count": key.rpm_429_count.unwrap_or(0),
        "last_429_at": key.last_429_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "last_429_type": key.last_429_type,
        "adjustment_count": adjustment_count,
        "recent_adjustments": recent_adjustments,
        "learning_confidence": status_snapshot.and_then(|value| value.get("learning_confidence")).cloned(),
        "enforcement_active": status_snapshot.and_then(|value| value.get("enforcement_active")).cloned(),
        "observation_count": status_snapshot
            .and_then(|value| value.get("observation_count"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        "header_observation_count": status_snapshot
            .and_then(|value| value.get("header_observation_count"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        "latest_upstream_limit": status_snapshot
            .and_then(|value| value.get("latest_upstream_limit"))
            .and_then(serde_json::Value::as_u64),
    })
}

pub fn build_admin_adaptive_toggle_mode_payload(
    updated: &StoredProviderCatalogKey,
    message: String,
) -> serde_json::Value {
    json!({
        "message": message,
        "key_id": updated.id,
        "is_adaptive": updated.rpm_limit.is_none(),
        "rpm_limit": updated.rpm_limit,
        "effective_limit": admin_adaptive_effective_limit(updated),
    })
}

pub fn build_admin_adaptive_set_limit_payload(
    updated: &StoredProviderCatalogKey,
    was_adaptive: bool,
    limit: u32,
) -> serde_json::Value {
    json!({
        "message": format!("已设置为固定限制模式，RPM 限制为 {limit}"),
        "key_id": updated.id,
        "is_adaptive": false,
        "rpm_limit": updated.rpm_limit,
        "previous_mode": if was_adaptive { "adaptive" } else { "fixed" },
    })
}

pub fn build_admin_adaptive_reset_learning_payload(key_id: &str) -> serde_json::Value {
    json!({
        "message": "学习状态已重置",
        "key_id": key_id,
    })
}

pub fn admin_adaptive_key_not_found_response(key_id: &str) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": format!("Key {key_id} 不存在") })),
    )
        .into_response()
}

pub fn admin_adaptive_dispatcher_not_found_response() -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": "Adaptive route not found" })),
    )
        .into_response()
}

pub fn admin_adaptive_key_id_from_path(path: &str) -> Option<String> {
    let normalized = path.trim_end_matches('/');
    let mut segments = normalized.split('/').filter(|segment| !segment.is_empty());
    match (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) {
        (Some("api"), Some("admin"), Some("adaptive"), Some("keys"), Some(key_id))
            if !key_id.is_empty() =>
        {
            Some(key_id.to_string())
        }
        _ => None,
    }
}

pub const ADMIN_PROXY_NODES_DATA_UNAVAILABLE_DETAIL: &str = "Admin proxy nodes data unavailable";

pub fn build_admin_proxy_nodes_data_unavailable_response() -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": ADMIN_PROXY_NODES_DATA_UNAVAILABLE_DETAIL })),
    )
        .into_response()
}

pub fn build_admin_proxy_nodes_invalid_status_response() -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({
            "detail": "status 必须是以下之一: ['offline', 'online']"
        })),
    )
        .into_response()
}

pub fn build_admin_proxy_nodes_not_found_response() -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": "Proxy node 不存在" })),
    )
        .into_response()
}

pub fn build_admin_proxy_node_payload(node: &StoredProxyNode) -> serde_json::Value {
    let mut payload = serde_json::Map::from_iter([
        ("id".to_string(), json!(node.id)),
        ("name".to_string(), json!(node.name)),
        ("ip".to_string(), json!(node.ip)),
        ("port".to_string(), json!(node.port)),
        ("region".to_string(), json!(node.region)),
        ("status".to_string(), json!(node.status)),
        ("is_manual".to_string(), json!(node.is_manual)),
        ("tunnel_mode".to_string(), json!(node.tunnel_mode)),
        ("tunnel_connected".to_string(), json!(node.tunnel_connected)),
        (
            "tunnel_connected_at".to_string(),
            json!(node
                .tunnel_connected_at_unix_secs
                .and_then(unix_secs_to_rfc3339)),
        ),
        ("registered_by".to_string(), json!(node.registered_by)),
        (
            "last_heartbeat_at".to_string(),
            json!(node
                .last_heartbeat_at_unix_secs
                .and_then(unix_secs_to_rfc3339)),
        ),
        (
            "heartbeat_interval".to_string(),
            json!(node.heartbeat_interval),
        ),
        (
            "active_connections".to_string(),
            json!(node.active_connections),
        ),
        ("total_requests".to_string(), json!(node.total_requests)),
        ("avg_latency_ms".to_string(), json!(node.avg_latency_ms)),
        ("failed_requests".to_string(), json!(node.failed_requests)),
        ("dns_failures".to_string(), json!(node.dns_failures)),
        ("stream_errors".to_string(), json!(node.stream_errors)),
        ("proxy_metadata".to_string(), json!(node.proxy_metadata)),
        ("hardware_info".to_string(), json!(node.hardware_info)),
        (
            "estimated_max_concurrency".to_string(),
            json!(node.estimated_max_concurrency),
        ),
        ("remote_config".to_string(), json!(node.remote_config)),
        ("config_version".to_string(), json!(node.config_version)),
        (
            "created_at".to_string(),
            json!(node.created_at_unix_secs.and_then(unix_secs_to_rfc3339)),
        ),
        (
            "updated_at".to_string(),
            json!(node.updated_at_unix_secs.and_then(unix_secs_to_rfc3339)),
        ),
    ]);

    if node.is_manual {
        payload.insert("proxy_url".to_string(), json!(node.proxy_url));
        payload.insert("proxy_username".to_string(), json!(node.proxy_username));
        payload.insert(
            "proxy_password".to_string(),
            json!(mask_admin_proxy_node_password(
                node.proxy_password.as_deref()
            )),
        );
    }

    serde_json::Value::Object(payload)
}

pub fn build_admin_proxy_node_event_payload(event: &StoredProxyNodeEvent) -> serde_json::Value {
    json!({
        "id": event.id,
        "event_type": event.event_type,
        "detail": event.detail,
        "created_at": event.created_at_unix_secs.and_then(unix_secs_to_rfc3339),
    })
}

pub fn admin_proxy_node_event_node_id_from_path(request_path: &str) -> Option<&str> {
    let node_id = request_path.strip_prefix("/api/admin/proxy-nodes/")?;
    let node_id = node_id.strip_suffix("/events")?;
    if node_id.is_empty() || node_id.contains('/') {
        None
    } else {
        Some(node_id)
    }
}

pub fn build_admin_proxy_nodes_list_payload_response(
    items: Vec<serde_json::Value>,
    total: usize,
    skip: usize,
    limit: usize,
) -> Response<Body> {
    Json(json!({
        "items": items,
        "total": total,
        "skip": skip,
        "limit": limit,
    }))
    .into_response()
}

pub fn build_admin_proxy_node_events_payload_response(
    items: Vec<serde_json::Value>,
) -> Response<Body> {
    Json(json!({ "items": items })).into_response()
}

fn system_config_is_set(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(value) => value
            .as_i64()
            .map(|value| value != 0)
            .or_else(|| value.as_u64().map(|value| value != 0))
            .or_else(|| value.as_f64().map(|value| value != 0.0))
            .unwrap_or(false),
        serde_json::Value::String(value) => !value.trim().is_empty(),
        serde_json::Value::Array(value) => !value.is_empty(),
        serde_json::Value::Object(value) => !value.is_empty(),
    }
}

fn build_admin_system_config_list_item(
    key: &str,
    value: &serde_json::Value,
    description: Option<&str>,
    updated_at_unix_secs: Option<u64>,
) -> serde_json::Value {
    let masked_value = if is_sensitive_admin_system_config_key(key) {
        serde_json::Value::Null
    } else {
        value.clone()
    };
    let is_set = is_sensitive_admin_system_config_key(key).then(|| system_config_is_set(value));
    let mut payload = json!({
        "key": key,
        "description": description,
        "updated_at": updated_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "value": masked_value,
    });
    if let Some(is_set) = is_set {
        payload["is_set"] = json!(is_set);
    }
    payload
}

fn unix_secs_to_rfc3339(unix_secs: u64) -> Option<String> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix_secs as i64, 0)
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn path_identifier_from_path(request_path: &str, prefix: &str) -> Option<String> {
    let value = request_path
        .strip_prefix(prefix)?
        .trim()
        .trim_matches('/')
        .to_string();
    if value.is_empty() || value.contains('/') {
        None
    } else {
        Some(value)
    }
}

fn suffixed_path_identifier_from_path(
    request_path: &str,
    prefix: &str,
    suffix: &str,
) -> Option<String> {
    request_path
        .strip_prefix(prefix)?
        .strip_suffix(suffix)
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty() && !value.contains('/'))
}

fn mask_admin_proxy_node_password(password: Option<&str>) -> Option<String> {
    let password = password?;
    if password.is_empty() {
        return None;
    }
    if password.len() < 8 {
        return Some("****".to_string());
    }
    Some(format!(
        "{}****{}",
        &password[..2],
        &password[password.len() - 2..]
    ))
}
