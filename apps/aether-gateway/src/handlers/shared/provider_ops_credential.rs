use aether_crypto::looks_like_python_fernet_ciphertext;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::AppState;

use super::{
    decrypt_catalog_secret_with_fallbacks, open_runtime_secret_payload, seal_runtime_secret_payload,
};

const PROVIDER_OPS_CREDENTIAL_ENVELOPE_FAMILY: &str = "aether-provider-ops-credential-";
const PROVIDER_OPS_CREDENTIAL_ENVELOPE_V2: &str = "aether-provider-ops-credential-v2:";
const PROVIDER_OPS_CREDENTIAL_PURPOSE_V2: &str = "provider-ops-credential-bound-v2";

pub(crate) const PROVIDER_OPS_PERSISTENT_SECRET_FIELDS: &[&str] = &[
    "api_key",
    "password",
    "refresh_token",
    "session_token",
    "session_cookie",
    "token_cookie",
    "auth_cookie",
    "cookie_string",
    "cookie",
];
pub(crate) const PROVIDER_OPS_TRANSIENT_SECRET_FIELDS: &[&str] = &["_cached_access_token"];
pub(crate) const PROVIDER_OPS_TRANSIENT_METADATA_FIELDS: &[&str] = &["_cached_token_expires_at"];

pub(crate) fn provider_ops_credential_field_is_secret(field: &str) -> bool {
    PROVIDER_OPS_PERSISTENT_SECRET_FIELDS.contains(&field)
        || PROVIDER_OPS_TRANSIENT_SECRET_FIELDS.contains(&field)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderOpsCanonicalDestination {
    canonical_base_url: String,
    canonical_origin: String,
}

impl ProviderOpsCanonicalDestination {
    pub(crate) fn base_url(&self) -> &str {
        &self.canonical_base_url
    }

    pub(crate) fn origin(&self) -> &str {
        &self.canonical_origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderOpsCredentialBinding {
    pub(crate) provider_id: String,
    pub(crate) architecture_id: String,
    pub(crate) auth_type: String,
    pub(crate) destination: ProviderOpsCanonicalDestination,
    pub(crate) outbound_policy_digest: String,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProviderOpsCredentialProjection {
    pub(crate) plaintext: String,
    pub(crate) protected: String,
    pub(crate) migration_required: bool,
}

pub(crate) fn canonicalize_provider_ops_base_url(
    raw: &str,
) -> Result<ProviderOpsCanonicalDestination, &'static str> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("Provider Ops base_url 不能为空");
    }
    let mut parsed = url::Url::parse(raw).map_err(|_| "Provider Ops base_url 无效")?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("Provider Ops base_url 必须是有效的 HTTP(S) URL");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Provider Ops base_url 不允许包含认证信息");
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("Provider Ops base_url 不允许包含 query 或 fragment");
    }

    let normalized_path = parsed.path().trim_end_matches('/').to_string();
    parsed.set_path(if normalized_path.is_empty() {
        "/"
    } else {
        normalized_path.as_str()
    });
    let canonical_origin = parsed.origin().ascii_serialization();
    let mut canonical_base_url = parsed.to_string();
    if parsed.path() == "/" {
        canonical_base_url.truncate(canonical_base_url.len().saturating_sub(1));
    }

    Ok(ProviderOpsCanonicalDestination {
        canonical_base_url,
        canonical_origin,
    })
}

pub(crate) fn resolve_provider_ops_same_origin_url(
    destination: &ProviderOpsCanonicalDestination,
    endpoint: &str,
) -> Result<String, &'static str> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(destination.canonical_base_url.clone());
    }
    if endpoint.starts_with("//") {
        return Err("Provider Ops endpoint 不允许使用 scheme-relative URL");
    }

    let candidate = if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        url::Url::parse(endpoint).map_err(|_| "Provider Ops endpoint URL 无效")?
    } else {
        if !endpoint.starts_with('/') {
            return Err("Provider Ops endpoint 必须是以 / 开头的路径或同源绝对 URL");
        }
        let base = url::Url::parse(&format!("{}/", destination.canonical_origin))
            .map_err(|_| "Provider Ops canonical origin 无效")?;
        base.join(endpoint)
            .map_err(|_| "Provider Ops endpoint 路径无效")?
    };
    if !matches!(candidate.scheme(), "http" | "https")
        || candidate.host_str().is_none()
        || !candidate.username().is_empty()
        || candidate.password().is_some()
        || candidate.fragment().is_some()
    {
        return Err("Provider Ops endpoint URL 无效");
    }
    if candidate.origin().ascii_serialization() != destination.canonical_origin {
        return Err("Provider Ops endpoint 必须与 base_url 同源");
    }
    Ok(candidate.to_string())
}

