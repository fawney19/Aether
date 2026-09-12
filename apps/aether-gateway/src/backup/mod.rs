pub(crate) mod config;
pub(crate) mod executor;
pub(crate) mod schedule;
pub(crate) mod scopes;
pub(crate) mod store;
pub(crate) mod task;
pub(crate) mod worker;

pub use executor::{
    restore_backup_json, BackupDecryptionKey, BackupRestoreError, BackupRestoreLimits,
    RestoredBackupJson, DEFAULT_BACKUP_MAX_ENCRYPTED_BYTES, DEFAULT_BACKUP_MAX_JSON_BYTES,
};

use axum::body::Bytes;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupRestoreScope {
    Config,
    Users,
    Data,
}

impl BackupRestoreScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Users => "users",
            Self::Data => "data",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("backup database apply failed: {0}")]
pub struct BackupApplyError(String);

pub async fn apply_restored_backup(
    app: &crate::AppState,
    restored: RestoredBackupJson,
    scope: BackupRestoreScope,
    operator_id: Option<&str>,
) -> Result<Result<Value, (http::StatusCode, Value)>, BackupApplyError> {
    let (json_bytes, authority) = restored.into_authenticated_parts();
    if authority.scope() != scope {
        return Err(BackupApplyError(format!(
            "authenticated {} backup cannot be applied to {} scope",
            authority.scope().as_str(),
            scope.as_str(),
        )));
    }
    let request_body = Bytes::from(json_bytes);
    let state = crate::admin_api::AdminAppState::new(app);
    let result = crate::admin_api::execute_admin_system_import_exclusively(app, async {
        match scope {
            BackupRestoreScope::Config => {
                state
                    .restore_admin_system_config_backup(&request_body, authority)
                    .await
            }
            BackupRestoreScope::Users => {
                state
                    .restore_admin_system_users_backup(&request_body, operator_id, authority)
                    .await
            }
            BackupRestoreScope::Data => {
                state
                    .restore_admin_system_data_backup(&request_body, operator_id, authority)
                    .await
            }
        }
    })
    .await
    .map_err(|error| {
        let message = match error {
            crate::admin_api::AdminSystemImportLockError::Conflict => {
                "another system import or restore is already running"
            }
            crate::admin_api::AdminSystemImportLockError::Unavailable => {
                "system import coordination is unavailable"
            }
            crate::admin_api::AdminSystemImportLockError::Lost => {
                "system import coordination lease was lost; restore was cancelled and may have partially applied changes"
            }
        };
        BackupApplyError(message.to_string())
    })?;
    result.map_err(|error| BackupApplyError(error.into_message()))
}

pub(crate) const S3_BACKUP_ENABLED_KEY: &str = "backup_s3_enabled";
pub(crate) const S3_BACKUP_LAST_SLOT_KEY: &str = "backup_s3_last_slot";

#[cfg(test)]
mod tests {
    use super::{
        apply_restored_backup, BackupDecryptionKey, BackupRestoreLimits, BackupRestoreScope,
        RestoredBackupJson,
    };
    use crate::backup::executor::encrypt_backup_bytes;
    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
    use serde_json::json;

    fn authenticated_users_backup() -> RestoredBackupJson {
        let object_key = "prod/aether-users-backup-20260830-120000.json.zst.aes256gcm";
        let compressed = zstd::stream::encode_all(
            serde_json::to_vec(&json!({
                "version": "1.5",
                "exported_at": "2026-08-30T12:00:00Z",
                "users": [],
                "standalone_keys": [],
            }))
            .expect("test backup should serialize")
            .as_slice(),
            0,
        )
        .expect("test backup should compress");
        let (envelope, _) =
            encrypt_backup_bytes(DEVELOPMENT_ENCRYPTION_KEY, object_key, &compressed)
                .expect("test backup should encrypt");
        super::restore_backup_json(
            object_key,
            &envelope,
            &[BackupDecryptionKey::current(DEVELOPMENT_ENCRYPTION_KEY)
                .expect("test restore key should build")],
            BackupRestoreLimits::default(),
        )
        .expect("test backup should authenticate")
    }

    #[tokio::test]
    async fn authenticated_backup_cannot_be_applied_to_a_different_scope() {
        let restored = authenticated_users_backup();

        let error = apply_restored_backup(
            &crate::AppState::new().expect("test state should build"),
            restored,
            BackupRestoreScope::Config,
            None,
        )
        .await
        .expect_err("scope mismatch must fail before database access");

        assert_eq!(
            error.to_string(),
            "backup database apply failed: authenticated users backup cannot be applied to config scope"
        );
    }

    #[tokio::test]
    async fn authenticated_backup_apply_uses_the_shared_system_import_lock() {
        let app = crate::AppState::new().expect("test state should build");
        let lock = crate::admin_api::try_acquire_admin_system_import_lease(&app)
            .await
            .expect("test should acquire the shared import lease");

        let error = apply_restored_backup(
            &app,
            authenticated_users_backup(),
            BackupRestoreScope::Users,
            None,
        )
        .await
        .expect_err("restore must not interleave with another system import");

        assert_eq!(
            error.to_string(),
            "backup database apply failed: another system import or restore is already running"
        );
        crate::admin_api::release_admin_system_import_lease(&app, &lock).await;
    }
}
