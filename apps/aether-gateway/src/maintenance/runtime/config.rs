use aether_data_contracts::DataLayerError;
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::data::GatewayDataState;

use super::{UsageCleanupSettings, UsageCleanupWindow};

pub(super) async fn system_config_bool(
    data: &GatewayDataState,
    key: &str,
    default: bool,
) -> Result<bool, DataLayerError> {
    Ok(match data.find_system_config_value(key).await? {
        Some(Value::Bool(value)) => value,
        Some(Value::Number(value)) => value.as_i64().map(|raw| raw != 0).unwrap_or(default),
        Some(Value::String(value)) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => true,
            "false" | "0" | "no" | "off" => false,
            _ => default,
        },
        _ => default,
    })
}

pub(super) async fn system_config_u64(
    data: &GatewayDataState,
    key: &str,
    default: u64,
) -> Result<u64, DataLayerError> {
    Ok(match data.find_system_config_value(key).await? {
        Some(Value::Number(value)) => value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|raw| u64::try_from(raw).ok()))
            .unwrap_or(default),
        Some(Value::String(value)) => value.trim().parse::<u64>().unwrap_or(default),
        _ => default,
    })
}

pub(super) async fn system_config_usize(
    data: &GatewayDataState,
    key: &str,
    default: usize,
) -> Result<usize, DataLayerError> {
    Ok(match data.find_system_config_value(key).await? {
        Some(Value::Number(value)) => value
            .as_u64()
            .and_then(|raw| usize::try_from(raw).ok())
            .or_else(|| {
                value
                    .as_i64()
                    .and_then(|raw| u64::try_from(raw).ok())
                    .and_then(|raw| usize::try_from(raw).ok())
            })
            .unwrap_or(default),
        Some(Value::String(value)) => value.trim().parse::<usize>().unwrap_or(default),
        _ => default,
    })
}

pub(super) async fn system_config_string(
    data: &GatewayDataState,
    key: &str,
    default: &str,
) -> Result<String, DataLayerError> {
    Ok(match data.find_system_config_value(key).await? {
        Some(Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                default.to_string()
            } else {
                trimmed.to_string()
            }
        }
        _ => default.to_string(),
    })
}

pub(super) async fn pending_cleanup_timeout_minutes(
    data: &GatewayDataState,
) -> Result<u64, DataLayerError> {
    system_config_u64(data, "pending_request_timeout_minutes", 10).await
}

pub(super) async fn pending_cleanup_batch_size(
    data: &GatewayDataState,
) -> Result<usize, DataLayerError> {
    Ok(system_config_usize(data, "cleanup_batch_size", 1_000)
        .await?
        .clamp(1, 200))
}

pub(super) async fn usage_cleanup_settings(
    data: &GatewayDataState,
) -> Result<UsageCleanupSettings, DataLayerError> {
    Ok(UsageCleanupSettings {
        detail_retention_days: system_config_u64(data, "detail_log_retention_days", 7).await?,
        compressed_retention_days: system_config_u64(data, "compressed_log_retention_days", 30)
            .await?,
        header_retention_days: system_config_u64(data, "header_retention_days", 90).await?,
        log_retention_days: system_config_u64(data, "log_retention_days", 365).await?,
        batch_size: system_config_usize(data, "cleanup_batch_size", 1_000)
            .await?
            .max(1),
        auto_delete_expired_keys: system_config_bool(data, "auto_delete_expired_keys", false)
            .await?,
    })
}

pub(super) fn usage_cleanup_window(
    now_utc: DateTime<Utc>,
    settings: UsageCleanupSettings,
) -> UsageCleanupWindow {
    let minutes = |days: u64| chrono::Duration::days(i64::try_from(days).unwrap_or(i64::MAX));
    UsageCleanupWindow {
        detail_cutoff: now_utc - minutes(settings.detail_retention_days),
        compressed_cutoff: now_utc - minutes(settings.compressed_retention_days),
        header_cutoff: now_utc - minutes(settings.header_retention_days),
        log_cutoff: now_utc - minutes(settings.log_retention_days),
    }
}
