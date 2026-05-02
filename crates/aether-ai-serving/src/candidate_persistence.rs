use aether_scheduler_core::SchedulerRankingOutcome;
use async_trait::async_trait;
use serde_json::Value;

#[async_trait]
pub trait AiAvailableCandidatePersistencePort: Send + Sync {
    type Candidate: Clone + Send + Sync;
    type Attempt: Send;
    type ExtraData: Clone + Send + Sync;
    type Error: Send;

    fn attempt_slot_count(&self, candidate: &Self::Candidate) -> u32;

    fn build_extra_data(&self, candidate: &Self::Candidate) -> Option<Self::ExtraData>;

    fn generate_candidate_id(&self) -> String;

    fn should_persist_available_candidate(&self, candidate: &Self::Candidate) -> bool;

    async fn persist_available_candidate(
        &self,
        candidate: &Self::Candidate,
        candidate_index: u32,
        retry_index: u32,
        generated_candidate_id: &str,
        extra_data: Option<Self::ExtraData>,
    ) -> Result<String, Self::Error>;

    fn build_attempt(
        &self,
        candidate: Self::Candidate,
        candidate_index: u32,
        retry_index: u32,
        candidate_id: String,
    ) -> Self::Attempt;
}

pub async fn run_ai_available_candidate_persistence<Port>(
    port: &Port,
    candidates: Vec<Port::Candidate>,
) -> Result<Vec<Port::Attempt>, Port::Error>
where
    Port: AiAvailableCandidatePersistencePort,
{
    let total_attempts = candidates
        .iter()
        .map(|candidate| port.attempt_slot_count(candidate) as usize)
        .sum();
    let mut materialized = Vec::with_capacity(total_attempts);

    for (candidate_index, candidate) in candidates.into_iter().enumerate() {
        let candidate_index = candidate_index as u32;
        let attempt_slots = port.attempt_slot_count(&candidate).max(1);
        let extra_data = port.build_extra_data(&candidate);
        let mut owned_candidate = Some(candidate);

        for retry_index in 0..attempt_slots {
            let candidate = owned_candidate
                .as_ref()
                .expect("candidate should remain available until final retry");
            let generated_candidate_id = port.generate_candidate_id();
            let candidate_id = if port.should_persist_available_candidate(candidate) {
                port.persist_available_candidate(
                    candidate,
                    candidate_index,
                    retry_index,
                    generated_candidate_id.as_str(),
                    extra_data.clone(),
                )
                .await?
            } else {
                generated_candidate_id
            };

            let candidate = if retry_index + 1 == attempt_slots {
                owned_candidate
                    .take()
                    .expect("final retry should consume owned candidate")
            } else {
                candidate.clone()
            };
            materialized.push(port.build_attempt(
                candidate,
                candidate_index,
                retry_index,
                candidate_id,
            ));
        }
    }

    Ok(materialized)
}

#[async_trait]
pub trait AiSkippedCandidatePersistencePort: Send + Sync {
    type Skipped: Send + Sync;
    type ExtraData: Send + Sync;
    type Error: Send;

    fn should_persist_skipped_candidate(&self, candidate: &Self::Skipped) -> bool;

    fn build_extra_data(&self, candidate: &Self::Skipped) -> Option<Self::ExtraData>;

    fn generate_candidate_id(&self) -> String;

    async fn persist_skipped_candidate(
        &self,
        candidate: &Self::Skipped,
        candidate_index: u32,
        generated_candidate_id: &str,
        extra_data: Option<Self::ExtraData>,
    ) -> Result<(), Self::Error>;
}

pub async fn run_ai_skipped_candidate_persistence<Port>(
    port: &Port,
    starting_candidate_index: u32,
    skipped_candidates: Vec<Port::Skipped>,
) -> Result<(), Port::Error>
where
    Port: AiSkippedCandidatePersistencePort,
{
    let mut next_candidate_index = starting_candidate_index;
    for skipped_candidate in skipped_candidates {
        if !port.should_persist_skipped_candidate(&skipped_candidate) {
            continue;
        }
        let generated_candidate_id = port.generate_candidate_id();
        let extra_data = port.build_extra_data(&skipped_candidate);
        port.persist_skipped_candidate(
            &skipped_candidate,
            next_candidate_index,
            generated_candidate_id.as_str(),
            extra_data,
        )
        .await?;
        next_candidate_index = next_candidate_index.saturating_add(1);
    }

    Ok(())
}

