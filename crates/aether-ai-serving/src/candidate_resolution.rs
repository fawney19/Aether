use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiCandidateResolutionMode {
    Standard,
    WithoutTransportPairGate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AiCandidateResolutionRequest<'a> {
    pub client_api_format: &'a str,
    pub requested_model: Option<&'a str>,
    pub mode: AiCandidateResolutionMode,
}

impl<'a> AiCandidateResolutionRequest<'a> {
    pub fn standard(client_api_format: &'a str, requested_model: Option<&'a str>) -> Self {
        Self {
            client_api_format,
            requested_model,
            mode: AiCandidateResolutionMode::Standard,
        }
    }

    pub fn without_transport_pair_gate(
        client_api_format: &'a str,
        requested_model: Option<&'a str>,
    ) -> Self {
        Self {
            client_api_format,
            requested_model,
            mode: AiCandidateResolutionMode::WithoutTransportPairGate,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiCandidateResolutionOutcome<Eligible, Skipped> {
    pub eligible_candidates: Vec<Eligible>,
    pub skipped_candidates: Vec<Skipped>,
}

#[async_trait]
pub trait AiCandidateResolutionPort: Send + Sync {
    type Candidate: Send;
    type Transport: Send + Sync;
    type Eligible: Send + Sync;
    type Skipped: Send;
    type Error: Send;

    async fn read_candidate_transport(
        &self,
        candidate: &Self::Candidate,
    ) -> Result<Option<Self::Transport>, Self::Error>;

    fn build_missing_transport_skipped_candidate(
        &self,
        candidate: Self::Candidate,
    ) -> Self::Skipped;

    fn candidate_common_skip_reason(
        &self,
        candidate: &Self::Candidate,
        transport: &Self::Transport,
        requested_model: Option<&str>,
    ) -> Option<&'static str>;

    fn candidate_transport_pair_skip_reason(
        &self,
        candidate: &Self::Candidate,
        transport: &Self::Transport,
        normalized_client_api_format: &str,
        requested_model: &str,
    ) -> Option<&'static str>;

    fn build_skipped_candidate(
        &self,
        candidate: Self::Candidate,
        transport: Self::Transport,
        skip_reason: &'static str,
    ) -> Self::Skipped;

    fn build_eligible_candidate(
        &self,
        candidate: Self::Candidate,
        transport: Self::Transport,
    ) -> Self::Eligible;

    async fn rank_eligible_candidates(
        &self,
        candidates: Vec<Self::Eligible>,
        normalized_client_api_format: &str,
    ) -> Result<Vec<Self::Eligible>, Self::Error>;

    async fn apply_pool_scheduler(
        &self,
        candidates: Vec<Self::Eligible>,
    ) -> Result<(Vec<Self::Eligible>, Vec<Self::Skipped>), Self::Error>;
}

pub async fn run_ai_candidate_resolution<Port>(
    port: &Port,
    candidates: Vec<Port::Candidate>,
    request: AiCandidateResolutionRequest<'_>,
) -> Result<AiCandidateResolutionOutcome<Port::Eligible, Port::Skipped>, Port::Error>
where
    Port: AiCandidateResolutionPort,
{
    let normalized_client_api_format = request.client_api_format.trim().to_ascii_lowercase();
    let requested_model = request
        .requested_model
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mut eligible = Vec::with_capacity(candidates.len());
    let mut skipped = Vec::with_capacity(candidates.len());

    for candidate in candidates {
        let Some(transport) = port.read_candidate_transport(&candidate).await? else {
            skipped.push(port.build_missing_transport_skipped_candidate(candidate));
            continue;
        };

        match candidate_skip_reason_for_mode(
            port,
            request.mode,
            &candidate,
            &transport,
            normalized_client_api_format.as_str(),
            requested_model,
        ) {
            Some(skip_reason) => {
                skipped.push(port.build_skipped_candidate(candidate, transport, skip_reason));
            }
            None => {
                eligible.push(port.build_eligible_candidate(candidate, transport));
            }
        }
    }

    let ranked = port
        .rank_eligible_candidates(eligible, normalized_client_api_format.as_str())
        .await?;
    let (ranked, pool_skipped) = port.apply_pool_scheduler(ranked).await?;
    skipped.extend(pool_skipped);

    Ok(AiCandidateResolutionOutcome {
        eligible_candidates: ranked,
        skipped_candidates: skipped,
    })
}

fn candidate_skip_reason_for_mode<Port>(
    port: &Port,
    mode: AiCandidateResolutionMode,
    candidate: &Port::Candidate,
    transport: &Port::Transport,
    normalized_client_api_format: &str,
    requested_model: Option<&str>,
) -> Option<&'static str>
where
    Port: AiCandidateResolutionPort,
{
    port.candidate_common_skip_reason(candidate, transport, requested_model)
        .or_else(|| match mode {
            AiCandidateResolutionMode::Standard => port.candidate_transport_pair_skip_reason(
                candidate,
                transport,
                normalized_client_api_format,
                requested_model.unwrap_or_default(),
            ),
            AiCandidateResolutionMode::WithoutTransportPairGate => None,
        })
}

pub fn extract_ai_pool_sticky_session_token(body_json: &serde_json::Value) -> Option<String> {
    fn non_empty_str(value: Option<&serde_json::Value>) -> Option<&str> {
        value
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    let object = body_json.as_object()?;

    non_empty_str(object.get("prompt_cache_key"))
        .or_else(|| non_empty_str(object.get("conversation_id")))
        .or_else(|| non_empty_str(object.get("conversationId")))
        .or_else(|| non_empty_str(object.get("session_id")))
        .or_else(|| non_empty_str(object.get("sessionId")))
        .or_else(|| {
            object
                .get("metadata")
                .and_then(serde_json::Value::as_object)
                .and_then(|metadata| {
                    non_empty_str(metadata.get("session_id"))
                        .or_else(|| non_empty_str(metadata.get("conversation_id")))
                })
        })
        .or_else(|| {
            object
                .get("conversationState")
                .and_then(serde_json::Value::as_object)
                .and_then(|state| {
                    non_empty_str(state.get("conversationId"))
                        .or_else(|| non_empty_str(state.get("sessionId")))
                })
        })
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct TestPort {
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl AiCandidateResolutionPort for TestPort {
        type Candidate = &'static str;
        type Transport = &'static str;
        type Eligible = String;
        type Skipped = String;
        type Error = std::convert::Infallible;

        async fn read_candidate_transport(
            &self,
            candidate: &Self::Candidate,
        ) -> Result<Option<Self::Transport>, Self::Error> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("transport:{candidate}"));
            Ok(match *candidate {
                "missing" => None,
                "inactive" => Some("inactive-transport"),
                _ => Some("active-transport"),
            })
        }

        fn build_missing_transport_skipped_candidate(
            &self,
            candidate: Self::Candidate,
        ) -> Self::Skipped {
            format!("{candidate}:transport_snapshot_missing")
        }

        fn candidate_common_skip_reason(
            &self,
            candidate: &Self::Candidate,
            _transport: &Self::Transport,
            requested_model: Option<&str>,
        ) -> Option<&'static str> {
            self.calls.lock().unwrap().push(format!(
                "common:{candidate}:{}",
                requested_model.unwrap_or_default()
            ));
            (*candidate == "inactive").then_some("provider_inactive")
        }

        fn candidate_transport_pair_skip_reason(
            &self,
            candidate: &Self::Candidate,
            _transport: &Self::Transport,
            normalized_client_api_format: &str,
            requested_model: &str,
        ) -> Option<&'static str> {
            self.calls.lock().unwrap().push(format!(
                "pair:{candidate}:{normalized_client_api_format}:{requested_model}"
            ));
            (*candidate == "unsupported").then_some("transport_unsupported")
        }

        fn build_skipped_candidate(
            &self,
            candidate: Self::Candidate,
            _transport: Self::Transport,
            skip_reason: &'static str,
        ) -> Self::Skipped {
            format!("{candidate}:{skip_reason}")
        }

        fn build_eligible_candidate(
            &self,
            candidate: Self::Candidate,
            _transport: Self::Transport,
        ) -> Self::Eligible {
            format!("eligible:{candidate}")
        }

        async fn rank_eligible_candidates(
            &self,
            mut candidates: Vec<Self::Eligible>,
            normalized_client_api_format: &str,
        ) -> Result<Vec<Self::Eligible>, Self::Error> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("rank:{normalized_client_api_format}"));
            candidates.reverse();
            Ok(candidates)
        }

