use std::future::Future;

use aether_crypto::looks_like_python_fernet_ciphertext;
use aether_data::repository::oauth_providers::{
    validate_oauth_redirect_uri, StoredOAuthProviderConfig, UpsertOAuthProviderConfigRecord,
};
use url::{Host, Url};

use crate::{AppState, GatewayError};

use super::{
    decrypt_catalog_secret_with_fallbacks, open_runtime_secret_payload, seal_runtime_secret_payload,
};

const IDENTITY_OAUTH_CLIENT_SECRET_MIGRATION_RETRIES: usize = 8;
const IDENTITY_OAUTH_CLIENT_SECRET_ENVELOPE_FAMILY: &str = "aether-identity-oauth-client-secret-";
const IDENTITY_OAUTH_CLIENT_SECRET_ENVELOPE_V2: &str = "aether-identity-oauth-client-secret-v2:";
const IDENTITY_OAUTH_CLIENT_SECRET_ENVELOPE_V3: &str = "aether-identity-oauth-client-secret-v3:";
const IDENTITY_OAUTH_CLIENT_SECRET_PURPOSE_V2: &str = "identity-oauth-client-secret-bound-v2";
const IDENTITY_OAUTH_CLIENT_SECRET_PURPOSE_V3: &str = "identity-oauth-client-secret-bound-v3";
const IDENTITY_OAUTH_CLIENT_SECRET_FIELD: &str = "client_secret_encrypted";
const LINUXDO_AUTHORIZATION_URL: &str = "https://connect.linux.do/oauth2/authorize";
const LINUXDO_TOKEN_URL: &str = "https://connect.linux.do/oauth2/token";
const LINUXDO_USERINFO_URL: &str = "https://connect.linux.do/api/user";

#[derive(Clone, PartialEq, Eq)]
struct IdentityOAuthClientSecretProjection {
    plaintext: String,
    protected: String,
    migration_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IdentityOAuthClientSecretBinding {
    provider_type: String,
    client_id: String,
    authorization_url: String,
    token_url: String,
    userinfo_url: String,
    redirect_uri: String,
}

fn normalized_identity_oauth_provider_type(provider_type: &str) -> Result<String, &'static str> {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    if provider_type.is_empty() {
        return Err("identity OAuth provider type is empty");
    }
    if provider_type.contains('\0') {
        return Err("identity OAuth provider type contains reserved framing");
    }
    Ok(provider_type)
}

fn identity_oauth_client_secret_purpose_v2(provider_type: &str) -> Result<String, &'static str> {
    let provider_type = normalized_identity_oauth_provider_type(provider_type)?;
    Ok(format!(
        "{IDENTITY_OAUTH_CLIENT_SECRET_PURPOSE_V2}\0provider-type-bytes={}\0{provider_type}\0field-bytes={}\0{IDENTITY_OAUTH_CLIENT_SECRET_FIELD}",
        provider_type.len(),
        IDENTITY_OAUTH_CLIENT_SECRET_FIELD.len(),
    ))
}

fn canonical_identity_oauth_endpoint(raw: &str) -> Result<String, &'static str> {
    if raw.contains('\0') {
        return Err("identity OAuth endpoint contains reserved framing");
    }
    let mut parsed = Url::parse(raw.trim()).map_err(|_| "identity OAuth endpoint is invalid")?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
        || matches!(parsed.host(), Some(Host::Ipv4(_)) | Some(Host::Ipv6(_)))
    {
        return Err("identity OAuth endpoint is not a canonical HTTPS DNS URL");
    }
    let host = parsed
        .host_str()
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
        .filter(|host| !host.is_empty())
        .ok_or("identity OAuth endpoint is missing a host")?;
    parsed
        .set_host(Some(&host))
        .map_err(|_| "identity OAuth endpoint host is invalid")?;
    if parsed.port() == Some(443) {
        parsed
            .set_port(None)
            .map_err(|_| "identity OAuth endpoint port is invalid")?;
    }
    Ok(parsed.to_string())
}

