use crate::handlers::admin::model::ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY;
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::shared::{
    bark_device_key_binding, encrypt_bark_device_key, encrypt_smtp_password, smtp_password_binding,
    system_config_bool, system_config_string, unix_secs_to_rfc3339,
};
use crate::GatewayError;
use aether_admin::system::{
    admin_system_config_default_value as admin_system_config_default_value_pure,
    admin_system_config_delete_keys as admin_system_config_delete_keys_pure,
    build_admin_system_config_deleted_payload,
    build_admin_system_config_detail_payload as build_admin_system_config_detail_payload_pure,
    build_admin_system_config_updated_payload,
    build_admin_system_configs_payload as build_admin_system_configs_payload_pure,
    is_sensitive_admin_system_config_key as is_sensitive_admin_system_config_key_pure,
    normalize_admin_system_config_key as normalize_admin_system_config_key_pure,
    parse_admin_system_config_update,
};
use axum::body::Bytes;
use axum::http;
use serde_json::json;

const ADMIN_EXTERNAL_MODELS_CONFIG_ROUTE: &str = "/api/admin/models/external/config";

fn is_external_models_proxy_node_config_key(key: &str) -> bool {
    key.trim()
        .eq_ignore_ascii_case(ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY)
}

fn external_models_proxy_node_config_owner_error(
    key: &str,
) -> Option<(http::StatusCode, serde_json::Value)> {
    is_external_models_proxy_node_config_key(key).then(|| {
        (
            http::StatusCode::BAD_REQUEST,
            json!({
                "detail": format!(
                    "配置项 '{}' 由模型目录管理，请使用 {}",
                    ADMIN_EXTERNAL_MODELS_PROXY_NODE_CONFIG_KEY,
                    ADMIN_EXTERNAL_MODELS_CONFIG_ROUTE,
                )
            }),
        )
    })
}

fn normalize_admin_system_config_key(requested_key: &str) -> String {
    normalize_admin_system_config_key_pure(requested_key)
}

fn admin_system_config_delete_keys(requested_key: &str) -> Vec<String> {
    admin_system_config_delete_keys_pure(requested_key)
}

pub(crate) fn is_sensitive_admin_system_config_key(key: &str) -> bool {
    is_sensitive_admin_system_config_key_pure(key)
}

fn admin_system_config_default_value(key: &str) -> Option<serde_json::Value> {
    admin_system_config_default_value_pure(key)
}

fn legacy_admin_system_config_fallback_key(normalized_key: &str) -> Option<&'static str> {
    match normalized_key {
        "module.server_chan_push.enabled" => {
            Some("module.important_notification.server_chan_enabled")
        }
        "module.server_chan_push.send_key" => {
            Some("module.important_notification.server_chan_send_key")
        }
        "module.server_chan_push.template" => {
            Some("module.important_notification.server_chan_template")
        }
        _ => None,
    }
}

pub(crate) fn build_admin_system_configs_payload(
    entries: &[aether_data::repository::system::StoredSystemConfigEntry],
) -> serde_json::Value {
    let visible_entries = entries
        .iter()
        .filter(|entry| !is_external_models_proxy_node_config_key(&entry.key))
        .cloned()
        .collect::<Vec<_>>();
    build_admin_system_configs_payload_pure(&visible_entries)
}

pub(crate) async fn build_admin_system_config_detail_payload(
    state: &AdminAppState<'_>,
    requested_key: &str,
) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError> {
    let requested_key = requested_key.trim();
    if let Some(error) = external_models_proxy_node_config_owner_error(requested_key) {
        return Ok(Err(error));
    }
    let normalized_key = normalize_admin_system_config_key(requested_key);
    let mut value = state.read_system_config_json_value(&normalized_key).await?;
    if value.is_none() {
        if let Some(legacy_key) = legacy_admin_system_config_fallback_key(&normalized_key) {
            value = state.read_system_config_json_value(legacy_key).await?;
        }
    }
    let value = value.or_else(|| admin_system_config_default_value(&normalized_key));
    Ok(build_admin_system_config_detail_payload_pure(
        requested_key,
        value,
    ))
}

