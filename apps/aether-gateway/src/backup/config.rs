use std::fmt;

use serde_json::{Map, Value};

use super::schedule::{BackupSchedule, BackupScheduleUnit};
use super::scopes::BackupScope;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct S3BackupConfig {
    pub(crate) enabled: bool,
    pub(crate) scope: BackupScope,
    pub(crate) endpoint: String,
    pub(crate) region: String,
    pub(crate) user_agent: String,
    pub(crate) bucket: String,
    pub(crate) prefix: String,
    pub(crate) access_key_id: String,
    pub(crate) secret_access_key: String,
    pub(crate) path_style: bool,
    pub(crate) compression: String,
    pub(crate) schedule: BackupSchedule,
    pub(crate) retention_count: u32,
}

impl fmt::Debug for S3BackupConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let endpoint_origin = sanitized_endpoint_origin(&self.endpoint);
        formatter
            .debug_struct("S3BackupConfig")
            .field("enabled", &self.enabled)
            .field("scope", &self.scope)
            .field("endpoint_origin", &endpoint_origin)
            .field("region", &self.region)
            .field("user_agent", &self.user_agent)
            .field("bucket", &self.bucket)
            .field("prefix", &self.prefix)
            .field("has_access_key_id", &!self.access_key_id.is_empty())
            .field("has_secret_access_key", &!self.secret_access_key.is_empty())
            .field("path_style", &self.path_style)
            .field("compression", &self.compression)
            .field("schedule", &self.schedule)
            .field("retention_count", &self.retention_count)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackupConfigError {
    message: String,
}

impl BackupConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for BackupConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BackupConfigError {}

impl S3BackupConfig {
    pub(crate) fn from_json_map(entries: &Map<String, Value>) -> Result<Self, BackupConfigError> {
        let enabled = optional_bool(entries, "backup_s3_enabled")?.unwrap_or(false);
        let mut schedule = BackupSchedule::default();
        schedule.unit = optional_string(entries, "backup_s3_schedule_unit")?
            .map(|value| {
                BackupScheduleUnit::from_config_value(&value)
                    .ok_or_else(|| BackupConfigError::new("Schedule Unit（计划单位）配置值无效"))
            })
            .transpose()?
            .unwrap_or(schedule.unit);
        schedule.interval =
            optional_u32(entries, "backup_s3_schedule_interval")?.unwrap_or(schedule.interval);
        schedule.minute =
            optional_u32(entries, "backup_s3_schedule_minute")?.unwrap_or(schedule.minute);
        schedule.hour = optional_u32(entries, "backup_s3_schedule_hour")?.unwrap_or(schedule.hour);
        schedule.weekday =
            optional_u32(entries, "backup_s3_schedule_weekday")?.unwrap_or(schedule.weekday);
        schedule.month_day =
            optional_u32(entries, "backup_s3_schedule_month_day")?.unwrap_or(schedule.month_day);
        validate_range("Interval（计划间隔）", schedule.interval, 1, u32::MAX)?;
        validate_range("Minute（计划分钟）", schedule.minute, 0, 59)?;
        validate_range("Hour（计划小时）", schedule.hour, 0, 23)?;
        validate_range("Weekday（计划星期）", schedule.weekday, 1, 7)?;
        validate_range("Month Day（计划月日）", schedule.month_day, 1, 31)?;

        let scope = optional_string(entries, "backup_s3_scope")?
            .map(|value| {
                BackupScope::from_config_value(&value)
                    .ok_or_else(|| BackupConfigError::new("Scope（备份范围）配置值无效"))
            })
            .transpose()?
            .unwrap_or(BackupScope::Data);
        let retention_count = optional_u32(entries, "backup_s3_retention_count")?.unwrap_or(7);
        validate_range("Retention（保留份数）", retention_count, 1, u32::MAX)?;
        let endpoint = required_or_disabled_string(
            entries,
            "backup_s3_endpoint",
            "Endpoint（S3 地址）",
            enabled,
        )?;
        if enabled {
            validate_s3_endpoint(&endpoint)?;
        }
        let bucket =
            required_or_disabled_string(entries, "backup_s3_bucket", "Bucket（存储桶）", enabled)?;
        let access_key_id = required_or_disabled_string(
            entries,
            "backup_s3_access_key_id",
            "Access Key ID（访问密钥 ID）",
            enabled,
        )?;
        let secret_access_key = required_or_disabled_string(
            entries,
            "backup_s3_secret_access_key",
            "Secret Access Key（访问密钥）",
            enabled,
        )?;

        let prefix = normalize_s3_prefix(
            &optional_string(entries, "backup_s3_prefix")?
                .unwrap_or_else(|| "aether/backups/".to_string()),
        )?;

        Ok(Self {
            enabled,
            scope,
            endpoint,
            region: optional_string(entries, "backup_s3_region")?
                .unwrap_or_else(|| "auto".to_string()),
            user_agent: optional_string(entries, "backup_s3_user_agent")?
                .unwrap_or_else(|| "rclone/v1.68.0".to_string()),
            bucket,
            prefix,
            access_key_id,
            secret_access_key,
            path_style: optional_bool(entries, "backup_s3_path_style")?.unwrap_or(true),
            compression: optional_string(entries, "backup_s3_compression")?
                .unwrap_or_else(|| "zstd".to_string()),
            schedule,
            retention_count,
        })
    }
}

