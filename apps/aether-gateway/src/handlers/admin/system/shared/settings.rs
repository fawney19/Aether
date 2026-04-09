use crate::handlers::admin::request::AdminAppState;
use crate::handlers::shared::{system_config_bool, system_config_string};
use crate::GatewayError;
use aether_admin::system::{
    build_admin_api_formats_payload as build_admin_api_formats_payload_pure,
    build_admin_system_check_update_payload as build_admin_system_check_update_payload_pure,
    build_admin_system_settings_payload as build_admin_system_settings_payload_pure,
    build_admin_system_settings_updated_payload,
    build_admin_system_stats_payload as build_admin_system_stats_payload_pure,
    parse_admin_system_settings_update,
};
use axum::body::Bytes;
use axum::http;
use serde_json::json;
use std::fs;

pub(crate) fn current_aether_version() -> String {
    let version_file =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/_version.py");
    if let Ok(contents) = fs::read_to_string(version_file) {
        for line in contents.lines() {
            let line = line.trim();
            if let Some(version) = line
                .strip_prefix("__version__ = version = '")
                .and_then(|value| value.strip_suffix('\''))
            {
                if !version.is_empty() {
                    return version.to_string();
                }
            }
        }
    }

    env!("CARGO_PKG_VERSION").to_string()
}

pub(crate) fn build_admin_system_check_update_payload() -> serde_json::Value {
    build_admin_system_check_update_payload_pure(current_aether_version())
}

pub(crate) async fn build_admin_system_stats_payload(
    state: &AdminAppState<'_>,
) -> Result<serde_json::Value, GatewayError> {
    let providers = state
        .list_provider_catalog_providers(false)
        .await
        .unwrap_or_default();
    let total_providers = providers.len() as u64;
    let active_providers = providers
        .iter()
        .filter(|provider| provider.is_active)
        .count() as u64;
    let stats = state.read_admin_system_stats().await?;

    Ok(build_admin_system_stats_payload_pure(
        stats.total_users,
        stats.active_users,
        total_providers,
        active_providers,
        stats.total_api_keys,
        stats.total_requests,
    ))
}

pub(crate) async fn build_admin_system_settings_payload(
    state: &AdminAppState<'_>,
) -> Result<serde_json::Value, GatewayError> {
    let default_provider_config = state
        .read_system_config_json_value("default_provider")
        .await?;
    let default_model_config = state.read_system_config_json_value("default_model").await?;
    let enable_usage_tracking_config = state
        .read_system_config_json_value("enable_usage_tracking")
        .await?;
    let password_policy_level_config = state
        .read_system_config_json_value("password_policy_level")
        .await?;

    let default_provider = match system_config_string(default_provider_config.as_ref()) {
        Some(value) => Some(value),
        None => state
            .list_provider_catalog_providers(false)
            .await
            .ok()
            .unwrap_or_default()
            .into_iter()
            .find(|provider| provider.is_active)
            .map(|provider| provider.name),
    };
    let default_model = system_config_string(default_model_config.as_ref());
    let enable_usage_tracking = system_config_bool(enable_usage_tracking_config.as_ref(), true);
    let password_policy_level = match system_config_string(password_policy_level_config.as_ref()) {
        Some(value) if matches!(value.as_str(), "weak" | "medium" | "strong") => value,
        _ => "weak".to_string(),
    };

    Ok(build_admin_system_settings_payload_pure(
        default_provider,
        default_model,
        enable_usage_tracking,
        password_policy_level,
    ))
}

pub(crate) async fn apply_admin_system_settings_update(
    state: &AdminAppState<'_>,
    request_body: &Bytes,
) -> Result<Result<serde_json::Value, (http::StatusCode, serde_json::Value)>, GatewayError> {
    let update = match parse_admin_system_settings_update(request_body) {
        Ok(update) => update,
        Err(err) => return Ok(Err(err)),
    };

    if let Some(default_provider) = update.default_provider {
        if let Some(default_provider) = default_provider {
            let provider_exists = state
                .list_provider_catalog_providers(false)
                .await
                .ok()
                .unwrap_or_default()
                .into_iter()
                .any(|provider| provider.is_active && provider.name == default_provider);
            if !provider_exists {
                return Ok(Err((
                    http::StatusCode::BAD_REQUEST,
                    json!({ "detail": format!("提供商 '{default_provider}' 不存在或未启用") }),
                )));
            }
            let _ = state
                .upsert_system_config_json_value(
                    "default_provider",
                    &json!(default_provider),
                    Some("系统默认提供商，当用户未设置个人提供商时使用"),
                )
                .await?;
        } else {
            let _ = state
                .upsert_system_config_json_value("default_provider", &serde_json::Value::Null, None)
                .await?;
        }
    }

    if let Some(default_model) = update.default_model {
        let config_value = default_model
            .map(|value| json!(value))
            .unwrap_or(serde_json::Value::Null);
        let _ = state
            .upsert_system_config_json_value("default_model", &config_value, None)
            .await?;
    }

    if let Some(enable_usage_tracking) = update.enable_usage_tracking {
        let _ = state
            .upsert_system_config_json_value(
                "enable_usage_tracking",
                &json!(enable_usage_tracking),
                None,
            )
            .await?;
    }

    if let Some(password_policy_level) = update.password_policy_level {
        let _ = state
            .upsert_system_config_json_value(
                "password_policy_level",
                &json!(password_policy_level),
                None,
            )
            .await?;
    }

    Ok(Ok(build_admin_system_settings_updated_payload()))
}

pub(crate) fn build_admin_api_formats_payload() -> serde_json::Value {
    build_admin_api_formats_payload_pure()
}
