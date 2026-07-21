use async_trait::async_trait;

pub const POOL_KIND_PROVIDER_KEY_POOL: &str = "provider_key_pool";
pub const POOL_MEMBER_KIND_PROVIDER_API_KEY: &str = "provider_api_key";
pub const POOL_SCORE_CAPABILITY_ACCOUNT: &str = "account";
pub const POOL_SCORE_SCOPE_KIND_ACCOUNT: &str = "account";
pub const POOL_SCORE_CAPABILITY_API_FORMAT: &str = POOL_SCORE_CAPABILITY_ACCOUNT;
pub const POOL_SCORE_SCOPE_KIND_MODEL: &str = POOL_SCORE_SCOPE_KIND_ACCOUNT;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PoolMemberHardState {
    Available,
    Unknown,
    Cooldown,
    QuotaExhausted,
    AuthInvalid,
    Banned,
    Inactive,
}

impl PoolMemberHardState {
    pub fn as_database(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Unknown => "unknown",
            Self::Cooldown => "cooldown",
            Self::QuotaExhausted => "quota_exhausted",
            Self::AuthInvalid => "auth_invalid",
            Self::Banned => "banned",
            Self::Inactive => "inactive",
        }
    }

    pub fn from_database(value: &str) -> Result<Self, crate::DataLayerError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "available" => Ok(Self::Available),
            "unknown" => Ok(Self::Unknown),
            "cooldown" => Ok(Self::Cooldown),
            "quota_exhausted" => Ok(Self::QuotaExhausted),
            "auth_invalid" => Ok(Self::AuthInvalid),
            "banned" => Ok(Self::Banned),
            "inactive" => Ok(Self::Inactive),
            other => Err(crate::DataLayerError::UnexpectedValue(format!(
                "unknown pool member hard_state: {other}"
            ))),
        }
    }

    pub fn schedulable(self) -> bool {
        matches!(self, Self::Available | Self::Unknown)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PoolMemberProbeStatus {
    Never,
    Ok,
    Failed,
    Stale,
    InProgress,
}

impl PoolMemberProbeStatus {
    pub fn as_database(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Stale => "stale",
            Self::InProgress => "in_progress",
        }
    }

    pub fn from_database(value: &str) -> Result<Self, crate::DataLayerError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "never" => Ok(Self::Never),
            "ok" => Ok(Self::Ok),
            "failed" => Ok(Self::Failed),
            "stale" => Ok(Self::Stale),
            "in_progress" => Ok(Self::InProgress),
            other => Err(crate::DataLayerError::UnexpectedValue(format!(
                "unknown pool member probe_status: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PoolScoreScope {
    pub capability: String,
    pub scope_kind: String,
    pub scope_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PoolMemberIdentity {
    pub pool_kind: String,
    pub pool_id: String,
    pub member_kind: String,
    pub member_id: String,
}

impl PoolMemberIdentity {
    pub fn provider_api_key(provider_id: impl Into<String>, key_id: impl Into<String>) -> Self {
        Self {
            pool_kind: POOL_KIND_PROVIDER_KEY_POOL.to_string(),
            pool_id: provider_id.into(),
            member_kind: POOL_MEMBER_KIND_PROVIDER_API_KEY.to_string(),
            member_id: key_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredPoolMemberScore {
    pub id: String,
    pub pool_kind: String,
    pub pool_id: String,
    pub member_kind: String,
    pub member_id: String,
    pub capability: String,
    pub scope_kind: String,
    pub scope_id: Option<String>,
    pub score: f64,
    pub hard_state: PoolMemberHardState,
    pub score_version: u64,
    pub score_reason: serde_json::Value,
    pub last_ranked_at: Option<u64>,
    pub last_scheduled_at: Option<u64>,
    pub last_success_at: Option<u64>,
    pub last_failure_at: Option<u64>,
    pub failure_count: u64,
    pub last_probe_attempt_at: Option<u64>,
    pub last_probe_success_at: Option<u64>,
    pub last_probe_failure_at: Option<u64>,
    pub probe_failure_count: u64,
    pub probe_status: PoolMemberProbeStatus,
    pub updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpsertPoolMemberScore {
    pub id: String,
    pub identity: PoolMemberIdentity,
    pub scope: PoolScoreScope,
    pub score: f64,
    pub hard_state: PoolMemberHardState,
    pub score_version: u64,
    pub score_reason: serde_json::Value,
    pub last_ranked_at: Option<u64>,
    pub last_scheduled_at: Option<u64>,
    pub last_success_at: Option<u64>,
    pub last_failure_at: Option<u64>,
    pub failure_count: u64,
    pub last_probe_attempt_at: Option<u64>,
    pub last_probe_success_at: Option<u64>,
    pub last_probe_failure_at: Option<u64>,
    pub probe_failure_count: u64,
    pub probe_status: PoolMemberProbeStatus,
    pub updated_at: u64,
}

impl UpsertPoolMemberScore {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        validate_non_empty(&self.id, "pool_member_scores.id")?;
        validate_non_empty(&self.identity.pool_kind, "pool_member_scores.pool_kind")?;
        validate_non_empty(&self.identity.pool_id, "pool_member_scores.pool_id")?;
        validate_non_empty(&self.identity.member_kind, "pool_member_scores.member_kind")?;
        validate_non_empty(&self.identity.member_id, "pool_member_scores.member_id")?;
        validate_non_empty(&self.scope.capability, "pool_member_scores.capability")?;
        validate_non_empty(&self.scope.scope_kind, "pool_member_scores.scope_kind")?;
        if !self.score.is_finite() {
            return Err(crate::DataLayerError::InvalidInput(
                "pool_member_scores.score must be finite".to_string(),
            ));
        }
        Ok(())
    }

    pub fn into_stored(self) -> StoredPoolMemberScore {
        StoredPoolMemberScore {
            id: self.id,
            pool_kind: self.identity.pool_kind,
            pool_id: self.identity.pool_id,
            member_kind: self.identity.member_kind,
            member_id: self.identity.member_id,
            capability: self.scope.capability,
            scope_kind: self.scope.scope_kind,
            scope_id: self.scope.scope_id,
            score: self.score,
            hard_state: self.hard_state,
            score_version: self.score_version,
            score_reason: self.score_reason,
            last_ranked_at: self.last_ranked_at,
            last_scheduled_at: self.last_scheduled_at,
            last_success_at: self.last_success_at,
            last_failure_at: self.last_failure_at,
            failure_count: self.failure_count,
            last_probe_attempt_at: self.last_probe_attempt_at,
            last_probe_success_at: self.last_probe_success_at,
            last_probe_failure_at: self.last_probe_failure_at,
            probe_failure_count: self.probe_failure_count,
            probe_status: self.probe_status,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ListRankedPoolMembersQuery {
    pub pool_kind: String,
    pub pool_id: String,
    pub capability: String,
    pub scope_kind: String,
    pub scope_id: Option<String>,
    pub hard_states: Vec<PoolMemberHardState>,
    pub probe_statuses: Option<Vec<PoolMemberProbeStatus>>,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ListPoolMemberScoresQuery {
    pub pool_kind: String,
    pub pool_id: String,
    pub capability: Option<String>,
    pub scope_kind: Option<String>,
    pub scope_id: Option<String>,
    pub hard_states: Vec<PoolMemberHardState>,
    pub probe_statuses: Option<Vec<PoolMemberProbeStatus>>,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ListPoolMemberProbeCandidatesQuery {
    pub pool_kind: String,
    pub pool_id: String,
    pub capability: Option<String>,
    pub stale_before_unix_secs: u64,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GetPoolMemberScoresByIdsQuery {
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PoolMemberProbeResult {
    pub identity: PoolMemberIdentity,
    pub scope: Option<PoolScoreScope>,
    pub attempted_at: u64,
    pub succeeded: bool,
    pub hard_state: Option<PoolMemberHardState>,
    pub probe_status: PoolMemberProbeStatus,
    pub score_reason_patch: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PoolMemberProbeAttempt {
    pub identity: PoolMemberIdentity,
    pub scope: Option<PoolScoreScope>,
    pub attempted_at: u64,
    pub score_reason_patch: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PoolMemberScheduleFeedback {
    pub identity: PoolMemberIdentity,
    pub scope: Option<PoolScoreScope>,
    pub scheduled_at: u64,
    pub succeeded: Option<bool>,
    pub hard_state: Option<PoolMemberHardState>,
    pub score_delta: Option<i32>,
    pub score_reason_patch: Option<serde_json::Value>,
}

#[async_trait]
pub trait PoolScoreReadRepository: Send + Sync {
    async fn list_ranked_pool_members(
        &self,
        query: &ListRankedPoolMembersQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, crate::DataLayerError>;

    async fn list_pool_member_scores(
        &self,
        query: &ListPoolMemberScoresQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, crate::DataLayerError>;

    async fn list_pool_member_probe_candidates(
        &self,
        query: &ListPoolMemberProbeCandidatesQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, crate::DataLayerError>;

    async fn get_pool_member_scores_by_ids(
        &self,
        query: &GetPoolMemberScoresByIdsQuery,
    ) -> Result<Vec<StoredPoolMemberScore>, crate::DataLayerError>;
}

#[async_trait]
pub trait PoolMemberScoreWriteRepository: Send + Sync {
    async fn upsert_pool_member_score(
        &self,
        score: UpsertPoolMemberScore,
    ) -> Result<StoredPoolMemberScore, crate::DataLayerError>;

    async fn mark_pool_member_probe_in_progress(
        &self,
        attempt: PoolMemberProbeAttempt,
    ) -> Result<usize, crate::DataLayerError>;

    async fn record_pool_member_probe_result(
        &self,
        result: PoolMemberProbeResult,
    ) -> Result<usize, crate::DataLayerError>;

    async fn record_pool_member_schedule_feedback(
        &self,
        feedback: PoolMemberScheduleFeedback,
    ) -> Result<usize, crate::DataLayerError>;

    async fn mark_pool_member_hard_state(
        &self,
        identity: &PoolMemberIdentity,
        scope: Option<&PoolScoreScope>,
        hard_state: PoolMemberHardState,
        updated_at: u64,
    ) -> Result<usize, crate::DataLayerError>;

    async fn delete_pool_member_scores_for_member(
        &self,
        identity: &PoolMemberIdentity,
    ) -> Result<usize, crate::DataLayerError>;
}

pub trait PoolMemberScoreRepository:
    PoolScoreReadRepository + PoolMemberScoreWriteRepository + Send + Sync
{
}

impl<T> PoolMemberScoreRepository for T where
    T: PoolScoreReadRepository + PoolMemberScoreWriteRepository + Send + Sync
{
}

fn validate_non_empty(value: &str, field: &str) -> Result<(), crate::DataLayerError> {
    if value.trim().is_empty() {
        return Err(crate::DataLayerError::InvalidInput(format!(
            "{field} is empty"
        )));
    }
    Ok(())
}
