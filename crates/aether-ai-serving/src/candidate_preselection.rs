use std::collections::BTreeSet;

use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiCandidatePreselectionOutcome<Candidate, Skipped> {
    pub candidates: Vec<Candidate>,
    pub skipped_candidates: Vec<Skipped>,
}

#[async_trait]
pub trait AiCandidatePreselectionPort: Send + Sync {
    type Candidate: Send;
    type Skipped: Send;
    type Error: Send;

    fn candidate_api_formats(&self) -> Vec<String>;

    fn candidate_api_format_matches_client(&self, candidate_api_format: &str) -> bool;

    async fn list_candidates_for_api_format(
        &self,
        candidate_api_format: &str,
        matches_client_format: bool,
    ) -> Result<(Vec<Self::Candidate>, Vec<Self::Skipped>), Self::Error>;

    fn candidate_allowed(
        &self,
        _candidate: &Self::Candidate,
        _candidate_api_format: &str,
        _matches_client_format: bool,
    ) -> bool {
        true
    }

    fn skipped_candidate_allowed(
        &self,
        _skipped_candidate: &Self::Skipped,
        _candidate_api_format: &str,
        _matches_client_format: bool,
    ) -> bool {
        true
    }

    fn candidate_key(&self, candidate: &Self::Candidate) -> String;

    fn skipped_candidate_key(&self, skipped_candidate: &Self::Skipped) -> String;
}

pub async fn run_ai_candidate_preselection<Port>(
    port: &Port,
) -> Result<AiCandidatePreselectionOutcome<Port::Candidate, Port::Skipped>, Port::Error>
where
    Port: AiCandidatePreselectionPort,
{
    let mut candidates = Vec::new();
    let mut skipped_candidates = Vec::new();
    let mut seen_candidates = BTreeSet::new();
    let mut seen_skipped_candidates = BTreeSet::new();

    for candidate_api_format in port.candidate_api_formats() {
        let matches_client_format =
            port.candidate_api_format_matches_client(candidate_api_format.as_str());
        let (selected, skipped) = port
            .list_candidates_for_api_format(candidate_api_format.as_str(), matches_client_format)
            .await?;

        for skipped_candidate in skipped {
            if !port.skipped_candidate_allowed(
                &skipped_candidate,
                candidate_api_format.as_str(),
                matches_client_format,
            ) {
                continue;
            }
            let candidate_key = port.skipped_candidate_key(&skipped_candidate);
            if seen_skipped_candidates.insert(candidate_key) {
                skipped_candidates.push(skipped_candidate);
            }
        }

        for candidate in selected {
            if !port.candidate_allowed(
                &candidate,
                candidate_api_format.as_str(),
                matches_client_format,
            ) {
                continue;
            }
            let candidate_key = port.candidate_key(&candidate);
            if seen_candidates.insert(candidate_key) {
                candidates.push(candidate);
            }
        }
    }

    Ok(AiCandidatePreselectionOutcome {
        candidates,
        skipped_candidates,
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
    impl AiCandidatePreselectionPort for TestPort {
        type Candidate = &'static str;
        type Skipped = &'static str;
        type Error = std::convert::Infallible;

        fn candidate_api_formats(&self) -> Vec<String> {
            vec!["same".to_string(), "cross".to_string()]
        }

        fn candidate_api_format_matches_client(&self, candidate_api_format: &str) -> bool {
            candidate_api_format == "same"
        }

        async fn list_candidates_for_api_format(
            &self,
            candidate_api_format: &str,
            matches_client_format: bool,
        ) -> Result<(Vec<Self::Candidate>, Vec<Self::Skipped>), Self::Error> {
            self.calls.lock().unwrap().push(format!(
                "list:{candidate_api_format}:{matches_client_format}"
            ));
            Ok(match candidate_api_format {
                "same" => (vec!["candidate-a"], vec!["skip-a"]),
                "cross" => (
                    vec!["candidate-a", "candidate-b", "blocked-candidate"],
                    vec!["skip-a", "skip-b", "blocked-skip"],
                ),
                _ => (Vec::new(), Vec::new()),
            })
        }

        fn candidate_allowed(
            &self,
            candidate: &Self::Candidate,
            _candidate_api_format: &str,
            matches_client_format: bool,
        ) -> bool {
            matches_client_format || *candidate != "blocked-candidate"
        }

        fn skipped_candidate_allowed(
            &self,
            skipped_candidate: &Self::Skipped,
            _candidate_api_format: &str,
            matches_client_format: bool,
        ) -> bool {
            matches_client_format || *skipped_candidate != "blocked-skip"
        }

        fn candidate_key(&self, candidate: &Self::Candidate) -> String {
            (*candidate).to_string()
        }

        fn skipped_candidate_key(&self, skipped_candidate: &Self::Skipped) -> String {
            (*skipped_candidate).to_string()
        }
    }

    #[tokio::test]
    async fn preselection_runs_formats_in_order_filters_cross_format_and_dedupes() {
        let port = TestPort::default();

        let outcome = run_ai_candidate_preselection(&port).await.unwrap();

        assert_eq!(outcome.candidates, ["candidate-a", "candidate-b"]);
        assert_eq!(outcome.skipped_candidates, ["skip-a", "skip-b"]);
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            ["list:same:true", "list:cross:false"]
        );
    }
}
