use aether_contracts::{
    TRANSPORT_BACKEND_REQWEST_RUSTLS, TRANSPORT_HTTP_MODE_AUTO, TRANSPORT_POOL_SCOPE_KEY,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

// Chrome impersonate profiles
const CHROME_IMPERSONATE_PROFILES: &[&str] = &[
    "chrome110",
    "chrome116",
    "chrome119",
    "chrome120",
    "chrome123",
    "chrome124",
    "chrome131",
    "chrome133",
];

const CHROME_VERSIONS: &[(&str, &str)] = &[
    ("chrome110", "110.0.5481.177"),
    ("chrome116", "116.0.5845.188"),
    ("chrome119", "119.0.6045.214"),
    ("chrome120", "120.0.6099.216"),
    ("chrome123", "123.0.6312.122"),
    ("chrome124", "124.0.6367.243"),
    ("chrome131", "131.0.6778.265"),
    ("chrome133", "133.0.6943.142"),
];

// (os, arch, platform_token, platform_info)
const PLATFORM_VARIANTS: &[(&str, &str, &str, &str)] = &[
    ("Linux", "x64", "X11; Linux x86_64", "Linux x86_64"),
    ("Linux", "arm64", "X11; Linux arm64", "Linux arm64"),
    (
        "Windows",
        "x64",
        "Windows NT 10.0; Win64; x64",
        "Windows x64",
    ),
    (
        "MacOS",
        "x64",
        "Macintosh; Intel Mac OS X 10_15_7",
        "Darwin x64",
    ),
    (
        "MacOS",
        "arm64",
        "Macintosh; ARM Mac OS X 14_0_0",
        "Darwin arm64",
    ),
];

const STAINLESS_PACKAGE_VERSIONS: &[&str] = &["0.68.0", "0.69.0", "0.70.0", "0.71.0"];
const NODE_VERSIONS: &[&str] = &["v20.18.1", "v22.12.0", "v22.14.0", "v24.13.0"];
const ELECTRON_VERSIONS: &[&str] = &["35.5.1", "36.7.1", "37.3.0", "38.7.0", "39.2.3"];
const STAINLESS_TIMEOUTS: &[&str] = &["600", "900"];
const CLAUDE_CODE_TRANSPORT_PROFILE_ID: &str = "claude_code_nodejs";

/// Deterministic hash-based index picker, compatible with Python implementation.
/// Each `slot` produces a different selection from the same seed.
struct SeededPicker {
    seed_bytes: [u8; 32],
}

impl SeededPicker {
    fn new(seed: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(seed.as_bytes());
        Self {
            seed_bytes: hasher.finalize().into(),
        }
    }

    /// Pick an index from `[0, len)` using a specific slot.
    /// Different slots produce independent-looking selections from the same seed.
    fn pick(&self, slot: u8, len: usize) -> usize {
        if len == 0 {
            return 0;
        }
        // Hash seed_bytes + slot to get a new digest, take first 8 bytes as u64
        let mut hasher = Sha256::new();
        hasher.update(self.seed_bytes);
        hasher.update([slot]);
        let hash = hasher.finalize();
        let value = u64::from_be_bytes(hash[..8].try_into().unwrap());
        (value % len as u64) as usize
    }
}

fn chrome_version_for_profile(profile: &str) -> &'static str {
    for (p, v) in CHROME_VERSIONS {
        if p.eq_ignore_ascii_case(profile) {
            return v;
        }
    }
    "120.0.6099.216"
}

fn build_user_agent(platform_token: &str, chrome_version: &str, electron_version: &str) -> String {
    format!(
        "Mozilla/5.0 ({platform_token}) AppleWebKit/537.36 (KHTML, like Gecko) \
         Chrome/{chrome_version} Electron/{electron_version} Safari/537.36"
    )
}

fn resolve_platform_token(os: &str, arch: &str) -> &'static str {
    let os_lower = os.to_ascii_lowercase();
    let arch_lower = arch.to_ascii_lowercase();

    if os_lower.starts_with("win") {
        return "Windows NT 10.0; Win64; x64";
    }
    if matches!(os_lower.as_str(), "darwin" | "mac" | "macos") {
        return if matches!(arch_lower.as_str(), "arm64" | "aarch64") {
            "Macintosh; ARM Mac OS X 14_0_0"
        } else {
            "Macintosh; Intel Mac OS X 10_15_7"
        };
    }
    if matches!(arch_lower.as_str(), "arm64" | "aarch64") {
        "X11; Linux arm64"
    } else {
        "X11; Linux x86_64"
    }
}

