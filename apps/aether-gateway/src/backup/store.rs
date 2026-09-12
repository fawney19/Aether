use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use futures_util::TryStreamExt;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path;
use object_store::{ClientOptions, ObjectStore, ObjectStoreExt, PutMode, PutOptions};
use reqwest::header::HeaderValue;
use tokio::sync::RwLock;

use super::config::S3BackupConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackupObjectCreateResult {
    Created,
    AlreadyExists,
}

#[async_trait::async_trait]
pub(crate) trait BackupObjectStore: Send + Sync {
    async fn put_object(&self, key: &str, bytes: Bytes) -> Result<(), BackupStoreError>;

    async fn put_object_if_absent(
        &self,
        key: &str,
        bytes: Bytes,
    ) -> Result<BackupObjectCreateResult, BackupStoreError>;

    async fn get_object_limited(
        &self,
        key: &str,
        max_bytes: usize,
    ) -> Result<Bytes, BackupStoreError>;

    async fn delete_object(&self, key: &str) -> Result<(), BackupStoreError>;

    async fn list_keys_limited(
        &self,
        prefix: &str,
        max_objects: usize,
    ) -> Result<Vec<String>, BackupStoreError>;
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

    async fn put_object_if_absent(
        &self,
        key: &str,
        bytes: Bytes,
    ) -> Result<BackupObjectCreateResult, BackupStoreError> {
        let mut objects = self.objects.write().await;
        if objects.contains_key(key) {
            Ok(BackupObjectCreateResult::AlreadyExists)
        } else {
            objects.insert(key.to_string(), bytes);
            Ok(BackupObjectCreateResult::Created)
        }
    }

    async fn get_object_limited(
        &self,
        key: &str,
        max_bytes: usize,
    ) -> Result<Bytes, BackupStoreError> {
        let bytes = self
            .objects
            .read()
            .await
            .get(key)
            .cloned()
            .ok_or_else(|| BackupStoreError::new(format!("backup object `{key}` not found")))?;
        if bytes.len() > max_bytes {
            return Err(BackupStoreError::new(format!(
                "backup object `{key}` exceeds the configured {max_bytes} byte read limit"
            )));
        }
        Ok(bytes)
    }

    async fn delete_object(&self, key: &str) -> Result<(), BackupStoreError> {
        self.objects.write().await.remove(key);
        Ok(())
    }

    async fn list_keys_limited(
        &self,
        prefix: &str,
        max_objects: usize,
    ) -> Result<Vec<String>, BackupStoreError> {
        let prefix = directory_list_prefix(prefix);
        let keys: Vec<_> = self
            .objects
            .read()
            .await
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        if keys.len() > max_objects {
            return Err(BackupStoreError::new(format!(
                "backup object listing exceeds the configured {max_objects} object limit"
            )));
        }
        Ok(keys)
    }
}

