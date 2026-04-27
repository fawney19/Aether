use crate::{
    ProviderKeyHealthBucket, SchedulerMinimalCandidateSelectionCandidate, SchedulerPriorityMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SchedulerRankingMode {
    FixedOrder,
    #[default]
    CacheAffinity,
    LoadBalance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum SchedulerTunnelAffinityBucket {
    LocalTunnel = 0,
    #[default]
    Neutral = 1,
    RemoteTunnel = 2,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchedulerRankableCandidate {
    pub provider_id: String,
    pub endpoint_id: String,
    pub key_id: String,
    pub selected_provider_model_name: String,
    pub provider_priority: i32,
    pub key_internal_priority: i32,
    pub key_global_priority_for_format: Option<i32>,
    pub capability_priority: (u32, u32),
    pub cached_affinity_match: bool,
    pub affinity_hash: Option<u64>,
    pub tunnel_bucket: SchedulerTunnelAffinityBucket,
    pub demote_cross_format: bool,
    pub format_preference: (u8, u8),
    pub health_bucket: Option<ProviderKeyHealthBucket>,
    pub health_score: f64,
    pub original_index: usize,
}

impl SchedulerRankableCandidate {
    pub fn from_candidate(
        candidate: &SchedulerMinimalCandidateSelectionCandidate,
        original_index: usize,
    ) -> Self {
        Self {
            provider_id: candidate.provider_id.clone(),
            endpoint_id: candidate.endpoint_id.clone(),
            key_id: candidate.key_id.clone(),
            selected_provider_model_name: candidate.selected_provider_model_name.clone(),
            provider_priority: candidate.provider_priority,
            key_internal_priority: candidate.key_internal_priority,
            key_global_priority_for_format: candidate.key_global_priority_for_format,
            capability_priority: (0, 0),
            cached_affinity_match: false,
            affinity_hash: None,
            tunnel_bucket: SchedulerTunnelAffinityBucket::Neutral,
            demote_cross_format: false,
            format_preference: (0, 0),
            health_bucket: None,
            health_score: 1.0,
            original_index,
        }
    }

    pub fn with_capability_priority(mut self, value: (u32, u32)) -> Self {
        self.capability_priority = value;
        self
    }

    pub fn with_cached_affinity_match(mut self, value: bool) -> Self {
        self.cached_affinity_match = value;
        self
    }

    pub fn with_affinity_hash(mut self, value: Option<u64>) -> Self {
        self.affinity_hash = value;
        self
    }

    pub fn with_tunnel_bucket(mut self, value: SchedulerTunnelAffinityBucket) -> Self {
        self.tunnel_bucket = value;
        self
    }

    pub fn with_format_state(
        mut self,
        demote_cross_format: bool,
        format_preference: (u8, u8),
    ) -> Self {
        self.demote_cross_format = demote_cross_format;
        self.format_preference = format_preference;
        self
    }

    pub fn with_health(mut self, bucket: Option<ProviderKeyHealthBucket>, score: f64) -> Self {
        self.health_bucket = bucket;
        self.health_score = score;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerRankingContext {
    pub priority_mode: SchedulerPriorityMode,
    pub ranking_mode: SchedulerRankingMode,
    pub include_health: bool,
    pub load_balance_seed: u64,
}

impl Default for SchedulerRankingContext {
    fn default() -> Self {
        Self {
            priority_mode: SchedulerPriorityMode::Provider,
            ranking_mode: SchedulerRankingMode::CacheAffinity,
            include_health: false,
            load_balance_seed: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SchedulerRankingOutcome {
    pub original_index: usize,
    pub ranking_index: usize,
    pub priority_mode: SchedulerPriorityMode,
    pub ranking_mode: SchedulerRankingMode,
    pub priority_slot: i32,
    pub promoted_by: Option<&'static str>,
    pub demoted_by: Option<&'static str>,
}