fn canonical_identity_oauth_redirect_uri(raw: &str) -> Result<String, &'static str> {
    if raw.contains('\0') {
        return Err("identity OAuth redirect URI contains reserved framing");
    }
    let raw = raw.trim();
    validate_oauth_redirect_uri(raw).map_err(|_| "identity OAuth redirect URI is invalid")?;
    Url::parse(raw)
        .map(|url| url.to_string())
        .map_err(|_| "identity OAuth redirect URI is invalid")
}

fn effective_identity_oauth_endpoint<'a>(
    provider_type: &str,
    override_value: Option<&'a str>,
    linuxdo_default: &'static str,
) -> Result<&'a str, &'static str> {
    if let Some(value) = override_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(value);
    }
    if provider_type == "linuxdo" {
        // The static default can be shortened to the caller lifetime.
        return Ok(linuxdo_default);
    }
    Err("identity OAuth endpoint is missing")
}

fn identity_oauth_client_secret_binding(
    provider_type: &str,
    client_id: &str,
    authorization_url_override: Option<&str>,
    token_url_override: Option<&str>,
    userinfo_url_override: Option<&str>,
    redirect_uri: &str,
) -> Result<IdentityOAuthClientSecretBinding, &'static str> {
    let provider_type = normalized_identity_oauth_provider_type(provider_type)?;
    let client_id = client_id.trim();
    if client_id.is_empty() {
        return Err("identity OAuth client ID is empty");
    }
    if client_id.contains('\0') {
        return Err("identity OAuth client ID contains reserved framing");
    }
    let authorization_url = effective_identity_oauth_endpoint(
        &provider_type,
        authorization_url_override,
        LINUXDO_AUTHORIZATION_URL,
    )?;
    let token_url =
        effective_identity_oauth_endpoint(&provider_type, token_url_override, LINUXDO_TOKEN_URL)?;
    let userinfo_url = effective_identity_oauth_endpoint(
        &provider_type,
        userinfo_url_override,
        LINUXDO_USERINFO_URL,
    )?;
    Ok(IdentityOAuthClientSecretBinding {
        provider_type,
        client_id: client_id.to_string(),
        authorization_url: canonical_identity_oauth_endpoint(authorization_url)?,
        token_url: canonical_identity_oauth_endpoint(token_url)?,
        userinfo_url: canonical_identity_oauth_endpoint(userinfo_url)?,
        redirect_uri: canonical_identity_oauth_redirect_uri(redirect_uri)?,
    })
}

fn stored_identity_oauth_client_secret_binding(
    provider: &StoredOAuthProviderConfig,
) -> Result<IdentityOAuthClientSecretBinding, &'static str> {
    identity_oauth_client_secret_binding(
        &provider.provider_type,
        &provider.client_id,
        provider.authorization_url_override.as_deref(),
        provider.token_url_override.as_deref(),
        provider.userinfo_url_override.as_deref(),
        &provider.redirect_uri,
    )
}

fn upsert_identity_oauth_client_secret_binding(
    provider: &UpsertOAuthProviderConfigRecord,
) -> Result<IdentityOAuthClientSecretBinding, &'static str> {
    identity_oauth_client_secret_binding(
        &provider.provider_type,
        &provider.client_id,
        provider.authorization_url_override.as_deref(),
        provider.token_url_override.as_deref(),
        provider.userinfo_url_override.as_deref(),
        &provider.redirect_uri,
    )
}

fn identity_oauth_client_secret_purpose_v3(binding: &IdentityOAuthClientSecretBinding) -> String {
    format!(
        "{IDENTITY_OAUTH_CLIENT_SECRET_PURPOSE_V3}\0provider-type-bytes={}\0{}\0client-id-bytes={}\0{}\0authorization-url-bytes={}\0{}\0token-url-bytes={}\0{}\0userinfo-url-bytes={}\0{}\0redirect-uri-bytes={}\0{}\0field-bytes={}\0{IDENTITY_OAUTH_CLIENT_SECRET_FIELD}",
        binding.provider_type.len(),
        binding.provider_type,
        binding.client_id.len(),
        binding.client_id,
        binding.authorization_url.len(),
        binding.authorization_url,
        binding.token_url.len(),
        binding.token_url,
        binding.userinfo_url.len(),
        binding.userinfo_url,
        binding.redirect_uri.len(),
        binding.redirect_uri,
        IDENTITY_OAUTH_CLIENT_SECRET_FIELD.len(),
    )
}