pub fn ai_should_persist_available_candidate_for_pool_key(pool_key_index: Option<u32>) -> bool {
    pool_key_index.is_none_or(|index| index == 0)
}

pub fn ai_should_persist_skipped_candidate_for_pool_membership(is_pool_candidate: bool) -> bool {
    !is_pool_candidate
}

pub fn ai_candidate_extra_data_with_ranking(
    extra_data: Option<Value>,
    ranking: Option<&SchedulerRankingOutcome>,
) -> Option<Value> {
    let Some(ranking) = ranking else {
        return extra_data;
    };

    let mut object = match extra_data {
        Some(Value::Object(object)) => object,
        Some(value) => {
            let mut object = serde_json::Map::new();
            object.insert("extra".to_string(), value);
            object
        }
        None => serde_json::Map::new(),
    };
    crate::append_ai_ranking_metadata_to_object(&mut object, ranking);
    Some(Value::Object(object))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestCandidate {
        id: &'static str,
        attempt_slots: u32,
        persist: bool,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestAttempt {
        id: &'static str,
        candidate_index: u32,
        retry_index: u32,
        candidate_id: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestSkipped {
        id: &'static str,
        persist: bool,
    }

    #[derive(Default)]
    struct TestPort {
        next_id: Mutex<u32>,
        calls: Mutex<Vec<String>>,
    }

    impl TestPort {
        fn next_candidate_id(&self) -> String {
            let mut next_id = self.next_id.lock().unwrap();
            *next_id += 1;
            format!("candidate-{next_id}")
        }
    }

    #[async_trait]
    impl AiAvailableCandidatePersistencePort for TestPort {
        type Candidate = TestCandidate;
        type Attempt = TestAttempt;
        type ExtraData = String;
        type Error = std::convert::Infallible;

        fn attempt_slot_count(&self, candidate: &Self::Candidate) -> u32 {
            candidate.attempt_slots
        }

        fn build_extra_data(&self, candidate: &Self::Candidate) -> Option<Self::ExtraData> {
            Some(format!("extra:{}", candidate.id))
        }

        fn generate_candidate_id(&self) -> String {
            self.next_candidate_id()
        }

        fn should_persist_available_candidate(&self, candidate: &Self::Candidate) -> bool {
            candidate.persist
        }

        async fn persist_available_candidate(
            &self,
            candidate: &Self::Candidate,
            candidate_index: u32,
            retry_index: u32,
            generated_candidate_id: &str,
            extra_data: Option<Self::ExtraData>,
        ) -> Result<String, Self::Error> {
            self.calls.lock().unwrap().push(format!(
                "available:{}:{candidate_index}:{retry_index}:{generated_candidate_id}:{}",
                candidate.id,
                extra_data.unwrap_or_default()
            ));
            Ok(format!("stored-{generated_candidate_id}"))
        }

        fn build_attempt(
            &self,
            candidate: Self::Candidate,
            candidate_index: u32,
            retry_index: u32,
            candidate_id: String,
        ) -> Self::Attempt {
            TestAttempt {
                id: candidate.id,
                candidate_index,
                retry_index,
                candidate_id,
            }
        }
    }

    #[async_trait]
    impl AiSkippedCandidatePersistencePort for TestPort {
        type Skipped = TestSkipped;
        type ExtraData = String;
        type Error = std::convert::Infallible;

        fn should_persist_skipped_candidate(&self, candidate: &Self::Skipped) -> bool {
            candidate.persist
        }

        fn build_extra_data(&self, candidate: &Self::Skipped) -> Option<Self::ExtraData> {
            Some(format!("extra:{}", candidate.id))
        }

        fn generate_candidate_id(&self) -> String {
            self.next_candidate_id()
        }

        async fn persist_skipped_candidate(
            &self,
            candidate: &Self::Skipped,
            candidate_index: u32,
            generated_candidate_id: &str,
            extra_data: Option<Self::ExtraData>,
        ) -> Result<(), Self::Error> {
            self.calls.lock().unwrap().push(format!(
                "skipped:{}:{candidate_index}:{generated_candidate_id}:{}",
                candidate.id,
                extra_data.unwrap_or_default()
            ));
            Ok(())
        }
    }

    #[tokio::test]
    async fn available_persistence_expands_candidates_into_retry_attempts() {
        let port = TestPort::default();

        let attempts = run_ai_available_candidate_persistence(
            &port,
            vec![
                TestCandidate {
                    id: "a",
                    attempt_slots: 2,
                    persist: true,
                },
                TestCandidate {
                    id: "b",
                    attempt_slots: 1,
                    persist: false,
                },
            ],
        )
        .await
        .unwrap();

        assert_eq!(
            attempts,
            [
                TestAttempt {
                    id: "a",
                    candidate_index: 0,
                    retry_index: 0,
                    candidate_id: "stored-candidate-1".to_string(),
                },
                TestAttempt {
                    id: "a",
                    candidate_index: 0,
                    retry_index: 1,
                    candidate_id: "stored-candidate-2".to_string(),
                },
                TestAttempt {
                    id: "b",
                    candidate_index: 1,
                    retry_index: 0,
                    candidate_id: "candidate-3".to_string(),
                },
            ]
        );
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            [
                "available:a:0:0:candidate-1:extra:a",
                "available:a:0:1:candidate-2:extra:a",
            ]
        );
    }

    #[tokio::test]
    async fn skipped_persistence_keeps_indices_for_persisted_candidates_only() {
        let port = TestPort::default();

        run_ai_skipped_candidate_persistence(
            &port,
            3,
            vec![
                TestSkipped {
                    id: "ignored",
                    persist: false,
                },
                TestSkipped {
                    id: "a",
                    persist: true,
                },
                TestSkipped {
                    id: "b",
                    persist: true,
                },
            ],
        )
        .await
        .unwrap();

        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            [
                "skipped:a:3:candidate-1:extra:a",
                "skipped:b:4:candidate-2:extra:b",
            ]
        );
    }

    #[test]
    fn pool_candidate_persistence_policy_persists_representatives_only() {
        assert!(ai_should_persist_available_candidate_for_pool_key(None));
        assert!(ai_should_persist_available_candidate_for_pool_key(Some(0)));
        assert!(!ai_should_persist_available_candidate_for_pool_key(Some(1)));

        assert!(ai_should_persist_skipped_candidate_for_pool_membership(
            false
        ));
        assert!(!ai_should_persist_skipped_candidate_for_pool_membership(
            true
        ));
    }

    #[test]
    fn candidate_extra_data_with_ranking_preserves_existing_shapes() {
        let ranking = SchedulerRankingOutcome {
            original_index: 1,
            ranking_index: 0,
            priority_mode: aether_scheduler_core::SchedulerPriorityMode::Provider,
            ranking_mode: aether_scheduler_core::SchedulerRankingMode::CacheAffinity,
            priority_slot: 3,
            promoted_by: Some("cached_affinity"),
            demoted_by: None,
        };

        let object = ai_candidate_extra_data_with_ranking(
            Some(serde_json::json!({"source": "test"})),
            Some(&ranking),
        )
        .expect("extra data should exist");
        assert_eq!(object.get("source"), Some(&serde_json::json!("test")));
        assert_eq!(object.get("ranking_index"), Some(&serde_json::json!(0)));
        assert_eq!(
            object.get("promoted_by"),
            Some(&serde_json::json!("cached_affinity"))
        );

        let scalar =
            ai_candidate_extra_data_with_ranking(Some(serde_json::json!("raw")), Some(&ranking))
                .expect("scalar extra data should be wrapped");
        assert_eq!(scalar.get("extra"), Some(&serde_json::json!("raw")));
        assert_eq!(scalar.get("priority_slot"), Some(&serde_json::json!(3)));
    }
}
