use super::catalog::decrypt_catalog_secret_with_fallbacks;
use super::runtime_secret::{open_runtime_secret_payload, seal_runtime_secret_payload};
use crate::{AppState, GatewayError};
use aether_crypto::looks_like_python_fernet_ciphertext;
use std::future::Future;
use url::Url;

const SYSTEM_CONFIG_SECRET_MIGRATION_RETRIES: usize = 8;
const LDAP_BIND_PASSWORD_MIGRATION_RETRIES: usize = 8;
const BARK_DEVICE_KEY_MIGRATION_RETRIES: usize = 8;
const SYSTEM_CONFIG_SECRET_ENVELOPE_FAMILY_PREFIX: &str = "aether-system-config-secret-";
const SYSTEM_CONFIG_SECRET_V2_PREFIX: &str = "aether-system-config-secret-v2:";
const SYSTEM_CONFIG_SECRET_BOUND_PURPOSE_VERSION: &str = "system-config-secret-bound-v2";
const SMTP_PASSWORD_V3_PREFIX: &str = "aether-smtp-password-v3:";
const SMTP_PASSWORD_BOUND_PURPOSE_V3: &str = "smtp-password-bound-v3";
const LDAP_BIND_PASSWORD_ENVELOPE_FAMILY_PREFIX: &str = "aether-ldap-bind-password-";
const LDAP_BIND_PASSWORD_V2_PREFIX: &str = "aether-ldap-bind-password-v2:";
const LDAP_BIND_PASSWORD_V3_PREFIX: &str = "aether-ldap-bind-password-v3:";
const LDAP_BIND_PASSWORD_BOUND_PURPOSE: &str = "ldap-bind-password-bound-v2";
const LDAP_BIND_PASSWORD_BOUND_PURPOSE_V3: &str = "ldap-bind-password-bound-v3";
const BARK_DEVICE_KEY_V2_PREFIX: &str = "aether-bark-device-key-v2:";
const BARK_DEVICE_KEY_BOUND_PURPOSE_V2: &str = "bark-device-key-bound-v2";
const BARK_DEVICE_KEY_CONFIG_KEY: &str = "module.bark_push.device_key";
const RUNTIME_SECRET_ENVELOPE_FAMILY_PREFIX: &str = "aether-runtime-secret-";

fn system_config_secret_purpose(key: &str) -> String {
    let key = aether_admin::system::normalize_admin_system_config_key(key);
    format!(
        "{SYSTEM_CONFIG_SECRET_BOUND_PURPOSE_VERSION}\0key-bytes={}\0{key}",
        key.len()
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SmtpPasswordBinding {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) user: String,
    pub(crate) use_tls: bool,
    pub(crate) use_ssl: bool,
}

pub(crate) fn smtp_password_binding(
    host: &str,
    port: u16,
    user: Option<&str>,
    use_tls: bool,
    use_ssl: bool,
) -> Option<SmtpPasswordBinding> {
    let host = host.trim();
    let user = user.unwrap_or("").trim();
    if host.is_empty()
        || user.is_empty()
        || host.contains('\0')
        || user.contains('\0')
        || host.bytes().any(|byte| matches!(byte, b'\r' | b'\n'))
        || user.bytes().any(|byte| matches!(byte, b'\r' | b'\n'))
    {
        return None;
    }
    Some(SmtpPasswordBinding {
        host: host.to_ascii_lowercase(),
        port,
        user: user.to_string(),
        use_tls,
        use_ssl,
    })
}

fn smtp_password_purpose(binding: &SmtpPasswordBinding) -> String {
    format!(
        "{SMTP_PASSWORD_BOUND_PURPOSE_V3}\0host-bytes={}\0{}\0port={}\0user-bytes={}\0{}\0tls={}\0ssl={}\0field-bytes={}\0smtp_password",
        binding.host.len(),
        binding.host,
        binding.port,
        binding.user.len(),
        binding.user,
        if binding.use_tls { 1 } else { 0 },
        if binding.use_ssl { 1 } else { 0 },
        "smtp_password".len(),
    )
}

pub(crate) fn encrypt_smtp_password(
    state: &AppState,
    binding: &SmtpPasswordBinding,
    plaintext: &str,
) -> Option<String> {
    if plaintext.contains('\0') {
        return None;
    }
    seal_runtime_secret_payload(state, &smtp_password_purpose(binding), plaintext)
        .map(|sealed| format!("{SMTP_PASSWORD_V3_PREFIX}{sealed}"))
}

fn decrypt_smtp_password_v3(
    state: &AppState,
    binding: &SmtpPasswordBinding,
    stored: &str,
) -> Option<String> {
    let sealed = stored.strip_prefix(SMTP_PASSWORD_V3_PREFIX)?;
    open_runtime_secret_payload(state, &smtp_password_purpose(binding), sealed)
        .filter(|plaintext| !plaintext.contains('\0'))
}

pub(crate) async fn decrypt_or_migrate_smtp_password(
    state: &AppState,
    binding: &SmtpPasswordBinding,
    stored: String,
) -> Result<String, GatewayError> {
    if let Some(plaintext) = decrypt_smtp_password_v3(state, binding, stored.trim()) {
        return Ok(plaintext);
    }
    if stored.trim().starts_with(SMTP_PASSWORD_V3_PREFIX)
        || stored.trim().starts_with("aether-smtp-password-")
    {
        return Err(system_config_secret_error(
            "stored SMTP password cannot be decrypted",
        ));
    }
    let plaintext = decrypt_system_config_secret(state, "smtp_password", stored.trim())
        .or_else(|| {
            if stored_secret_uses_known_envelope_family(stored.trim()) {
                return None;
            }
            decrypt_catalog_secret_with_fallbacks(state.encryption_key(), stored.trim()).or_else(
                || {
                    (!stored.trim().is_empty()
                        && !looks_like_python_fernet_ciphertext(stored.trim()))
                    .then(|| stored.trim().to_string())
                },
            )
        })
        .ok_or_else(|| system_config_secret_error("stored SMTP password cannot be decrypted"))?;
    if plaintext.contains('\0') {
        return Err(system_config_secret_error(
            "stored SMTP password contains reserved secret framing",
        ));
    }
    let replacement = encrypt_smtp_password(state, binding, &plaintext)
        .ok_or_else(|| system_config_secret_error("SMTP password migration is unavailable"))?;
    if state
        .compare_and_set_system_config_string_value("smtp_password", stored.trim(), &replacement)
        .await?
    {
        return Ok(plaintext);
    }
    let current = state
        .read_system_config_json_value_strong("smtp_password")
        .await?
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| system_config_secret_error("stored SMTP password is unavailable"))?;
    decrypt_smtp_password_v3(state, binding, current.trim())
        .ok_or_else(|| system_config_secret_error("stored SMTP password changed during migration"))
}