pub(crate) fn identity_oauth_provider_secret_binding_matches(
    stored: &StoredOAuthProviderConfig,
    replacement: &UpsertOAuthProviderConfigRecord,
) -> Result<bool, &'static str> {
    Ok(stored_identity_oauth_client_secret_binding(stored)?
        == upsert_identity_oauth_client_secret_binding(replacement)?)
}

pub(crate) fn seal_identity_oauth_provider_client_secret(
    state: &AppState,
    provider: &UpsertOAuthProviderConfigRecord,
    plaintext: &str,
) -> Result<String, &'static str> {
    if plaintext.contains('\0') {
        return Err("identity OAuth client secret contains reserved framing");
    }
    let binding = upsert_identity_oauth_client_secret_binding(provider)?;
    let purpose = identity_oauth_client_secret_purpose_v3(&binding);
    let sealed = seal_runtime_secret_payload(state, &purpose, plaintext)
        .ok_or("identity OAuth client secret encryption key is not configured")?;
    Ok(format!(
        "{IDENTITY_OAUTH_CLIENT_SECRET_ENVELOPE_V3}{sealed}"
    ))
}

fn seal_identity_oauth_provider_client_secret_for_binding(
    state: &AppState,
    binding: &IdentityOAuthClientSecretBinding,
    plaintext: &str,
) -> Result<String, &'static str> {
    if plaintext.contains('\0') {
        return Err("identity OAuth client secret contains reserved framing");
    }
    let purpose = identity_oauth_client_secret_purpose_v3(binding);
    let sealed = seal_runtime_secret_payload(state, &purpose, plaintext)
        .ok_or("identity OAuth client secret encryption key is not configured")?;
    Ok(format!(
        "{IDENTITY_OAUTH_CLIENT_SECRET_ENVELOPE_V3}{sealed}"
    ))
}

fn open_identity_oauth_provider_client_secret(
    state: &AppState,
    provider: &StoredOAuthProviderConfig,
    stored: &str,
) -> Result<IdentityOAuthClientSecretProjection, &'static str> {
    let binding = stored_identity_oauth_client_secret_binding(provider)?;
    let purpose = identity_oauth_client_secret_purpose_v3(&binding);
    if let Some(sealed) = stored.strip_prefix(IDENTITY_OAUTH_CLIENT_SECRET_ENVELOPE_V3) {
        let plaintext = open_runtime_secret_payload(state, &purpose, sealed)
            .ok_or("identity OAuth client secret authentication failed")?;
        if plaintext.contains('\0') {
            return Err("identity OAuth client secret contains reserved framing");
        }
        return Ok(IdentityOAuthClientSecretProjection {
            plaintext,
            protected: stored.to_string(),
            migration_required: false,
        });
    }
    if let Some(sealed) = stored.strip_prefix(IDENTITY_OAUTH_CLIENT_SECRET_ENVELOPE_V2) {
        let legacy_purpose = identity_oauth_client_secret_purpose_v2(&provider.provider_type)?;
        let plaintext = open_runtime_secret_payload(state, &legacy_purpose, sealed)
            .ok_or("identity OAuth client secret authentication failed")?;
        if plaintext.contains('\0') {
            return Err("identity OAuth client secret contains reserved framing");
        }
        let protected =
            seal_identity_oauth_provider_client_secret_for_binding(state, &binding, &plaintext)?;
        return Ok(IdentityOAuthClientSecretProjection {
            plaintext,
            protected,
            migration_required: true,
        });
    }
    if stored.starts_with(IDENTITY_OAUTH_CLIENT_SECRET_ENVELOPE_FAMILY) {
        return Err("unsupported identity OAuth client secret envelope");
    }
    if stored.starts_with("aether-") {
        return Err("Aether secret envelope has the wrong record binding");
    }
    if !looks_like_python_fernet_ciphertext(stored) {
        return Err("identity OAuth client secret is not an authenticated ciphertext");
    }

    let plaintext = decrypt_catalog_secret_with_fallbacks(state.encryption_key(), stored)
        .ok_or("legacy identity OAuth client secret authentication failed")?;
    if plaintext.contains('\0') {
        return Err("legacy identity OAuth client secret contains reserved framing");
    }
    let protected =
        seal_identity_oauth_provider_client_secret_for_binding(state, &binding, &plaintext)?;
    Ok(IdentityOAuthClientSecretProjection {
        plaintext,
        protected,
        migration_required: true,
    })
}

