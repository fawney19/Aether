use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use bytes::Bytes;
use futures_util::TryStreamExt;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path;
use object_store::{ClientOptions, ObjectStore};
use reqwest::header::HeaderValue;
use tokio::sync::RwLock;

use super::config::S3BackupConfig;

#[async_trait::async_trait]
pub(crate) trait BackupObjectStore: Send + Sync {
    async fn put_object(&self, key: &str, bytes: Bytes) -> Result<(), BackupStoreError>;

    async fn list_keys(&self, prefix: &str) -> Result<Vec<String>, BackupStoreError>;

    async fn delete_object(&self, key: &str) -> Result<(), BackupStoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackupStoreError {
    message: String,
}

impl BackupStoreError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn object_store(operation: &str, key: &str, error: impl fmt::Display) -> Self {
        Self::new(format!(
            "S3 backup object store {operation} failed for `{key}`: {error}"
        ))
    }
}

impl fmt::Display for BackupStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BackupStoreError {}

#[derive(Debug, Default, Clone)]
pub(crate) struct FakeBackupObjectStore {
    objects: Arc<RwLock<BTreeMap<String, Bytes>>>,
}

#[async_trait::async_trait]
impl BackupObjectStore for FakeBackupObjectStore {
    async fn put_object(&self, key: &str, bytes: Bytes) -> Result<(), BackupStoreError> {
        self.objects.write().await.insert(key.to_string(), bytes);
        Ok(())
    }

    async fn list_keys(&self, prefix: &str) -> Result<Vec<String>, BackupStoreError> {
        let prefix = directory_list_prefix(prefix);
        Ok(self
            .objects
            .read()
            .await
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect())
    }

    async fn delete_object(&self, key: &str) -> Result<(), BackupStoreError> {
        self.objects.write().await.remove(key);
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct ObjectStoreS3BackupStore {
    store: object_store::aws::AmazonS3,
}

impl ObjectStoreS3BackupStore {
    pub(crate) fn from_config(config: &S3BackupConfig) -> Result<Self, BackupStoreError> {
        // 部分 S3 兼容网关（如中国科技云 s3.cstcloud.cn）按 User-Agent 放行请求，
        // object_store 默认 UA 会被拒，因此允许自定义 User-Agent。
        let client_options = if config.user_agent.trim().is_empty() {
            ClientOptions::new()
        } else {
            ClientOptions::new().with_user_agent(
                HeaderValue::from_str(config.user_agent.trim()).map_err(|error| {
                    BackupStoreError::new(format!("S3 备份 User-Agent 配置无效: {error}"))
                })?,
            )
        };
        let store = AmazonS3Builder::new()
            .with_client_options(client_options)
            .with_endpoint(config.endpoint.clone())
            .with_region(config.region.clone())
            .with_bucket_name(config.bucket.clone())
            .with_access_key_id(config.access_key_id.clone())
            .with_secret_access_key(config.secret_access_key.clone())
            .with_virtual_hosted_style_request(!config.path_style)
            .build()
            .map_err(|error| {
                BackupStoreError::new(format!(
                    "S3 backup object store configuration failed: {error}"
                ))
            })?;

        Ok(Self { store })
    }
}

#[async_trait::async_trait]
impl BackupObjectStore for ObjectStoreS3BackupStore {
    async fn put_object(&self, key: &str, bytes: Bytes) -> Result<(), BackupStoreError> {
        self.store
            .put(&Path::from(key), bytes.into())
            .await
            .map(|_| ())
            .map_err(|error| BackupStoreError::object_store("put", key, error))
    }

    async fn list_keys(&self, prefix: &str) -> Result<Vec<String>, BackupStoreError> {
        let prefix_path = list_prefix_path(prefix);
        let mut keys = self
            .store
            .list(prefix_path.as_ref())
            .map_ok(|meta| meta.location.to_string())
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| BackupStoreError::object_store("list", prefix, error))?;
        keys.sort();
        Ok(keys)
    }

    async fn delete_object(&self, key: &str) -> Result<(), BackupStoreError> {
        self.store
            .delete(&Path::from(key))
            .await
            .map_err(|error| BackupStoreError::object_store("delete", key, error))
    }
}

fn directory_list_prefix(prefix: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}/")
    }
}

fn list_prefix_path(prefix: &str) -> Option<Path> {
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        None
    } else {
        Some(Path::from(prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::{list_prefix_path, BackupObjectStore, FakeBackupObjectStore};

    #[tokio::test]
    async fn fake_backup_object_store_puts_lists_and_deletes() {
        let store = FakeBackupObjectStore::default();
        store
            .put_object(
                "prod/aether-data-backup-20260524-010000.json.zst",
                bytes::Bytes::from_static(b"one"),
            )
            .await
            .unwrap();
        store
            .put_object(
                "prod/aether-data-backup-20260524-020000.json.zst",
                bytes::Bytes::from_static(b"two"),
            )
            .await
            .unwrap();

        let keys = store.list_keys("prod/").await.unwrap();
        assert_eq!(keys.len(), 2);

        store
            .delete_object("prod/aether-data-backup-20260524-010000.json.zst")
            .await
            .unwrap();
        let keys = store.list_keys("prod/").await.unwrap();
        assert_eq!(
            keys,
            vec!["prod/aether-data-backup-20260524-020000.json.zst"]
        );
    }

    #[tokio::test]
    async fn fake_backup_object_store_lists_normalized_directory_prefixes() {
        let store = FakeBackupObjectStore::default();
        store
            .put_object(
                "prod/aether-data-backup-20260524-010000.json.zst",
                bytes::Bytes::from_static(b"one"),
            )
            .await
            .unwrap();
        store
            .put_object(
                "prod-backups/aether-data-backup-20260524-010000.json.zst",
                bytes::Bytes::from_static(b"two"),
            )
            .await
            .unwrap();

        let keys = store.list_keys("prod").await.unwrap();

        assert_eq!(
            keys,
            vec!["prod/aether-data-backup-20260524-010000.json.zst"]
        );
    }

    #[test]
    fn s3_list_prefix_path_lets_object_store_add_directory_delimiter() {
        assert_eq!(
            list_prefix_path("prod/")
                .as_ref()
                .map(std::string::ToString::to_string)
                .as_deref(),
            Some("prod")
        );
        assert!(list_prefix_path("").is_none());
    }
}
