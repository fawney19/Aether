use aether_crypto::looks_like_python_fernet_ciphertext;
use sha2::{Digest, Sha256};

use crate::{AppState, GatewayError};

use super::{
    decrypt_catalog_secret_with_fallbacks, open_runtime_secret_payload, seal_runtime_secret_payload,
};

const AUTH_API_KEY_SECRET_MIGRATION_RETRIES: usize = 8;
const AUTH_API_KEY_SECRET_ENVELOPE_FAMILY: &str = "aether-auth-api-key-secret-";
const AUTH_API_KEY_SECRET_ENVELOPE_V2: &str = "aether-auth-api-key-secret-v2:";
const AUTH_API_KEY_SECRET_PURPOSE_V2: &str = "auth-api-key-secret-bound-v2";
const AETHER_ENVELOPE_FAMILY: &str = "aether-";

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct AuthApiKeySecretProjection {
    pub(crate) plaintext: String,
    pub(crate) protected: String,
    pub(crate) migration_required: bool,
}

fn auth_api_key_secret_purpose(
    user_id: &str,
    api_key_id: &str,
    key_hash: &str,
    is_standalone: bool,
) -> Result<String, &'static str> {
    if user_id.is_empty() {
        return Err("API-key secret owner is empty");
    }
    if api_key_id.is_empty() {
        return Err("API-key secret record ID is empty");
    }
    if key_hash.is_empty() {
        return Err("API-key secret hash is empty");
    }
    let scope = if is_standalone { "standalone" } else { "user" };
    Ok(format!(
        "{AUTH_API_KEY_SECRET_PURPOSE_V2}\0scope={scope}\0owner-bytes={}\0{user_id}\0api-key-id-bytes={}\0{api_key_id}\0hash-bytes={}\0{key_hash}\0field=key",
        user_id.len(),
        api_key_id.len(),
        key_hash.len(),
    ))
}

pub(crate) fn seal_auth_api_key_secret(
    state: &AppState,
    user_id: &str,
    api_key_id: &str,
    key_hash: &str,
    is_standalone: bool,
    plaintext: &str,
) -> Result<String, &'static str> {
    if plaintext.contains('\0') {
        return Err("API-key plaintext contains reserved secret framing");
    }
    if sha256_hex(plaintext) != key_hash {
        return Err("API-key plaintext does not match its hash");
    }
    let purpose = auth_api_key_secret_purpose(user_id, api_key_id, key_hash, is_standalone)?;
    let sealed = seal_runtime_secret_payload(state, &purpose, plaintext)
        .ok_or("API-key encryption key is not configured")?;
    Ok(format!("{AUTH_API_KEY_SECRET_ENVELOPE_V2}{sealed}"))
}

pub(crate) fn open_auth_api_key_secret(
    state: &AppState,
    record: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
) -> Result<AuthApiKeySecretProjection, &'static str> {
    let observed_raw = record
        .key_encrypted
        .as_deref()
        .ok_or("API-key ciphertext is not stored")?;
    let stored = observed_raw.trim();
    if stored.is_empty() {
        return Err("API-key ciphertext is empty");
    }
    let purpose = auth_api_key_secret_purpose(
        &record.user_id,
        &record.api_key_id,
        &record.key_hash,
        record.is_standalone,
    )?;

    let (plaintext, protected, migration_required) =
        if let Some(sealed) = stored.strip_prefix(AUTH_API_KEY_SECRET_ENVELOPE_V2) {
            let plaintext = open_runtime_secret_payload(state, &purpose, sealed)
                .ok_or("API-key secret authentication failed")?;
            (
                plaintext,
                stored.to_string(),
                observed_raw.as_bytes() != stored.as_bytes(),
            )
        } else {
            if stored.starts_with(AUTH_API_KEY_SECRET_ENVELOPE_FAMILY) {
                return Err("unsupported API-key secret envelope");
            }
            // Every purpose-bound secret family in Aether uses an `aether-` envelope. Never feed
            // a foreign or future envelope into the legacy Fernet path.
            if stored.starts_with(AETHER_ENVELOPE_FAMILY) {
                return Err("secret envelope has the wrong purpose");
            }
            if !looks_like_python_fernet_ciphertext(stored) {
                return Err("API-key secret is not an authenticated ciphertext");
            }
            let plaintext = decrypt_catalog_secret_with_fallbacks(state.encryption_key(), stored)
                .ok_or("legacy API-key secret authentication failed")?;
            let protected = seal_auth_api_key_secret(
                state,
                &record.user_id,
                &record.api_key_id,
                &record.key_hash,
                record.is_standalone,
                &plaintext,
            )?;
            (plaintext, protected, true)
        };

    if plaintext.contains('\0') {
        return Err("API-key plaintext contains reserved secret framing");
    }
    if sha256_hex(&plaintext) != record.key_hash {
        return Err("API-key plaintext integrity check failed");
    }
    Ok(AuthApiKeySecretProjection {
        plaintext,
        protected,
        migration_required,
    })
}