pub(crate) async fn decrypt_or_migrate_identity_oauth_provider_client_secret(
    state: &AppState,
    provider: &StoredOAuthProviderConfig,
) -> Result<Option<String>, GatewayError> {
    decrypt_or_migrate_identity_oauth_provider_client_secret_with_before_compare(
        state,
        provider,
        || async {},
    )
    .await
}

async fn decrypt_or_migrate_identity_oauth_provider_client_secret_with_before_compare<
    BeforeCompare,
    CompareFuture,
>(
    state: &AppState,
    provider: &StoredOAuthProviderConfig,
    before_compare: BeforeCompare,
) -> Result<Option<String>, GatewayError>
where
    BeforeCompare: Fn() -> CompareFuture,
    CompareFuture: Future<Output = ()>,
{
    let provider_storage_key = provider.provider_type.trim();
    let original_binding =
        stored_identity_oauth_client_secret_binding(provider).map_err(secret_error)?;

    for _ in 0..IDENTITY_OAUTH_CLIENT_SECRET_MIGRATION_RETRIES {
        let current = state
            .get_oauth_provider_config(provider_storage_key)
            .await?
            .ok_or_else(|| secret_error("identity OAuth provider is unavailable"))?;
        if stored_identity_oauth_client_secret_binding(&current).map_err(secret_error)?
            != original_binding
        {
            return Err(secret_error(
                "identity OAuth provider record binding changed unexpectedly",
            ));
        }
        let Some(observed) = current.client_secret_encrypted.as_deref() else {
            return Ok(None);
        };
        if observed.is_empty() {
            return Err(secret_error("stored identity OAuth client secret is empty"));
        }
        let projection = open_identity_oauth_provider_client_secret(state, &current, observed)
            .map_err(secret_error)?;
        if !projection.migration_required {
            return Ok(Some(projection.plaintext));
        }

        before_compare().await;
        if state
            .compare_and_swap_oauth_provider_client_secret(
                provider_storage_key,
                observed,
                &projection.protected,
            )
            .await?
        {
            return Ok(Some(projection.plaintext));
        }
    }

    Err(secret_error(
        "identity OAuth client secret migration did not stabilize",
    ))
}