fn normalize_s3_prefix(prefix: &str) -> Result<String, BackupConfigError> {
    let prefix = prefix.trim().trim_matches('/');
    if prefix.is_empty() {
        return Ok(String::new());
    }
    if prefix
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        || prefix.contains('\\')
    {
        return Err(BackupConfigError::new(
            "Prefix（备份前缀）不能包含空路径段、相对路径段或反斜杠",
        ));
    }

    Ok(format!("{prefix}/"))
}

fn validate_s3_endpoint(endpoint: &str) -> Result<(), BackupConfigError> {
    let parsed = url::Url::parse(endpoint)
        .map_err(|_| BackupConfigError::new("Endpoint（S3 地址）必须是有效的 HTTPS URL"))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(BackupConfigError::new(
            "Endpoint（S3 地址）必须使用 HTTPS，且不能包含用户凭据、查询参数或片段",
        ));
    }
    Ok(())
}

fn sanitized_endpoint_origin(endpoint: &str) -> String {
    url::Url::parse(endpoint)
        .ok()
        .map(|parsed| parsed.origin().ascii_serialization())
        .unwrap_or_else(|| "<invalid>".to_string())
}

fn validate_range(label: &str, value: u32, min: u32, max: u32) -> Result<(), BackupConfigError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(BackupConfigError::new(format!(
            "{label}配置值无效，应在 {min}..={max} 范围内"
        )))
    }
}

fn required_or_disabled_string(
    entries: &Map<String, Value>,
    key: &str,
    label: &str,
    enabled: bool,
) -> Result<String, BackupConfigError> {
    if enabled {
        required_string(entries, key, label)
    } else {
        Ok(optional_string(entries, key)?.unwrap_or_default())
    }
}

fn required_string(
    entries: &Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<String, BackupConfigError> {
    optional_string(entries, key)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| BackupConfigError::new(format!("{label}为必填配置")))
}

