use crate::AppState;

use super::catalog::{
    decrypt_catalog_secret_with_fallbacks, encrypt_catalog_secret_with_configured_key_fallbacks,
    encrypt_catalog_secret_with_fallbacks,
};

const RUNTIME_SECRET_ENVELOPE_PREFIX: &str = "aether-runtime-secret-v1:";

pub(crate) fn seal_runtime_secret_payload(
    state: &AppState,
    purpose: &str,
    plaintext: &str,
) -> Option<String> {
    if purpose.is_empty() || plaintext.contains('\0') {
        return None;
    }
    let protected = format!("{purpose}\0{plaintext}");
    encrypt_catalog_secret_with_fallbacks(state, &protected)
        .map(|ciphertext| format!("{RUNTIME_SECRET_ENVELOPE_PREFIX}{ciphertext}"))
}

pub(crate) fn seal_runtime_secret_payload_with_encryption_key(
    encryption_key: Option<&str>,
    purpose: &str,
    plaintext: &str,
) -> Option<String> {
    if purpose.is_empty() || plaintext.contains('\0') {
        return None;
    }
    let protected = format!("{purpose}\0{plaintext}");
    encrypt_catalog_secret_with_configured_key_fallbacks(encryption_key, &protected)
        .map(|ciphertext| format!("{RUNTIME_SECRET_ENVELOPE_PREFIX}{ciphertext}"))
}

pub(crate) fn open_runtime_secret_payload(
    state: &AppState,
    purpose: &str,
    stored: &str,
) -> Option<String> {
    open_runtime_secret_payload_with_encryption_key(state.encryption_key(), purpose, stored)
}

pub(crate) fn open_runtime_secret_payload_with_encryption_key(
    encryption_key: Option<&str>,
    purpose: &str,
    stored: &str,
) -> Option<String> {
    if purpose.is_empty() {
        return None;
    }
    let ciphertext = stored.strip_prefix(RUNTIME_SECRET_ENVELOPE_PREFIX)?;
    let protected = decrypt_catalog_secret_with_fallbacks(encryption_key, ciphertext)?;
    let plaintext = protected.strip_prefix(purpose)?.strip_prefix('\0')?;
    (!plaintext.contains('\0')).then(|| plaintext.to_owned())
}

pub(crate) fn runtime_secret_payload_is_sealed(value: &str) -> bool {
    value.starts_with(RUNTIME_SECRET_ENVELOPE_PREFIX)
}

#[cfg(test)]
mod tests {
    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;

    use super::{open_runtime_secret_payload, seal_runtime_secret_payload};
    use crate::{data::GatewayDataState, AppState};

    fn state_with_encryption_key() -> AppState {
        AppState::new()
            .expect("test state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    #[test]
    fn runtime_secret_envelope_hides_payload_and_binds_purpose() {
        let state = state_with_encryption_key();
        let payload = r#"{"pkce_verifier":"pkce-runtime-secret-marker"}"#;

        let sealed = seal_runtime_secret_payload(&state, "identity-oauth-state", payload)
            .expect("runtime secret should encrypt");

        assert!(sealed.starts_with("aether-runtime-secret-v1:"));
        assert!(!sealed.contains("pkce-runtime-secret-marker"));
        assert_eq!(
            open_runtime_secret_payload(&state, "identity-oauth-state", &sealed).as_deref(),
            Some(payload)
        );
        assert!(open_runtime_secret_payload(&state, "provider-oauth-state", &sealed).is_none());
    }

    #[test]
    fn runtime_secret_reader_rejects_legacy_plaintext() {
        let state = state_with_encryption_key();
        let legacy = r#"{"pkce_verifier":"legacy-verifier"}"#;

        assert!(open_runtime_secret_payload(&state, "identity-oauth-state", legacy).is_none());
    }

    #[test]
    fn runtime_secret_envelope_requires_exact_purpose_boundary() {
        let state = state_with_encryption_key();

        assert!(seal_runtime_secret_payload(&state, "", "secret").is_none());
        assert!(seal_runtime_secret_payload(&state, "purpose", "prefix\0secret").is_none());

        // Structured, field-bound purposes intentionally contain NUL
        // separators. A shorter prefix must not be accepted as that same
        // purpose, while the complete structured purpose remains readable.
        let structured_purpose = "purpose\0prefix";
        let structured = seal_runtime_secret_payload(&state, structured_purpose, "secret")
            .expect("structured purpose should encrypt");
        assert_eq!(
            open_runtime_secret_payload(&state, structured_purpose, &structured).as_deref(),
            Some("secret")
        );
        assert!(open_runtime_secret_payload(&state, "purpose", &structured).is_none());

        // A legacy payload with an extra NUL in the plaintext is rejected;
        // the final NUL is not allowed to silently redefine the boundary.
        let protected = "purpose\0prefix\0secret";
        let ciphertext = super::encrypt_catalog_secret_with_fallbacks(&state, protected)
            .expect("historical ambiguous payload should encrypt");
        let stored = format!("{}{}", super::RUNTIME_SECRET_ENVELOPE_PREFIX, ciphertext);
        assert!(open_runtime_secret_payload(&state, "purpose", &stored).is_none());
    }
}