#[cfg(test)]
impl FakeBackupObjectStore {
    pub(crate) async fn object_bytes(&self, key: &str) -> Option<Bytes> {
        self.objects.read().await.get(key).cloned()
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

    async fn put_object_if_absent(
        &self,
        key: &str,
        bytes: Bytes,
    ) -> Result<BackupObjectCreateResult, BackupStoreError> {
        let options = PutOptions {
            mode: PutMode::Create,
            ..PutOptions::default()
        };
        match self
            .store
            .put_opts(&Path::from(key), bytes.into(), options)
            .await
        {
            Ok(_) => Ok(BackupObjectCreateResult::Created),
            Err(object_store::Error::AlreadyExists { .. }) => {
                Ok(BackupObjectCreateResult::AlreadyExists)
            }
            Err(error) => Err(BackupStoreError::object_store(
                "conditional put",
                key,
                error,
            )),
        }
    }

    async fn get_object_limited(
        &self,
        key: &str,
        max_bytes: usize,
    ) -> Result<Bytes, BackupStoreError> {
        let result = self
            .store
            .get(&Path::from(key))
            .await
            .map_err(|error| BackupStoreError::object_store("get", key, error))?;
        if result.meta.size > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
            return Err(BackupStoreError::new(format!(
                "backup object `{key}` exceeds the configured {max_bytes} byte read limit"
            )));
        }
        let object_size = result.meta.size;
        let mut stream = result.into_stream();
        let mut bytes = BytesMut::with_capacity(
            usize::try_from(object_size)
                .unwrap_or(max_bytes)
                .min(max_bytes)
                .min(8 * 1024 * 1024),
        );
        while let Some(chunk) = stream
            .try_next()
            .await
            .map_err(|error| BackupStoreError::object_store("read", key, error))?
        {
            if bytes.len().saturating_add(chunk.len()) > max_bytes {
                return Err(BackupStoreError::new(format!(
                    "backup object `{key}` exceeds the configured {max_bytes} byte read limit"
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes.freeze())
    }

    async fn delete_object(&self, key: &str) -> Result<(), BackupStoreError> {
        self.store
            .delete(&Path::from(key))
            .await
            .map_err(|error| BackupStoreError::object_store("delete", key, error))
    }

    async fn list_keys_limited(
        &self,
        prefix: &str,
        max_objects: usize,
    ) -> Result<Vec<String>, BackupStoreError> {
        let prefix_path = list_prefix_path(prefix);
        let mut objects = self.store.list(prefix_path.as_ref());
        let mut keys = Vec::new();
        while let Some(meta) = objects
            .try_next()
            .await
            .map_err(|error| BackupStoreError::object_store("list", prefix, error))?
        {
            if keys.len() >= max_objects {
                return Err(BackupStoreError::new(format!(
                    "backup object listing exceeds the configured {max_objects} object limit"
                )));
            }
            keys.push(meta.location.to_string());
        }
        keys.sort();
        Ok(keys)
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
    use super::{
        list_prefix_path, BackupObjectCreateResult, BackupObjectStore, FakeBackupObjectStore,
    };

    #[tokio::test]
    async fn fake_backup_object_store_puts_and_lists() {
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

        let keys = store.list_keys_limited("prod/", 2).await.unwrap();
        assert_eq!(
            keys,
            vec![
                "prod/aether-data-backup-20260524-010000.json.zst",
                "prod/aether-data-backup-20260524-020000.json.zst",
            ]
        );
    }

    #[tokio::test]
    async fn fake_backup_object_store_enforces_read_and_listing_limits() {
        let store = FakeBackupObjectStore::default();
        store
            .put_object("prod/one", bytes::Bytes::from_static(b"1234"))
            .await
            .unwrap();
        store
            .put_object("prod/two", bytes::Bytes::from_static(b"5678"))
            .await
            .unwrap();

        assert!(store.get_object_limited("prod/one", 3).await.is_err());
        assert_eq!(
            store.get_object_limited("prod/one", 4).await.unwrap(),
            bytes::Bytes::from_static(b"1234")
        );
        assert!(store.list_keys_limited("prod/", 1).await.is_err());
        assert_eq!(store.list_keys_limited("prod/", 2).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn fake_backup_object_store_conditional_put_never_overwrites() {
        let store = FakeBackupObjectStore::default();
        let key = "prod/aether-data-backup-20260524-010000.json.zst.aes256gcm";

        assert_eq!(
            store
                .put_object_if_absent(key, bytes::Bytes::from_static(b"first"))
                .await
                .unwrap(),
            BackupObjectCreateResult::Created
        );
        assert_eq!(
            store
                .put_object_if_absent(key, bytes::Bytes::from_static(b"second"))
                .await
                .unwrap(),
            BackupObjectCreateResult::AlreadyExists
        );
        assert_eq!(
            store.object_bytes(key).await.as_deref(),
            Some(b"first".as_slice())
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

        let keys = store.list_keys_limited("prod", 10).await.unwrap();

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