        async fn apply_pool_scheduler(
            &self,
            candidates: Vec<Self::Eligible>,
        ) -> Result<(Vec<Self::Eligible>, Vec<Self::Skipped>), Self::Error> {
            self.calls.lock().unwrap().push("pool".to_string());
            Ok((candidates, vec!["pool:cooldown".to_string()]))
        }
    }

    #[tokio::test]
    async fn resolution_reads_transport_gates_candidates_then_ranks_and_applies_pool() {
        let port = TestPort::default();

        let outcome = run_ai_candidate_resolution(
            &port,
            vec!["first", "missing", "inactive", "unsupported", "second"],
            AiCandidateResolutionRequest::standard(" OpenAI:Chat ", Some(" gpt-4.1 ")),
        )
        .await
        .unwrap();

        assert_eq!(
            outcome.eligible_candidates,
            ["eligible:second", "eligible:first"]
        );
        assert_eq!(
            outcome.skipped_candidates,
            [
                "missing:transport_snapshot_missing",
                "inactive:provider_inactive",
                "unsupported:transport_unsupported",
                "pool:cooldown",
            ]
        );
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            [
                "transport:first",
                "common:first:gpt-4.1",
                "pair:first:openai:chat:gpt-4.1",
                "transport:missing",
                "transport:inactive",
                "common:inactive:gpt-4.1",
                "transport:unsupported",
                "common:unsupported:gpt-4.1",
                "pair:unsupported:openai:chat:gpt-4.1",
                "transport:second",
                "common:second:gpt-4.1",
                "pair:second:openai:chat:gpt-4.1",
                "rank:openai:chat",
                "pool",
            ]
        );
    }

    #[tokio::test]
    async fn resolution_mode_can_skip_transport_pair_gate() {
        let port = TestPort::default();

        let outcome = run_ai_candidate_resolution(
            &port,
            vec!["unsupported"],
            AiCandidateResolutionRequest::without_transport_pair_gate("openai:chat", None),
        )
        .await
        .unwrap();

        assert_eq!(outcome.eligible_candidates, ["eligible:unsupported"]);
        assert_eq!(outcome.skipped_candidates, ["pool:cooldown"]);
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            [
                "transport:unsupported",
                "common:unsupported:",
                "rank:openai:chat",
                "pool",
            ]
        );
    }

    #[test]
    fn sticky_session_token_is_extracted_from_known_request_fields() {
        assert_eq!(
            extract_ai_pool_sticky_session_token(&json!({
                "prompt_cache_key": " cache-a ",
                "conversation_id": "conversation-b"
            }))
            .as_deref(),
            Some("cache-a")
        );

        assert_eq!(
            extract_ai_pool_sticky_session_token(&json!({
                "metadata": {"conversation_id": " conversation-c "}
            }))
            .as_deref(),
            Some("conversation-c")
        );

        assert_eq!(
            extract_ai_pool_sticky_session_token(&json!({
                "conversationState": {"sessionId": " session-d "}
            }))
            .as_deref(),
            Some("session-d")
        );
    }
}