/// Generate a complete Claude Code transport fingerprint from a seed.
pub fn generate_fingerprint(seed: &str) -> Value {
    wrap_header_fingerprint(generate_header_fingerprint(seed))
}

fn generate_header_fingerprint(seed: &str) -> Value {
    let picker = SeededPicker::new(seed);

    let impersonate =
        CHROME_IMPERSONATE_PROFILES[picker.pick(0, CHROME_IMPERSONATE_PROFILES.len())];
    let chrome_version = chrome_version_for_profile(impersonate);
    let node_version = NODE_VERSIONS[picker.pick(1, NODE_VERSIONS.len())];
    let electron_version = ELECTRON_VERSIONS[picker.pick(2, ELECTRON_VERSIONS.len())];
    let platform = PLATFORM_VARIANTS[picker.pick(3, PLATFORM_VARIANTS.len())];
    let (stainless_os, stainless_arch, platform_token, platform_info) = platform;
    let stainless_package_version =
        STAINLESS_PACKAGE_VERSIONS[picker.pick(4, STAINLESS_PACKAGE_VERSIONS.len())];
    let stainless_timeout = STAINLESS_TIMEOUTS[picker.pick(5, STAINLESS_TIMEOUTS.len())];

    let vscode_session_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("aether:fingerprint:{seed}").as_bytes(),
    )
    .simple()
    .to_string();

    let user_agent = build_user_agent(platform_token, chrome_version, electron_version);

    serde_json::json!({
        "impersonate": impersonate,
        "stainless_package_version": stainless_package_version,
        "stainless_os": stainless_os,
        "stainless_arch": stainless_arch,
        "stainless_runtime_version": node_version,
        "stainless_timeout": stainless_timeout,
        "node_version": node_version,
        "chrome_version": chrome_version,
        "electron_version": electron_version,
        "vscode_session_id": vscode_session_id,
        "platform_info": platform_info,
        "user_agent": user_agent,
    })
}

