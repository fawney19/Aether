use aether_ai_serving::{AiExecutionAttempt, STICKY_KEY_ATTEMPTS_REPORT_FIELD};
use aether_routing_core::DEFAULT_STICKY_KEY_ATTEMPTS;
use aether_runtime_state::RuntimeLockLease;
use aether_scheduler_core::parse_request_candidate_report_context;
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExecutionAttemptIdentity {
    pub(crate) candidate_index: u32,
    pub(crate) retry_index: u32,
    pub(crate) pool_key_index: Option<u32>,
}

impl ExecutionAttemptIdentity {
    pub(crate) const fn new(candidate_index: u32, retry_index: u32) -> Self {
        Self {
            candidate_index,
            retry_index,
            pool_key_index: None,
        }
    }

    pub(crate) const fn with_pool_key_index(mut self, pool_key_index: Option<u32>) -> Self {
        self.pool_key_index = pool_key_index;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LocalExecutionCandidateMetadata {
    pub(crate) candidate_group_id: Option<String>,
    pub(crate) pool_key_index: Option<u32>,
    pub(crate) pool_key_lease: Option<RuntimeLockLease>,
    pub(crate) scheduler_affinity_epoch: Option<u64>,
    /// Routing-policy `sticky_key_attempts` in effect for this request. `None`
    /// means the policy default applies.
    pub(crate) sticky_key_attempts: Option<u32>,
}

pub(crate) const SCHEDULER_AFFINITY_EPOCH_REPORT_FIELD: &str = "scheduler_affinity_epoch";
pub(crate) const ROUTING_POOL_POLICY_OVERRIDE_REPORT_FIELD: &str = "routing_pool_policy_override";
pub(crate) const POOL_KEY_LEASE_KEY_REPORT_FIELD: &str = "pool_key_lease_key";
pub(crate) const POOL_KEY_LEASE_OWNER_REPORT_FIELD: &str = "pool_key_lease_owner";
pub(crate) const POOL_KEY_LEASE_TOKEN_REPORT_FIELD: &str = "pool_key_lease_token";
pub(crate) const POOL_KEY_LEASE_FENCING_REPORT_FIELD: &str = "pool_key_lease_fencing_token";
pub(crate) const POOL_KEY_LEASE_TTL_MS_REPORT_FIELD: &str = "pool_key_lease_ttl_ms";

/// Pool-expanded keys encode `pool_key_index * STRIDE + retry_index` into the
/// persisted `retry_index` so a pool group's keys stay ordered in one candidate
/// slot. Same-key retries on a pool key are therefore bounded by the stride.
pub(crate) const POOL_KEY_RETRY_INDEX_STRIDE: u32 = 100;

pub(crate) fn attempt_identity_from_report_context(
    report_context: Option<&Value>,
) -> Option<ExecutionAttemptIdentity> {
    let metadata = parse_request_candidate_report_context(report_context)?;
    let candidate_metadata = local_execution_candidate_metadata_from_report_context(report_context);

    Some(ExecutionAttemptIdentity {
        candidate_index: metadata.candidate_index?,
        retry_index: metadata.retry_index,
        pool_key_index: candidate_metadata.pool_key_index,
    })
}

pub(crate) fn local_execution_candidate_metadata_from_report_context(
    report_context: Option<&Value>,
) -> LocalExecutionCandidateMetadata {
    LocalExecutionCandidateMetadata {
        candidate_group_id: report_context
            .and_then(Value::as_object)
            .and_then(|value| value.get("candidate_group_id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        pool_key_index: report_context
            .and_then(|value| value.get("pool_key_index"))
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
        pool_key_lease: pool_key_lease_from_report_context(report_context),
        scheduler_affinity_epoch: report_context
            .and_then(|value| value.get(SCHEDULER_AFFINITY_EPOCH_REPORT_FIELD))
            .and_then(Value::as_u64),
        sticky_key_attempts: report_context
            .and_then(|value| value.get(STICKY_KEY_ATTEMPTS_REPORT_FIELD))
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
    }
}

pub(crate) fn insert_pool_key_lease_report_context_fields(
    extra_fields: &mut serde_json::Map<String, Value>,
    lease: Option<&RuntimeLockLease>,
) {
    let Some(lease) = lease else {
        return;
    };
    extra_fields.insert(
        POOL_KEY_LEASE_KEY_REPORT_FIELD.to_string(),
        Value::String(lease.key.clone()),
    );
    extra_fields.insert(
        POOL_KEY_LEASE_OWNER_REPORT_FIELD.to_string(),
        Value::String(lease.owner.clone()),
    );
    extra_fields.insert(
        POOL_KEY_LEASE_TOKEN_REPORT_FIELD.to_string(),
        Value::String(lease.token.clone()),
    );
    extra_fields.insert(
        POOL_KEY_LEASE_FENCING_REPORT_FIELD.to_string(),
        Value::Number(lease.fencing_token.into()),
    );
    extra_fields.insert(
        POOL_KEY_LEASE_TTL_MS_REPORT_FIELD.to_string(),
        Value::Number(lease.ttl_ms.into()),
    );
}

fn pool_key_lease_from_report_context(report_context: Option<&Value>) -> Option<RuntimeLockLease> {
    let report_context = report_context?;
    let key = report_context
        .get(POOL_KEY_LEASE_KEY_REPORT_FIELD)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let owner = report_context
        .get(POOL_KEY_LEASE_OWNER_REPORT_FIELD)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let token = report_context
        .get(POOL_KEY_LEASE_TOKEN_REPORT_FIELD)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let ttl_ms = report_context
        .get(POOL_KEY_LEASE_TTL_MS_REPORT_FIELD)
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)?;
    let fencing_token = report_context
        .get(POOL_KEY_LEASE_FENCING_REPORT_FIELD)
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .unwrap_or(1);

    Some(RuntimeLockLease {
        key: key.to_string(),
        owner: owner.to_string(),
        token: token.to_string(),
        fencing_token,
        ttl_ms,
    })
}

/// Retry index of the next same-key attempt, or `None` when the sticky-key
/// budget for this candidate is used up.
///
/// Only the first-ranked candidate (index `0`, the cache-affinity sticky key)
/// is retried on the same key; every later candidate gets exactly one attempt
/// so that once failover has started it keeps advancing. `sticky_key_attempts`
/// is the *total* attempt count on that key: `2` means one retry, `0` and `1`
/// mean none. There is no upper bound: attempts are derived one at a time
/// after each failure, never materialized ahead of time.
///
/// Inside a pool group only the first key (`pool_key_index == 0`) is treated
/// as sticky, and its retries stay below `POOL_KEY_RETRY_INDEX_STRIDE` so the
/// encoded retry index never collides with the next pool key.
pub(crate) fn next_same_key_retry_index(
    identity: ExecutionAttemptIdentity,
    sticky_key_attempts: Option<u32>,
) -> Option<u32> {
    if identity.candidate_index != 0 {
        return None;
    }
    let pool_limit = match identity.pool_key_index {
        None => u32::MAX,
        Some(0) => POOL_KEY_RETRY_INDEX_STRIDE,
        Some(_) => return None,
    };
    let budget = sticky_key_attempts.unwrap_or(DEFAULT_STICKY_KEY_ATTEMPTS);
    let attempts_so_far = identity.retry_index.checked_add(1)?;
    if attempts_so_far >= budget || attempts_so_far >= pool_limit {
        return None;
    }
    Some(attempts_so_far)
}

/// Derive the next same-key attempt for `attempt` after a candidate-scoped
/// failure, reading the attempt identity and sticky budget from its report
/// context. Returns `None` when no further same-key retry is allowed.
pub(crate) fn next_same_key_retry_attempt<A: AiExecutionAttempt>(attempt: &A) -> Option<A> {
    let owned_report_context = attempt
        .report_context_ref()
        .is_none()
        .then(|| attempt.report_context())
        .flatten();
    let report_context = attempt
        .report_context_ref()
        .or(owned_report_context.as_ref());
    let identity = attempt_identity_from_report_context(report_context)?;
    let metadata = local_execution_candidate_metadata_from_report_context(report_context);
    let retry_index = next_same_key_retry_index(identity, metadata.sticky_key_attempts)?;
    attempt.with_same_key_retry(retry_index, Uuid::new_v4().to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        attempt_identity_from_report_context,
        local_execution_candidate_metadata_from_report_context, next_same_key_retry_attempt,
        next_same_key_retry_index, ExecutionAttemptIdentity, LocalExecutionCandidateMetadata,
        POOL_KEY_RETRY_INDEX_STRIDE,
    };
    use aether_ai_serving::{AiExecutionAttempt, AiSyncAttempt};
    use aether_runtime_state::RuntimeLockLease;

    #[test]
    fn first_candidate_defaults_to_one_same_key_retry() {
        assert_eq!(
            next_same_key_retry_index(ExecutionAttemptIdentity::new(0, 0), None),
            Some(1)
        );
        assert_eq!(
            next_same_key_retry_index(ExecutionAttemptIdentity::new(0, 1), None),
            None
        );
    }

    #[test]
    fn first_candidate_uses_policy_sticky_key_attempts_without_upper_bound() {
        assert_eq!(
            next_same_key_retry_index(ExecutionAttemptIdentity::new(0, 2), Some(3)),
            None
        );
        assert_eq!(
            next_same_key_retry_index(ExecutionAttemptIdentity::new(0, 1), Some(3)),
            Some(2)
        );
        assert_eq!(
            next_same_key_retry_index(ExecutionAttemptIdentity::new(0, 4_999), Some(10_000)),
            Some(5_000)
        );
    }

    #[test]
    fn zero_and_one_mean_single_attempt() {
        assert_eq!(
            next_same_key_retry_index(ExecutionAttemptIdentity::new(0, 0), Some(0)),
            None
        );
        assert_eq!(
            next_same_key_retry_index(ExecutionAttemptIdentity::new(0, 0), Some(1)),
            None
        );
    }

    #[test]
    fn failover_candidates_never_retry_on_the_same_key() {
        for candidate_index in 1..5 {
            assert_eq!(
                next_same_key_retry_index(ExecutionAttemptIdentity::new(candidate_index, 0), None),
                None
            );
            assert_eq!(
                next_same_key_retry_index(
                    ExecutionAttemptIdentity::new(candidate_index, 0),
                    Some(50)
                ),
                None
            );
        }
    }

    #[test]
    fn pool_groups_only_retry_their_first_key_within_the_stride() {
        let first_pool_key = ExecutionAttemptIdentity::new(0, 0).with_pool_key_index(Some(0));
        assert_eq!(next_same_key_retry_index(first_pool_key, Some(3)), Some(1));

        let at_stride_limit = ExecutionAttemptIdentity::new(0, POOL_KEY_RETRY_INDEX_STRIDE - 1)
            .with_pool_key_index(Some(0));
        assert_eq!(
            next_same_key_retry_index(at_stride_limit, Some(10_000)),
            None
        );

        let second_pool_key = ExecutionAttemptIdentity::new(0, POOL_KEY_RETRY_INDEX_STRIDE)
            .with_pool_key_index(Some(1));
        assert_eq!(
            next_same_key_retry_index(second_pool_key, Some(10_000)),
            None
        );
    }

    #[test]
    fn next_same_key_retry_attempt_rewrites_candidate_id_and_retry_index() {
        let attempt = AiSyncAttempt {
            plan: aether_contracts::ExecutionPlan {
                request_id: "trace-1".to_string(),
                candidate_id: Some("candidate-a".to_string()),
                provider_name: None,
                provider_id: "provider-1".to_string(),
                endpoint_id: "endpoint-1".to_string(),
                key_id: "key-1".to_string(),
                method: "POST".to_string(),
                url: "https://example.com".to_string(),
                headers: Default::default(),
                content_type: None,
                content_encoding: None,
                body: aether_contracts::RequestBody {
                    json_body: None,
                    body_bytes_b64: None,
                    body_ref: None,
                },
                stream: false,
                client_api_format: "openai:chat".to_string(),
                provider_api_format: "openai:chat".to_string(),
                model_name: None,
                proxy: None,
                transport_profile: None,
                timeouts: None,
            },
            report_kind: None,
            report_context: Some(json!({
                "candidate_id": "candidate-a",
                "candidate_index": 0,
                "retry_index": 0,
                "sticky_key_attempts": 2,
            })),
        };

        let retry = next_same_key_retry_attempt(&attempt).expect("one same-key retry remains");
        let retry_candidate_id = retry.plan.candidate_id.clone().expect("fresh candidate id");
        assert_ne!(retry_candidate_id, "candidate-a");
        assert_eq!(retry.plan.key_id, "key-1");
        let context = retry.report_context_ref().expect("context retained");
        assert_eq!(context["candidate_id"], json!(retry_candidate_id));
        assert_eq!(context["retry_index"], json!(1));
        assert_eq!(context["candidate_index"], json!(0));

        assert!(
            next_same_key_retry_attempt(&retry).is_none(),
            "budget of 2 attempts is exhausted after one retry"
        );
    }

    #[test]
    fn parse_attempt_identity_from_report_context_reads_candidate_and_retry_indices() {
        let identity = attempt_identity_from_report_context(Some(&json!({
            "candidate_index": 4,
            "retry_index": 1,
            "pool_key_index": 7,
        })))
        .expect("attempt identity should parse");

        assert_eq!(
            identity,
            ExecutionAttemptIdentity {
                candidate_index: 4,
                retry_index: 1,
                pool_key_index: Some(7),
            }
        );
    }

    #[test]
    fn parse_candidate_metadata_from_report_context_reads_group_and_pool_metadata() {
        let metadata = local_execution_candidate_metadata_from_report_context(Some(&json!({
            "candidate_group_id": "group-1",
            "pool_key_index": 3,
            "pool_key_lease_key": "ap:provider-1:lease:key-1",
            "pool_key_lease_owner": "gateway-1",
            "pool_key_lease_token": "gateway-1:token-1",
            "pool_key_lease_fencing_token": 7,
            "pool_key_lease_ttl_ms": 900000,
            "sticky_key_attempts": 3,
        })));

        assert_eq!(
            metadata,
            LocalExecutionCandidateMetadata {
                candidate_group_id: Some("group-1".to_string()),
                pool_key_index: Some(3),
                pool_key_lease: Some(RuntimeLockLease {
                    key: "ap:provider-1:lease:key-1".to_string(),
                    owner: "gateway-1".to_string(),
                    token: "gateway-1:token-1".to_string(),
                    fencing_token: 7,
                    ttl_ms: 900000,
                }),
                scheduler_affinity_epoch: None,
                sticky_key_attempts: Some(3),
            }
        );
    }
}
