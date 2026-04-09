use aether_data::redis::RedisKeyspace;

pub(super) fn pool_sticky_pattern(keyspace: &RedisKeyspace, provider_id: &str) -> String {
    keyspace.key(&format!("ap:{provider_id}:sticky:*"))
}

pub(super) fn pool_lru_key(keyspace: &RedisKeyspace, provider_id: &str) -> String {
    keyspace.key(&format!("ap:{provider_id}:lru"))
}

pub(super) fn pool_cooldown_key(
    keyspace: &RedisKeyspace,
    provider_id: &str,
    key_id: &str,
) -> String {
    keyspace.key(&format!("ap:{provider_id}:cooldown:{key_id}"))
}

pub(super) fn pool_cooldown_index_key(keyspace: &RedisKeyspace, provider_id: &str) -> String {
    keyspace.key(&format!("ap:{provider_id}:cooldown_idx"))
}

pub(super) fn pool_cost_key(keyspace: &RedisKeyspace, provider_id: &str, key_id: &str) -> String {
    keyspace.key(&format!("ap:{provider_id}:cost:{key_id}"))
}

pub(super) fn parse_pool_cost_member(member: &str) -> u64 {
    member
        .rsplit_once(':')
        .and_then(|(_, suffix)| suffix.parse::<u64>().ok())
        .unwrap_or(0)
}

pub(super) fn pool_cooldown_keys(
    keyspace: &RedisKeyspace,
    provider_id: &str,
    key_ids: &[String],
) -> Vec<String> {
    key_ids
        .iter()
        .map(|key_id| pool_cooldown_key(keyspace, provider_id, key_id))
        .collect()
}

pub(super) fn pool_cost_keys(
    keyspace: &RedisKeyspace,
    provider_id: &str,
    key_ids: &[String],
) -> Vec<String> {
    key_ids
        .iter()
        .map(|key_id| pool_cost_key(keyspace, provider_id, key_id))
        .collect()
}
