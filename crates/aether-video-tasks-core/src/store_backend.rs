use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aether_crypto::{decrypt_python_fernet_ciphertext, encrypt_python_fernet_plaintext};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::{
    GeminiVideoTaskSeed, LocalVideoTaskReadResponse, LocalVideoTaskRegistryMutation,
    LocalVideoTaskSnapshot, OpenAiVideoTaskSeed, VideoTaskRegistry, VideoTaskStore,
};

#[derive(Default)]
pub struct InMemoryVideoTaskStore {
    registry: Mutex<VideoTaskRegistry>,
}

impl std::fmt::Debug for InMemoryVideoTaskStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InMemoryVideoTaskStore")
            .field("registry", &"[redacted]")
            .finish()
    }
}

pub struct FileVideoTaskStore {
    path: PathBuf,
    encryption_key: String,
    registry: Mutex<VideoTaskRegistry>,
    persisted_file: Mutex<PersistedVideoTaskStore>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PersistedVideoTaskStore {
    Missing,
    Bytes(Vec<u8>),
}

struct LoadedVideoTaskRegistry {
    registry: VideoTaskRegistry,
    persisted_file: PersistedVideoTaskStore,
    needs_rewrite: bool,
}

impl std::fmt::Debug for FileVideoTaskStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileVideoTaskStore")
            .field("path", &self.path)
            .field("encryption_key", &"[redacted]")
            .field("registry", &"[redacted]")
            .finish()
    }
}

const ENCRYPTED_VIDEO_TASK_STORE_PREFIX: &str = "aether-video-tasks-v2\n";
const LEGACY_ENCRYPTED_VIDEO_TASK_STORE_PREFIX: &str = "aether-video-tasks-v1\n";
const VIDEO_TASK_STORE_PURPOSE: &str = "video-task-file-store";

impl VideoTaskStore for InMemoryVideoTaskStore {
    fn insert(&self, snapshot: LocalVideoTaskSnapshot) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.insert(snapshot);
        }
    }

    fn read_openai(&self, task_id: &str) -> Option<LocalVideoTaskReadResponse> {
        let registry = self.registry.lock().ok()?;
        registry.read_openai(task_id)
    }

    fn read_gemini(&self, short_id: &str) -> Option<LocalVideoTaskReadResponse> {
        let registry = self.registry.lock().ok()?;
        registry.read_gemini(short_id)
    }

    fn clone_openai(&self, task_id: &str) -> Option<OpenAiVideoTaskSeed> {
        let registry = self.registry.lock().ok()?;
        registry.clone_openai(task_id)
    }

    fn clone_gemini(&self, short_id: &str) -> Option<GeminiVideoTaskSeed> {
        let registry = self.registry.lock().ok()?;
        registry.clone_gemini(short_id)
    }

    fn list_active_snapshots(&self, limit: usize) -> Vec<LocalVideoTaskSnapshot> {
        let Ok(registry) = self.registry.lock() else {
            return Vec::new();
        };
        registry.list_active_snapshots(limit)
    }

    fn apply_mutation(&self, mutation: LocalVideoTaskRegistryMutation) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.apply_mutation(mutation);
        }
    }

    fn project_openai(&self, task_id: &str, provider_body: &Map<String, Value>) -> bool {
        let Ok(mut registry) = self.registry.lock() else {
            return false;
        };
        registry.project_openai(task_id, provider_body)
    }

    fn project_gemini(&self, short_id: &str, provider_body: &Map<String, Value>) -> bool {
        let Ok(mut registry) = self.registry.lock() else {
            return false;
        };
        registry.project_gemini(short_id, provider_body)
    }
}

