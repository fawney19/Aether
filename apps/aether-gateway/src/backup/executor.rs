use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::Aes256Gcm;
use aether_crypto::derive_python_fernet_key;
use bytes::Bytes;
use chrono::{DateTime, SecondsFormat, Utc};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Read;

use super::config::S3BackupConfig;
use super::scopes::BackupScope;
use super::store::{BackupObjectCreateResult, BackupObjectStore, BackupStoreError};
use super::BackupRestoreScope;

// V1: magic || version || nonce || ciphertext/tag.
// V2: magic || version || fixed-length key ID || nonce || ciphertext/tag.
const BACKUP_ENVELOPE_MAGIC: &[u8; 8] = b"AETHERBK";
const BACKUP_ENVELOPE_VERSION_V1: u8 = 1;
const BACKUP_ENVELOPE_VERSION_V2: u8 = 2;
const BACKUP_ENVELOPE_KEY_ID_LEN: usize = 16;
const BACKUP_ENVELOPE_NONCE_LEN: usize = 12;
const BACKUP_ENCRYPTION_CONTEXT_V1: &[u8] = b"aether-s3-backup-aes-256-gcm-v1";
const BACKUP_ENCRYPTION_CONTEXT_V2: &[u8] = b"aether-s3-backup-aes-256-gcm-v2";
const BACKUP_KEY_ID_CONTEXT: &[u8] = b"aether-s3-backup-key-id-v1";
const BACKUP_ENCRYPTION_NAME: &str = "aes-256-gcm-v2";
const MAX_LEGACY_V1_CANDIDATES: usize = 16;
const MAX_V2_CANDIDATES: usize = 256;
const MAX_BACKUP_OBJECTS_PER_PREFIX: usize = 10_000;
const MAX_ENCRYPTED_VARIANTS_PER_LEGACY_OBJECT: usize = 32;
const ZSTD_MAX_WINDOW_LOG: u32 = 27;
const MIN_NEW_BACKUP_ENCRYPTION_SECRET_BYTES: usize = 32;
const INSECURE_NEW_BACKUP_ENCRYPTION_SECRETS: &[&str] = &[
    "change-this-to-another-secure-random-string",
    "change-this-to-a-secure-random-string",
    "dev-encryption-key-do-not-use-in-production",
];
pub const DEFAULT_BACKUP_MAX_ENCRYPTED_BYTES: usize = 512 * 1024 * 1024;
pub const DEFAULT_BACKUP_MAX_JSON_BYTES: usize = 1024 * 1024 * 1024;
type HmacSha256 = Hmac<Sha256>;

pub struct BackupDecryptionKey {
    secret: String,
    v2_key_id: [u8; BACKUP_ENVELOPE_KEY_ID_LEN],
    allow_legacy_v1: bool,
}

impl BackupDecryptionKey {
    pub fn current(secret: impl Into<String>) -> Result<Self, BackupRestoreError> {
        Self::new(secret, false)
    }

    pub fn historical(secret: impl Into<String>) -> Result<Self, BackupRestoreError> {
        Self::new(secret, true)
    }

    pub fn v2_only(secret: impl Into<String>) -> Result<Self, BackupRestoreError> {
        Self::new(secret, false)
    }

    fn new(secret: impl Into<String>, allow_legacy_v1: bool) -> Result<Self, BackupRestoreError> {
        let secret = secret.into();
        if secret.trim().is_empty() {
            return Err(BackupRestoreError::InvalidKeyMaterial);
        }
        let v2_key = derive_backup_encryption_key(&secret, BACKUP_ENCRYPTION_CONTEXT_V2)
            .map_err(|_| BackupRestoreError::InvalidKeyMaterial)?;
        let v2_key_id =
            derive_backup_key_id(&v2_key).map_err(|_| BackupRestoreError::InvalidKeyMaterial)?;
        Ok(Self {
            secret,
            v2_key_id,
            allow_legacy_v1,
        })
    }
}

impl std::fmt::Debug for BackupDecryptionKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BackupDecryptionKey")
            .field("secret", &"[REDACTED]")
            .field("v2_key_id", &encode_key_id(&self.v2_key_id))
            .field("allow_legacy_v1", &self.allow_legacy_v1)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupRestoreLimits {
    pub max_encrypted_bytes: usize,
    pub max_json_bytes: usize,
}

impl Default for BackupRestoreLimits {
    fn default() -> Self {
        Self {
            max_encrypted_bytes: DEFAULT_BACKUP_MAX_ENCRYPTED_BYTES,
            max_json_bytes: DEFAULT_BACKUP_MAX_JSON_BYTES,
        }
    }
}

#[derive(Debug)]
pub struct RestoredBackupJson {
    json_bytes: Vec<u8>,
    pub envelope_version: u8,
    pub key_id: Option<String>,
    pub export_version: Option<String>,
    pub exported_at: Option<String>,
    restore_authority: BackupRestoreAuthority,
}

#[derive(Debug)]
pub(crate) struct BackupRestoreAuthority {
    scope: BackupRestoreScope,
}

impl BackupRestoreAuthority {
    fn new(scope: BackupRestoreScope) -> Self {
        Self { scope }
    }

    pub(crate) fn scope(&self) -> BackupRestoreScope {
        self.scope
    }
}

impl RestoredBackupJson {
    pub fn json_bytes(&self) -> &[u8] {
        &self.json_bytes
    }

    pub fn scope(&self) -> BackupRestoreScope {
        self.restore_authority.scope()
    }

    pub(crate) fn into_authenticated_parts(self) -> (Vec<u8>, BackupRestoreAuthority) {
        (self.json_bytes, self.restore_authority)
    }
}

#[derive(Debug, Deserialize)]
struct BackupJsonMetadata {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    exported_at: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum BackupRestoreError {
    #[error("backup envelope is invalid or unsupported")]
    InvalidEnvelope,

    #[error("backup object key is invalid")]
    InvalidObjectKey,