pub(crate) async fn decrypt_or_migrate_auth_api_key_secret(
    state: &AppState,
    initial: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
) -> Result<String, GatewayError> {
    let identity = (
        initial.user_id.clone(),
        initial.api_key_id.clone(),
        initial.key_hash.clone(),
        initial.is_standalone,
    );
    let mut current = initial.clone();

    for _ in 0..AUTH_API_KEY_SECRET_MIGRATION_RETRIES {
        if current.user_id != identity.0
            || current.api_key_id != identity.1
            || current.key_hash != identity.2
            || current.is_standalone != identity.3
        {
            return Err(api_key_secret_error(
                "stored API-key secret identity changed during migration",
            ));
        }
        let projection = open_auth_api_key_secret(state, &current)
            .map_err(|message| api_key_secret_error(message))?;
        if !projection.migration_required {
            return Ok(projection.plaintext);
        }
        let observed = current.key_encrypted.clone().ok_or_else(|| {
            api_key_secret_error("stored API-key ciphertext disappeared during migration")
        })?;
        let mutation = aether_data::repository::auth::CompareAndSwapAuthApiKeyCiphertext {
            user_id: identity.0.clone(),
            api_key_id: identity.1.clone(),
            key_hash: identity.2.clone(),
            is_standalone: identity.3,
            expected_key_encrypted: observed,
            key_encrypted: projection.protected,
        };
        if state.compare_and_swap_api_key_ciphertext(&mutation).await? {
            return Ok(projection.plaintext);
        }

        let mut matches = state
            .list_auth_api_key_export_records_by_ids(std::slice::from_ref(&identity.1))
            .await?
            .into_iter()
            .filter(|record| record.api_key_id == identity.1);
        let Some(next) = matches.next() else {
            return Err(api_key_secret_error(
                "stored API-key secret is unavailable during migration",
            ));
        };
        if matches.next().is_some() {
            return Err(api_key_secret_error(
                "stored API-key identity is not unique during migration",
            ));
        }
        current = next;
    }

    Err(api_key_secret_error(
        "stored API-key secret migration did not stabilize",
    ))
}

fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn api_key_secret_error(message: &str) -> GatewayError {
    GatewayError::Internal(message.to_string())
}

#[cfg(test)]
mod tests {
    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;

    use super::{open_auth_api_key_secret, seal_auth_api_key_secret, sha256_hex};
    use crate::handlers::shared::encrypt_catalog_secret_with_fallbacks;
    use crate::{data::GatewayDataState, AppState};

    fn state_with_encryption_key() -> AppState {
        AppState::new()
            .expect("test state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    fn record(
        plaintext: &str,
        user_id: &str,
        api_key_id: &str,
        is_standalone: bool,
        key_encrypted: Option<String>,
    ) -> aether_data::repository::auth::StoredAuthApiKeyExportRecord {
        aether_data::repository::auth::StoredAuthApiKeyExportRecord {
            user_id: user_id.to_string(),
            api_key_id: api_key_id.to_string(),
            key_hash: sha256_hex(plaintext),
            key_encrypted,
            name: None,
            allowed_providers: None,
            allowed_api_formats: None,
            allowed_models: None,
            ip_rules: None,
            rate_limit: None,
            concurrent_limit: None,
            force_capabilities: None,
            feature_settings: None,
            is_active: true,
            expires_at_unix_secs: None,
            auto_delete_on_expiry: false,
            total_requests: 0,
            total_tokens: 0,
            total_cost_usd: 0.0,
            last_used_at_unix_secs: None,
            created_at_unix_secs: None,
            updated_at_unix_secs: None,
            is_standalone,
        }
    }

    #[test]
    fn v2_ciphertext_is_bound_to_owner_record_scope_and_hash() {
        let state = state_with_encryption_key();
        let plaintext = "sk-record-bound-secret";
        let key_hash = sha256_hex(plaintext);
        let ciphertext =
            seal_auth_api_key_secret(&state, "owner-a", "key-a", &key_hash, false, plaintext)
                .expect("API-key secret should seal");
        let source = record(
            plaintext,
            "owner-a",
            "key-a",
            false,
            Some(ciphertext.clone()),
        );
        assert_eq!(
            open_auth_api_key_secret(&state, &source)
                .expect("source record should open")
                .plaintext,
            plaintext
        );

        for mut copied in [
            record(
                plaintext,
                "owner-b",
                "key-a",
                false,
                Some(ciphertext.clone()),
            ),
            record(
                plaintext,
                "owner-a",
                "key-b",
                false,
                Some(ciphertext.clone()),
            ),
            record(
                plaintext,
                "owner-a",
                "key-a",
                true,
                Some(ciphertext.clone()),
            ),
        ] {
            assert!(open_auth_api_key_secret(&state, &copied).is_err());
            copied.key_hash = sha256_hex("different-secret");
            assert!(open_auth_api_key_secret(&state, &copied).is_err());
        }
    }

    #[test]
    fn legacy_ciphertext_requires_matching_record_hash_before_migration() {
        let state = state_with_encryption_key();
        let plaintext = "sk-legacy-record-secret";
        let ciphertext = encrypt_catalog_secret_with_fallbacks(&state, plaintext)
            .expect("legacy secret should encrypt");
        let source = record(
            plaintext,
            "owner-a",
            "key-a",
            false,
            Some(ciphertext.clone()),
        );
        let projection =
            open_auth_api_key_secret(&state, &source).expect("legacy secret should open");
        assert_eq!(projection.plaintext, plaintext);
        assert!(projection.migration_required);
        assert!(projection
            .protected
            .starts_with("aether-auth-api-key-secret-v2:"));

        let copied = record(
            "another-record-secret",
            "owner-b",
            "key-b",
            false,
            Some(ciphertext),
        );
        assert!(open_auth_api_key_secret(&state, &copied).is_err());
    }

    #[test]
    fn foreign_and_unknown_envelopes_never_fall_back_to_legacy_decryption() {
        let state = state_with_encryption_key();
        for ciphertext in [
            "aether-auth-api-key-secret-v3:unknown",
            "aether-system-config-secret-v2:unknown",
            "plaintext-secret",
        ] {
            let record = record(
                "plaintext-secret",
                "owner-a",
                "key-a",
                false,
                Some(ciphertext.to_string()),
            );
            assert!(open_auth_api_key_secret(&state, &record).is_err());
        }
    }
}
