use aether_data_contracts::repository::pool_scores::{
    PoolMemberHardState, PoolMemberIdentity, PoolMemberProbeStatus, PoolScoreScope,
};
use serde_json::{json, Value};

pub const POOL_SCORE_VERSION: u64 = 1;
pub const PROBE_FRESHNESS_TTL_SECONDS: u64 = 30 * 60;
pub const UNSCHEDULABLE_SCORE_CAP: f64 = 0.05;
pub const PROBE_FAILURE_PENALTY: f64 = 0.05;
pub const REQUEST_FAILURE_PENALTY: f64 = 0.005;
pub const PROBE_FAILURE_COOLDOWN_THRESHOLD: u64 = 3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoolMemberScoreWeights {
    pub manual_priority: f64,
    pub health: f64,
    pub probe_freshness: f64,
    pub quota_remaining: f64,
    pub latency: f64,
    pub cost_lru: f64,
}

impl Default for PoolMemberScoreWeights {
    fn default() -> Self {
        Self {
            manual_priority: 0.30,
            health: 0.20,
            probe_freshness: 0.15,
            quota_remaining: 0.15,
            latency: 0.10,
            cost_lru: 0.10,
        }
    }
}

impl PoolMemberScoreWeights {
    pub fn normalized(self) -> Self {
        let sanitized = Self {
            manual_priority: finite_non_negative(self.manual_priority),
            health: finite_non_negative(self.health),
            probe_freshness: finite_non_negative(self.probe_freshness),
            quota_remaining: finite_non_negative(self.quota_remaining),
            latency: finite_non_negative(self.latency),
            cost_lru: finite_non_negative(self.cost_lru),
        };
        let total = sanitized.manual_priority
            + sanitized.health
            + sanitized.probe_freshness
            + sanitized.quota_remaining
            + sanitized.latency
            + sanitized.cost_lru;
        if total <= f64::EPSILON {
            return sanitized;
        }
        Self {
            manual_priority: sanitized.manual_priority / total,
            health: sanitized.health / total,
            probe_freshness: sanitized.probe_freshness / total,
            quota_remaining: sanitized.quota_remaining / total,
            latency: sanitized.latency / total,
            cost_lru: sanitized.cost_lru / total,
        }
    }

