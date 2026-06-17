use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::SystemTime;

pub const OPENCODE_FREE_USER_AGENT: &str = "opencode/1.15.5";
const OPENCODE_FREE_CLIENT: &str = "cli";
const BASE62_CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

static PROCESS_SESSION_ID: LazyLock<String> =
    LazyLock::new(|| generate_prefixed_id("ses_"));

/// Monotonic counter to guarantee uniqueness within the same millisecond.
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn current_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_millis()
}

fn hex_timestamp_ms() -> String {
    let ms = current_time_ms();
    format!("{:012x}", ms & 0xFFFF_FFFF_FFFF)
}

fn base62_from_u64(mut seed: u64) -> String {
    let mut result = String::with_capacity(14);
    for i in 0..14 {
        // xorshift64
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let idx = ((seed.wrapping_add(i as u64)) % 62) as usize;
        result.push(BASE62_CHARS[idx] as char);
    }
    result
}

fn base62_random_14() -> String {
    let time = current_time_ms() as u64;
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let addr = &counter as *const u64 as u64;
    let seed = time.wrapping_add(counter).wrapping_add(addr);
    base62_from_u64(seed)
}

fn generate_prefixed_id(prefix: &str) -> String {
    format!("{}{}{}", prefix, hex_timestamp_ms(), base62_random_14())
}

pub fn generate_session_id() -> &'static str {
    &PROCESS_SESSION_ID
}

pub fn generate_request_id() -> String {
    generate_prefixed_id("msg_")
}

pub fn build_opencode_free_headers() -> BTreeMap<String, String> {
    let session_id = generate_session_id();
    let request_id = generate_request_id();

    BTreeMap::from([
        (
            String::from("x-opencode-session"),
            session_id.to_string(),
        ),
        (
            String::from("x-opencode-request"),
            request_id,
        ),
        (
            String::from("User-Agent"),
            String::from(OPENCODE_FREE_USER_AGENT),
        ),
        (
            String::from("x-opencode-client"),
            String::from(OPENCODE_FREE_CLIENT),
        ),
        (
            String::from("Content-Type"),
            String::from("application/json"),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_id_format() {
        let id = generate_session_id();
        assert!(id.starts_with("ses_"), "session ID should start with ses_, got: {id}");
        let body = &id[4..];
        assert_eq!(body.len(), 26, "session ID body should be 26 chars, got {}: len={}", body, body.len());
        let (hex_part, base62_part) = body.split_at(12);
        assert!(
            hex_part.chars().all(|c| c.is_ascii_hexdigit()),
            "hex timestamp part should be hex: {hex_part}"
        );
        assert!(
            base62_part.chars().all(|c| c.is_ascii_alphanumeric()),
            "random part should be base62: {base62_part}"
        );
    }

    #[test]
    fn test_request_id_format() {
        let id = generate_request_id();
        assert!(id.starts_with("msg_"), "request ID should start with msg_, got: {id}");
        let body = &id[4..];
        assert_eq!(body.len(), 26, "request ID body should be 26 chars, got {}: len={}", body, body.len());
        let (hex_part, base62_part) = body.split_at(12);
        assert!(
            hex_part.chars().all(|c| c.is_ascii_hexdigit()),
            "hex timestamp part should be hex: {hex_part}"
        );
        assert!(
            base62_part.chars().all(|c| c.is_ascii_alphanumeric()),
            "random part should be base62: {base62_part}"
        );
    }

    #[test]
    fn test_request_ids_are_unique() {
        let id1 = generate_request_id();
        let id2 = generate_request_id();
        assert_ne!(id1, id2, "consecutive request IDs should be unique");
    }

    #[test]
    fn test_session_id_is_stable() {
        let id1 = generate_session_id();
        let id2 = generate_session_id();
        assert_eq!(id1, id2, "session ID should be stable within process");
    }

    #[test]
    fn test_no_authorization_header() {
        let headers = build_opencode_free_headers();
        let has_auth = headers
            .keys()
            .any(|k| k.eq_ignore_ascii_case("authorization"));
        assert!(
            !has_auth,
            "headers should NOT contain authorization, got keys: {:?}",
            headers.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_headers_contain_expected_keys() {
        let headers = build_opencode_free_headers();
        assert!(headers.contains_key("x-opencode-session"));
        assert!(headers.contains_key("x-opencode-request"));
        assert_eq!(headers.get("User-Agent").map(String::as_str), Some("opencode/1.15.5"));
        assert_eq!(headers.get("x-opencode-client").map(String::as_str), Some("cli"));
        assert_eq!(headers.get("Content-Type").map(String::as_str), Some("application/json"));
    }
}