impl FileVideoTaskStore {
    pub fn new(
        path: impl Into<PathBuf>,
        encryption_key: impl Into<String>,
    ) -> std::io::Result<Self> {
        let path = path.into();
        let encryption_key = encryption_key.into();
        if encryption_key.trim().is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "video task file store encryption key cannot be empty",
            ));
        }
        let LoadedVideoTaskRegistry {
            registry,
            persisted_file,
            needs_rewrite,
        } = Self::load_registry(&path, &encryption_key)?;
        let store = Self {
            path,
            encryption_key,
            registry: Mutex::new(registry),
            persisted_file: Mutex::new(persisted_file),
        };
        if needs_rewrite {
            let registry = store
                .registry
                .lock()
                .map_err(|_| std::io::Error::other("video task store lock poisoned"))?;
            store.persist_registry(&registry)?;
        }
        Ok(store)
    }

    fn load_registry(
        path: &Path,
        encryption_key: &str,
    ) -> std::io::Result<LoadedVideoTaskRegistry> {
        let persisted_file = read_persisted_video_task_store(path)?;
        let PersistedVideoTaskStore::Bytes(bytes) = &persisted_file else {
            return Ok(LoadedVideoTaskRegistry {
                registry: VideoTaskRegistry::default(),
                persisted_file,
                needs_rewrite: false,
            });
        };
        if bytes.is_empty() {
            return Err(invalid_store_data("video task store is empty"));
        }
        if let Some(ciphertext) = bytes.strip_prefix(ENCRYPTED_VIDEO_TASK_STORE_PREFIX.as_bytes()) {
            let ciphertext = std::str::from_utf8(ciphertext)
                .map_err(|_| invalid_store_data("encrypted video task store is not UTF-8"))?;
            let protected = decrypt_python_fernet_ciphertext(encryption_key, ciphertext.trim())
                .map_err(|_| invalid_store_data("video task store decryption failed"))?;
            let plaintext = protected
                .strip_prefix(VIDEO_TASK_STORE_PURPOSE)
                .and_then(|value| value.strip_prefix('\0'))
                .ok_or_else(|| invalid_store_data("video task store purpose mismatch"))?;
            let mut registry: VideoTaskRegistry = serde_json::from_str(plaintext)
                .map_err(|_| invalid_store_data("decrypted video task store is invalid"))?;
            let needs_rewrite = registry.sanitize_persisted_diagnostics();
            return Ok(LoadedVideoTaskRegistry {
                registry,
                persisted_file,
                needs_rewrite,
            });
        }
        if let Some(ciphertext) =
            bytes.strip_prefix(LEGACY_ENCRYPTED_VIDEO_TASK_STORE_PREFIX.as_bytes())
        {
            let ciphertext = std::str::from_utf8(ciphertext)
                .map_err(|_| invalid_store_data("encrypted video task store is not UTF-8"))?;
            let plaintext = decrypt_python_fernet_ciphertext(encryption_key, ciphertext.trim())
                .map_err(|_| invalid_store_data("video task store decryption failed"))?;
            let mut registry: VideoTaskRegistry = serde_json::from_str(&plaintext)
                .map_err(|_| invalid_store_data("decrypted video task store is invalid"))?;
            registry.sanitize_persisted_diagnostics();
            return Ok(LoadedVideoTaskRegistry {
                registry,
                persisted_file,
                needs_rewrite: true,
            });
        }

        if bytes.starts_with(b"aether-") {
            return Err(invalid_store_data(
                "unsupported encrypted video task store envelope",
            ));
        }
        Err(invalid_store_data(
            "plaintext video task stores are not accepted",
        ))
    }

    fn persist_registry(&self, registry: &VideoTaskRegistry) -> std::io::Result<()> {
        let bytes = encrypted_video_task_store_bytes(&self.encryption_key, registry)?;
        let mut persisted_file = self
            .persisted_file
            .lock()
            .map_err(|_| std::io::Error::other("video task persisted-file lock poisoned"))?;
        replace_video_task_store_if_unchanged(&self.path, &persisted_file, &bytes)?;
        *persisted_file = PersistedVideoTaskStore::Bytes(bytes);
        Ok(())
    }

    fn mutate_registry(&self, mutator: impl FnOnce(&mut VideoTaskRegistry) -> bool) -> bool {
        let Ok(mut registry) = self.registry.lock() else {
            return false;
        };
        let mut updated_registry = registry.clone();
        if !mutator(&mut updated_registry) {
            return false;
        }
        if self.persist_registry(&updated_registry).is_err() {
            return false;
        }
        *registry = updated_registry;
        true
    }
}

