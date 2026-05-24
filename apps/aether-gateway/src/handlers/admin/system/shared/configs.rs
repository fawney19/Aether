use crate::handlers::admin::request::AdminAppState;
use crate::handlers::shared::unix_secs_to_rfc3339;
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
use aether_crypto::encrypt_python_fernet_plaintext;
use axum::body::Bytes;
use axum::http;
use serde_json::json;

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
    build_admin_system_configs_payload_pure(entries)
}

pub(crate) async fn build_admin_system_config_detail_payload(
    state: &AdminAppState<'_>,
    requested_key: &str,
) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError> {
    let requested_key = requested_key.trim();
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
        let Some(encryption_key) = state
            .encryption_key()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(Err((
                http::StatusCode::SERVICE_UNAVAILABLE,
                json!({ "detail": "系统配置写入需要可用的加密密钥" }),
            )));
        };
        let plaintext = value.as_str().unwrap();
        value = json!(encrypt_python_fernet_plaintext(encryption_key, plaintext)
            .map_err(|err| GatewayError::Internal(err.to_string()))?);
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