fn optional_string(
    entries: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, BackupConfigError> {
    let Some(value) = entries.get(key) else {
        return Ok(None);
    };
    match value {
        Value::String(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Value::Null => Ok(None),
        _ => Err(BackupConfigError::new(format!(
            "{} 字符串值无效",
            config_label(key)
        ))),
    }
}

fn config_label(key: &str) -> &str {
    match key {
        "backup_s3_endpoint" => "Endpoint（S3 地址）",
        "backup_s3_region" => "Region（S3 区域）",
        "backup_s3_user_agent" => "User-Agent（请求头标识）",
        "backup_s3_bucket" => "Bucket（存储桶）",
        "backup_s3_prefix" => "Prefix（备份前缀）",
        "backup_s3_access_key_id" => "Access Key ID（访问密钥 ID）",
        "backup_s3_secret_access_key" => "Secret Access Key（访问密钥）",
        "backup_s3_compression" => "Compression（压缩格式）",
        "backup_s3_scope" => "Scope（备份范围）",
        "backup_s3_schedule_unit" => "Schedule Unit（计划单位）",
        _ => key,
    }
}

fn optional_bool(
    entries: &Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, BackupConfigError> {
    let Some(value) = entries.get(key) else {
        return Ok(None);
    };
    match value {
        Value::Bool(value) => Ok(Some(*value)),
        Value::String(value) => match value.trim() {
            "true" => Ok(Some(true)),
            "false" => Ok(Some(false)),
            "" => Ok(None),
            _ => Err(BackupConfigError::new(format!("{key} 布尔值无效"))),
        },
        Value::Null => Ok(None),
        _ => Err(BackupConfigError::new(format!("{key} 布尔值无效"))),
    }
}

fn optional_u32(entries: &Map<String, Value>, key: &str) -> Result<Option<u32>, BackupConfigError> {
    let Some(value) = entries.get(key) else {
        return Ok(None);
    };
    match value {
        Value::Number(value) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| BackupConfigError::new(format!("{key} 数值无效"))),
        Value::String(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                trimmed
                    .parse::<u32>()
                    .map(Some)
                    .map_err(|_| BackupConfigError::new(format!("{key} 数值无效")))
            }
        }
        Value::Null => Ok(None),
        _ => Err(BackupConfigError::new(format!("{key} 数值无效"))),
    }
}

#[cfg(test)]
mod tests {
    use super::super::schedule::BackupScheduleUnit;
    use super::super::scopes::BackupScope;
    use super::S3BackupConfig;

    #[test]
    fn parses_minimal_valid_s3_backup_config() {
        let entries = serde_json::json!({
            "backup_s3_enabled": true,
            "backup_s3_scope": "data",
            "backup_s3_endpoint": "https://s3.example.com",
            "backup_s3_region": "auto",
            "backup_s3_bucket": "aether-backups",
            "backup_s3_prefix": "prod/",
            "backup_s3_access_key_id": "access",
            "backup_s3_secret_access_key": "secret",
            "backup_s3_path_style": true,
            "backup_s3_compression": "zstd",
            "backup_s3_schedule_unit": "days",
            "backup_s3_schedule_interval": 1,
            "backup_s3_schedule_hour": 3,
            "backup_s3_schedule_minute": 15,
            "backup_s3_retention_count": 7
        });

        let config = S3BackupConfig::from_json_map(entries.as_object().unwrap())
            .expect("config should parse");

        assert_eq!(config.scope, BackupScope::Data);
        assert_eq!(config.bucket, "aether-backups");
        assert_eq!(config.prefix, "prod/");
        assert_eq!(config.schedule.unit, BackupScheduleUnit::Days);
        assert_eq!(config.retention_count, 7);
    }

    #[test]
    fn rejects_missing_bucket_for_backup() {
        let entries = serde_json::json!({
            "backup_s3_enabled": true,
            "backup_s3_endpoint": "https://s3.example.com",
            "backup_s3_access_key_id": "access",
            "backup_s3_secret_access_key": "secret"
        });

        let err = S3BackupConfig::from_json_map(entries.as_object().unwrap())
            .expect_err("bucket is required");

        assert!(err.to_string().contains("Bucket"));
    }

    #[test]
    fn parses_disabled_default_s3_backup_config_with_null_credentials() {
        let entries = serde_json::json!({
            "backup_s3_enabled": false,
            "backup_s3_scope": "data",
            "backup_s3_endpoint": null,
            "backup_s3_region": "auto",
            "backup_s3_bucket": null,
            "backup_s3_prefix": "aether/backups/",
            "backup_s3_access_key_id": null,
            "backup_s3_secret_access_key": null,
            "backup_s3_path_style": true,
            "backup_s3_compression": "zstd",
            "backup_s3_schedule_unit": "days",
            "backup_s3_schedule_interval": 1,
            "backup_s3_schedule_hour": 3,
            "backup_s3_schedule_minute": 0,
            "backup_s3_schedule_weekday": 1,
            "backup_s3_schedule_month_day": 1,
            "backup_s3_retention_count": 7
        });

        let config = S3BackupConfig::from_json_map(entries.as_object().unwrap())
            .expect("disabled default config should parse");

        assert_eq!(config.enabled, false);
        assert_eq!(config.endpoint, "");
        assert_eq!(config.bucket, "");
        assert_eq!(config.access_key_id, "");
        assert_eq!(config.secret_access_key, "");
        assert_eq!(config.schedule.unit, BackupScheduleUnit::Days);
    }

    #[test]
    fn rejects_invalid_schedule_numbers() {
        let cases = [
            ("backup_s3_schedule_interval", 0, "Interval"),
            ("backup_s3_schedule_minute", 60, "Minute"),
            ("backup_s3_schedule_hour", 24, "Hour"),
            ("backup_s3_schedule_weekday", 0, "Weekday"),
            ("backup_s3_schedule_month_day", 32, "Month Day"),
            ("backup_s3_retention_count", 0, "Retention"),
        ];

        for (key, value, label) in cases {
            let mut entries = serde_json::json!({
                "backup_s3_enabled": true,
                "backup_s3_endpoint": "https://s3.example.com",
                "backup_s3_bucket": "aether-backups",
                "backup_s3_access_key_id": "access",
                "backup_s3_secret_access_key": "secret"
            });
            entries.as_object_mut().unwrap().insert(
                key.to_string(),
                serde_json::Value::Number(serde_json::Number::from(value)),
            );

            let err = S3BackupConfig::from_json_map(entries.as_object().unwrap())
                .expect_err("invalid numeric config should fail");

            assert!(
                err.to_string().contains(label),
                "{key} should mention {label}, got {err}"
            );
        }
    }

    #[test]
    fn rejects_non_string_endpoint_config() {
        let entries = serde_json::json!({
            "backup_s3_enabled": true,
            "backup_s3_endpoint": {"url": "https://s3.example.com"},
            "backup_s3_bucket": "aether-backups",
            "backup_s3_access_key_id": "access",
            "backup_s3_secret_access_key": "secret"
        });

        let err = S3BackupConfig::from_json_map(entries.as_object().unwrap())
            .expect_err("endpoint object should fail");

        assert!(err.to_string().contains("Endpoint"));
    }

    #[test]
    fn rejects_insecure_or_credential_bearing_endpoints() {
        for endpoint in [
            "http://s3.example.com",
            "https://user:password@s3.example.com",
            "https://s3.example.com?token=secret",
            "https://s3.example.com/#fragment",
        ] {
            let entries = serde_json::json!({
                "backup_s3_enabled": true,
                "backup_s3_endpoint": endpoint,
                "backup_s3_bucket": "aether-backups",
                "backup_s3_access_key_id": "access",
                "backup_s3_secret_access_key": "secret"
            });

            let error = S3BackupConfig::from_json_map(entries.as_object().unwrap())
                .expect_err("unsafe endpoint should fail closed");
            assert!(error.to_string().contains("Endpoint"));
        }
    }

    #[test]
    fn debug_output_does_not_expose_s3_credentials() {
        let entries = serde_json::json!({
            "backup_s3_enabled": true,
            "backup_s3_endpoint": "https://s3.example.com/path",
            "backup_s3_bucket": "aether-backups",
            "backup_s3_access_key_id": "access-key-value",
            "backup_s3_secret_access_key": "secret-key-value"
        });
        let config = S3BackupConfig::from_json_map(entries.as_object().unwrap())
            .expect("config should parse");

        let debug = format!("{config:?}");
        assert!(debug.contains("https://s3.example.com"));
        assert!(!debug.contains("/path"));
        assert!(!debug.contains("access-key-value"));
        assert!(!debug.contains("secret-key-value"));
    }

    #[test]
    fn canonicalizes_s3_backup_prefix_once() {
        let entries = serde_json::json!({
            "backup_s3_enabled": true,
            "backup_s3_endpoint": "https://s3.example.com",
            "backup_s3_bucket": "aether-backups",
            "backup_s3_prefix": "/prod/backups//",
            "backup_s3_access_key_id": "access",
            "backup_s3_secret_access_key": "secret"
        });
        let config = S3BackupConfig::from_json_map(entries.as_object().unwrap())
            .expect("prefix should be canonicalized");

        assert_eq!(config.prefix, "prod/backups/");

        for invalid_prefix in ["prod//backups", "prod/../backups", "prod\\backups"] {
            let mut entries = entries.clone();
            entries["backup_s3_prefix"] = serde_json::json!(invalid_prefix);
            assert!(S3BackupConfig::from_json_map(entries.as_object().unwrap()).is_err());
        }
    }

    #[test]
    fn applies_default_values_from_system_config_contract() {
        let entries = serde_json::json!({
            "backup_s3_endpoint": "https://s3.example.com",
            "backup_s3_bucket": "aether-backups",
            "backup_s3_access_key_id": "access",
            "backup_s3_secret_access_key": "secret"
        });

        let config = S3BackupConfig::from_json_map(entries.as_object().unwrap())
            .expect("config should parse with defaults");

        assert_eq!(config.scope, BackupScope::Data);
        assert_eq!(config.region, "auto");
        assert_eq!(config.user_agent, "rclone/v1.68.0");
        assert_eq!(config.prefix, "aether/backups/");
        assert_eq!(config.path_style, true);
        assert_eq!(config.compression, "zstd");
        assert_eq!(config.schedule.unit, BackupScheduleUnit::Days);
        assert_eq!(config.schedule.interval, 1);
        assert_eq!(config.schedule.hour, 3);
        assert_eq!(config.schedule.minute, 0);
        assert_eq!(config.schedule.weekday, 1);
        assert_eq!(config.schedule.month_day, 1);
        assert_eq!(config.retention_count, 7);
    }
}