fn invalid_store_data(message: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}

fn encrypted_video_task_store_bytes(
    encryption_key: &str,
    registry: &VideoTaskRegistry,
) -> std::io::Result<Vec<u8>> {
    let plaintext = serde_json::to_string(registry)
        .map_err(|_| invalid_store_data("video task store serialization failed"))?;
    let protected = format!("{VIDEO_TASK_STORE_PURPOSE}\0{plaintext}");
    let ciphertext = encrypt_python_fernet_plaintext(encryption_key, &protected)
        .map_err(|_| invalid_store_data("video task store encryption failed"))?;
    let mut bytes = ENCRYPTED_VIDEO_TASK_STORE_PREFIX.as_bytes().to_vec();
    bytes.extend_from_slice(ciphertext.as_bytes());
    bytes.push(b'\n');
    Ok(bytes)
}

fn read_persisted_video_task_store(path: &Path) -> std::io::Result<PersistedVideoTaskStore> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(PersistedVideoTaskStore::Bytes(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(PersistedVideoTaskStore::Missing)
        }
        Err(error) => Err(error),
    }
}

fn replace_video_task_store_if_unchanged(
    path: &Path,
    expected: &PersistedVideoTaskStore,
    replacement: &[u8],
) -> std::io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }

    let lock_path = video_task_store_lock_path(path);
    let lock_file = open_private_lock_file(&lock_path)?;
    // Lock a stable sidecar inode: the store inode itself is replaced by rename.
    lock_file.lock()?;

    // The bytes captured before parsing/decryption are the migration/write CAS token.
    let observed = read_persisted_video_task_store(path)?;
    if &observed != expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "video task store changed before compare-and-replace",
        ));
    }

    let temp_path = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    if let Err(error) = write_private_file(&temp_path, replacement) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(error);
    }
    Ok(())
}

fn video_task_store_lock_path(path: &Path) -> PathBuf {
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    PathBuf::from(lock_path)
}

fn open_private_lock_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

impl VideoTaskStore for FileVideoTaskStore {
    fn insert(&self, snapshot: LocalVideoTaskSnapshot) {
        let _ = self.mutate_registry(|registry| {
            registry.insert(snapshot);
            true
        });
    }

    fn read_openai(&self, task_id: &str) -> Option<LocalVideoTaskReadResponse> {
        let registry = self.registry.lock().ok()?;
        registry.read_openai(task_id)
    }

    fn read_gemini(&self, short_id: &str) -> Option<LocalVideoTaskReadResponse> {
        let registry = self.registry.lock().ok()?;
        registry.read_gemini(short_id)
    }

    fn clone_openai(&self, task_id: &str) -> Option<OpenAiVideoTaskSeed> {
        let registry = self.registry.lock().ok()?;
        registry.clone_openai(task_id)
    }

    fn clone_gemini(&self, short_id: &str) -> Option<GeminiVideoTaskSeed> {
        let registry = self.registry.lock().ok()?;
        registry.clone_gemini(short_id)
    }

    fn list_active_snapshots(&self, limit: usize) -> Vec<LocalVideoTaskSnapshot> {
        let Ok(registry) = self.registry.lock() else {
            return Vec::new();
        };
        registry.list_active_snapshots(limit)
    }

    fn apply_mutation(&self, mutation: LocalVideoTaskRegistryMutation) {
        let _ = self.mutate_registry(|registry| {
            registry.apply_mutation(mutation);
            true
        });
    }

    fn project_openai(&self, task_id: &str, provider_body: &Map<String, Value>) -> bool {
        self.mutate_registry(|registry| registry.project_openai(task_id, provider_body))
    }