    fn as_reason_json(self) -> Value {
        json!({
            "manual_priority": self.manual_priority,
            "health": self.health,
            "probe_freshness": self.probe_freshness,
            "quota_remaining": self.quota_remaining,
            "latency": self.latency,
            "cost_lru": self.cost_lru
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoolMemberScoreRules {
    pub weights: PoolMemberScoreWeights,
    pub probe_freshness_ttl_seconds: u64,
    pub unschedulable_score_cap: f64,
    pub probe_failure_penalty: f64,
    pub request_failure_penalty: f64,
    pub probe_failure_cooldown_threshold: u64,
}

impl Default for PoolMemberScoreRules {
    fn default() -> Self {
        Self {
            weights: PoolMemberScoreWeights::default(),
            probe_freshness_ttl_seconds: PROBE_FRESHNESS_TTL_SECONDS,
            unschedulable_score_cap: UNSCHEDULABLE_SCORE_CAP,
            probe_failure_penalty: PROBE_FAILURE_PENALTY,
            request_failure_penalty: REQUEST_FAILURE_PENALTY,
            probe_failure_cooldown_threshold: PROBE_FAILURE_COOLDOWN_THRESHOLD,
        }
    }
}

impl PoolMemberScoreRules {
    pub fn effective(self) -> Self {
        let defaults = Self::default();
        Self {
            weights: self.weights.normalized(),
            probe_freshness_ttl_seconds: if self.probe_freshness_ttl_seconds == 0 {
                defaults.probe_freshness_ttl_seconds
            } else {
                self.probe_freshness_ttl_seconds
            },
            unschedulable_score_cap: if self.unschedulable_score_cap.is_finite() {
                self.unschedulable_score_cap.clamp(0.0, 1.0)
            } else {
                defaults.unschedulable_score_cap
            },
            probe_failure_penalty: if self.probe_failure_penalty.is_finite() {
                self.probe_failure_penalty.clamp(0.0, 1.0)
            } else {
                defaults.probe_failure_penalty
            },
            request_failure_penalty: if self.request_failure_penalty.is_finite() {
                self.request_failure_penalty.clamp(0.0, 1.0)
            } else {
                defaults.request_failure_penalty
            },
            probe_failure_cooldown_threshold: self.probe_failure_cooldown_threshold,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PoolMemberScoreInput {
    pub identity: PoolMemberIdentity,
    pub scope: PoolScoreScope,
    pub internal_priority: i32,
    pub is_active: bool,
    pub health_score: Option<f64>,
    pub quota_usage_ratio: Option<f64>,
    pub quota_exhausted: bool,
    pub account_blocked: bool,
    pub oauth_invalid_reason: Option<String>,
    pub success_count: u64,
    pub error_count: u64,
    pub total_response_time_ms: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub last_used_at: Option<u64>,
    pub last_probe_success_at: Option<u64>,
    pub probe_failure_count: u64,
    pub probe_status: PoolMemberProbeStatus,
    pub now_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PoolMemberScoreOutput {
    pub score: f64,
    pub hard_state: PoolMemberHardState,
    pub score_reason: Value,
}

pub fn score_pool_member(input: &PoolMemberScoreInput) -> PoolMemberScoreOutput {
    score_pool_member_with_rules(input, PoolMemberScoreRules::default())
}

pub fn score_pool_member_with_rules(
    input: &PoolMemberScoreInput,
    rules: PoolMemberScoreRules,
) -> PoolMemberScoreOutput {
    let rules = rules.effective();
    let weights = rules.weights;
    let hard_state = derive_hard_state(input, &rules);
    let manual_priority = manual_priority_score(input.internal_priority);
    let health = input.health_score.unwrap_or(0.5).clamp(0.0, 1.0);
    let probe_freshness = probe_freshness_score_with_ttl(
        input.last_probe_success_at,
        input.probe_status,
        input.now_unix_secs,
        rules.probe_freshness_ttl_seconds,
    );
    let quota_remaining = input
        .quota_usage_ratio
        .map(|ratio| 1.0 - ratio.clamp(0.0, 1.0))
        .unwrap_or(0.5);
    let latency = latency_score(input.success_count, input.total_response_time_ms);
    let cost_lru = cost_lru_score(input.total_cost_usd, input.total_tokens, input.last_used_at);

    let weighted_score = manual_priority * weights.manual_priority
        + health * weights.health
        + probe_freshness * weights.probe_freshness
        + quota_remaining * weights.quota_remaining
        + latency * weights.latency
        + cost_lru * weights.cost_lru;
    let probe_failure_penalty =
        (input.probe_failure_count.min(10) as f64 * rules.probe_failure_penalty).min(0.5);
    let request_failure_penalty =
        (input.error_count.min(20) as f64 * rules.request_failure_penalty).min(0.5);
    let total_penalty = (probe_failure_penalty + request_failure_penalty).min(1.0);
    let mut score = weighted_score - total_penalty;
    if !hard_state.schedulable() {
        score = score.min(rules.unschedulable_score_cap);
    }
    score = score.clamp(0.0, 1.0);

    PoolMemberScoreOutput {
        score,
        hard_state,
        score_reason: json!({
            "weights": weights.as_reason_json(),
            "factors": {
                "manual_priority": manual_priority,
                "health": health,
                "probe_freshness": probe_freshness,
                "quota_remaining": quota_remaining,
                "latency": latency,
                "cost_lru": cost_lru
            },
            "rules": {
                "probe_freshness_ttl_seconds": rules.probe_freshness_ttl_seconds,
                "unschedulable_score_cap": rules.unschedulable_score_cap,
                "probe_failure_penalty": rules.probe_failure_penalty,
                "request_failure_penalty": rules.request_failure_penalty,
                "probe_failure_cooldown_threshold": rules.probe_failure_cooldown_threshold
            },
            "penalties": {
                "probe_failure": probe_failure_penalty,
                "request_failure": request_failure_penalty,
                "total": total_penalty
            },
            "hard_state": hard_state.as_database(),
            "score_version": POOL_SCORE_VERSION
        }),
    }
}

fn finite_non_negative(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn derive_hard_state(
    input: &PoolMemberScoreInput,
    rules: &PoolMemberScoreRules,
) -> PoolMemberHardState {
    if !input.is_active {
        return PoolMemberHardState::Inactive;
    }
    if let Some(reason) = input.oauth_invalid_reason.as_deref() {
        let reason = reason.to_ascii_lowercase();
        if reason.contains("ban") || reason.contains("blocked") || reason.contains("suspended") {
            return PoolMemberHardState::Banned;
        }
        return PoolMemberHardState::AuthInvalid;
    }
    if input.account_blocked {
        return PoolMemberHardState::Banned;
    }
    if input.quota_exhausted {
        return PoolMemberHardState::QuotaExhausted;
    }
    if input.probe_status == PoolMemberProbeStatus::Failed
        && rules.probe_failure_cooldown_threshold > 0
        && input.probe_failure_count >= rules.probe_failure_cooldown_threshold
    {
        return PoolMemberHardState::Cooldown;
    }
    if input.health_score.is_some() || input.probe_status == PoolMemberProbeStatus::Ok {
        PoolMemberHardState::Available
    } else {
        PoolMemberHardState::Unknown
    }
}

fn manual_priority_score(internal_priority: i32) -> f64 {
    (1.0 - (f64::from(internal_priority).clamp(0.0, 100.0) / 100.0)).clamp(0.0, 1.0)
}

pub fn probe_freshness_score(
    last_probe_success_at: Option<u64>,
    probe_status: PoolMemberProbeStatus,
    now_unix_secs: u64,
) -> f64 {
    probe_freshness_score_with_ttl(
        last_probe_success_at,
        probe_status,
        now_unix_secs,
        PROBE_FRESHNESS_TTL_SECONDS,
    )
}

pub fn probe_freshness_score_with_ttl(
    last_probe_success_at: Option<u64>,
    probe_status: PoolMemberProbeStatus,
    now_unix_secs: u64,
    ttl_seconds: u64,
) -> f64 {
    if probe_status != PoolMemberProbeStatus::Ok {
        return 0.0;
    }
    let Some(success_at) = last_probe_success_at else {
        return 0.0;
    };
    let ttl_seconds = ttl_seconds.max(1);
    let age = now_unix_secs.saturating_sub(success_at);
    if age >= ttl_seconds {
        0.0
    } else {
        1.0 - (age as f64 / ttl_seconds as f64)
    }
}

fn latency_score(success_count: u64, total_response_time_ms: u64) -> f64 {
    if success_count == 0 || total_response_time_ms == 0 {
        return 0.5;
    }
    let avg = total_response_time_ms as f64 / success_count as f64;
    if avg <= 500.0 {
        1.0
    } else if avg >= 60_000.0 {
        0.0
    } else {
        1.0 - ((avg - 500.0) / 59_500.0)
    }
}

fn cost_lru_score(total_cost_usd: f64, total_tokens: u64, last_used_at: Option<u64>) -> f64 {
    let cost_penalty = if total_cost_usd.is_finite() {
        (total_cost_usd.max(0.0) / 100.0).min(0.5)
    } else {
        0.0
    };
    let token_penalty = (total_tokens as f64 / 10_000_000.0).min(0.25);
    let lru_bonus = if last_used_at.unwrap_or(0) == 0 {
        0.25
    } else {
        0.0
    };
    (0.75 - cost_penalty - token_penalty + lru_bonus).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_data_contracts::repository::pool_scores::{
        POOL_KIND_PROVIDER_KEY_POOL, POOL_MEMBER_KIND_PROVIDER_API_KEY, POOL_SCORE_SCOPE_KIND_MODEL,
    };

    fn input() -> PoolMemberScoreInput {
        PoolMemberScoreInput {
            identity: PoolMemberIdentity {
                pool_kind: POOL_KIND_PROVIDER_KEY_POOL.to_string(),
                pool_id: "provider-1".to_string(),
                member_kind: POOL_MEMBER_KIND_PROVIDER_API_KEY.to_string(),
                member_id: "key-1".to_string(),
            },
            scope: PoolScoreScope {
                capability: "openai:responses".to_string(),
                scope_kind: POOL_SCORE_SCOPE_KIND_MODEL.to_string(),
                scope_id: Some("model-1".to_string()),
            },
            internal_priority: 10,
            is_active: true,
            health_score: Some(1.0),
            quota_usage_ratio: Some(0.1),
            quota_exhausted: false,
            account_blocked: false,
            oauth_invalid_reason: None,
            success_count: 10,
            error_count: 0,
            total_response_time_ms: 2_000,
            total_tokens: 10,
            total_cost_usd: 0.01,
            last_used_at: None,
            last_probe_success_at: Some(1_000),
            probe_failure_count: 0,
            probe_status: PoolMemberProbeStatus::Ok,
            now_unix_secs: 1_000,
        }
    }

    #[test]
    fn hard_state_caps_unavailable_member_score() {
        let mut input = input();
        input.oauth_invalid_reason = Some("token invalid".to_string());

        let output = score_pool_member(&input);

        assert_eq!(output.hard_state, PoolMemberHardState::AuthInvalid);
        assert!(output.score <= 0.05);
    }

    #[test]
    fn probe_freshness_has_ttl() {
        assert_eq!(
            probe_freshness_score(Some(1_000), PoolMemberProbeStatus::Ok, 1_000),
            1.0
        );
        assert_eq!(
            probe_freshness_score(
                Some(1_000),
                PoolMemberProbeStatus::Ok,
                1_000 + PROBE_FRESHNESS_TTL_SECONDS
            ),
            0.0
        );
    }

    #[test]
    fn custom_rules_change_weights_and_probe_ttl() {
        let mut input = input();
        input.last_probe_success_at = Some(1_000);
        input.now_unix_secs = 1_900;
        let rules = PoolMemberScoreRules {
            weights: PoolMemberScoreWeights {
                manual_priority: 0.0,
                health: 0.0,
                probe_freshness: 1.0,
                quota_remaining: 0.0,
                latency: 0.0,
                cost_lru: 0.0,
            },
            probe_freshness_ttl_seconds: 1_000,
            unschedulable_score_cap: 0.05,
            probe_failure_penalty: 0.0,
            request_failure_penalty: 0.0,
            probe_failure_cooldown_threshold: PROBE_FAILURE_COOLDOWN_THRESHOLD,
        };

        let output = score_pool_member_with_rules(&input, rules);

        assert!((output.score - 0.1).abs() < 0.000_001);
        assert_eq!(
            output.score_reason["rules"]["probe_freshness_ttl_seconds"],
            1_000
        );
        assert_eq!(output.score_reason["weights"]["probe_freshness"], 1.0);
    }

    #[test]
    fn custom_rules_normalize_weights() {
        let rules = PoolMemberScoreRules {
            weights: PoolMemberScoreWeights {
                manual_priority: 2.0,
                health: 2.0,
                probe_freshness: 0.0,
                quota_remaining: 0.0,
                latency: -1.0,
                cost_lru: f64::NAN,
            },
            probe_freshness_ttl_seconds: 0,
            unschedulable_score_cap: f64::INFINITY,
            probe_failure_penalty: f64::NAN,
            request_failure_penalty: f64::INFINITY,
            probe_failure_cooldown_threshold: 2,
        }
        .effective();

        assert_eq!(rules.weights.manual_priority, 0.5);
        assert_eq!(rules.weights.health, 0.5);
        assert_eq!(
            rules.probe_freshness_ttl_seconds,
            PROBE_FRESHNESS_TTL_SECONDS
        );
        assert_eq!(rules.unschedulable_score_cap, UNSCHEDULABLE_SCORE_CAP);
        assert_eq!(rules.probe_failure_penalty, PROBE_FAILURE_PENALTY);
        assert_eq!(rules.request_failure_penalty, REQUEST_FAILURE_PENALTY);
        assert_eq!(rules.probe_failure_cooldown_threshold, 2);
    }

    #[test]
    fn custom_rules_preserve_zero_weight_total() {
        let rules = PoolMemberScoreRules {
            weights: PoolMemberScoreWeights {
                manual_priority: 0.0,
                health: 0.0,
                probe_freshness: 0.0,
                quota_remaining: 0.0,
                latency: 0.0,
                cost_lru: 0.0,
            },
            ..PoolMemberScoreRules::default()
        }
        .effective();

        assert_eq!(
            rules.weights,
            PoolMemberScoreWeights {
                manual_priority: 0.0,
                health: 0.0,
                probe_freshness: 0.0,
                quota_remaining: 0.0,
                latency: 0.0,
                cost_lru: 0.0,
            }
        );
    }

    #[test]
    fn repeated_probe_failures_move_member_to_cooldown() {
        let mut input = input();
        input.probe_status = PoolMemberProbeStatus::Failed;
        input.probe_failure_count = 3;

        let output = score_pool_member(&input);

        assert_eq!(output.hard_state, PoolMemberHardState::Cooldown);
        assert!(output.score <= UNSCHEDULABLE_SCORE_CAP);
        assert_eq!(
            output.score_reason["rules"]["probe_failure_cooldown_threshold"],
            PROBE_FAILURE_COOLDOWN_THRESHOLD
        );
    }

    #[test]
    fn probe_and_request_failures_penalize_schedulable_score() {
        let mut input = input();
        input.last_probe_success_at = None;
        input.probe_status = PoolMemberProbeStatus::Never;
        input.probe_failure_count = 1;
        input.error_count = 2;
        let rules = PoolMemberScoreRules {
            probe_failure_penalty: 0.1,
            request_failure_penalty: 0.01,
            probe_failure_cooldown_threshold: 3,
            ..PoolMemberScoreRules::default()
        };

        let output = score_pool_member_with_rules(&input, rules);

        assert_eq!(output.hard_state, PoolMemberHardState::Available);
        assert_eq!(output.score_reason["penalties"]["probe_failure"], 0.1);
        assert_eq!(output.score_reason["penalties"]["request_failure"], 0.02);
    }
}