pub(crate) fn provider_ops_outbound_policy_digest(
    architecture_id: &str,
    auth_type: &str,
    destination: &ProviderOpsCanonicalDestination,
    connector_config: Option<&Value>,
    actions: Option<&Value>,
) -> String {
    let policy = serde_json::json!({
        "architecture_id": architecture_id,
        "auth_type": auth_type,
        "canonical_base_url": destination.base_url(),
        "canonical_origin": destination.origin(),
        "connector_config": connector_config.cloned().unwrap_or_else(|| serde_json::json!({})),
        "actions": actions.cloned().unwrap_or_else(|| serde_json::json!({})),
    });
    let mut canonical = String::new();
    append_canonical_json(&policy, &mut canonical);
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

pub(crate) fn provider_ops_credential_binding_from_config(
    provider_id: &str,
    provider_ops_config: &serde_json::Map<String, Value>,
    effective_base_url: &str,
) -> Result<ProviderOpsCredentialBinding, &'static str> {
    if provider_id.trim().is_empty() {
        return Err("Provider Ops provider_id 不能为空");
    }
    let raw_architecture_id = provider_ops_config
        .get("architecture_id")
        .and_then(Value::as_str)
        .unwrap_or("generic_api")
        .trim();
    let architecture_id =
        aether_admin::provider::ops::normalize_architecture_id(raw_architecture_id);
    if !raw_architecture_id.is_empty() && raw_architecture_id != architecture_id {
        return Err("Provider Ops architecture_id 无效");
    }
    let connector = provider_ops_config
        .get("connector")
        .and_then(Value::as_object);
    let auth_type = connector
        .and_then(|connector| connector.get("auth_type"))
        .and_then(Value::as_str)
        .unwrap_or("api_key")
        .trim();
    if !aether_admin::provider::ops::admin_provider_ops_is_supported_auth_type(auth_type) {
        return Err("Provider Ops connector.auth_type 无效");
    }
    let destination = canonicalize_provider_ops_base_url(effective_base_url)?;
    let outbound_policy_digest = provider_ops_outbound_policy_digest(
        architecture_id,
        auth_type,
        &destination,
        connector.and_then(|connector| connector.get("config")),
        provider_ops_config.get("actions"),
    );
    Ok(ProviderOpsCredentialBinding {
        provider_id: provider_id.to_string(),
        architecture_id: architecture_id.to_string(),
        auth_type: auth_type.to_string(),
        destination,
        outbound_policy_digest,
    })
}

pub(crate) fn seal_provider_ops_credential(
    state: &AppState,
    binding: &ProviderOpsCredentialBinding,
    field: &str,
    plaintext: &str,
) -> Result<String, &'static str> {
    if plaintext.contains('\0') {
        return Err("Provider Ops credential 包含保留分隔符");
    }
    let purpose = provider_ops_credential_purpose(binding, field)?;
    let sealed = seal_runtime_secret_payload(state, &purpose, plaintext)
        .ok_or("gateway 未配置 Provider Ops 加密密钥")?;
    Ok(format!("{PROVIDER_OPS_CREDENTIAL_ENVELOPE_V2}{sealed}"))
}