pub(crate) fn encrypt_system_config_secret(
    state: &AppState,
    key: &str,
    plaintext: &str,
) -> Option<String> {
    if plaintext.contains('\0') {
        return None;
    }
    let purpose = system_config_secret_purpose(key);
    seal_runtime_secret_payload(state, &purpose, plaintext)
        .map(|sealed| format!("{SYSTEM_CONFIG_SECRET_V2_PREFIX}{sealed}"))
}

pub(crate) fn decrypt_system_config_secret(
    state: &AppState,
    key: &str,
    stored: &str,
) -> Option<String> {
    let sealed = stored.strip_prefix(SYSTEM_CONFIG_SECRET_V2_PREFIX)?;
    let purpose = system_config_secret_purpose(key);
    open_runtime_secret_payload(state, &purpose, sealed)
        .filter(|plaintext| !plaintext.contains('\0'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LdapBindPasswordBinding {
    server_url: String,
    bind_dn: String,
    base_dn: String,
    use_starttls: bool,
}

/// Parse and canonicalize an LDAP transport endpoint.
///
/// LDAP simple binds carry credentials, so plaintext `ldap://` is only valid
/// when StartTLS is explicitly requested.  The parser deliberately does not
/// reject private or loopback hosts: LDAP deployments commonly run on an
/// internal network and this function is a transport-integrity check, not an
/// outbound SSRF policy.  The `mockldap` scheme exists only in test builds.
pub(crate) fn normalize_ldap_transport_server_url(raw: &str, use_starttls: bool) -> Option<String> {
    #[cfg(test)]
    {
        // Gateway unit/integration fixtures use an in-process mock endpoint. Keep
        // this exception behind the gateway test configuration; production code
        // always uses the strict parser without custom schemes.
        aether_admin::system::normalize_ldap_transport_server_url_for_tests(raw, use_starttls)
    }
    #[cfg(not(test))]
    {
        aether_admin::system::normalize_ldap_transport_server_url(raw, use_starttls)
    }
}

/// Return whether an LDAP user search filter is safe to use with the escaped
/// `{username}` substitution performed by the login path.  LDAP filters are
/// always parenthesized; keeping the same bounded shape at every config
/// ingress prevents malformed/imported values from reaching the query layer.
pub(crate) fn ldap_search_filter_is_valid(value: &str) -> bool {
    aether_admin::system::ldap_search_filter_is_valid(value)
}

pub(crate) fn ldap_distinguished_name_is_valid(value: &str) -> bool {
    aether_admin::system::ldap_distinguished_name_is_valid(value)
}

pub(crate) fn ldap_attribute_description_is_valid(value: &str) -> bool {
    aether_admin::system::ldap_attribute_description_is_valid(value)
}

pub(crate) fn ldap_module_config_is_valid(
    config: Option<&aether_data::repository::auth_modules::StoredLdapModuleConfig>,
) -> bool {
    config.is_some_and(|config| {
        normalize_ldap_transport_server_url(&config.server_url, config.use_starttls).is_some()
            && aether_admin::system::ldap_module_config_fields_are_valid(config)
    })
}

fn canonical_ldap_server_url(raw: &str, use_starttls: bool) -> Option<String> {
    normalize_ldap_transport_server_url(raw, use_starttls)
}

fn ldap_bind_password_binding(
    config: &aether_data::repository::auth_modules::StoredLdapModuleConfig,
) -> Option<LdapBindPasswordBinding> {
    let server_url = canonical_ldap_server_url(&config.server_url, config.use_starttls)?;
    let bind_dn = config.bind_dn.trim();
    let base_dn = config.base_dn.trim();
    if !ldap_distinguished_name_is_valid(&config.bind_dn)
        || !ldap_distinguished_name_is_valid(&config.base_dn)
    {
        return None;
    }
    Some(LdapBindPasswordBinding {
        server_url,
        bind_dn: bind_dn.to_string(),
        base_dn: base_dn.to_string(),
        use_starttls: config.use_starttls,
    })
}

pub(crate) fn ldap_bind_password_binding_matches(
    stored: &aether_data::repository::auth_modules::StoredLdapModuleConfig,
    replacement: &aether_data::repository::auth_modules::StoredLdapModuleConfig,
) -> Result<bool, &'static str> {
    let stored =
        ldap_bind_password_binding(stored).ok_or("stored LDAP bind password binding is invalid")?;
    let replacement = ldap_bind_password_binding(replacement)
        .ok_or("replacement LDAP bind password binding is invalid")?;
    Ok(stored == replacement)
}

fn ldap_bind_password_purpose_v3(binding: &LdapBindPasswordBinding) -> String {
    format!(
        "{LDAP_BIND_PASSWORD_BOUND_PURPOSE_V3}\0server-url-bytes={}\0{}\0bind-dn-bytes={}\0{}\0base-dn-bytes={}\0{}\0starttls={}\0field-bytes={}\0bind_password_encrypted",
        binding.server_url.len(),
        binding.server_url,
        binding.bind_dn.len(),
        binding.bind_dn,
        binding.base_dn.len(),
        binding.base_dn,
        if binding.use_starttls { 1 } else { 0 },
        "bind_password_encrypted".len(),
    )
}

pub(crate) fn encrypt_ldap_bind_password(
    state: &AppState,
    config: &aether_data::repository::auth_modules::StoredLdapModuleConfig,
    plaintext: &str,
) -> Option<String> {
    if plaintext.contains('\0') {
        return None;
    }
    let binding = ldap_bind_password_binding(config)?;
    seal_runtime_secret_payload(state, &ldap_bind_password_purpose_v3(&binding), plaintext)
        .map(|sealed| format!("{LDAP_BIND_PASSWORD_V3_PREFIX}{sealed}"))
}

fn decrypt_ldap_bind_password_v2(state: &AppState, stored: &str) -> Option<String> {
    let sealed = stored.strip_prefix(LDAP_BIND_PASSWORD_V2_PREFIX)?;
    open_runtime_secret_payload(state, LDAP_BIND_PASSWORD_BOUND_PURPOSE, sealed)
        .filter(|plaintext| !plaintext.contains('\0'))
}

fn decrypt_ldap_bind_password_v3(
    state: &AppState,
    config: &aether_data::repository::auth_modules::StoredLdapModuleConfig,
    stored: &str,
) -> Option<String> {
    let sealed = stored.strip_prefix(LDAP_BIND_PASSWORD_V3_PREFIX)?;
    let binding = ldap_bind_password_binding(config)?;
    open_runtime_secret_payload(state, &ldap_bind_password_purpose_v3(&binding), sealed)
        .filter(|plaintext| !plaintext.contains('\0'))
}

fn stored_secret_uses_known_envelope_family(value: &str) -> bool {
    value.starts_with(SYSTEM_CONFIG_SECRET_ENVELOPE_FAMILY_PREFIX)
        || value.starts_with(LDAP_BIND_PASSWORD_ENVELOPE_FAMILY_PREFIX)
        || value.starts_with(RUNTIME_SECRET_ENVELOPE_FAMILY_PREFIX)
        || value.starts_with("aether-")
}

pub(crate) fn module_available_from_env(env_key: &str, default_available: bool) -> bool {
    match std::env::var(env_key) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "true" | "1" | "yes"
        ),
        Err(_) => default_available,
    }
}

