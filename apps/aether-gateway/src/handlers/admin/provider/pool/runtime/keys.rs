use sha2::{Digest, Sha256};

pub(super) fn pool_sticky_pattern(provider_id: &str) -> String {
    format!("ap:{provider_id}:sticky:*")
}

pub(super) fn pool_sticky_key(provider_id: &str, session_token: &str) -> String {
    let digest = Sha256::digest(
        format!(
            "aether-provider-pool-sticky-v1\0{}\0{}",
            provider_id.trim(),
            session_token.trim()
        )
        .as_bytes(),
    );
    format!("ap:{provider_id}:sticky:v1:{digest:x}")
}

pub(super) fn pool_lru_key(provider_id: &str) -> String {
    format!("ap:{provider_id}:lru")
}

pub(super) fn pool_cooldown_key(provider_id: &str, key_id: &str) -> String {
    format!("ap:{provider_id}:cooldown:{key_id}")
}

pub(super) fn pool_cooldown_index_key(provider_id: &str) -> String {
    format!("ap:{provider_id}:cooldown_idx")
}

pub(super) fn pool_cost_key(provider_id: &str, key_id: &str) -> String {
    format!("ap:{provider_id}:cost:{key_id}")
}

pub(super) fn pool_latency_key(provider_id: &str, key_id: &str) -> String {
    format!("ap:{provider_id}:latency:{key_id}")
}

pub(super) fn pool_stream_timeout_key(provider_id: &str, key_id: &str) -> String {
    format!("ap:{provider_id}:stream_timeout:{key_id}")
}

pub(super) fn pool_cooldown_keys(provider_id: &str, key_ids: &[String]) -> Vec<String> {
    key_ids
        .iter()
        .map(|key_id| pool_cooldown_key(provider_id, key_id))
        .collect()
}

pub(super) fn pool_cost_keys(provider_id: &str, key_ids: &[String]) -> Vec<String> {
    key_ids
        .iter()
        .map(|key_id| pool_cost_key(provider_id, key_id))
        .collect()
}

pub(super) fn pool_latency_keys(provider_id: &str, key_ids: &[String]) -> Vec<String> {
    key_ids
        .iter()
        .map(|key_id| pool_latency_key(provider_id, key_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::pool_sticky_key;

    #[test]
    fn sticky_key_does_not_expose_client_session_identifier() {
        let session_token = "customer@example.test/private-conversation";
        let key = pool_sticky_key("provider-a", session_token);

        assert_eq!(key.len(), "ap:provider-a:sticky:v1:".len() + 64);
        assert!(!key.contains(session_token));
        assert_eq!(key, pool_sticky_key("provider-a", session_token));
        assert_ne!(key, pool_sticky_key("provider-b", session_token));
    }
}