fn wrap_header_fingerprint(header_fingerprint: Value) -> Value {
    serde_json::json!({
        "transport_profile": {
            "profile_id": CLAUDE_CODE_TRANSPORT_PROFILE_ID,
            "backend": TRANSPORT_BACKEND_REQWEST_RUSTLS,
            "http_mode": TRANSPORT_HTTP_MODE_AUTO,
            "pool_scope": TRANSPORT_POOL_SCOPE_KEY,
            "header_fingerprint": header_fingerprint,
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
    let random_seed = Uuid::new_v4().to_string();
    generate_fingerprint(&random_seed)
}

/// Sanitize an existing fingerprint JSON, filling missing fields with
/// deterministic fallbacks derived from `key_id`.
pub fn sanitize_fingerprint(raw: &Value, key_id: &str) -> Value {
    let generated = generate_header_fingerprint(key_id);
    let gen_map = generated.as_object().unwrap();
    let raw_map = header_fingerprint_from_fingerprint(raw);

    let mut out = Map::new();

    // Start with generated values, then overlay non-empty raw values
    for (key, gen_value) in gen_map {
        let value = raw_map
            .and_then(|raw_map| raw_map.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| Value::String(v.to_string()))
            .unwrap_or_else(|| gen_value.clone());
        out.insert(key.clone(), value);
    }

    // Normalize impersonate to known profile
    let impersonate = out
        .get("impersonate")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let is_known = CHROME_IMPERSONATE_PROFILES
        .iter()
        .any(|p| p.eq_ignore_ascii_case(&impersonate));
    if !is_known {
        out.insert("impersonate".to_string(), gen_map["impersonate"].clone());
    }

    // Ensure chrome_version matches impersonate profile
    let profile = out
        .get("impersonate")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let chrome_version = chrome_version_for_profile(profile);
    out.insert(
        "chrome_version".to_string(),
        Value::String(chrome_version.to_string()),
    );

    // Rebuild user_agent if missing
    let has_ua = out
        .get("user_agent")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|v| !v.is_empty());
    if !has_ua {
        let os = out
            .get("stainless_os")
            .and_then(Value::as_str)
            .unwrap_or("Linux");
        let arch = out
            .get("stainless_arch")
            .and_then(Value::as_str)
            .unwrap_or("x64");
        let electron = out
            .get("electron_version")
            .and_then(Value::as_str)
            .unwrap_or("38.7.0");
        let platform_token = resolve_platform_token(os, arch);
        out.insert(
            "user_agent".to_string(),
            Value::String(build_user_agent(platform_token, chrome_version, electron)),
        );
    }

    wrap_header_fingerprint(Value::Object(out))
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
    fn different_seeds_produce_different_fingerprints() {
        let fp1 = generate_fingerprint("key-1");
        let fp2 = generate_fingerprint("key-2");
        // At least one field should differ (statistically near-certain)
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn generated_fingerprint_has_all_fields() {
        let fp = generate_fingerprint("test-key");
        let expected_keys = [
            "impersonate",
            "stainless_package_version",
            "stainless_os",
            "stainless_arch",
            "stainless_runtime_version",
            "stainless_timeout",
            "node_version",
            "chrome_version",
            "electron_version",
            "vscode_session_id",
            "platform_info",
            "user_agent",
        ];
        let map = header_fingerprint_from_fingerprint(&fp).unwrap();
        for key in expected_keys {
            assert!(map.contains_key(key), "missing field: {key}");
            let value = map[key].as_str().unwrap();
            assert!(!value.is_empty(), "empty field: {key}");
        }
    }

    #[test]
    fn sanitize_preserves_user_overrides() {
        let raw = serde_json::json!({
            "transport_profile": {
                "profile_id": "claude_code_nodejs",
                "header_fingerprint": {
                    "stainless_os": "MacOS",
                    "stainless_arch": "arm64",
                    "stainless_timeout": "900",
                    "user_agent": "Custom-Agent/1.0"
                }
            }
        });
        let sanitized = sanitize_fingerprint(&raw, "test-key");
        let map = header_fingerprint_from_fingerprint(&sanitized).unwrap();
        assert_eq!(map["stainless_os"].as_str(), Some("MacOS"));
        assert_eq!(map["stainless_arch"].as_str(), Some("arm64"));
        assert_eq!(map["stainless_timeout"].as_str(), Some("900"));
        assert_eq!(map["user_agent"].as_str(), Some("Custom-Agent/1.0"));
        // Other fields should be filled from generation
        assert!(map.contains_key("impersonate"));
        assert!(map.contains_key("stainless_package_version"));
    }

    #[test]
    fn sanitize_fills_missing_fields_from_seed() {
        let raw = serde_json::json!({});
        let sanitized = sanitize_fingerprint(&raw, "test-key");
        let generated = generate_header_fingerprint("test-key");
        // All fields should match generated since raw is empty
        let s = header_fingerprint_from_fingerprint(&sanitized).unwrap();
        let g = generated.as_object().unwrap();
        for key in g.keys() {
            assert!(s.contains_key(key), "sanitized missing key: {key}");
            assert!(
                !s[key].as_str().unwrap().is_empty(),
                "sanitized empty key: {key}"
            );
        }
    }

    #[test]
    fn sanitize_normalizes_unknown_impersonate_profile() {
        let raw = serde_json::json!({
            "transport_profile": {
                "profile_id": "claude_code_nodejs",
                "header_fingerprint": {
                    "impersonate": "firefox99"
                }
            }
        });
        let sanitized = sanitize_fingerprint(&raw, "test-key");
        let profile = header_fingerprint_from_fingerprint(&sanitized).unwrap()["impersonate"]
            .as_str()
            .unwrap();
        assert!(
            CHROME_IMPERSONATE_PROFILES
                .iter()
                .any(|p| p.eq_ignore_ascii_case(profile)),
            "should normalize to known profile, got: {profile}"
        );
    }

    #[test]
    fn random_fingerprint_differs_each_call() {
        let fp1 = generate_random_fingerprint();
        let fp2 = generate_random_fingerprint();
        assert_ne!(fp1, fp2);
    }
}
