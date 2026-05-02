use aether_scheduler_core::{
    apply_scheduler_candidate_ranking, requested_capability_priority_for_candidate,
    SchedulerMinimalCandidateSelectionCandidate, SchedulerPriorityMode, SchedulerRankableCandidate,
    SchedulerRankingContext, SchedulerRankingMode, SchedulerRankingOutcome,
    SchedulerTunnelAffinityBucket,
};
use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiRankingSchedulingMode {
    FixedOrder,
    CacheAffinity,
    LoadBalance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiRankingContextConfig {
    pub priority_mode: SchedulerPriorityMode,
    pub scheduling_mode: AiRankingSchedulingMode,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AiRankableCandidateParts<'a> {
    pub candidate: &'a SchedulerMinimalCandidateSelectionCandidate,
    pub original_index: usize,
    pub normalized_client_api_format: &'a str,
    pub provider_api_format: &'a str,
    pub required_capabilities: Option<&'a serde_json::Value>,
    pub cached_affinity_match: bool,
    pub tunnel_bucket: SchedulerTunnelAffinityBucket,
    pub keep_priority_on_conversion: bool,
}

#[async_trait]
pub trait AiCandidateRankingPort: Send + Sync {
    type Candidate: Send + Sync;
    type AffinityTarget: Send + Sync;
    type Error: Send;

    fn affinity_requested_model(&self, candidates: &[Self::Candidate]) -> Option<String>;

    async fn read_cached_affinity_target(
        &self,
        normalized_client_api_format: &str,
        affinity_requested_model: Option<&str>,
    ) -> Result<Option<Self::AffinityTarget>, Self::Error>;

    fn cached_affinity_matches(
        &self,
        candidate: &Self::Candidate,
        target: &Self::AffinityTarget,
    ) -> bool;

    async fn build_rankable_candidate(
        &self,
        candidate: &Self::Candidate,
        original_index: usize,
        normalized_client_api_format: &str,
        cached_affinity_match: bool,
    ) -> Result<SchedulerRankableCandidate, Self::Error>;

    fn ranking_context(&self) -> SchedulerRankingContext;

    fn apply_ranking_outcome(
        &self,
        candidate: &mut Self::Candidate,
        outcome: SchedulerRankingOutcome,
    );
}

pub fn build_ai_rankable_candidate(
    parts: AiRankableCandidateParts<'_>,
) -> SchedulerRankableCandidate {
    let is_same_format = aether_ai_formats::api_format_alias_matches(
        parts.provider_api_format,
        parts.normalized_client_api_format,
    );
    let format_preference = aether_ai_formats::request_candidate_api_format_preference(
        parts.normalized_client_api_format,
        parts.provider_api_format,
    )
    .unwrap_or((u8::MAX, u8::MAX));

    let mut rankable =
        SchedulerRankableCandidate::from_candidate(parts.candidate, parts.original_index);
    // The scheduler order is the upstream tie-breaker; AI serving only adds transport facts.
    rankable.provider_id.clear();
    rankable.endpoint_id.clear();
    rankable.key_id.clear();
    rankable.selected_provider_model_name.clear();

    rankable
        .with_capability_priority(requested_capability_priority_for_candidate(
            parts.required_capabilities,
            parts.candidate,
        ))
        .with_cached_affinity_match(parts.cached_affinity_match)
        .with_tunnel_bucket(parts.tunnel_bucket)
        .with_format_state(
            !is_same_format && !parts.keep_priority_on_conversion,
            format_preference,
        )
}

pub fn ai_ranking_context(config: AiRankingContextConfig) -> SchedulerRankingContext {
    SchedulerRankingContext {
        priority_mode: config.priority_mode,
        ranking_mode: ai_ranking_mode(config.scheduling_mode),
        include_health: false,
        load_balance_seed: 0,
    }
}

fn ai_ranking_mode(mode: AiRankingSchedulingMode) -> SchedulerRankingMode {
    match mode {
        AiRankingSchedulingMode::FixedOrder => SchedulerRankingMode::FixedOrder,
        AiRankingSchedulingMode::CacheAffinity => SchedulerRankingMode::CacheAffinity,
        AiRankingSchedulingMode::LoadBalance => SchedulerRankingMode::LoadBalance,
    }
}

pub async fn run_ai_candidate_ranking<Port>(
    port: &Port,
    mut candidates: Vec<Port::Candidate>,
    normalized_client_api_format: &str,
) -> Result<Vec<Port::Candidate>, Port::Error>
where
    Port: AiCandidateRankingPort,
{
    let affinity_requested_model = port.affinity_requested_model(&candidates);
    let cached_affinity_target = port
        .read_cached_affinity_target(
            normalized_client_api_format,
            affinity_requested_model.as_deref(),
        )
        .await?;

    let mut rankables = Vec::with_capacity(candidates.len());
    for (original_index, candidate) in candidates.iter().enumerate() {
        let cached_affinity_match = cached_affinity_target
            .as_ref()
            .is_some_and(|target| port.cached_affinity_matches(candidate, target));
        rankables.push(
            port.build_rankable_candidate(
                candidate,
                original_index,
                normalized_client_api_format,
                cached_affinity_match,
            )
            .await?,
        );
    }

    let outcomes =
        apply_scheduler_candidate_ranking(&mut candidates, &rankables, port.ranking_context());
    for outcome in outcomes {
        let ranking_index = outcome.ranking_index;
        if let Some(candidate) = candidates.get_mut(ranking_index) {
            port.apply_ranking_outcome(candidate, outcome);
        }
    }

    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_scheduler_core::{SchedulerPriorityMode, SchedulerRankingMode};
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestCandidate {
        id: &'static str,
        priority: i32,
        ranking_index: Option<usize>,
        cached_affinity: bool,
    }

    #[derive(Default)]
    struct TestPort {
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl AiCandidateRankingPort for TestPort {
        type Candidate = TestCandidate;
        type AffinityTarget = &'static str;
        type Error = std::convert::Infallible;

        fn affinity_requested_model(&self, candidates: &[Self::Candidate]) -> Option<String> {
            candidates.first().map(|_| "model-a".to_string())
        }

        async fn read_cached_affinity_target(
            &self,
            normalized_client_api_format: &str,
            affinity_requested_model: Option<&str>,
        ) -> Result<Option<Self::AffinityTarget>, Self::Error> {
            self.calls.lock().unwrap().push(format!(
                "affinity:{normalized_client_api_format}:{}",
                affinity_requested_model.unwrap_or_default()
            ));
            Ok(Some("candidate-b"))
        }

        fn cached_affinity_matches(
            &self,
            candidate: &Self::Candidate,
            target: &Self::AffinityTarget,
        ) -> bool {
            candidate.id == *target
        }

        async fn build_rankable_candidate(
            &self,
            candidate: &Self::Candidate,
            original_index: usize,
            _normalized_client_api_format: &str,
            cached_affinity_match: bool,
        ) -> Result<SchedulerRankableCandidate, Self::Error> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("rankable:{}:{cached_affinity_match}", candidate.id));
            Ok(SchedulerRankableCandidate {
                provider_id: candidate.id.to_string(),
                endpoint_id: String::new(),
                key_id: String::new(),
                selected_provider_model_name: String::new(),
                provider_priority: candidate.priority,
                key_internal_priority: 0,
                key_global_priority_for_format: None,
                capability_priority: (0, 0),
                cached_affinity_match,
                affinity_hash: None,
                tunnel_bucket: Default::default(),
                demote_cross_format: false,
                format_preference: (0, 0),
                health_bucket: None,
                health_score: 1.0,
                original_index,
            })
        }

        fn ranking_context(&self) -> SchedulerRankingContext {
            SchedulerRankingContext {
                priority_mode: SchedulerPriorityMode::Provider,
                ranking_mode: SchedulerRankingMode::CacheAffinity,
                include_health: false,
                load_balance_seed: 0,
            }
        }

        fn apply_ranking_outcome(
            &self,
            candidate: &mut Self::Candidate,
            outcome: SchedulerRankingOutcome,
        ) {
            candidate.ranking_index = Some(outcome.ranking_index);
            candidate.cached_affinity = outcome.promoted_by.is_some();
        }
    }

    #[tokio::test]
    async fn ranking_builds_rankables_applies_scheduler_order_and_writes_outcomes() {
        let port = TestPort::default();
        let candidates = vec![
            TestCandidate {
                id: "candidate-a",
                priority: 10,
                ranking_index: None,
                cached_affinity: false,
            },
            TestCandidate {
                id: "candidate-b",
                priority: 20,
                ranking_index: None,
                cached_affinity: false,
            },
        ];

        let ranked = run_ai_candidate_ranking(&port, candidates, "openai:chat")
            .await
            .unwrap();

        assert_eq!(ranked[0].id, "candidate-b");
        assert_eq!(ranked[0].ranking_index, Some(0));
        assert!(ranked[0].cached_affinity);
        assert_eq!(ranked[1].id, "candidate-a");
        assert_eq!(ranked[1].ranking_index, Some(1));
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            [
                "affinity:openai:chat:model-a",
                "rankable:candidate-a:false",
                "rankable:candidate-b:true",
            ]
        );
    }
}