    #[error("backup encryption key is empty or invalid")]
    InvalidKeyMaterial,

    #[error("no configured backup key matches key ID {0}")]
    UnknownKeyId(String),

    #[error("backup authentication failed")]
    AuthenticationFailed,

    #[error("legacy v1 backup restore allows at most 16 candidate keys")]
    TooManyLegacyKeys,

    #[error("backup restore allows at most 256 v2 candidate keys")]
    TooManyV2Keys,

    #[error("encrypted backup exceeds the configured {limit} byte limit")]
    EncryptedSizeLimit { limit: usize },

    #[error("decompressed backup JSON exceeds the configured {limit} byte limit")]
    JsonSizeLimit { limit: usize },

    #[error("backup zstd decompression failed: {0}")]
    Decompression(#[source] std::io::Error),

    #[error("decompressed backup is not valid JSON: {0}")]
    InvalidJson(#[source] serde_json::Error),
}

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
    pub(crate) encryption: String,
    pub(crate) key_id: String,
    pub(crate) legacy_encrypted_copies_created: usize,
    pub(crate) legacy_encrypted_copies_verified: usize,
    pub(crate) legacy_plaintext_objects_deleted: usize,
    pub(crate) legacy_plaintext_objects_retained: usize,
    pub(crate) retention_cleanup_candidates: usize,
    pub(crate) versioned_storage_cleanup_required: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct LegacyMigrationResult {
    encrypted_copies_created: usize,
    encrypted_copies_verified: usize,
    plaintext_objects_deleted: usize,
    plaintext_objects_retained: usize,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum BackupExecutionError {
    #[error("S3 backup JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),

    #[error("S3 backup compression failed: {0}")]
    Compression(#[from] std::io::Error),

    #[error("S3 backup encryption failed")]
    Encryption,

    #[error("S3 backup encryption key is unsafe for creating new backups: {0}")]
    UnsafeEncryptionKey(&'static str),

    #[error("{0}")]
    Store(#[from] BackupStoreError),

    #[error("S3 backup compression `{0}` is not supported; expected `zstd`")]
    UnknownCompression(String),

    #[error(
        "legacy backup `{key}` has more than {limit} encrypted variants; refusing unbounded verification"
    )]
    TooManyEncryptedVariants { key: String, limit: usize },
}

pub(crate) async fn run_backup_with_store<S>(
    config: &S3BackupConfig,
    store: &S,
    payload: Value,
    now_utc: DateTime<Utc>,
    encryption_secret: &str,
) -> Result<BackupRunResult, BackupExecutionError>
where
    S: BackupObjectStore + ?Sized,
{
    validate_new_backup_encryption_secret(encryption_secret)?;
    let export_version = payload_string_field(&payload, "version").unwrap_or_default();
    let exported_at = payload_string_field(&payload, "exported_at")
        .unwrap_or_else(|| now_utc.to_rfc3339_opts(SecondsFormat::Secs, true));
    let json_bytes = serde_json::to_vec(&payload)?;
    let compression = config.compression.trim().to_string();
    let compressed_bytes = match compression.as_str() {
        "zstd" => zstd::stream::encode_all(json_bytes.as_slice(), 0)?,
        other => return Err(BackupExecutionError::UnknownCompression(other.to_string())),
    };
    let timestamp = now_utc.format("%Y%m%d-%H%M%S").to_string();
    let preferred_object_key = config.scope.object_key(&config.prefix, &timestamp);
    let (upload_bytes, key_id) =
        encrypt_backup_bytes(encryption_secret, &preferred_object_key, &compressed_bytes)?;
    let bytes = upload_bytes.len();
    let sha256 = format!("{:x}", Sha256::digest(&upload_bytes));

    let object_key = put_encrypted_object_without_overwrite(
        store,
        &preferred_object_key,
        Bytes::from(upload_bytes),
    )
    .await?;
    let legacy_migration =
        migrate_legacy_plaintext_backups(config, store, encryption_secret).await?;
    let retention_cleanup_candidates =
        count_retention_cleanup_candidates(config, store, &object_key).await?;

    Ok(BackupRunResult {
        scope: config.scope,
        bucket: config.bucket.clone(),
        object_key,
        bytes,
        sha256,
        export_version,
        exported_at,
        compression,
        encryption: BACKUP_ENCRYPTION_NAME.to_string(),
        key_id: encode_key_id(&key_id),
        legacy_encrypted_copies_created: legacy_migration.encrypted_copies_created,
        legacy_encrypted_copies_verified: legacy_migration.encrypted_copies_verified,
        legacy_plaintext_objects_deleted: legacy_migration.plaintext_objects_deleted,
        legacy_plaintext_objects_retained: legacy_migration.plaintext_objects_retained,
        retention_cleanup_candidates,
        versioned_storage_cleanup_required: legacy_migration.plaintext_objects_deleted > 0
            || legacy_migration.plaintext_objects_retained > 0
            || retention_cleanup_candidates > 0,
    })
}

fn validate_new_backup_encryption_secret(
    encryption_secret: &str,
) -> Result<(), BackupExecutionError> {
    let encryption_secret = encryption_secret.trim();
    if encryption_secret.as_bytes().len() < MIN_NEW_BACKUP_ENCRYPTION_SECRET_BYTES {
        return Err(BackupExecutionError::UnsafeEncryptionKey(
            "must contain at least 32 bytes",
        ));
    }
    if INSECURE_NEW_BACKUP_ENCRYPTION_SECRETS.contains(&encryption_secret) {
        return Err(BackupExecutionError::UnsafeEncryptionKey(
            "must not use a published example or development value",
        ));
    }
    Ok(())
}

pub(crate) fn encrypt_backup_bytes(
    encryption_secret: &str,
    object_key: &str,
    plaintext: &[u8],
) -> Result<(Vec<u8>, [u8; BACKUP_ENVELOPE_KEY_ID_LEN]), BackupExecutionError> {
    let key = derive_backup_encryption_key(encryption_secret, BACKUP_ENCRYPTION_CONTEXT_V2)?;
    let key_id = derive_backup_key_id(&key)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| BackupExecutionError::Encryption)?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let aad = backup_envelope_aad(BACKUP_ENVELOPE_VERSION_V2, Some(&key_id), object_key)?;
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| BackupExecutionError::Encryption)?;

    let envelope_header_len = BACKUP_ENVELOPE_MAGIC.len() + 1 + BACKUP_ENVELOPE_KEY_ID_LEN;
    let mut envelope = Vec::with_capacity(envelope_header_len + nonce.len() + ciphertext.len());
    envelope.extend_from_slice(&aad[..envelope_header_len]);
    envelope.extend_from_slice(&nonce);
    envelope.extend_from_slice(&ciphertext);
    Ok((envelope, key_id))
}

fn derive_backup_encryption_key(
    encryption_secret: &str,
    context: &[u8],
) -> Result<[u8; 32], BackupExecutionError> {
    let encryption_secret = encryption_secret.trim();
    if encryption_secret.is_empty() {
        return Err(BackupExecutionError::Encryption);
    }
    let root_key = derive_python_fernet_key(encryption_secret);
    let mut mac = <HmacSha256 as Mac>::new_from_slice(root_key.as_bytes())
        .map_err(|_| BackupExecutionError::Encryption)?;
    mac.update(context);
    Ok(mac.finalize().into_bytes().into())
}

fn derive_backup_key_id(
    encryption_key: &[u8; 32],
) -> Result<[u8; BACKUP_ENVELOPE_KEY_ID_LEN], BackupExecutionError> {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(encryption_key)
        .map_err(|_| BackupExecutionError::Encryption)?;
    mac.update(BACKUP_KEY_ID_CONTEXT);
    let digest = mac.finalize().into_bytes();
    let mut key_id = [0_u8; BACKUP_ENVELOPE_KEY_ID_LEN];
    key_id.copy_from_slice(&digest[..BACKUP_ENVELOPE_KEY_ID_LEN]);
    Ok(key_id)
}

fn backup_envelope_aad(
    version: u8,
    key_id: Option<&[u8; BACKUP_ENVELOPE_KEY_ID_LEN]>,
    object_key: &str,
) -> Result<Vec<u8>, BackupExecutionError> {
    let object_key = canonical_encrypted_object_key(object_key)?;
    let key_id_len = key_id.map_or(0, |_| BACKUP_ENVELOPE_KEY_ID_LEN);
    let mut aad =
        Vec::with_capacity(BACKUP_ENVELOPE_MAGIC.len() + 1 + key_id_len + object_key.len());
    aad.extend_from_slice(BACKUP_ENVELOPE_MAGIC);
    aad.push(version);
    if let Some(key_id) = key_id {
        aad.extend_from_slice(key_id);
    }
    aad.extend_from_slice(object_key.as_bytes());
    Ok(aad)
}

fn encode_key_id(key_id: &[u8; BACKUP_ENVELOPE_KEY_ID_LEN]) -> String {
    key_id.iter().map(|byte| format!("{byte:02x}")).collect()
}

async fn put_encrypted_object_without_overwrite<S>(
    store: &S,
    preferred_key: &str,
    bytes: Bytes,
) -> Result<String, BackupExecutionError>
where
    S: BackupObjectStore + ?Sized,
{
    match store
        .put_object_if_absent(preferred_key, bytes.clone())
        .await
    {
        Ok(BackupObjectCreateResult::Created) => return Ok(preferred_key.to_string()),
        Ok(BackupObjectCreateResult::AlreadyExists) => {}
        Err(error) => return Err(error.into()),
    }

    let collision_key = collision_safe_encrypted_key(preferred_key, &bytes)?;
    match store
        .put_object_if_absent(&collision_key, bytes.clone())
        .await
    {
        Ok(BackupObjectCreateResult::Created) => Ok(collision_key),
        Ok(BackupObjectCreateResult::AlreadyExists) => {
            let existing = store
                .get_object_limited(&collision_key, bytes.len())
                .await?;
            if existing == bytes {
                Ok(collision_key)
            } else {
                Err(BackupExecutionError::Encryption)
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn collision_safe_encrypted_key(
    preferred_key: &str,
    encrypted_bytes: &[u8],
) -> Result<String, BackupExecutionError> {
    const SUFFIX: &str = ".json.zst.aes256gcm";
    let Some(stem) = preferred_key.strip_suffix(SUFFIX) else {
        return Err(BackupExecutionError::Encryption);
    };
    let digest = format!("{:x}", Sha256::digest(encrypted_bytes));
    Ok(format!("{stem}-{digest}{SUFFIX}"))
}

fn canonical_encrypted_object_key(object_key: &str) -> Result<String, BackupExecutionError> {
    const SUFFIX: &str = ".json.zst.aes256gcm";
    if BackupScope::from_encrypted_object_key(object_key).is_none() {
        return Err(BackupExecutionError::Encryption);
    }
    let Some(stem) = object_key.strip_suffix(SUFFIX) else {
        return Err(BackupExecutionError::Encryption);
    };
    let canonical_stem = stem
        .rsplit_once('-')
        .filter(|(_, digest)| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .map(|(canonical_stem, _)| canonical_stem)
        .unwrap_or(stem);
    Ok(format!("{canonical_stem}{SUFFIX}"))
}

fn decrypt_backup_bytes(
    encryption_secret: &str,
    object_key: &str,
    envelope: &[u8],
) -> Result<Vec<u8>, BackupExecutionError> {
    let version = envelope
        .get(BACKUP_ENVELOPE_MAGIC.len())
        .copied()
        .ok_or(BackupExecutionError::Encryption)?;
    let (context, key_id_len) = match version {
        BACKUP_ENVELOPE_VERSION_V1 => (BACKUP_ENCRYPTION_CONTEXT_V1, 0),
        BACKUP_ENVELOPE_VERSION_V2 => (BACKUP_ENCRYPTION_CONTEXT_V2, BACKUP_ENVELOPE_KEY_ID_LEN),
        _ => return Err(BackupExecutionError::Encryption),
    };
    let envelope_header_len = BACKUP_ENVELOPE_MAGIC.len() + 1 + key_id_len;
    let header_len = envelope_header_len + BACKUP_ENVELOPE_NONCE_LEN;
    if envelope.len() <= header_len || !envelope.starts_with(BACKUP_ENVELOPE_MAGIC) {
        return Err(BackupExecutionError::Encryption);
    }
    let key_id = if version == BACKUP_ENVELOPE_VERSION_V2 {
        let mut key_id = [0_u8; BACKUP_ENVELOPE_KEY_ID_LEN];
        key_id.copy_from_slice(&envelope[BACKUP_ENVELOPE_MAGIC.len() + 1..envelope_header_len]);
        Some(key_id)
    } else {
        None
    };
    let aad = backup_envelope_aad(version, key_id.as_ref(), object_key)?;
    if envelope.get(..envelope_header_len) != Some(&aad[..envelope_header_len]) {
        return Err(BackupExecutionError::Encryption);
    }
    let key = derive_backup_encryption_key(encryption_secret, context)?;
    if key_id.is_some_and(|expected| derive_backup_key_id(&key).ok() != Some(expected)) {
        return Err(BackupExecutionError::Encryption);
    }
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| BackupExecutionError::Encryption)?;
    let nonce = aes_gcm::Nonce::from_slice(&envelope[envelope_header_len..header_len]);
    cipher
        .decrypt(
            nonce,
            Payload {
                msg: &envelope[header_len..],
                aad: &aad,
            },
        )
        .map_err(|_| BackupExecutionError::Encryption)
}

pub fn restore_backup_json(
    object_key: &str,
    envelope: &[u8],
    candidates: &[BackupDecryptionKey],
    limits: BackupRestoreLimits,
) -> Result<RestoredBackupJson, BackupRestoreError> {
    if envelope.len() > limits.max_encrypted_bytes {
        return Err(BackupRestoreError::EncryptedSizeLimit {
            limit: limits.max_encrypted_bytes,
        });
    }
    if candidates.len() > MAX_V2_CANDIDATES {
        return Err(BackupRestoreError::TooManyV2Keys);
    }
    let scope = BackupScope::from_encrypted_object_key(object_key)
        .ok_or(BackupRestoreError::InvalidObjectKey)?;
    canonical_encrypted_object_key(object_key).map_err(|_| BackupRestoreError::InvalidObjectKey)?;
    if envelope.len() <= BACKUP_ENVELOPE_MAGIC.len() || !envelope.starts_with(BACKUP_ENVELOPE_MAGIC)
    {
        return Err(BackupRestoreError::InvalidEnvelope);
    }

    let version = envelope[BACKUP_ENVELOPE_MAGIC.len()];
    let (compressed, key_id) = match version {
        BACKUP_ENVELOPE_VERSION_V2 => {
            let key_id_start = BACKUP_ENVELOPE_MAGIC.len() + 1;
            let key_id_end = key_id_start + BACKUP_ENVELOPE_KEY_ID_LEN;
            let key_id_bytes = envelope
                .get(key_id_start..key_id_end)
                .ok_or(BackupRestoreError::InvalidEnvelope)?;
            let mut key_id = [0_u8; BACKUP_ENVELOPE_KEY_ID_LEN];
            key_id.copy_from_slice(key_id_bytes);
            let encoded_key_id = encode_key_id(&key_id);
            let candidate = candidates
                .iter()
                .find(|candidate| candidate.v2_key_id == key_id)
                .ok_or_else(|| BackupRestoreError::UnknownKeyId(encoded_key_id.clone()))?;
            let compressed = decrypt_backup_bytes(&candidate.secret, object_key, envelope)
                .map_err(|_| BackupRestoreError::AuthenticationFailed)?;
            (compressed, Some(encoded_key_id))
        }
        BACKUP_ENVELOPE_VERSION_V1 => {
            let legacy_candidates: Vec<_> = candidates
                .iter()
                .filter(|candidate| candidate.allow_legacy_v1)
                .collect();
            if legacy_candidates.len() > MAX_LEGACY_V1_CANDIDATES {
                return Err(BackupRestoreError::TooManyLegacyKeys);
            }
            let compressed = legacy_candidates
                .into_iter()
                .find_map(|candidate| {
                    decrypt_backup_bytes(&candidate.secret, object_key, envelope).ok()
                })
                .ok_or(BackupRestoreError::AuthenticationFailed)?;
            (compressed, None)
        }
        _ => return Err(BackupRestoreError::InvalidEnvelope),
    };

    let json_bytes = decompress_backup_json(&compressed, limits.max_json_bytes)?;
    let mut deserializer = serde_json::Deserializer::from_slice(&json_bytes);
    let metadata = BackupJsonMetadata::deserialize(&mut deserializer)
        .map_err(BackupRestoreError::InvalidJson)?;
    deserializer
        .end()
        .map_err(BackupRestoreError::InvalidJson)?;
    if metadata
        .version
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
        || metadata
            .exported_at
            .as_deref()
            .map(str::trim)
            .is_none_or(str::is_empty)
    {
        return Err(BackupRestoreError::InvalidJson(serde_json::Error::io(
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "backup JSON must include non-empty version and exported_at fields",
            ),
        )));
    }
    Ok(RestoredBackupJson {
        json_bytes,
        envelope_version: version,
        key_id,
        export_version: metadata.version,
        exported_at: metadata.exported_at,
        restore_authority: BackupRestoreAuthority::new(match scope {
            BackupScope::Config => BackupRestoreScope::Config,
            BackupScope::Users => BackupRestoreScope::Users,
            BackupScope::Data => BackupRestoreScope::Data,
        }),
    })
}

fn decompress_backup_json(
    compressed: &[u8],
    max_json_bytes: usize,
) -> Result<Vec<u8>, BackupRestoreError> {
    let mut decoder =
        zstd::stream::read::Decoder::new(compressed).map_err(BackupRestoreError::Decompression)?;
    decoder
        .window_log_max(ZSTD_MAX_WINDOW_LOG)
        .map_err(BackupRestoreError::Decompression)?;
    let read_limit = u64::try_from(max_json_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut limited = decoder.take(read_limit);
    let mut json_bytes = Vec::with_capacity(max_json_bytes.min(8 * 1024 * 1024));
    limited
        .read_to_end(&mut json_bytes)
        .map_err(BackupRestoreError::Decompression)?;
    if json_bytes.len() > max_json_bytes {
        return Err(BackupRestoreError::JsonSizeLimit {
            limit: max_json_bytes,
        });
    }
    Ok(json_bytes)
}

async fn migrate_legacy_plaintext_backups<S>(
    config: &S3BackupConfig,
    store: &S,
    encryption_secret: &str,
) -> Result<LegacyMigrationResult, BackupExecutionError>
where
    S: BackupObjectStore + ?Sized,
{
    let keys = store
        .list_keys_limited(&config.prefix, MAX_BACKUP_OBJECTS_PER_PREFIX)
        .await?;
    let mut legacy_plaintext_keys = Vec::new();
    let mut encrypted_keys = Vec::new();
    for scope in [BackupScope::Config, BackupScope::Users, BackupScope::Data] {
        legacy_plaintext_keys.extend(
            scope.matching_legacy_plaintext_backup_keys(&config.prefix, keys.iter().cloned()),
        );
        encrypted_keys
            .extend(scope.matching_encrypted_backup_keys(&config.prefix, keys.iter().cloned()));
    }
    legacy_plaintext_keys.sort();
    legacy_plaintext_keys.dedup();
    encrypted_keys.sort();
    encrypted_keys.dedup();

    let mut result = LegacyMigrationResult::default();
    for key in legacy_plaintext_keys {
        let plaintext = store
            .get_object_limited(&key, DEFAULT_BACKUP_MAX_ENCRYPTED_BYTES)
            .await?;
        let candidates: Vec<_> = encrypted_keys
            .iter()
            .filter(|encrypted_key| is_encrypted_variant_of_legacy_key(&key, encrypted_key))
            .collect();
        if candidates.len() > MAX_ENCRYPTED_VARIANTS_PER_LEGACY_OBJECT {
            return Err(BackupExecutionError::TooManyEncryptedVariants {
                key,
                limit: MAX_ENCRYPTED_VARIANTS_PER_LEGACY_OBJECT,
            });
        }
        let mut matching_encrypted_copy_exists = false;
        for candidate in candidates {
            let existing = store
                .get_object_limited(candidate, DEFAULT_BACKUP_MAX_ENCRYPTED_BYTES)
                .await?;
            if decrypt_backup_bytes(encryption_secret, candidate, &existing)
                .is_ok_and(|decrypted| decrypted.as_slice() == &plaintext[..])
            {
                matching_encrypted_copy_exists = true;
                break;
            }
        }

        if matching_encrypted_copy_exists {
            result.encrypted_copies_verified += 1;
            store.delete_object(&key).await?;
            result.plaintext_objects_deleted += 1;
            continue;
        }

        let encrypted_key = format!("{key}.aes256gcm");
        let (encrypted, _) = encrypt_backup_bytes(encryption_secret, &encrypted_key, &plaintext)?;
        let created_key =
            put_encrypted_object_without_overwrite(store, &encrypted_key, Bytes::from(encrypted))
                .await?;
        let stored_encrypted = store
            .get_object_limited(&created_key, DEFAULT_BACKUP_MAX_ENCRYPTED_BYTES)
            .await?;
        let verified_plaintext =
            decrypt_backup_bytes(encryption_secret, &created_key, stored_encrypted.as_ref())
                .map_err(|_| BackupExecutionError::Encryption)?;
        if verified_plaintext.as_slice() != plaintext.as_ref() {
            return Err(BackupExecutionError::Encryption);
        }
        encrypted_keys.push(created_key);
        result.encrypted_copies_created += 1;
        store.delete_object(&key).await?;
        result.plaintext_objects_deleted += 1;
    }
    Ok(result)
}

fn is_encrypted_variant_of_legacy_key(legacy_key: &str, encrypted_key: &str) -> bool {
    const LEGACY_SUFFIX: &str = ".json.zst";
    const ENCRYPTED_SUFFIX: &str = ".json.zst.aes256gcm";

    if encrypted_key == format!("{legacy_key}.aes256gcm") {
        return true;
    }

    let Some(legacy_stem) = legacy_key.strip_suffix(LEGACY_SUFFIX) else {
        return false;
    };
    let Some(digest) = encrypted_key
        .strip_prefix(legacy_stem)
        .and_then(|rest| rest.strip_prefix('-'))
        .and_then(|rest| rest.strip_suffix(ENCRYPTED_SUFFIX))
    else {
        return false;
    };

    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

async fn count_retention_cleanup_candidates<S>(
    config: &S3BackupConfig,
    store: &S,
    current_object_key: &str,
) -> Result<usize, BackupExecutionError>
where
    S: BackupObjectStore + ?Sized,
{
    let keys = store
        .list_keys_limited(&config.prefix, MAX_BACKUP_OBJECTS_PER_PREFIX)
        .await?;
    let mut matching_keys = config
        .scope
        .matching_encrypted_backup_keys(&config.prefix, keys);
    matching_keys.sort_by(|left, right| right.cmp(left));

    let mut cleanup_candidates = 0;
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

        cleanup_candidates += 1;
    }

    Ok(cleanup_candidates)
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
    use super::{
        backup_envelope_aad, decrypt_backup_bytes, derive_backup_encryption_key,
        encrypt_backup_bytes, restore_backup_json, run_backup_with_store,
        validate_new_backup_encryption_secret, BackupDecryptionKey, BackupRestoreError,
        BackupRestoreLimits, BACKUP_ENCRYPTION_CONTEXT_V1, BACKUP_ENVELOPE_MAGIC,
        BACKUP_ENVELOPE_NONCE_LEN, BACKUP_ENVELOPE_VERSION_V1, BACKUP_ENVELOPE_VERSION_V2,
    };
    use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
    use aes_gcm::Aes256Gcm;
    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
    use bytes::Bytes;
    use chrono::{DateTime, Utc};
    use serde_json::json;

    const TEST_NEW_BACKUP_ENCRYPTION_SECRET: &str = "test-only-backup-encryption-secret-2026-08-30";

    fn compressed_json(value: serde_json::Value) -> Vec<u8> {
        zstd::stream::encode_all(serde_json::to_vec(&value).unwrap().as_slice(), 0).unwrap()
    }

    fn encrypt_v1_for_test(secret: &str, object_key: &str, plaintext: &[u8]) -> Vec<u8> {
        let key = derive_backup_encryption_key(secret, BACKUP_ENCRYPTION_CONTEXT_V1).unwrap();
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let aad = backup_envelope_aad(BACKUP_ENVELOPE_VERSION_V1, None, object_key).unwrap();
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .unwrap();
        let mut envelope = Vec::with_capacity(
            BACKUP_ENVELOPE_MAGIC.len() + 1 + BACKUP_ENVELOPE_NONCE_LEN + ciphertext.len(),
        );
        envelope.extend_from_slice(BACKUP_ENVELOPE_MAGIC);
        envelope.push(BACKUP_ENVELOPE_VERSION_V1);
        envelope.extend_from_slice(&nonce);
        envelope.extend_from_slice(&ciphertext);
        envelope
    }

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
    async fn backup_executor_encrypts_then_deletes_legacy_plaintext_objects() {
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

        let result = run_backup_with_store(
            &config,
            &store,
            payload,
            now_utc,
            TEST_NEW_BACKUP_ENCRYPTION_SECRET,
        )
        .await
        .expect("backup should succeed");

        assert_eq!(result.scope, BackupScope::Data);
        assert_eq!(result.bucket, "aether-backups");
        assert_eq!(
            result.object_key,
            "prod/aether-data-backup-20260523-191500.json.zst.aes256gcm"
        );
        assert!(result.bytes > 0);
        assert_eq!(result.sha256.len(), 64);
        assert_eq!(result.export_version, "1.0");
        assert_eq!(result.exported_at, "2026-05-24T03:15:00Z");
        assert_eq!(result.compression, "zstd");
        assert_eq!(result.encryption, "aes-256-gcm-v2");
        assert_eq!(result.key_id.len(), 32);
        assert!(result.key_id.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(result.legacy_encrypted_copies_created, 2);
        assert_eq!(result.legacy_encrypted_copies_verified, 0);
        assert_eq!(result.legacy_plaintext_objects_deleted, 2);
        assert_eq!(result.legacy_plaintext_objects_retained, 0);
        assert_eq!(result.retention_cleanup_candidates, 1);
        assert!(result.versioned_storage_cleanup_required);

        let keys = store.list_keys_limited("prod/", 10).await.unwrap();
        assert!(keys
            .iter()
            .any(|key| key == "prod/aether-config-backup-20260524-010000.json.zst.aes256gcm"));
        assert!(!keys
            .iter()
            .any(|key| key == "prod/aether-config-backup-20260524-010000.json.zst"));
        assert!(keys
            .iter()
            .any(|key| key == "prod/aether-data-backup-20260523-191500.json.zst.aes256gcm"));
        assert!(!keys
            .iter()
            .any(|key| key == "prod/aether-data-backup-20260524-010000.json.zst"));

        let uploaded = store
            .object_bytes(&result.object_key)
            .await
            .expect("uploaded backup should be readable in the fake store");
        assert!(uploaded.starts_with(BACKUP_ENVELOPE_MAGIC));
        assert!(!uploaded
            .windows("config_data".len())
            .any(|window| window == b"config_data"));
        let compressed = decrypt_backup_bytes(
            TEST_NEW_BACKUP_ENCRYPTION_SECRET,
            &result.object_key,
            &uploaded,
        )
        .expect("encrypted backup should decrypt");
        let decoded = zstd::stream::decode_all(compressed.as_slice())
            .expect("decrypted backup should decompress");
        let decoded: serde_json::Value =
            serde_json::from_slice(&decoded).expect("decrypted backup should contain JSON");
        assert_eq!(decoded["version"], "1.0");

        let migrated_copy = store
            .object_bytes("prod/aether-config-backup-20260524-010000.json.zst.aes256gcm")
            .await
            .expect("legacy backup should be migrated");
        assert_eq!(
            decrypt_backup_bytes(
                TEST_NEW_BACKUP_ENCRYPTION_SECRET,
                "prod/aether-config-backup-20260524-010000.json.zst.aes256gcm",
                &migrated_copy,
            )
            .expect("migrated backup should decrypt")
            .as_slice(),
            b"keep-config".as_slice()
        );
    }

    #[tokio::test]
    async fn legacy_migration_never_overwrites_an_existing_encrypted_object() {
        let store = FakeBackupObjectStore::default();
        let legacy_key = "prod/aether-data-backup-20260524-010000.json.zst";
        let preferred_encrypted_key = "prod/aether-data-backup-20260524-010000.json.zst.aes256gcm";
        store
            .put_object(legacy_key, Bytes::from_static(b"legacy payload"))
            .await
            .unwrap();
        store
            .put_object(
                preferred_encrypted_key,
                Bytes::from_static(b"existing encrypted object"),
            )
            .await
            .unwrap();

        let config = sample_backup_config(BackupScope::Data, 10);
        let migration =
            super::migrate_legacy_plaintext_backups(&config, &store, DEVELOPMENT_ENCRYPTION_KEY)
                .await
                .expect("legacy backup should migrate without overwriting");

        assert_eq!(migration.encrypted_copies_created, 1);
        assert_eq!(migration.encrypted_copies_verified, 0);
        assert_eq!(migration.plaintext_objects_deleted, 1);
        assert_eq!(migration.plaintext_objects_retained, 0);
        assert!(store.object_bytes(legacy_key).await.is_none());
        assert_eq!(
            store.object_bytes(preferred_encrypted_key).await.as_deref(),
            Some(b"existing encrypted object".as_slice())
        );

        let keys = store.list_keys_limited("prod/", 10).await.unwrap();
        let collision_key = keys
            .iter()
            .find(|key| {
                key.starts_with("prod/aether-data-backup-20260524-010000-")
                    && key.ends_with(".json.zst.aes256gcm")
            })
            .expect("migration should create a collision-safe encrypted object");
        let migrated_bytes = store.object_bytes(collision_key).await.unwrap();
        assert_eq!(
            decrypt_backup_bytes(DEVELOPMENT_ENCRYPTION_KEY, collision_key, &migrated_bytes)
                .expect("collision-safe backup should decrypt")
                .as_slice(),
            b"legacy payload".as_slice()
        );
    }

    #[tokio::test]
    async fn legacy_migration_retry_reuses_matching_encrypted_copy() {
        let store = FakeBackupObjectStore::default();
        let legacy_key = "prod/aether-users-backup-20260524-010000.json.zst";
        let encrypted_key = "prod/aether-users-backup-20260524-010000.json.zst.aes256gcm";
        let (encrypted, _) = super::encrypt_backup_bytes(
            DEVELOPMENT_ENCRYPTION_KEY,
            encrypted_key,
            b"legacy payload",
        )
        .expect("test payload should encrypt");
        store
            .put_object(legacy_key, Bytes::from_static(b"legacy payload"))
            .await
            .unwrap();
        store
            .put_object(encrypted_key, Bytes::from(encrypted.clone()))
            .await
            .unwrap();

        let config = sample_backup_config(BackupScope::Data, 10);
        let migration =
            super::migrate_legacy_plaintext_backups(&config, &store, DEVELOPMENT_ENCRYPTION_KEY)
                .await
                .expect("retry should recognize the existing encrypted copy");

        assert_eq!(migration.encrypted_copies_created, 0);
        assert_eq!(migration.encrypted_copies_verified, 1);
        assert_eq!(migration.plaintext_objects_deleted, 1);
        assert_eq!(migration.plaintext_objects_retained, 0);
        assert!(store.object_bytes(legacy_key).await.is_none());
        assert_eq!(
            store.object_bytes(encrypted_key).await.as_deref(),
            Some(encrypted.as_slice())
        );
        assert_eq!(store.list_keys_limited("prod/", 10).await.unwrap().len(), 1);
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

        let error = run_backup_with_store(
            &config,
            &store,
            payload,
            now_utc,
            TEST_NEW_BACKUP_ENCRYPTION_SECRET,
        )
        .await
        .expect_err("unknown compression should fail");

        assert!(error.to_string().contains("brotli"));
    }

    #[test]
    fn new_backup_key_policy_rejects_weak_creation_keys_but_keeps_restore_compatibility() {
        for insecure in [
            "short-backup-key",
            "change-this-to-another-secure-random-string",
            "change-this-to-a-secure-random-string",
            DEVELOPMENT_ENCRYPTION_KEY,
        ] {
            let error = validate_new_backup_encryption_secret(insecure)
                .expect_err("new backup creation must reject weak or published keys");
            assert!(error.to_string().contains("unsafe"));

            BackupDecryptionKey::historical(insecure)
                .expect("historical restore must continue accepting legacy weak keys");
        }

        validate_new_backup_encryption_secret(TEST_NEW_BACKUP_ENCRYPTION_SECRET)
            .expect("strong new backup key should be accepted");
    }

    #[test]
    fn backup_envelope_rejects_wrong_keys_and_tampering() {
        let object_key = "prod/aether-data-backup-20260524-010000.json.zst.aes256gcm";
        let (encrypted, _) =
            super::encrypt_backup_bytes(DEVELOPMENT_ENCRYPTION_KEY, object_key, b"secret payload")
                .expect("backup should encrypt");
        assert!(decrypt_backup_bytes("wrong-key", object_key, &encrypted).is_err());
        assert!(decrypt_backup_bytes(
            DEVELOPMENT_ENCRYPTION_KEY,
            "prod/aether-users-backup-20260524-010000.json.zst.aes256gcm",
            &encrypted,
        )
        .is_err());

        let mut tampered = encrypted;
        let last = tampered.last_mut().expect("envelope should not be empty");
        *last ^= 1;
        assert!(decrypt_backup_bytes(DEVELOPMENT_ENCRYPTION_KEY, object_key, &tampered).is_err());
    }

    #[test]
    fn v2_envelope_has_stable_non_secret_key_id_and_restores_json() {
        let object_key = "prod/aether-data-backup-20260524-010000.json.zst.aes256gcm";
        let compressed = compressed_json(json!({
            "version": "1.6",
            "exported_at": "2026-05-24T01:00:00Z",
            "value": 42
        }));
        let (first, first_id) =
            encrypt_backup_bytes(DEVELOPMENT_ENCRYPTION_KEY, object_key, &compressed).unwrap();
        let (second, second_id) =
            encrypt_backup_bytes(DEVELOPMENT_ENCRYPTION_KEY, object_key, &compressed).unwrap();

        assert_eq!(
            first[BACKUP_ENVELOPE_MAGIC.len()],
            BACKUP_ENVELOPE_VERSION_V2
        );
        assert_eq!(first_id, second_id);
        assert_ne!(
            first, second,
            "fresh nonces must produce different envelopes"
        );
        let (_, other_id) =
            encrypt_backup_bytes("different-secret", object_key, &compressed).unwrap();
        assert_ne!(first_id, other_id);

        let restored = restore_backup_json(
            object_key,
            &first,
            &[
                BackupDecryptionKey::current("wrong-secret").unwrap(),
                BackupDecryptionKey::current(DEVELOPMENT_ENCRYPTION_KEY).unwrap(),
            ],
            BackupRestoreLimits::default(),
        )
        .unwrap();
        assert_eq!(restored.envelope_version, BACKUP_ENVELOPE_VERSION_V2);
        assert_eq!(restored.scope(), super::BackupRestoreScope::Data);
        let expected_key_id = super::encode_key_id(&first_id);
        assert_eq!(restored.key_id.as_deref(), Some(expected_key_id.as_str()));
        let json: serde_json::Value = serde_json::from_slice(restored.json_bytes()).unwrap();
        assert_eq!(json["value"], 42);
    }

    #[test]
    fn v2_unknown_key_id_fails_before_authenticated_decryption() {
        let object_key = "prod/aether-data-backup-20260524-010000.json.zst.aes256gcm";
        let compressed = compressed_json(json!({
            "version": "1.6",
            "exported_at": "2026-05-24T01:00:00Z"
        }));
        let (encrypted, _) =
            encrypt_backup_bytes(DEVELOPMENT_ENCRYPTION_KEY, object_key, &compressed).unwrap();
        let mut forged_key_id = encrypted.clone();
        forged_key_id[BACKUP_ENVELOPE_MAGIC.len() + 1] ^= 1;

        let error = restore_backup_json(
            object_key,
            &forged_key_id,
            &[BackupDecryptionKey::current(DEVELOPMENT_ENCRYPTION_KEY).unwrap()],
            BackupRestoreLimits::default(),
        )
        .unwrap_err();
        assert!(matches!(error, BackupRestoreError::UnknownKeyId(_)));
    }

    #[test]
    fn v2_aad_rejects_object_key_swap() {
        let object_key = "prod/aether-data-backup-20260524-010000.json.zst.aes256gcm";
        let compressed = compressed_json(json!({
            "version": "1.6",
            "exported_at": "2026-05-24T01:00:00Z"
        }));
        let (encrypted, _) =
            encrypt_backup_bytes(DEVELOPMENT_ENCRYPTION_KEY, object_key, &compressed).unwrap();
        let error = restore_backup_json(
            "prod/aether-users-backup-20260524-010000.json.zst.aes256gcm",
            &encrypted,
            &[BackupDecryptionKey::current(DEVELOPMENT_ENCRYPTION_KEY).unwrap()],
            BackupRestoreLimits::default(),
        )
        .unwrap_err();
        assert!(matches!(error, BackupRestoreError::AuthenticationFailed));
    }

    #[test]
    fn restore_rejects_traversal_and_non_backup_object_keys_before_decryption() {
        for object_key in [
            "../aether-data-backup-20260524-010000.json.zst.aes256gcm",
            "/aether-data-backup-20260524-010000.json.zst.aes256gcm",
            "prod//aether-data-backup-20260524-010000.json.zst.aes256gcm",
            "prod/aether-data-backup-invalid.json.zst.aes256gcm",
            "prod/not-an-aether-backup-20260524-010000.json.zst.aes256gcm",
        ] {
            let error = restore_backup_json(object_key, b"", &[], BackupRestoreLimits::default())
                .expect_err("unsafe object key must fail before envelope processing");
            assert!(
                matches!(error, BackupRestoreError::InvalidObjectKey),
                "unexpected error for {object_key}: {error}"
            );
        }
    }

    #[test]
    fn restore_keeps_v1_compatibility_and_tries_only_legacy_candidates() {
        let object_key = "prod/aether-config-backup-20260524-010000.json.zst.aes256gcm";
        let compressed = compressed_json(json!({
            "version": "2.3",
            "exported_at": "2026-05-24T01:00:00Z",
            "config_data": {}
        }));
        let encrypted = encrypt_v1_for_test(DEVELOPMENT_ENCRYPTION_KEY, object_key, &compressed);
        let restored = restore_backup_json(
            object_key,
            &encrypted,
            &[
                BackupDecryptionKey::v2_only(DEVELOPMENT_ENCRYPTION_KEY).unwrap(),
                BackupDecryptionKey::historical("wrong-legacy-secret").unwrap(),
                BackupDecryptionKey::historical(DEVELOPMENT_ENCRYPTION_KEY).unwrap(),
            ],
            BackupRestoreLimits::default(),
        )
        .unwrap();
        assert_eq!(restored.envelope_version, BACKUP_ENVELOPE_VERSION_V1);
        assert_eq!(restored.key_id, None);
        assert_eq!(restored.export_version.as_deref(), Some("2.3"));

        let too_many: Vec<_> = (0..17)
            .map(|index| BackupDecryptionKey::historical(format!("legacy-{index}")).unwrap())
            .collect();
        assert!(matches!(
            restore_backup_json(
                object_key,
                &encrypted,
                &too_many,
                BackupRestoreLimits::default()
            ),
            Err(BackupRestoreError::TooManyLegacyKeys)
        ));
    }

    #[test]
    fn restore_rejects_zstd_output_over_limit() {
        let object_key = "prod/aether-users-backup-20260524-010000.json.zst.aes256gcm";
        let compressed = compressed_json(json!({
            "version": "1.6",
            "exported_at": "2026-05-24T01:00:00Z",
            "padding": "x".repeat(4096)
        }));
        let (encrypted, _) =
            encrypt_backup_bytes(DEVELOPMENT_ENCRYPTION_KEY, object_key, &compressed).unwrap();
        let error = restore_backup_json(
            object_key,
            &encrypted,
            &[BackupDecryptionKey::current(DEVELOPMENT_ENCRYPTION_KEY).unwrap()],
            BackupRestoreLimits {
                max_encrypted_bytes: encrypted.len(),
                max_json_bytes: 128,
            },
        )
        .unwrap_err();
        assert!(matches!(error, BackupRestoreError::JsonSizeLimit { .. }));
    }
}