    fn project_gemini(&self, short_id: &str, provider_body: &Map<String, Value>) -> bool {
        self.mutate_registry(|registry| registry.project_gemini(short_id, provider_body))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
    use serde_json::json;

    use super::*;
    use crate::{LocalVideoTaskPersistence, LocalVideoTaskStatus, LocalVideoTaskTransport};

    fn temp_store_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "aether-video-store-{name}-{}-{}.json",
            std::process::id(),
            Uuid::new_v4()
        ))
    }

    fn cleanup_store_path(path: &Path) {
        std::fs::remove_file(path).ok();
        std::fs::remove_file(video_task_store_lock_path(path)).ok();
    }

    fn sensitive_gemini_snapshot() -> LocalVideoTaskSnapshot {
        LocalVideoTaskSnapshot::Gemini(GeminiVideoTaskSeed {
            local_short_id: "task-sensitive".to_string(),
            upstream_operation_name: "operations/upstream-sensitive".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            model: "veo-3".to_string(),
            status: LocalVideoTaskStatus::Failed,
            progress_percent: 100,
            error_code: Some("Bearer code-secret".to_string()),
            error_message: Some("Authorization: Bearer error-secret".to_string()),
            metadata: json!({
                "debug": "metadata-secret",
                "url": "https://internal.test/result?token=metadata-query-secret"
            }),
            persistence: LocalVideoTaskPersistence {
                request_id: "request-1".to_string(),
                username: Some("alice".to_string()),
                api_key_name: Some("primary".to_string()),
                client_api_format: "gemini:video".to_string(),
                provider_api_format: "gemini:video".to_string(),
                original_request_body: json!({"prompt": "create a video"}),
                format_converted: false,
            },
            transport: LocalVideoTaskTransport {
                upstream_base_url: "https://generativelanguage.googleapis.com".to_string(),
                provider_name: Some("gemini".to_string()),
                provider_id: "provider-1".to_string(),
                endpoint_id: "endpoint-1".to_string(),
                key_id: "key-1".to_string(),
                headers: BTreeMap::from([(
                    "x-goog-api-key".to_string(),
                    "transport-key-required-for-resume".to_string(),
                )]),
                content_type: Some("application/json".to_string()),
                model_name: Some("veo-3".to_string()),
                proxy: None,
                transport_profile: None,
                timeouts: None,
            },
        })
    }

    #[test]
    fn completed_gemini_download_survives_encrypted_file_reload() {
        let path = temp_store_path("completed-download");
        let video_url = "https://cdn.example.test/video.mp4?signature=a%2Fb%2Bc%3D";
        let mut snapshot = sensitive_gemini_snapshot();
        snapshot.apply_provider_body(
            json!({
                "done": true,
                "debug": "private-debug",
                "response": {
                    "generateVideoResponse": {
                        "generatedSamples": [{"video": {"uri": video_url}}]
                    }
                }
            })
            .as_object()
            .expect("provider body"),
        );
        let store = FileVideoTaskStore::new(&path, DEVELOPMENT_ENCRYPTION_KEY).expect("store");
        store.insert(snapshot);
        drop(store);
        let bytes = std::fs::read_to_string(&path).expect("encrypted store file");
        assert!(bytes.starts_with(ENCRYPTED_VIDEO_TASK_STORE_PREFIX));
        assert!(!bytes.contains(video_url));
        assert!(!bytes.contains("transport-key-required-for-resume"));
        let restored =
            FileVideoTaskStore::new(&path, DEVELOPMENT_ENCRYPTION_KEY).expect("restored store");
        let task = restored
            .clone_gemini("task-sensitive")
            .expect("completed Gemini task");
        let record = task.to_upsert_record();
        assert_eq!(
            record.status,
            aether_data_contracts::repository::video_tasks::VideoTaskStatus::Completed
        );
        assert_eq!(record.video_url.as_deref(), Some(video_url));
        assert_eq!(record.prompt.as_deref(), Some("create a video"));
        assert!(!task.metadata.to_string().contains("private-debug"));
        assert!(record.request_metadata.is_none());
        drop(restored);
        cleanup_store_path(&path);
    }

    #[test]
    fn loading_encrypted_store_rewrites_legacy_provider_diagnostics() {
        let path = temp_store_path("diagnostic-migration");
        let legacy_plaintext = serde_json::to_string(&json!({
            "openai": {},
            "gemini": {
                "task-sensitive": sensitive_gemini_snapshot()
            }
        }))
        .expect("legacy registry should serialize");
        let ciphertext =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, &legacy_plaintext)
                .expect("legacy registry should encrypt");
        std::fs::write(
            &path,
            format!("{LEGACY_ENCRYPTED_VIDEO_TASK_STORE_PREFIX}{ciphertext}\n"),
        )
        .expect("legacy encrypted registry should be written");

        let store = FileVideoTaskStore::new(&path, DEVELOPMENT_ENCRYPTION_KEY)
            .expect("legacy encrypted registry should load");

        let response = store
            .read_gemini("task-sensitive")
            .expect("migrated task should remain readable");
        assert_eq!(response.body_json["error"]["code"], "provider_error");
        assert_eq!(
            response.body_json["error"]["message"],
            "Video generation failed"
        );

        let bytes = std::fs::read(&path).expect("migrated registry should be readable");
        let ciphertext = bytes
            .strip_prefix(ENCRYPTED_VIDEO_TASK_STORE_PREFIX.as_bytes())
            .expect("migrated registry should stay encrypted");
        let ciphertext = std::str::from_utf8(ciphertext)
            .expect("ciphertext should be UTF-8")
            .trim();
        let migrated_plaintext =
            decrypt_python_fernet_ciphertext(DEVELOPMENT_ENCRYPTION_KEY, ciphertext)
                .expect("migrated registry should decrypt");
        for secret in [
            "code-secret",
            "error-secret",
            "metadata-secret",
            "metadata-query-secret",
        ] {
            assert!(
                !migrated_plaintext.contains(secret),
                "migrated registry leaked {secret}"
            );
        }
        assert!(migrated_plaintext.contains("transport-key-required-for-resume"));
        assert!(migrated_plaintext.contains("provider_error"));
        cleanup_store_path(&path);
    }

    #[test]
    fn rejects_plaintext_registry_without_rewriting_it() {
        let path = temp_store_path("plaintext-injection");
        let plaintext = serde_json::to_vec(&json!({
            "openai": {},
            "gemini": {},
        }))
        .expect("plaintext registry should serialize");
        std::fs::write(&path, &plaintext).expect("plaintext registry should be written");

        let error = FileVideoTaskStore::new(&path, DEVELOPMENT_ENCRYPTION_KEY)
            .expect_err("plaintext registry must fail closed");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("plaintext"));
        assert_eq!(
            std::fs::read(&path).expect("rejected registry should remain readable"),
            plaintext,
            "rejected plaintext must not be rewritten into an authenticated envelope",
        );
        cleanup_store_path(&path);
    }

    #[test]
    fn rejects_unknown_aether_envelopes_without_rewriting_them() {
        for (name, bytes) in [
            (
                "unknown-video-version",
                b"aether-video-tasks-v999\nopaque".as_slice(),
            ),
            (
                "foreign-aether-envelope",
                b"aether-other-v1\nopaque".as_slice(),
            ),
        ] {
            let path = temp_store_path(name);
            std::fs::write(&path, bytes).expect("unknown envelope should be written");

            let error = FileVideoTaskStore::new(&path, DEVELOPMENT_ENCRYPTION_KEY)
                .expect_err("unknown aether envelope must fail closed");

            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            assert!(error.to_string().contains("unsupported"));
            assert_eq!(
                std::fs::read(&path).expect("rejected envelope should remain readable"),
                bytes,
            );
            cleanup_store_path(&path);
        }
    }

    #[test]
    fn rejects_tampered_authenticated_store_without_rewriting_it() {
        let path = temp_store_path("tampered-v2");
        let mut bytes = encrypted_video_task_store_bytes(
            DEVELOPMENT_ENCRYPTION_KEY,
            &VideoTaskRegistry::default(),
        )
        .expect("encrypted registry should serialize");
        let tampered_index = ENCRYPTED_VIDEO_TASK_STORE_PREFIX.len() + 12;
        bytes[tampered_index] = if bytes[tampered_index] == b'A' {
            b'B'
        } else {
            b'A'
        };
        std::fs::write(&path, &bytes).expect("tampered registry should be written");

        let error = FileVideoTaskStore::new(&path, DEVELOPMENT_ENCRYPTION_KEY)
            .expect_err("tampered authenticated registry must fail closed");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("decryption failed"));
        assert_eq!(
            std::fs::read(&path).expect("tampered registry should remain readable"),
            bytes,
        );
        cleanup_store_path(&path);
    }

    #[test]
    fn legacy_migration_compare_before_replace_preserves_concurrent_replacement() {
        let path = temp_store_path("legacy-migration-race");
        let legacy_plaintext = serde_json::to_string(&json!({
            "openai": {},
            "gemini": {
                "task-sensitive": sensitive_gemini_snapshot(),
            },
        }))
        .expect("legacy registry should serialize");
        let legacy_ciphertext =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, &legacy_plaintext)
                .expect("legacy registry should encrypt");
        let legacy_bytes =
            format!("{LEGACY_ENCRYPTED_VIDEO_TASK_STORE_PREFIX}{legacy_ciphertext}\n").into_bytes();
        std::fs::write(&path, &legacy_bytes).expect("legacy registry should be written");

        let loaded = FileVideoTaskStore::load_registry(&path, DEVELOPMENT_ENCRYPTION_KEY)
            .expect("authenticated legacy registry should load");
        assert!(loaded.needs_rewrite);
        assert_eq!(
            loaded.persisted_file,
            PersistedVideoTaskStore::Bytes(legacy_bytes),
        );
        let store = FileVideoTaskStore {
            path: path.clone(),
            encryption_key: DEVELOPMENT_ENCRYPTION_KEY.to_string(),
            registry: Mutex::new(loaded.registry),
            persisted_file: Mutex::new(loaded.persisted_file),
        };

        let concurrent_replacement = encrypted_video_task_store_bytes(
            DEVELOPMENT_ENCRYPTION_KEY,
            &VideoTaskRegistry::default(),
        )
        .expect("replacement registry should encrypt");
        std::fs::write(&path, &concurrent_replacement)
            .expect("concurrent replacement should be written");

        let registry = store.registry.lock().expect("registry lock should succeed");
        let error = store
            .persist_registry(&registry)
            .expect_err("stale legacy migration must report a conflict");
        drop(registry);

        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(
            std::fs::read(&path).expect("concurrent replacement should remain readable"),
            concurrent_replacement,
            "stale migration must not overwrite a newer exact byte snapshot",
        );
        cleanup_store_path(&path);
    }

    #[test]
    fn rejects_valid_fernet_ciphertext_from_another_purpose() {
        let path = temp_store_path("purpose-mismatch");
        let protected = format!(
            "another-purpose\0{}",
            serde_json::to_string(&VideoTaskRegistry::default())
                .expect("empty registry should serialize")
        );
        let ciphertext = encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, &protected)
            .expect("foreign payload should encrypt");
        std::fs::write(
            &path,
            format!("{ENCRYPTED_VIDEO_TASK_STORE_PREFIX}{ciphertext}\n"),
        )
        .expect("foreign ciphertext should be written");

        let error = FileVideoTaskStore::new(&path, DEVELOPMENT_ENCRYPTION_KEY)
            .expect_err("foreign-purpose ciphertext must fail closed");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        cleanup_store_path(&path);
    }
}