pub(crate) fn system_config_bool(value: Option<&serde_json::Value>, default: bool) -> bool {
    match value {
        Some(serde_json::Value::Bool(value)) => *value,
        Some(serde_json::Value::Number(value)) => {
            value.as_i64().map(|value| value != 0).unwrap_or(default)
        }
        Some(serde_json::Value::String(value)) => {
            match value.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => true,
                "false" | "0" | "no" | "off" => false,
                _ => default,
            }
        }
        _ => default,
    }
}

pub(crate) fn system_config_string(value: Option<&serde_json::Value>) -> Option<String> {
    match value {
        Some(serde_json::Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BarkDeviceKeyBinding {
    pub(crate) server_url: String,
}

/// Canonicalize the Bark base URL before it participates in a secret binding.
/// Host spelling and default ports are normalized so equivalent destinations
/// use the same binding, while credentials and URL-controlled request data are
/// rejected entirely.
pub(crate) fn canonical_bark_server_url(raw: &str) -> Option<String> {
    if raw.contains('\0') {
        return None;
    }
    let raw = raw.trim().trim_end_matches('/');
    if raw.is_empty() || raw.contains('@') {
        return None;
    }
    let mut parsed = Url::parse(raw).ok()?;
    if !matches!(parsed.scheme(), "https" | "http")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let host = parsed
        .host_str()?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    parsed.set_host(Some(&host)).ok()?;
    let default_port = if parsed.scheme() == "https" { 443 } else { 80 };
    if parsed.port() == Some(default_port) {
        parsed.set_port(None).ok()?;
    }
    Some(parsed.as_str().trim_end_matches('/').to_string())
}

pub(crate) fn bark_device_key_binding(server_url: &str) -> Option<BarkDeviceKeyBinding> {
    Some(BarkDeviceKeyBinding {
        server_url: canonical_bark_server_url(server_url)?,
    })
}

fn bark_device_key_purpose(binding: &BarkDeviceKeyBinding) -> String {
    format!(
        "{BARK_DEVICE_KEY_BOUND_PURPOSE_V2}\0server-url-bytes={}\0{}\0field-bytes={}\0{}",
        binding.server_url.len(),
        binding.server_url,
        BARK_DEVICE_KEY_CONFIG_KEY.len(),
        BARK_DEVICE_KEY_CONFIG_KEY,
    )
}

pub(crate) fn encrypt_bark_device_key(
    state: &AppState,
    binding: &BarkDeviceKeyBinding,
    plaintext: &str,
) -> Option<String> {
    if plaintext.contains('\0') {
        return None;
    }
    seal_runtime_secret_payload(state, &bark_device_key_purpose(binding), plaintext)
        .map(|sealed| format!("{BARK_DEVICE_KEY_V2_PREFIX}{sealed}"))
}

fn decrypt_bark_device_key_v2(
    state: &AppState,
    binding: &BarkDeviceKeyBinding,
    stored: &str,
) -> Option<String> {
    let sealed = stored.strip_prefix(BARK_DEVICE_KEY_V2_PREFIX)?;
    open_runtime_secret_payload(state, &bark_device_key_purpose(binding), sealed)
        .filter(|plaintext| !plaintext.contains('\0'))
}

pub(crate) async fn decrypt_or_migrate_bark_device_key(
    state: &AppState,
    binding: &BarkDeviceKeyBinding,
    stored_value: String,
) -> Result<String, GatewayError> {
    let mut observed_raw = stored_value;
    for _ in 0..BARK_DEVICE_KEY_MIGRATION_RETRIES {
        let current_raw =
            read_strong_system_config_secret(state, BARK_DEVICE_KEY_CONFIG_KEY).await?;
        if current_raw != observed_raw {
            observed_raw = current_raw;
            continue;
        }
        let observed = observed_raw.trim();
        if observed.starts_with(BARK_DEVICE_KEY_V2_PREFIX) {
            return decrypt_bark_device_key_v2(state, binding, observed).ok_or_else(|| {
                system_config_secret_error("stored Bark device key cannot be decrypted")
            });
        }

        // Older Bark entries were written by the generic system-config secret
        // path. They may migrate once, but all new writes are destination-bound.
        let plaintext = if let Some(plaintext) =
            decrypt_system_config_secret(state, BARK_DEVICE_KEY_CONFIG_KEY, observed)
        {
            plaintext
        } else {
            if stored_secret_uses_known_envelope_family(observed) {
                return Err(system_config_secret_error(
                    "stored Bark device key cannot be decrypted",
                ));
            }
            match decrypt_catalog_secret_with_fallbacks(state.encryption_key(), observed) {
                Some(plaintext) => plaintext,
                None if looks_like_python_fernet_ciphertext(observed) => {
                    return Err(system_config_secret_error(
                        "stored Bark device key cannot be decrypted",
                    ));
                }
                None => observed.to_string(),
            }
        };
        if plaintext.contains('\0') {
            return Err(system_config_secret_error(
                "stored Bark device key contains reserved secret framing",
            ));
        }
        let encrypted = encrypt_bark_device_key(state, binding, &plaintext).ok_or_else(|| {
            system_config_secret_error("Bark device key migration is unavailable")
        })?;
        if state
            .compare_and_set_system_config_string_value(
                BARK_DEVICE_KEY_CONFIG_KEY,
                &observed_raw,
                &encrypted,
            )
            .await?
        {
            return Ok(plaintext);
        }
        observed_raw = read_strong_system_config_secret(state, BARK_DEVICE_KEY_CONFIG_KEY).await?;
    }

    Err(system_config_secret_error(
        "Bark device key migration did not stabilize",
    ))
}

pub(crate) async fn decrypt_or_migrate_system_config_secret(
    state: &AppState,
    key: &str,
    stored_value: String,
) -> Result<String, GatewayError> {
    decrypt_or_migrate_system_config_secret_with_before_compare(
        state,
        key,
        stored_value,
        || async {},
    )
    .await
}

pub(crate) async fn decrypt_or_migrate_ldap_bind_password(
    state: &AppState,
    config: &aether_data::repository::auth_modules::StoredLdapModuleConfig,
) -> Result<Option<String>, GatewayError> {
    let mut current = config.clone();
    for _ in 0..LDAP_BIND_PASSWORD_MIGRATION_RETRIES {
        let Some(observed_raw) = current
            .bind_password_encrypted
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        let observed = observed_raw.trim();
        if observed.starts_with(LDAP_BIND_PASSWORD_V3_PREFIX) {
            let plaintext =
                decrypt_ldap_bind_password_v3(state, &current, observed).ok_or_else(|| {
                    ldap_bind_password_error("stored LDAP bind password cannot be decrypted")
                })?;
            return Ok((!plaintext.trim().is_empty()).then_some(plaintext));
        }
        if observed.starts_with(LDAP_BIND_PASSWORD_V2_PREFIX) {
            let plaintext = decrypt_ldap_bind_password_v2(state, observed).ok_or_else(|| {
                ldap_bind_password_error("stored LDAP bind password cannot be decrypted")
            })?;
            if plaintext.trim().is_empty() {
                return Ok(None);
            }
            // v2 was bound only to the LDAP field name. Re-seal it with the
            // complete current LDAP destination before returning it so a
            // legacy ciphertext cannot remain portable across configurations.
            let encrypted =
                encrypt_ldap_bind_password(state, &current, &plaintext).ok_or_else(|| {
                    ldap_bind_password_error("LDAP bind password migration is unavailable")
                })?;
            if state
                .compare_and_swap_ldap_bind_password(observed_raw, &encrypted)
                .await?
            {
                return Ok(Some(plaintext));
            }
            current = state.get_ldap_module_config().await?.ok_or_else(|| {
                ldap_bind_password_error("stored LDAP configuration is unavailable")
            })?;
            continue;
        }
        if stored_secret_uses_known_envelope_family(observed) {
            return Err(ldap_bind_password_error(
                "stored LDAP bind password cannot be decrypted",
            ));
        }
        let plaintext =
            match decrypt_catalog_secret_with_fallbacks(state.encryption_key(), observed) {
                Some(plaintext) => plaintext,
                None if looks_like_python_fernet_ciphertext(observed) => {
                    return Err(ldap_bind_password_error(
                        "stored LDAP bind password cannot be decrypted",
                    ));
                }
                None => observed.to_string(),
            };
        if plaintext.contains('\0') {
            return Err(ldap_bind_password_error(
                "stored LDAP bind password contains reserved secret framing",
            ));
        }
        let encrypted =
            encrypt_ldap_bind_password(state, &current, &plaintext).ok_or_else(|| {
                ldap_bind_password_error("LDAP bind password migration is unavailable")
            })?;
        if state
            .compare_and_swap_ldap_bind_password(observed_raw, &encrypted)
            .await?
        {
            return Ok((!plaintext.trim().is_empty()).then_some(plaintext));
        }
        current = state
            .get_ldap_module_config()
            .await?
            .ok_or_else(|| ldap_bind_password_error("stored LDAP configuration is unavailable"))?;
    }

    Err(ldap_bind_password_error(
        "LDAP bind password migration did not stabilize",
    ))
}

async fn decrypt_or_migrate_system_config_secret_with_before_compare<BeforeCompare, CompareFuture>(
    state: &AppState,
    key: &str,
    stored_value: String,
    before_compare: BeforeCompare,
) -> Result<String, GatewayError>
where
    BeforeCompare: Fn() -> CompareFuture,
    CompareFuture: Future<Output = ()>,
{
    let mut observed_raw = stored_value;
    for _ in 0..SYSTEM_CONFIG_SECRET_MIGRATION_RETRIES {
        let current_raw = read_strong_system_config_secret(state, key).await?;
        if current_raw != observed_raw {
            observed_raw = current_raw;
            continue;
        }
        let observed = observed_raw.trim();

        if observed.starts_with(SYSTEM_CONFIG_SECRET_V2_PREFIX) {
            let plaintext =
                decrypt_system_config_secret(state, key, observed).ok_or_else(|| {
                    system_config_secret_error(
                        "stored system configuration secret cannot be decrypted",
                    )
                })?;
            return Ok(plaintext);
        }
        if stored_secret_uses_known_envelope_family(observed) {
            return Err(system_config_secret_error(
                "stored system configuration secret cannot be decrypted",
            ));
        }
        let plaintext =
            match decrypt_catalog_secret_with_fallbacks(state.encryption_key(), observed) {
                Some(plaintext) => plaintext,
                None if looks_like_python_fernet_ciphertext(observed) => {
                    return Err(system_config_secret_error(
                        "stored system configuration secret cannot be decrypted",
                    ));
                }
                None => observed.to_string(),
            };
        if plaintext.contains('\0') {
            return Err(system_config_secret_error(
                "stored system configuration secret contains reserved secret framing",
            ));
        }
        let encrypted = encrypt_system_config_secret(state, key, &plaintext).ok_or_else(|| {
            system_config_secret_error("system configuration secret migration is unavailable")
        })?;

        before_compare().await;
        if state
            .compare_and_set_system_config_string_value(key, &observed_raw, &encrypted)
            .await?
        {
            return Ok(plaintext);
        }
        observed_raw = read_strong_system_config_secret(state, key).await?;
    }

    Err(system_config_secret_error(
        "system configuration secret migration did not stabilize",
    ))
}

async fn read_strong_system_config_secret(
    state: &AppState,
    key: &str,
) -> Result<String, GatewayError> {
    state
        .read_system_config_json_value_strong(key)
        .await?
        .and_then(|value| {
            value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(ToOwned::to_owned)
        })
        .ok_or_else(|| {
            system_config_secret_error("stored system configuration secret is unavailable")
        })
}

fn system_config_secret_error(message: &str) -> GatewayError {
    GatewayError::Internal(message.to_string())
}

fn ldap_bind_password_error(message: &str) -> GatewayError {
    GatewayError::Internal(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        bark_device_key_binding, decrypt_bark_device_key_v2, decrypt_ldap_bind_password_v2,
        decrypt_ldap_bind_password_v3, decrypt_or_migrate_bark_device_key,
        decrypt_or_migrate_ldap_bind_password, decrypt_or_migrate_smtp_password,
        decrypt_or_migrate_system_config_secret,
        decrypt_or_migrate_system_config_secret_with_before_compare, decrypt_system_config_secret,
        encrypt_bark_device_key, encrypt_ldap_bind_password, encrypt_smtp_password,
        encrypt_system_config_secret, ldap_module_config_is_valid,
        normalize_ldap_transport_server_url, smtp_password_binding, LDAP_BIND_PASSWORD_V2_PREFIX,
        LDAP_BIND_PASSWORD_V3_PREFIX, SMTP_PASSWORD_V3_PREFIX, SYSTEM_CONFIG_SECRET_V2_PREFIX,
    };
    use crate::data::GatewayDataState;
    use crate::AppState;
    use aether_crypto::{
        encrypt_python_fernet_plaintext, looks_like_python_fernet_ciphertext,
        DEVELOPMENT_ENCRYPTION_KEY,
    };
    use aether_data::repository::auth_modules::{
        AuthModuleReadRepository, InMemoryAuthModuleReadRepository, StoredLdapModuleConfig,
    };
    use futures_util::future::join_all;
    use serde_json::json;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use tokio::sync::Barrier;

    const TEST_KEY: &str = "smtp_password";
    const TEST_SECRET: &str = "legacy-plaintext-password";
    const BARK_DEVICE_KEY: &str = "module.bark_push.device_key";

    fn state_with_stored_secret(value: &str) -> AppState {
        state_with_named_stored_secret(TEST_KEY, value)
    }

    fn state_with_named_stored_secret(key: &str, value: &str) -> AppState {
        let data = GatewayDataState::disabled()
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY)
            .with_system_config_values_for_tests([(key.to_string(), json!(value))]);
        let mut state = AppState::new().expect("gateway state should build");
        state.replace_data_state(Arc::new(data));
        state
    }

    #[tokio::test]
    async fn smtp_password_migrates_legacy_formats_to_bound_v3() {
        let binding = smtp_password_binding(
            "smtp.example.com",
            587,
            Some("ops@example.com"),
            true,
            false,
        )
        .expect("SMTP binding should build");
        let fixture_state = state_with_stored_secret(TEST_SECRET);
        let legacy_values = [
            TEST_SECRET.to_string(),
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, TEST_SECRET)
                .expect("legacy SMTP password should encrypt"),
            encrypt_system_config_secret(&fixture_state, TEST_KEY, TEST_SECRET)
                .expect("v2 SMTP password should encrypt"),
        ];

        for legacy in legacy_values {
            let state = state_with_stored_secret(&legacy);
            let plaintext = decrypt_or_migrate_smtp_password(&state, &binding, legacy.clone())
                .await
                .expect("legacy SMTP password should migrate");
            assert_eq!(plaintext, TEST_SECRET);
            let migrated = state
                .read_system_config_json_value_strong(TEST_KEY)
                .await
                .expect("SMTP password should read")
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .expect("SMTP password should be a string");
            assert!(migrated.starts_with(SMTP_PASSWORD_V3_PREFIX));
            assert_ne!(migrated, legacy);
            assert_eq!(
                decrypt_or_migrate_smtp_password(&state, &binding, migrated.clone())
                    .await
                    .expect("migrated SMTP password should decrypt"),
                TEST_SECRET
            );
            assert_eq!(
                state
                    .read_system_config_json_value_strong(TEST_KEY)
                    .await
                    .unwrap(),
                Some(json!(migrated))
            );
        }
    }

    #[tokio::test]
    async fn smtp_password_rejects_invalid_ciphertext_without_rewriting() {
        let binding = smtp_password_binding(
            "smtp.example.com",
            587,
            Some("ops@example.com"),
            true,
            false,
        )
        .expect("SMTP binding should build");
        let fixture_state = state_with_stored_secret(TEST_SECRET);
        let mut tampered = encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, TEST_SECRET)
            .expect("legacy SMTP password should encrypt");
        tampered.replace_range(tampered.len() - 2.., "AA");
        let invalid_values = [
            tampered,
            encrypt_python_fernet_plaintext("unavailable-historical-key", TEST_SECRET)
                .expect("wrong-key SMTP password should encrypt"),
            encrypt_system_config_secret(&fixture_state, "other_secret", TEST_SECRET)
                .expect("wrong-purpose secret should encrypt"),
            "aether-system-config-secret-v2:invalid".to_string(),
            "aether-smtp-password-v3:invalid".to_string(),
            "aether-runtime-secret-v1:invalid".to_string(),
            "aether-unknown-secret-v4:invalid".to_string(),
        ];

        for stored in invalid_values {
            let state = state_with_stored_secret(&stored);
            let error = decrypt_or_migrate_smtp_password(&state, &binding, stored.clone())
                .await
                .expect_err("invalid ciphertext must not become an SMTP password");
            assert_eq!(
                error.into_message(),
                "stored SMTP password cannot be decrypted"
            );
            assert_eq!(
                state
                    .read_system_config_json_value_strong(TEST_KEY)
                    .await
                    .unwrap(),
                Some(json!(stored))
            );
        }
    }

    #[tokio::test]
    async fn smtp_password_v3_rejects_changed_transport_binding() {
        let binding = smtp_password_binding(
            "smtp.example.com",
            587,
            Some("ops@example.com"),
            true,
            false,
        )
        .expect("SMTP binding should build");
        let stored = encrypt_smtp_password(
            &state_with_stored_secret(TEST_SECRET),
            &binding,
            TEST_SECRET,
        )
        .expect("SMTP password should encrypt");
        let state = state_with_stored_secret(&stored);
        for changed_binding in [
            smtp_password_binding(
                "other.example.com",
                587,
                Some("ops@example.com"),
                true,
                false,
            ),
            smtp_password_binding(
                "smtp.example.com",
                465,
                Some("ops@example.com"),
                true,
                false,
            ),
            smtp_password_binding(
                "smtp.example.com",
                587,
                Some("other@example.com"),
                true,
                false,
            ),
            smtp_password_binding(
                "smtp.example.com",
                587,
                Some("ops@example.com"),
                false,
                false,
            ),
            smtp_password_binding("smtp.example.com", 587, Some("ops@example.com"), true, true),
        ] {
            assert!(decrypt_or_migrate_smtp_password(
                &state,
                &changed_binding.expect("changed binding should build"),
                stored.clone(),
            )
            .await
            .is_err());
        }
        assert_eq!(
            state
                .read_system_config_json_value_strong(TEST_KEY)
                .await
                .unwrap(),
            Some(json!(stored))
        );
    }

    fn ldap_config(bind_password: &str) -> StoredLdapModuleConfig {
        StoredLdapModuleConfig {
            server_url: "ldaps://ldap.example.com".to_string(),
            bind_dn: "cn=admin,dc=example,dc=com".to_string(),
            bind_password_encrypted: Some(bind_password.to_string()),
            base_dn: "dc=example,dc=com".to_string(),
            user_search_filter: Some("(uid={username})".to_string()),
            username_attr: Some("uid".to_string()),
            email_attr: Some("mail".to_string()),
            display_name_attr: Some("displayName".to_string()),
            is_enabled: true,
            is_exclusive: false,
            use_starttls: false,
            connect_timeout: Some(10),
        }
    }

    #[test]
    fn ldap_transport_url_requires_tls_and_rejects_url_control_data() {
        assert_eq!(
            normalize_ldap_transport_server_url(" ldaps://LDAP.Example.COM:636/ ", false)
                .as_deref(),
            Some("ldaps://ldap.example.com")
        );
        assert_eq!(
            normalize_ldap_transport_server_url("ldap://10.20.30.40:389", true).as_deref(),
            Some("ldap://10.20.30.40")
        );
        assert!(normalize_ldap_transport_server_url("ldap://10.20.30.40", false).is_none());
        assert!(
            normalize_ldap_transport_server_url("ldap://user:password@ldap.example.com", true)
                .is_none()
        );
        assert!(normalize_ldap_transport_server_url("ldaps://@ldap.example.com", false).is_none());
        assert!(
            normalize_ldap_transport_server_url("ldaps://ldap.example.com?x=1", false).is_none()
        );
        assert!(
            normalize_ldap_transport_server_url("ldaps://ldap.example.com#fragment", false)
                .is_none()
        );
        assert!(
            normalize_ldap_transport_server_url("ldaps://ldap.example.com/dc=example", false)
                .is_none()
        );
        assert!(normalize_ldap_transport_server_url("https://ldap.example.com", false).is_none());
        assert!(normalize_ldap_transport_server_url("ldaps://ldap.example.com\n", false).is_none());
        // The in-process mock is intentionally available only to test builds.
        assert!(
            normalize_ldap_transport_server_url("mockldap://ldap.example.com", false).is_some()
        );
    }

    #[test]
    fn gateway_ldap_validation_reuses_shared_field_rules_with_test_transport() {
        let mut config = ldap_config("sealed-password");
        config.server_url = "mockldap://ldap.example.com".to_string();
        config.use_starttls = false;
        assert!(ldap_module_config_is_valid(Some(&config)));

        config.user_search_filter = Some("(uid={username})(objectClass=*)".to_string());
        assert!(!ldap_module_config_is_valid(Some(&config)));
        config.user_search_filter = Some("(uid={username})".to_string());
        config.username_attr = Some("uid)(|(objectClass=*)".to_string());
        assert!(!ldap_module_config_is_valid(Some(&config)));
        config.username_attr = Some("uid".to_string());
        config.bind_dn = "cn=admin,dc=example,dc=com\n".to_string();
        assert!(!ldap_module_config_is_valid(Some(&config)));
    }

    fn state_with_ldap_bind_password(
        value: &str,
    ) -> (AppState, Arc<InMemoryAuthModuleReadRepository>) {
        let repository = Arc::new(InMemoryAuthModuleReadRepository::seed(
            Vec::new(),
            Some(ldap_config(value)),
        ));
        let data = GatewayDataState::with_auth_module_repository_for_tests(repository.clone())
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);
        let mut state = AppState::new().expect("gateway state should build");
        state.replace_data_state(Arc::new(data));
        (state, repository)
    }

    #[tokio::test]
    async fn legacy_ldap_bind_password_is_lazily_migrated() {
        let (state, repository) = state_with_ldap_bind_password(TEST_SECRET);
        let config = repository
            .get_ldap_config()
            .await
            .expect("LDAP config should read")
            .expect("LDAP config should exist");

        let plaintext = decrypt_or_migrate_ldap_bind_password(&state, &config)
            .await
            .expect("legacy LDAP bind password should migrate");
        assert_eq!(plaintext.as_deref(), Some(TEST_SECRET));

        let stored = repository
            .get_ldap_config()
            .await
            .expect("LDAP config should read")
            .and_then(|config| config.bind_password_encrypted)
            .expect("LDAP bind password should exist");
        assert!(stored.starts_with(LDAP_BIND_PASSWORD_V3_PREFIX));
        assert_eq!(
            decrypt_ldap_bind_password_v3(&state, &config, &stored)
                .expect("migrated LDAP password should decrypt"),
            TEST_SECRET
        );
    }

    #[tokio::test]
    async fn tampered_ldap_bind_password_ciphertext_fails_closed() {
        let mut tampered = encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, TEST_SECRET)
            .expect("LDAP password should encrypt");
        tampered.replace_range(tampered.len() - 2.., "AA");
        assert!(looks_like_python_fernet_ciphertext(&tampered));
        let (state, repository) = state_with_ldap_bind_password(&tampered);
        let config = repository
            .get_ldap_config()
            .await
            .expect("LDAP config should read")
            .expect("LDAP config should exist");

        let error = decrypt_or_migrate_ldap_bind_password(&state, &config)
            .await
            .expect_err("tampered LDAP ciphertext must not be used as plaintext");
        assert!(format!("{error:?}").contains("cannot be decrypted"));
        assert_eq!(
            repository
                .get_ldap_config()
                .await
                .expect("LDAP config should read")
                .and_then(|config| config.bind_password_encrypted),
            Some(tampered)
        );
    }

    #[tokio::test]
    async fn bound_secret_envelopes_cannot_move_between_ldap_and_system_config() {
        let (ldap_state, _) = state_with_ldap_bind_password(TEST_SECRET);
        let ldap_sealed =
            encrypt_ldap_bind_password(&ldap_state, &ldap_config("legacy"), TEST_SECRET)
                .expect("LDAP bind password should seal");
        let system_sealed = encrypt_system_config_secret(&ldap_state, TEST_KEY, TEST_SECRET)
            .expect("system config secret should seal");

        let (ldap_with_system_secret, repository) = state_with_ldap_bind_password(&system_sealed);
        let config = repository
            .get_ldap_config()
            .await
            .expect("LDAP config should read")
            .expect("LDAP config should exist");
        let ldap_error = decrypt_or_migrate_ldap_bind_password(&ldap_with_system_secret, &config)
            .await
            .expect_err("system config ciphertext must not become an LDAP password");
        assert!(ldap_error.into_message().contains("cannot be decrypted"));

        let system_with_ldap_secret = state_with_stored_secret(&ldap_sealed);
        let system_error = decrypt_or_migrate_system_config_secret(
            &system_with_ldap_secret,
            TEST_KEY,
            ldap_sealed.clone(),
        )
        .await
        .expect_err("LDAP ciphertext must not become a system config secret");
        assert!(system_error.into_message().contains("cannot be decrypted"));

        let stripped_system = system_sealed
            .strip_prefix(SYSTEM_CONFIG_SECRET_V2_PREFIX)
            .and_then(|value| value.strip_prefix("aether-runtime-secret-v1:"))
            .expect("system secret should contain a nested runtime envelope");
        let (ldap_with_stripped_system, repository) =
            state_with_ldap_bind_password(stripped_system);
        let config = repository
            .get_ldap_config()
            .await
            .expect("LDAP config should read")
            .expect("LDAP config should exist");
        let error = decrypt_or_migrate_ldap_bind_password(&ldap_with_stripped_system, &config)
            .await
            .expect_err("stripped system framing must not become an LDAP password");
        assert!(error.into_message().contains("reserved secret framing"));

        let stripped_ldap = ldap_sealed
            .strip_prefix(LDAP_BIND_PASSWORD_V3_PREFIX)
            .and_then(|value| value.strip_prefix("aether-runtime-secret-v1:"))
            .expect("LDAP secret should contain a nested runtime envelope");
        let system_with_stripped_ldap = state_with_stored_secret(stripped_ldap);
        let error = decrypt_or_migrate_system_config_secret(
            &system_with_stripped_ldap,
            TEST_KEY,
            stripped_ldap.to_string(),
        )
        .await
        .expect_err("stripped LDAP framing must not become a system secret");
        assert!(error.into_message().contains("reserved secret framing"));
    }

    #[tokio::test]
    async fn legacy_plaintext_secret_is_lazily_migrated() {
        let state = state_with_stored_secret(TEST_SECRET);

        let plaintext =
            decrypt_or_migrate_system_config_secret(&state, TEST_KEY, TEST_SECRET.to_string())
                .await
                .expect("legacy secret should migrate");
        assert_eq!(plaintext, TEST_SECRET);

        let stored = state
            .read_system_config_json_value_strong(TEST_KEY)
            .await
            .expect("stored secret should read")
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .expect("stored secret should be a string");
        assert_ne!(stored, TEST_SECRET);
        assert!(stored.starts_with(SYSTEM_CONFIG_SECRET_V2_PREFIX));
        assert_eq!(
            decrypt_system_config_secret(&state, TEST_KEY, &stored)
                .expect("migrated secret should decrypt"),
            TEST_SECRET
        );
    }

    #[tokio::test]
    async fn legacy_secret_migration_compares_the_untrimmed_stored_value() {
        let stored_raw = format!("  {TEST_SECRET}  ");
        let state = state_with_stored_secret(&stored_raw);

        let plaintext =
            decrypt_or_migrate_system_config_secret(&state, TEST_KEY, TEST_SECRET.to_string())
                .await
                .expect("whitespace-wrapped legacy secret should migrate");
        assert_eq!(plaintext, TEST_SECRET);

        let stored = state
            .read_system_config_json_value_strong(TEST_KEY)
            .await
            .expect("stored secret should read")
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .expect("stored secret should be a string");
        assert_eq!(
            decrypt_system_config_secret(&state, TEST_KEY, &stored).as_deref(),
            Some(TEST_SECRET)
        );
    }

    #[tokio::test]
    async fn concurrent_legacy_reads_do_not_double_encrypt() {
        let state = state_with_stored_secret(TEST_SECRET);
        let reads = (0..16).map(|_| {
            decrypt_or_migrate_system_config_secret(&state, TEST_KEY, TEST_SECRET.to_string())
        });
        for result in join_all(reads).await {
            assert_eq!(
                result.expect("concurrent migration should succeed"),
                TEST_SECRET
            );
        }

        let stored = state
            .read_system_config_json_value_strong(TEST_KEY)
            .await
            .expect("stored secret should read")
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .expect("stored secret should be a string");
        assert_eq!(
            decrypt_system_config_secret(&state, TEST_KEY, &stored)
                .expect("migrated secret should decrypt exactly once"),
            TEST_SECRET
        );
    }

    #[tokio::test]
    async fn stale_cached_ciphertext_does_not_bypass_strong_read() {
        let stale = encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            "credential-before-rotation",
        )
        .expect("stale fixture should encrypt");
        let current = encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            "credential-after-rotation",
        )
        .expect("current fixture should encrypt");
        let state = state_with_stored_secret(&current);

        let plaintext = decrypt_or_migrate_system_config_secret(&state, TEST_KEY, stale)
            .await
            .expect("strong read should use the rotated credential");

        assert_eq!(plaintext, "credential-after-rotation");
        let stored = state
            .read_system_config_json_value_strong(TEST_KEY)
            .await
            .expect("stored secret should read")
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .expect("stored secret should be a string");
        assert_ne!(stored, current);
        assert_eq!(
            decrypt_system_config_secret(&state, TEST_KEY, &stored).as_deref(),
            Some("credential-after-rotation")
        );
    }

    #[tokio::test]
    async fn administrator_rotation_wins_race_with_plaintext_migration() {
        let state = state_with_stored_secret(TEST_SECRET);
        let rotated_plaintext = "credential-after-administrator-rotation";
        let rotated_ciphertext =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, rotated_plaintext)
                .expect("rotated fixture should encrypt");
        let reached_compare = Arc::new(Barrier::new(2));
        let resume_compare = Arc::new(Barrier::new(2));
        let first_compare = Arc::new(AtomicBool::new(true));
        let migration_reached_compare = Arc::clone(&reached_compare);
        let migration_resume_compare = Arc::clone(&resume_compare);
        let migration_first_compare = Arc::clone(&first_compare);

        let migration = decrypt_or_migrate_system_config_secret_with_before_compare(
            &state,
            TEST_KEY,
            TEST_SECRET.to_string(),
            move || {
                let reached_compare = Arc::clone(&migration_reached_compare);
                let resume_compare = Arc::clone(&migration_resume_compare);
                let first_compare = Arc::clone(&migration_first_compare);
                async move {
                    if first_compare.swap(false, Ordering::SeqCst) {
                        reached_compare.wait().await;
                        resume_compare.wait().await;
                    }
                }
            },
        );
        let rotation = async {
            reached_compare.wait().await;
            state
                .upsert_system_config_json_value(TEST_KEY, &json!(rotated_ciphertext.clone()), None)
                .await
                .expect("administrator rotation should persist");
            resume_compare.wait().await;
        };

        let (migration_result, ()) = tokio::join!(migration, rotation);
        assert_eq!(
            migration_result.expect("migration should retry against the rotated value"),
            rotated_plaintext
        );
        let stored = state
            .read_system_config_json_value_strong(TEST_KEY)
            .await
            .expect("stored secret should read")
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .expect("stored secret should be a string");
        assert_ne!(stored, rotated_ciphertext);
        assert_eq!(
            decrypt_system_config_secret(&state, TEST_KEY, &stored).as_deref(),
            Some(rotated_plaintext)
        );
    }

    #[tokio::test]
    async fn failed_compare_and_set_invalidates_cached_system_config_value() {
        let data = Arc::new(
            GatewayDataState::disabled()
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY)
                .with_system_config_values_for_tests([(
                    TEST_KEY.to_string(),
                    json!("value-cached-on-this-node"),
                )]),
        );
        let mut state = AppState::new().expect("gateway state should build");
        state.replace_data_state(Arc::clone(&data));
        assert_eq!(
            state
                .read_system_config_json_value(TEST_KEY)
                .await
                .expect("initial config should cache"),
            Some(json!("value-cached-on-this-node"))
        );

        data.upsert_system_config_value(TEST_KEY, &json!("value-rotated-by-another-node"), None)
            .await
            .expect("simulated remote rotation should persist");
        assert!(!state
            .compare_and_set_system_config_string_value(
                TEST_KEY,
                "value-cached-on-this-node",
                "stale-migration-replacement",
            )
            .await
            .expect("stale compare-and-set should complete"));

        assert_eq!(
            state
                .read_system_config_json_value(TEST_KEY)
                .await
                .expect("config should reload after failed compare-and-set"),
            Some(json!("value-rotated-by-another-node"))
        );
    }

    #[tokio::test]
    async fn undecryptable_fernet_secret_fails_without_plaintext_fallback() {
        let ciphertext = encrypt_python_fernet_plaintext("unavailable-historical-key", TEST_SECRET)
            .expect("fixture should encrypt");
        let state = state_with_stored_secret(&ciphertext);

        let error = decrypt_or_migrate_system_config_secret(&state, TEST_KEY, ciphertext.clone())
            .await
            .expect_err("unknown Fernet ciphertext must fail closed");
        let error_text = error.into_message();
        assert!(!error_text.contains(TEST_SECRET));
        assert!(!error_text.contains(&ciphertext));
        assert_eq!(
            state
                .read_system_config_json_value_strong(TEST_KEY)
                .await
                .expect("stored secret should read"),
            Some(json!(ciphertext))
        );
    }

    #[tokio::test]
    async fn system_config_secret_ciphertext_is_bound_to_its_config_key() {
        let sealed = encrypt_system_config_secret(
            &state_with_stored_secret(TEST_SECRET),
            TEST_KEY,
            TEST_SECRET,
        )
        .expect("system config secret should seal");
        let state = state_with_stored_secret(&sealed);

        assert_eq!(
            decrypt_or_migrate_system_config_secret(&state, TEST_KEY, sealed.clone(),)
                .await
                .expect("matching system config secret should open"),
            TEST_SECRET
        );
        let wrong_key = "backup_s3_secret_access_key";
        let wrong_state = state_with_named_stored_secret(wrong_key, &sealed);
        let wrong_key_error =
            decrypt_or_migrate_system_config_secret(&wrong_state, wrong_key, sealed.clone())
                .await
                .expect_err("copied system config secret must fail closed");
        assert!(wrong_key_error
            .into_message()
            .contains("cannot be decrypted"));
        assert_eq!(
            decrypt_system_config_secret(&state, "SMTP_PASSWORD", &sealed).as_deref(),
            Some(TEST_SECRET)
        );
    }

    #[test]
    fn bark_device_key_ciphertext_is_bound_to_canonical_server_url() {
        let state = state_with_stored_secret(TEST_SECRET);
        let binding = bark_device_key_binding(" HTTPS://Example.COM:443/api/// ")
            .expect("Bark server URL should canonicalize");
        assert_eq!(binding.server_url, "https://example.com/api");
        let equivalent = bark_device_key_binding("https://example.com:443/api")
            .expect("equivalent Bark server URL should canonicalize");
        let sealed = encrypt_bark_device_key(&state, &binding, TEST_SECRET)
            .expect("Bark device key should seal");

        assert_eq!(
            decrypt_bark_device_key_v2(&state, &equivalent, &sealed).as_deref(),
            Some(TEST_SECRET)
        );
        let changed = bark_device_key_binding("https://example.com/other")
            .expect("changed Bark server URL should parse");
        assert!(decrypt_bark_device_key_v2(&state, &changed, &sealed).is_none());
    }

    #[tokio::test]
    async fn legacy_bark_device_key_migrates_to_destination_bound_envelope() {
        let state = state_with_named_stored_secret(BARK_DEVICE_KEY, TEST_SECRET);
        let binding =
            bark_device_key_binding("https://api.day.app").expect("Bark URL should parse");
        let plaintext =
            decrypt_or_migrate_bark_device_key(&state, &binding, TEST_SECRET.to_string())
                .await
                .expect("legacy Bark device key should migrate");
        assert_eq!(plaintext, TEST_SECRET);

        let stored = state
            .read_system_config_json_value_strong(BARK_DEVICE_KEY)
            .await
            .expect("Bark device key should read")
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .expect("Bark device key should remain a string");
        assert!(stored.starts_with("aether-bark-device-key-v2:"));
        assert_eq!(
            decrypt_bark_device_key_v2(&state, &binding, &stored).as_deref(),
            Some(TEST_SECRET)
        );
    }

    #[tokio::test]
    async fn bark_device_key_rejects_a_ciphertext_bound_to_another_server_url() {
        let state = state_with_stored_secret(TEST_SECRET);
        let original =
            bark_device_key_binding("https://api.day.app").expect("Bark URL should parse");
        let sealed = encrypt_bark_device_key(&state, &original, TEST_SECRET)
            .expect("Bark device key should seal");
        let state = state_with_named_stored_secret(BARK_DEVICE_KEY, &sealed);
        let changed = bark_device_key_binding("https://bark.example.test/api")
            .expect("changed Bark URL should parse");

        let error = decrypt_or_migrate_bark_device_key(&state, &changed, sealed.clone())
            .await
            .expect_err("Bark device key must not move between destinations");
        assert!(error.into_message().contains("cannot be decrypted"));
        assert_eq!(
            state
                .read_system_config_json_value_strong(BARK_DEVICE_KEY)
                .await
                .expect("Bark device key should remain readable"),
            Some(json!(sealed))
        );
    }
}
