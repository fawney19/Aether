use aether_contracts::{
    TRANSPORT_BACKEND_REQWEST_RUSTLS, TRANSPORT_HTTP_MODE_AUTO, TRANSPORT_POOL_SCOPE_KEY,
};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::profile::current_claude_code_transport_identity_profile;

/// Generate the transport-profile metadata and per-key pool partition from a
/// stable key seed. HTTP identity headers are owned by the versioned profile
/// and are not overridden from this stored value.
pub fn generate_fingerprint(seed: &str) -> Value {
    wrap_header_fingerprint(generate_header_fingerprint(seed))
}

fn generate_header_fingerprint(seed: &str) -> Value {
    let profile = current_claude_code_transport_identity_profile();
    let vscode_session_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("aether:fingerprint:{seed}").as_bytes(),
    )
    .simple()
    .to_string();

    serde_json::json!({
        "identity_profile_version": profile.version().as_str(),
        "cli_version": profile.cli_version(),
        "billing_cli_version": profile.billing_cli_version(),
        "stainless_lang": profile.stainless_lang(),
        "stainless_package_version": profile.stainless_package_version(),
        "stainless_os": profile.stainless_os(),
        "stainless_arch": profile.stainless_arch(),
        "stainless_runtime": profile.stainless_runtime(),
        "stainless_runtime_version": profile.stainless_runtime_version(),
        "stainless_retry_count": profile.stainless_retry_count(),
        "stainless_timeout": profile.stainless_timeout(),
        "vscode_session_id": vscode_session_id,
        "user_agent": profile.user_agent(),
    })
}

fn wrap_header_fingerprint(header_fingerprint: Value) -> Value {
    let profile = current_claude_code_transport_identity_profile();
    serde_json::json!({
        "transport_profile": {
            "profile_id": profile.transport_profile_id(),
            "backend": TRANSPORT_BACKEND_REQWEST_RUSTLS,
            "http_mode": TRANSPORT_HTTP_MODE_AUTO,
            "pool_scope": TRANSPORT_POOL_SCOPE_KEY,
            "header_fingerprint": header_fingerprint,
            "extra": {
                "claude_code_identity_profile_version": profile.version().as_str(),
                "claude_code_cli_version": profile.cli_version(),
            }
        }
    })
}

pub fn header_fingerprint_from_fingerprint(fingerprint: &Value) -> Option<&Map<String, Value>> {
    fingerprint
        .get("transport_profile")
        .and_then(Value::as_object)
        .and_then(|profile| profile.get("header_fingerprint"))
        .and_then(Value::as_object)
}

/// Generate a random (non-deterministic) fingerprint.
pub fn generate_random_fingerprint() -> Value {
    generate_fingerprint(&Uuid::new_v4().to_string())
}

/// Upgrade stored transport metadata to the current typed identity profile.
/// Only the per-key pool partition (kept under its legacy session-id field) is
/// retained; fixed CLI and Stainless values remain one coherent version set.
pub fn sanitize_fingerprint(raw: &Value, key_id: &str) -> Value {
    let mut generated = generate_header_fingerprint(key_id);
    let Some(generated) = generated.as_object_mut() else {
        return generate_fingerprint(key_id);
    };

    if let Some(session_id) = header_fingerprint_from_fingerprint(raw)
        .and_then(|raw| raw.get("vscode_session_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        generated.insert(
            "vscode_session_id".to_string(),
            Value::String(session_id.to_string()),
        );
    }

    wrap_header_fingerprint(Value::Object(generated.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_generation_from_seed() {
        let fp1 = generate_fingerprint("key-abc-123");
        let fp2 = generate_fingerprint("key-abc-123");
        assert_eq!(fp1, fp2, "same seed should produce identical fingerprint");
    }

    #[test]
    fn different_seeds_produce_different_session_fingerprints() {
        let fp1 = generate_fingerprint("key-1");
        let fp2 = generate_fingerprint("key-2");
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn generated_fingerprint_matches_current_identity_profile() {
        let fp = generate_fingerprint("test-key");
        let map = header_fingerprint_from_fingerprint(&fp).expect("header fingerprint");

        assert_eq!(map["identity_profile_version"], "2026-04");
        assert_eq!(map["cli_version"], "2.1.284");
        assert_eq!(map["billing_cli_version"], "2.1.284");
        assert_eq!(map["stainless_package_version"], "0.112.1");
        assert_eq!(map["stainless_runtime_version"], "v26.3.0");
        assert_eq!(map["user_agent"], "claude-cli/2.1.284 (external, cli)");
        assert_eq!(
            fp["transport_profile"]["extra"]["claude_code_identity_profile_version"],
            "2026-04"
        );
    }

    #[test]
    fn sanitize_upgrades_stale_identity_as_one_version_set() {
        let raw = serde_json::json!({
            "transport_profile": {
                "profile_id": "claude_code_nodejs",
                "header_fingerprint": {
                    "stainless_package_version": "0.68.0",
                    "stainless_runtime_version": "v20.18.1",
                    "user_agent": "Mozilla/5.0 stale",
                    "vscode_session_id": "existing-session"
                }
            }
        });

        let sanitized = sanitize_fingerprint(&raw, "test-key");
        let map = header_fingerprint_from_fingerprint(&sanitized).expect("header fingerprint");
        assert_eq!(map["stainless_package_version"], "0.112.1");
        assert_eq!(map["stainless_runtime_version"], "v26.3.0");
        assert_eq!(map["user_agent"], "claude-cli/2.1.284 (external, cli)");
        assert_eq!(map["vscode_session_id"], "existing-session");
    }

    #[test]
    fn random_fingerprint_differs_each_call() {
        let fp1 = generate_random_fingerprint();
        let fp2 = generate_random_fingerprint();
        assert_ne!(fp1, fp2);
    }
}
