use bytes::Bytes;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::config::S3BackupConfig;
use super::scopes::BackupScope;
use super::store::{BackupObjectStore, BackupStoreError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackupRunResult {
    pub(crate) scope: BackupScope,
    pub(crate) bucket: String,
    pub(crate) object_key: String,
    pub(crate) bytes: usize,
    pub(crate) sha256: String,
    pub(crate) export_version: String,
    pub(crate) exported_at: String,
    pub(crate) compression: String,
    pub(crate) deleted_old_objects: usize,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum BackupExecutionError {
    #[error("S3 backup JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),

    #[error("S3 backup compression failed: {0}")]
    Compression(#[from] std::io::Error),

    #[error("{0}")]
    Store(#[from] BackupStoreError),

    #[error("S3 backup compression `{0}` is not supported; expected `zstd`")]
    UnknownCompression(String),
}

pub(crate) async fn run_backup_with_store<S>(
    config: &S3BackupConfig,
    store: &S,
    payload: Value,
    now_utc: DateTime<Utc>,
) -> Result<BackupRunResult, BackupExecutionError>
where
    S: BackupObjectStore + ?Sized,
{
    let export_version = payload_string_field(&payload, "version").unwrap_or_default();
    let exported_at = payload_string_field(&payload, "exported_at")
        .unwrap_or_else(|| now_utc.to_rfc3339_opts(SecondsFormat::Secs, true));
    let json_bytes = serde_json::to_vec(&payload)?;
    let compression = config.compression.trim().to_string();
    let upload_bytes = match compression.as_str() {
        "zstd" => zstd::stream::encode_all(json_bytes.as_slice(), 0)?,
        other => return Err(BackupExecutionError::UnknownCompression(other.to_string())),
    };
    let bytes = upload_bytes.len();
    let sha256 = format!("{:x}", Sha256::digest(&upload_bytes));
    let timestamp = now_utc.format("%Y%m%d-%H%M%S").to_string();
    let object_key = config.scope.object_key(&config.prefix, &timestamp);

    store
        .put_object(&object_key, Bytes::from(upload_bytes))
        .await?;
    let deleted_old_objects = prune_old_backups(config, store, &object_key).await?;

    Ok(BackupRunResult {
        scope: config.scope,
        bucket: config.bucket.clone(),
        object_key,
        bytes,
        sha256,
        export_version,
        exported_at,
        compression,
        deleted_old_objects,
    })
}

async fn prune_old_backups<S>(
    config: &S3BackupConfig,
    store: &S,
    current_object_key: &str,
) -> Result<usize, BackupExecutionError>
where
    S: BackupObjectStore + ?Sized,
{
    let keys = store.list_keys(&config.prefix).await?;
    let mut matching_keys = config.scope.matching_backup_keys(&config.prefix, keys);
    matching_keys.sort_by(|left, right| right.cmp(left));

    let mut deleted = 0;
    let mut retained = usize::from(
        config.retention_count > 0 && matching_keys.iter().any(|key| key == current_object_key),
    );
    for key in matching_keys {
        if config.retention_count > 0 && key == current_object_key {
            continue;
        }
        if retained < config.retention_count as usize {
            retained += 1;
            continue;
        }

        store.delete_object(&key).await?;
        deleted += 1;
    }

    Ok(deleted)
}

fn payload_string_field(payload: &Value, field: &str) -> Option<String> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::super::config::S3BackupConfig;
    use super::super::schedule::BackupSchedule;
    use super::super::scopes::BackupScope;
    use super::super::store::{BackupObjectStore, FakeBackupObjectStore};
    use super::run_backup_with_store;
    use bytes::Bytes;
    use chrono::{DateTime, Utc};
    use serde_json::json;

    fn sample_backup_config(scope: BackupScope, retention_count: u32) -> S3BackupConfig {
        S3BackupConfig {
            enabled: true,
            scope,
            endpoint: "https://example.com".to_string(),
            region: "auto".to_string(),
            user_agent: "rclone/v1.68.0".to_string(),
            bucket: "aether-backups".to_string(),
            prefix: "prod/".to_string(),
            access_key_id: "test-access-key".to_string(),
            secret_access_key: "test-secret-key".to_string(),
            path_style: true,
            compression: "zstd".to_string(),
            schedule: BackupSchedule::default(),
            retention_count,
        }
    }

    #[tokio::test]
    async fn backup_executor_uploads_payload_and_prunes_same_scope_only() {
        let store = FakeBackupObjectStore::default();
        store
            .put_object(
                "prod/aether-data-backup-20260524-010000.json.zst",
                Bytes::from_static(b"old"),
            )
            .await
            .unwrap();
        store
            .put_object(
                "prod/aether-config-backup-20260524-010000.json.zst",
                Bytes::from_static(b"keep-config"),
            )
            .await
            .unwrap();

        let config = sample_backup_config(BackupScope::Data, 1);
        let payload = json!({
            "version": "1.0",
            "exported_at": "2026-05-24T03:15:00Z",
            "config_data": {},
            "user_data": {}
        });
        let now_utc = DateTime::parse_from_rfc3339("2026-05-24T03:15:00+08:00")
            .unwrap()
            .with_timezone(&Utc);

        let result = run_backup_with_store(&config, &store, payload, now_utc)
            .await
            .expect("backup should succeed");

        assert_eq!(result.scope, BackupScope::Data);
        assert_eq!(result.bucket, "aether-backups");
        assert_eq!(
            result.object_key,
            "prod/aether-data-backup-20260523-191500.json.zst"
        );
        assert!(result.bytes > 0);
        assert_eq!(result.sha256.len(), 64);
        assert_eq!(result.export_version, "1.0");
        assert_eq!(result.exported_at, "2026-05-24T03:15:00Z");
        assert_eq!(result.compression, "zstd");
        assert_eq!(result.deleted_old_objects, 1);

        let keys = store.list_keys("prod/").await.unwrap();
        assert!(keys
            .iter()
            .any(|key| key == "prod/aether-config-backup-20260524-010000.json.zst"));
        assert!(keys
            .iter()
            .any(|key| key == "prod/aether-data-backup-20260523-191500.json.zst"));
        assert!(!keys
            .iter()
            .any(|key| key == "prod/aether-data-backup-20260524-010000.json.zst"));
    }

    #[tokio::test]
    async fn backup_executor_rejects_unknown_compression() {
        let store = FakeBackupObjectStore::default();
        let mut config = sample_backup_config(BackupScope::Data, 1);
        config.compression = "brotli".to_string();

        let payload = json!({
            "version": "1.0",
            "exported_at": "2026-05-24T03:15:00Z"
        });
        let now_utc = DateTime::parse_from_rfc3339("2026-05-24T03:15:00+08:00")
            .unwrap()
            .with_timezone(&Utc);

        let error = run_backup_with_store(&config, &store, payload, now_utc)
            .await
            .expect_err("unknown compression should fail");

        assert!(error.to_string().contains("brotli"));
    }
}