pub(crate) fn open_provider_ops_credential(
    state: &AppState,
    binding: &ProviderOpsCredentialBinding,
    field: &str,
    stored: &str,
) -> Result<ProviderOpsCredentialProjection, &'static str> {
    let purpose = provider_ops_credential_purpose(binding, field)?;
    if let Some(sealed) = stored.strip_prefix(PROVIDER_OPS_CREDENTIAL_ENVELOPE_V2) {
        let plaintext = open_runtime_secret_payload(state, &purpose, sealed)
            .ok_or("Provider Ops credential 认证或绑定校验失败")?;
        if plaintext.contains('\0') {
            return Err("Provider Ops credential 包含保留分隔符");
        }
        return Ok(ProviderOpsCredentialProjection {
            plaintext,
            protected: stored.to_string(),
            migration_required: false,
        });
    }
    if stored.starts_with(PROVIDER_OPS_CREDENTIAL_ENVELOPE_FAMILY) {
        return Err("不支持的 Provider Ops credential envelope 版本");
    }
    if stored.starts_with("aether-") {
        return Err("Aether secret envelope 的 Provider Ops 记录绑定错误");
    }

    let plaintext = if looks_like_python_fernet_ciphertext(stored) {
        decrypt_catalog_secret_with_fallbacks(state.encryption_key(), stored)
            .ok_or("历史 Provider Ops credential 密文无法解密")?
    } else {
        stored.to_string()
    };
    if plaintext.contains('\0') {
        return Err("历史 Provider Ops credential 包含保留分隔符");
    }
    let protected = seal_provider_ops_credential(state, binding, field, &plaintext)?;
    Ok(ProviderOpsCredentialProjection {
        plaintext,
        protected,
        migration_required: !stored.is_empty(),
    })
}

fn provider_ops_credential_purpose(
    binding: &ProviderOpsCredentialBinding,
    field: &str,
) -> Result<String, &'static str> {
    for value in [
        binding.provider_id.as_str(),
        binding.architecture_id.as_str(),
        binding.auth_type.as_str(),
        binding.destination.base_url(),
        binding.destination.origin(),
        binding.outbound_policy_digest.as_str(),
        field,
    ] {
        if value.is_empty() || value.contains('\0') {
            return Err("Provider Ops credential binding 无效");
        }
    }
    Ok(format!(
        "{PROVIDER_OPS_CREDENTIAL_PURPOSE_V2}\0provider-id-bytes={}\0{}\0architecture-id-bytes={}\0{}\0auth-type-bytes={}\0{}\0base-url-bytes={}\0{}\0origin-bytes={}\0{}\0policy-sha256={}\0field-bytes={}\0{}",
        binding.provider_id.len(),
        binding.provider_id,
        binding.architecture_id.len(),
        binding.architecture_id,
        binding.auth_type.len(),
        binding.auth_type,
        binding.destination.base_url().len(),
        binding.destination.base_url(),
        binding.destination.origin().len(),
        binding.destination.origin(),
        binding.outbound_policy_digest,
        field.len(),
        field,
    ))
}

fn append_canonical_json(value: &Value, output: &mut String) {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&value.to_string()),
        Value::String(value) => output.push_str(
            &serde_json::to_string(value).expect("serializing a JSON string cannot fail"),
        ),
        Value::Array(items) => {
            output.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                append_canonical_json(item, output);
            }
            output.push(']');
        }
        Value::Object(map) => {
            output.push('{');
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(
                    &serde_json::to_string(key).expect("serializing a JSON key cannot fail"),
                );
                output.push(':');
                append_canonical_json(&map[key], output);
            }
            output.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        canonicalize_provider_ops_base_url, provider_ops_outbound_policy_digest,
        resolve_provider_ops_same_origin_url,
    };

    #[test]
    fn canonical_destination_normalizes_and_enforces_origin() {
        let destination = canonicalize_provider_ops_base_url(" HTTPS://Example.COM:443/api/ ")
            .expect("base URL should normalize");
        assert_eq!(destination.base_url(), "https://example.com/api");
        assert_eq!(destination.origin(), "https://example.com");
        assert!(resolve_provider_ops_same_origin_url(&destination, "/v1/me").is_ok());
        assert!(
            resolve_provider_ops_same_origin_url(&destination, "https://example.com/v1/me").is_ok()
        );
        assert!(
            resolve_provider_ops_same_origin_url(&destination, "https://evil.test/v1/me").is_err()
        );
        assert!(resolve_provider_ops_same_origin_url(&destination, "//evil.test/v1/me").is_err());
    }

    #[test]
    fn policy_digest_is_independent_of_json_object_order() {
        let destination = canonicalize_provider_ops_base_url("https://example.com")
            .expect("base URL should normalize");
        let left = serde_json::json!({"b": 2, "a": 1});
        let right = serde_json::json!({"a": 1, "b": 2});
        assert_eq!(
            provider_ops_outbound_policy_digest(
                "generic_api",
                "api_key",
                &destination,
                Some(&left),
                None,
            ),
            provider_ops_outbound_policy_digest(
                "generic_api",
                "api_key",
                &destination,
                Some(&right),
                None,
            )
        );
    }
}
