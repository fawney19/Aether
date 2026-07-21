use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiCandidateMaterializationOutcome<Attempt> {
    pub attempts: Vec<Attempt>,
    pub candidate_count: usize,
}

#[async_trait]
pub trait AiCandidateMaterializationPort: Send + Sync {
    type Candidate: Send;
    type Eligible: Send + Sync;
    type Skipped: Send;
    type Attempt: Send;
    type Error: Send;

    async fn resolve_and_rank_candidates(
        &self,
        candidates: Vec<Self::Candidate>,
    ) -> Result<(Vec<Self::Eligible>, Vec<Self::Skipped>), Self::Error>;

    fn decorate_skipped_candidate(&self, skipped: Self::Skipped) -> Self::Skipped {
        skipped
    }

    fn remember_first_candidate_affinity(&self, candidates: &[Self::Eligible]);

    async fn persist_available_candidates(
        &self,
        candidates: Vec<Self::Eligible>,
    ) -> Result<Vec<Self::Attempt>, Self::Error>;

    async fn persist_skipped_candidates(
        &self,
        starting_candidate_index: u32,
        skipped_candidates: Vec<Self::Skipped>,
    ) -> Result<(), Self::Error>;
}

pub async fn run_ai_candidate_materialization<Port>(
    port: &Port,
    candidates: Vec<Port::Candidate>,
    preselection_skipped_candidates: Vec<Port::Skipped>,
) -> Result<AiCandidateMaterializationOutcome<Port::Attempt>, Port::Error>
where
    Port: AiCandidateMaterializationPort,
{
    let (candidates, skipped_candidates) = port.resolve_and_rank_candidates(candidates).await?;
    let skipped_candidates = preselection_skipped_candidates
        .into_iter()
        .chain(skipped_candidates)
        .map(|candidate| port.decorate_skipped_candidate(candidate))
        .collect::<Vec<_>>();
    let candidate_count = candidates.len() + skipped_candidates.len();

    port.remember_first_candidate_affinity(&candidates);
    let available_candidate_count = candidates.len() as u32;
    let attempts = port.persist_available_candidates(candidates).await?;
    port.persist_skipped_candidates(available_candidate_count, skipped_candidates)
        .await?;

    Ok(AiCandidateMaterializationOutcome {
        attempts,
        candidate_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct TestPort {
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl AiCandidateMaterializationPort for TestPort {
        type Candidate = &'static str;
        type Eligible = &'static str;
        type Skipped = &'static str;
        type Attempt = &'static str;
        type Error = std::convert::Infallible;

        async fn resolve_and_rank_candidates(
            &self,
            candidates: Vec<Self::Candidate>,
        ) -> Result<(Vec<Self::Eligible>, Vec<Self::Skipped>), Self::Error> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("resolve:{}", candidates.join(",")));
            Ok((vec!["eligible-a", "eligible-b"], vec!["resolved-skip"]))
        }

        fn decorate_skipped_candidate(&self, skipped: Self::Skipped) -> Self::Skipped {
            self.calls
                .lock()
                .unwrap()
                .push(format!("decorate:{skipped}"));
            skipped
        }

        fn remember_first_candidate_affinity(&self, candidates: &[Self::Eligible]) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("affinity:{}", candidates.join(",")));
        }

        async fn persist_available_candidates(
            &self,
            candidates: Vec<Self::Eligible>,
        ) -> Result<Vec<Self::Attempt>, Self::Error> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("available:{}", candidates.join(",")));
            Ok(vec!["attempt-a", "attempt-b"])
        }

        async fn persist_skipped_candidates(
            &self,
            starting_candidate_index: u32,
            skipped_candidates: Vec<Self::Skipped>,
        ) -> Result<(), Self::Error> {
            self.calls.lock().unwrap().push(format!(
                "skipped:{starting_candidate_index}:{}",
                skipped_candidates.join(",")
            ));
            Ok(())
        }
    }

    #[tokio::test]
    async fn materialization_runs_in_serving_order_and_counts_all_candidates() {
        let port = TestPort::default();

        let outcome =
            run_ai_candidate_materialization(&port, vec!["candidate-a"], vec!["pre-skip"])
                .await
                .unwrap();

        assert_eq!(outcome.attempts, ["attempt-a", "attempt-b"]);
        assert_eq!(outcome.candidate_count, 4);
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            [
                "resolve:candidate-a",
                "decorate:pre-skip",
                "decorate:resolved-skip",
                "affinity:eligible-a,eligible-b",
                "available:eligible-a,eligible-b",
                "skipped:2:pre-skip,resolved-skip",
            ]
        );
    }
}
