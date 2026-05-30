use aether_data_contracts::repository::pool_scores::{
    PoolMemberIdentity, PoolMemberProbeStatus, PoolScoreScope, UpsertPoolMemberScore,
    POOL_SCORE_CAPABILITY_ACCOUNT, POOL_SCORE_SCOPE_KIND_ACCOUNT,
};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use aether_pool_core::{
    score_pool_member_with_rules, PoolMemberScoreInput, PoolMemberScoreRules, POOL_SCORE_VERSION,
};
use serde_json::Value;

use crate::handlers::shared::{provider_key_health_summary, provider_key_status_snapshot_payload};

pub(crate) fn build_provider_key_pool_score_upsert(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    existing: Option<&aether_data_contracts::repository::pool_scores::StoredPoolMemberScore>,
    now_unix_secs: u64,
    score_rules: PoolMemberScoreRules,
) -> UpsertPoolMemberScore {
    let identity = PoolMemberIdentity::provider_api_key(key.provider_id.clone(), key.id.clone());
    let scope = provider_key_pool_score_scope();
    let input = provider_key_score_input(
        key,
        provider_type,
        identity.clone(),
        scope.clone(),
        existing,
        now_unix_secs,
    );
    let output = score_pool_member_with_rules(&input, score_rules);
    UpsertPoolMemberScore {
        id: provider_key_pool_score_id(&identity, &scope),
        identity,
        scope,
        score: output.score,
        hard_state: output.hard_state,
        score_version: POOL_SCORE_VERSION,
        score_reason: output.score_reason,
        last_ranked_at: Some(now_unix_secs),
        last_scheduled_at: existing.and_then(|score| score.last_scheduled_at),
        last_success_at: existing.and_then(|score| score.last_success_at),
        last_failure_at: existing.and_then(|score| score.last_failure_at),
        failure_count: existing.map(|score| score.failure_count).unwrap_or(0),
        last_probe_attempt_at: existing.and_then(|score| score.last_probe_attempt_at),
        last_probe_success_at: existing.and_then(|score| score.last_probe_success_at),
        last_probe_failure_at: existing.and_then(|score| score.last_probe_failure_at),
        probe_failure_count: existing.map(|score| score.probe_failure_count).unwrap_or(0),
        probe_status: existing
            .map(|score| score.probe_status)
            .unwrap_or(PoolMemberProbeStatus::Never),
        updated_at: now_unix_secs,
    }
}

pub(crate) fn provider_key_pool_score_scope() -> PoolScoreScope {
    PoolScoreScope {
        capability: POOL_SCORE_CAPABILITY_ACCOUNT.to_string(),
        scope_kind: POOL_SCORE_SCOPE_KIND_ACCOUNT.to_string(),
        scope_id: None,
    }
}

pub(crate) fn provider_key_pool_score_id(
    identity: &PoolMemberIdentity,
    scope: &PoolScoreScope,
) -> String {
    let raw = format!(
        "{}:{}:{}:{}:{}:{}:{}",
        identity.pool_kind,
        identity.pool_id,
        identity.member_kind,
        identity.member_id,
        scope.capability,
        scope.scope_kind,
        scope.scope_id.as_deref().unwrap_or("*")
    );
    format!(
        "pms-{:016x}-{:016x}",
        stable_hash(raw.as_bytes()),
        stable_hash(identity.member_id.as_bytes())
    )
}

fn provider_key_score_input(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    identity: PoolMemberIdentity,
    scope: PoolScoreScope,
    existing: Option<&aether_data_contracts::repository::pool_scores::StoredPoolMemberScore>,
    now_unix_secs: u64,
) -> PoolMemberScoreInput {
    let status_snapshot = provider_key_status_snapshot_payload(key, provider_type);
    let quota_snapshot = status_snapshot
        .as_object()
        .and_then(|snapshot| snapshot.get("quota"))
        .and_then(Value::as_object);
    let account_snapshot = status_snapshot
        .as_object()
        .and_then(|snapshot| snapshot.get("account"))
        .and_then(Value::as_object);
    let (health_score, _, _, _, _) = provider_key_health_summary(key);
    let health_score = key
        .health_by_format
        .as_ref()
        .and_then(Value::as_object)
        .filter(|payload| !payload.is_empty())
        .map(|_| health_score);

    PoolMemberScoreInput {
        identity,
        scope: scope.clone(),
        internal_priority: key.internal_priority,
        is_active: key.is_active,
        health_score,
        quota_usage_ratio: quota_snapshot
            .and_then(|quota| quota.get("usage_ratio"))
            .and_then(json_f64)
            .map(|value| value.clamp(0.0, 1.0)),
        quota_exhausted: quota_snapshot
            .and_then(|quota| quota.get("exhausted"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        account_blocked: account_snapshot
            .and_then(|account| account.get("blocked"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        oauth_invalid_reason: key.oauth_invalid_reason.clone(),
        success_count: key.success_count.unwrap_or(0).into(),
        error_count: key.error_count.unwrap_or(0).into(),
        total_response_time_ms: key.total_response_time_ms.unwrap_or(0).into(),
        total_tokens: key.total_tokens,
        total_cost_usd: key.total_cost_usd,
        last_used_at: key.last_used_at_unix_secs,
        last_probe_success_at: existing.and_then(|score| score.last_probe_success_at),
        probe_failure_count: existing.map(|score| score.probe_failure_count).unwrap_or(0),
        probe_status: existing
            .map(|score| score.probe_status)
            .unwrap_or(PoolMemberProbeStatus::Never),
        now_unix_secs,
    }
}

fn json_f64(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| {
        value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse::<f64>().ok())
    })
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_data_contracts::repository::pool_scores::PoolMemberHardState;
    use serde_json::json;

    fn sample_key_with_circuit_next_probe(
        next_probe_at_unix_secs: u64,
    ) -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            "key-gemini-5".to_string(),
            "provider-google-api".to_string(),
            "5".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("sample key should be valid");
        key.health_by_format = Some(json!({
            "gemini:generate_content": {
                "health_score": 0.2,
                "consecutive_failures": 8
            }
        }));
        key.circuit_breaker_by_format = Some(json!({
            "gemini:generate_content": {
                "open": true,
                "reason": "consecutive_failures_8",
                "next_probe_at_unix_secs": next_probe_at_unix_secs
            }
        }));
        key
    }

    #[test]
    fn expired_circuit_probe_deadline_does_not_leave_pool_score_in_cooldown() {
        let now_unix_secs = 1_000;
        let key = sample_key_with_circuit_next_probe(900);

        let score = build_provider_key_pool_score_upsert(
            &key,
            "custom",
            None,
            now_unix_secs,
            PoolMemberScoreRules::default(),
        );

        assert_eq!(score.hard_state, PoolMemberHardState::Available);
    }

    #[test]
    fn future_key_circuit_probe_deadline_does_not_drive_pool_score_cooldown() {
        let now_unix_secs = 1_000;
        let key = sample_key_with_circuit_next_probe(1_100);

        let score = build_provider_key_pool_score_upsert(
            &key,
            "custom",
            None,
            now_unix_secs,
            PoolMemberScoreRules::default(),
        );

        assert_eq!(score.hard_state, PoolMemberHardState::Available);
    }
}