fn secret_error(message: &'static str) -> GatewayError {
    GatewayError::Internal(message.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
    use aether_data::repository::oauth_providers::{
        EncryptedSecretUpdate, InMemoryOAuthProviderRepository, OAuthProviderReadRepository,
        OAuthProviderWriteRepository, StoredOAuthProviderConfig, UpsertOAuthProviderConfigRecord,
    };

    use super::{
        decrypt_or_migrate_identity_oauth_provider_client_secret,
        decrypt_or_migrate_identity_oauth_provider_client_secret_with_before_compare,
        open_identity_oauth_provider_client_secret, seal_identity_oauth_provider_client_secret,
        IDENTITY_OAUTH_CLIENT_SECRET_ENVELOPE_V3,
    };
    use crate::handlers::shared::{
        encrypt_catalog_secret_with_fallbacks, seal_runtime_secret_payload,
    };
    use crate::{data::GatewayDataState, AppState};

    fn state_with_encryption_key() -> AppState {
        AppState::new()
            .expect("test state should build")
            .with_data_state_for_tests(
                GatewayDataState::disabled()
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
    }

    fn sample_provider(provider_type: &str, encrypted: &str) -> StoredOAuthProviderConfig {
        let normalized_provider_type = provider_type.trim().to_ascii_lowercase();
        StoredOAuthProviderConfig::new(
            provider_type.to_string(),
            format!("{normalized_provider_type} display"),
            format!("{normalized_provider_type}-client"),
            format!("https://{normalized_provider_type}.example.com/redirect"),
            "https://frontend.example.com/auth/callback".to_string(),
        )
        .expect("provider should build")
        .with_config_fields(
            Some(encrypted.to_string()),
            Some("https://connect.linux.do/oauth2/authorize".to_string()),
            Some("https://connect.linux.do/oauth2/token".to_string()),
            None,
            Some(vec!["openid".to_string()]),
            None,
            None,
            None,
            true,
        )
        .with_timestamps(Some(10), Some(20))
    }

    fn sample_upsert(
        provider_type: &str,
        encrypted: EncryptedSecretUpdate,
        display_name: &str,
    ) -> UpsertOAuthProviderConfigRecord {
        UpsertOAuthProviderConfigRecord {
            provider_type: provider_type.to_string(),
            display_name: display_name.to_string(),
            client_id: format!("{provider_type}-client"),
            client_secret_encrypted: encrypted,
            authorization_url_override: Some(
                "https://connect.linux.do/oauth2/authorize".to_string(),
            ),
            token_url_override: Some("https://connect.linux.do/oauth2/token".to_string()),
            userinfo_url_override: None,
            scopes: Some(vec!["openid".to_string()]),
            redirect_uri: format!("https://{provider_type}.example.com/redirect"),
            frontend_callback_url: "https://frontend.example.com/auth/callback".to_string(),
            attribute_mapping: None,
            extra_config: None,
            icon_url: None,
            is_enabled: true,
        }
    }

    fn sample_binding_upsert() -> UpsertOAuthProviderConfigRecord {
        sample_upsert("linuxdo", EncryptedSecretUpdate::Preserve, "Linux.do")
    }

    #[test]
    fn v2_round_trip_binds_normalized_provider_and_rejects_tampering() {
        let state = state_with_encryption_key();
        let record = sample_binding_upsert();
        let sealed = seal_identity_oauth_provider_client_secret(&state, &record, "client-secret")
            .expect("client secret should seal");
        assert!(sealed.starts_with(IDENTITY_OAUTH_CLIENT_SECRET_ENVELOPE_V3));
        let provider = sample_provider(" LinuxDo ", &sealed);
        assert_eq!(
            open_identity_oauth_provider_client_secret(&state, &provider, &sealed)
                .expect("matching provider should open")
                .plaintext,
            "client-secret"
        );
        let wrong_provider = sample_provider("github", &sealed);
        assert!(
            open_identity_oauth_provider_client_secret(&state, &wrong_provider, &sealed).is_err()
        );

        let mut tampered = sealed.into_bytes();
        let last = tampered
            .last_mut()
            .expect("sealed value should not be empty");
        *last = if *last == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(tampered).expect("ciphertext should remain UTF-8");
        assert!(open_identity_oauth_provider_client_secret(&state, &provider, &tampered).is_err());
    }

    #[test]
    fn reader_rejects_unknown_and_cross_family_aether_envelopes() {
        let state = state_with_encryption_key();
        for stored in [
            "aether-identity-oauth-client-secret-v3:unknown",
            "aether-system-config-secret-v2:foreign",
            "aether-proxy-node-secret-v2:foreign",
            "plaintext-secret",
        ] {
            assert!(
                open_identity_oauth_provider_client_secret(
                    &state,
                    &sample_provider("linuxdo", stored),
                    stored
                )
                .is_err(),
                "unexpectedly accepted {stored}"
            );
        }
        let other_runtime = seal_runtime_secret_payload(&state, "another-purpose", "secret")
            .expect("runtime secret should seal");
        assert!(open_identity_oauth_provider_client_secret(
            &state,
            &sample_provider("linuxdo", &other_runtime),
            &other_runtime,
        )
        .is_err());
        let stripped = other_runtime
            .strip_prefix("aether-runtime-secret-v1:")
            .expect("runtime envelope should contain its Fernet payload");
        assert!(
            open_identity_oauth_provider_client_secret(
                &state,
                &sample_provider("linuxdo", stripped),
                stripped
            )
            .is_err(),
            "stripping a foreign runtime envelope must not turn it into a legacy secret",
        );
    }

    #[tokio::test]
    async fn legacy_fernet_is_migrated_to_record_bound_v2() {
        let bootstrap = state_with_encryption_key();
        let legacy = encrypt_catalog_secret_with_fallbacks(&bootstrap, "legacy-secret")
            .expect("legacy secret should encrypt");
        let provider = sample_provider("linuxdo", &legacy);
        let repository = Arc::new(InMemoryOAuthProviderRepository::seed([provider.clone()]));
        let state = AppState::new()
            .expect("test state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_oauth_provider_repository_for_tests(Arc::clone(&repository))
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );

        assert_eq!(
            decrypt_or_migrate_identity_oauth_provider_client_secret(&state, &provider)
                .await
                .expect("legacy secret should migrate")
                .as_deref(),
            Some("legacy-secret")
        );
        let stored = repository
            .get_oauth_provider_config("linuxdo")
            .await
            .expect("provider should read")
            .expect("provider should exist")
            .client_secret_encrypted
            .expect("secret should remain configured");
        assert!(stored.starts_with(IDENTITY_OAUTH_CLIENT_SECRET_ENVELOPE_V3));
    }

    #[tokio::test]
    async fn migration_cas_miss_rereads_and_preserves_concurrent_non_secret_update() {
        let bootstrap = state_with_encryption_key();
        let legacy_before = encrypt_catalog_secret_with_fallbacks(&bootstrap, "before")
            .expect("legacy secret should encrypt");
        let legacy_after = encrypt_catalog_secret_with_fallbacks(&bootstrap, "after")
            .expect("rotated legacy secret should encrypt");
        let provider = sample_provider("linuxdo", &legacy_before);
        let repository = Arc::new(InMemoryOAuthProviderRepository::seed([provider.clone()]));
        let state = AppState::new()
            .expect("test state should build")
            .with_data_state_for_tests(
                GatewayDataState::with_oauth_provider_repository_for_tests(Arc::clone(&repository))
                    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            );
        let first_compare = Arc::new(AtomicBool::new(true));
        let repository_for_race = Arc::clone(&repository);
        let legacy_after_for_race = legacy_after.clone();

        let plaintext =
            decrypt_or_migrate_identity_oauth_provider_client_secret_with_before_compare(
                &state,
                &provider,
                move || {
                    let first_compare = Arc::clone(&first_compare);
                    let repository = Arc::clone(&repository_for_race);
                    let legacy_after = legacy_after_for_race.clone();
                    async move {
                        if first_compare.swap(false, Ordering::SeqCst) {
                            repository
                                .upsert_oauth_provider_config(&sample_upsert(
                                    "linuxdo",
                                    EncryptedSecretUpdate::Set(legacy_after),
                                    "concurrent display",
                                ))
                                .await
                                .expect("concurrent provider update should persist");
                        }
                    }
                },
            )
            .await
            .expect("migration should retry after CAS miss")
            .expect("secret should remain configured");
        assert_eq!(plaintext, "after");

        let current = repository
            .get_oauth_provider_config("linuxdo")
            .await
            .expect("provider should read")
            .expect("provider should exist");
        assert_eq!(current.display_name, "concurrent display");
        let stored = current
            .client_secret_encrypted
            .expect("secret should remain configured");
        assert!(stored.starts_with(IDENTITY_OAUTH_CLIENT_SECRET_ENVELOPE_V3));
        assert_eq!(
            open_identity_oauth_provider_client_secret(
                &state,
                &repository
                    .get_oauth_provider_config("linuxdo")
                    .await
                    .unwrap()
                    .unwrap(),
                &stored
            )
            .expect("migrated secret should open")
            .plaintext,
            "after"
        );
    }
}