pub(crate) async fn apply_admin_system_config_update(
    state: &AdminAppState<'_>,
    requested_key: &str,
    request_body: &Bytes,
) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError> {
    if let Some(error) = external_models_proxy_node_config_owner_error(requested_key) {
        return Ok(Err(error));
    }
    let update = match parse_admin_system_config_update(requested_key, request_body) {
        Ok(update) => update,
        Err(err) => return Ok(Err(err)),
    };
    let mut value = update.value;
    let normalized_key = update.normalized_key;
    let description = update.description;

    if is_sensitive_admin_system_config_key(&normalized_key)
        && value.as_str().is_some_and(|raw| !raw.is_empty())
    {
        let plaintext = value
            .as_str()
            .expect("sensitive config value was a non-empty string");
        let encrypted = if normalized_key.eq_ignore_ascii_case("smtp_password") {
            let host = state
                .read_system_config_json_value("smtp_host")
                .await?
                .and_then(|value| system_config_string(Some(&value)));
            let port = state
                .read_system_config_json_value("smtp_port")
                .await?
                .map(|value| crate::email_delivery::system_config_u16(Some(&value), 587))
                .unwrap_or(587);
            let user = state
                .read_system_config_json_value("smtp_user")
                .await?
                .and_then(|value| system_config_string(Some(&value)));
            let use_tls = state
                .read_system_config_json_value("smtp_use_tls")
                .await?
                .map(|value| system_config_bool(Some(&value), true))
                .unwrap_or(true);
            let use_ssl = state
                .read_system_config_json_value("smtp_use_ssl")
                .await?
                .map(|value| system_config_bool(Some(&value), false))
                .unwrap_or(false);
            let Some(binding) = host.as_deref().and_then(|host| {
                smtp_password_binding(host, port, user.as_deref(), use_tls, use_ssl)
            }) else {
                return Ok(Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({
                        "detail": "保存 SMTP 密码前必须先配置有效的 smtp_host 和 smtp_user"
                    }),
                )));
            };
            encrypt_smtp_password(state.app(), &binding, plaintext)
        } else if normalized_key.eq_ignore_ascii_case("module.bark_push.device_key") {
            let server_url = state
                .read_system_config_json_value("module.bark_push.server_url")
                .await?
                .and_then(|value| system_config_string(Some(&value)))
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "https://api.day.app".to_string());
            let Some(binding) = bark_device_key_binding(&server_url) else {
                return Ok(Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({
                        "detail": "保存 Bark Device Key 前必须先配置有效的 module.bark_push.server_url"
                    }),
                )));
            };
            encrypt_bark_device_key(state.app(), &binding, plaintext)
        } else {
            state.encrypt_system_config_secret(&normalized_key, plaintext)
        };
        let Some(encrypted) = encrypted else {
            return Ok(Err((
                http::StatusCode::SERVICE_UNAVAILABLE,
                json!({ "detail": "系统配置写入需要可用的加密密钥" }),
            )));
        };
        value = json!(encrypted);
    }

    let updated = state
        .upsert_system_config_entry(&normalized_key, &value, description.as_deref())
        .await?;
    let display_value = if is_sensitive_admin_system_config_key(&normalized_key) {
        json!("********")
    } else {
        updated.value.clone()
    };
    Ok(Ok(build_admin_system_config_updated_payload(
        updated.key,
        display_value,
        updated.description,
        updated.updated_at_unix_secs,
    )))
}

pub(crate) async fn delete_admin_system_config(
    state: &AdminAppState<'_>,
    requested_key: &str,
) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError> {
    if let Some(error) = external_models_proxy_node_config_owner_error(requested_key) {
        return Ok(Err(error));
    }
    let delete_keys = admin_system_config_delete_keys(requested_key);
    let mut deleted = false;
    for key in &delete_keys {
        deleted |= state.delete_system_config_value(key).await?;
    }
    if !deleted {
        return Ok(Err((
            http::StatusCode::NOT_FOUND,
            json!({ "detail": format!("配置项 '{requested_key}' 不存在") }),
        )));
    }
    Ok(Ok(build_admin_system_config_deleted_payload(requested_key)))
}
