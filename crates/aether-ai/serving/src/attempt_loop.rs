use async_trait::async_trait;

pub trait AiExecutionAttempt {
    fn execution_plan(&self) -> &aether_contracts::ExecutionPlan;

    fn report_kind(&self) -> Option<String>;

    fn report_context(&self) -> Option<serde_json::Value>;

    /// Borrow the stored report context when the attempt owns one. This keeps
    /// watchdog/telemetry paths from cloning a potentially large JSON value.
    /// Implementations that synthesize a context may use the default.
    fn report_context_ref(&self) -> Option<&serde_json::Value> {
        None
    }

    /// Re-issue this attempt against the same key as a fresh attempt with the
    /// given retry index and candidate id. Attempt types that cannot be
    /// re-issued return `None`, which disables same-key retries for them.
    fn with_same_key_retry(&self, _retry_index: u32, _candidate_id: String) -> Option<Self>
    where
        Self: Sized,
    {
        None
    }
}

/// Report-context field carrying the routing policy's sticky-key attempt
/// budget for the request, so the attempt loop can derive same-key retries
/// lazily instead of pre-materializing them.
pub const STICKY_KEY_ATTEMPTS_REPORT_FIELD: &str = "sticky_key_attempts";

#[derive(Debug)]
pub enum AiAttemptLoopOutcome<Response, Exhaustion> {
    Responded(Response),
    Deferred(Response),
    Exhausted(Exhaustion),
    NoPath,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AiAttemptRetryScope {
    #[default]
    Candidate,
    Credential,
    Endpoint,
    Provider,
}

#[derive(Debug)]
pub enum AiAttemptExecutionOutcome<Response> {
    Responded(Response),
    Retry {
        scope: AiAttemptRetryScope,
        fallback_response: Option<Response>,
    },
}

impl<Response> AiAttemptExecutionOutcome<Response> {
    pub fn retry(scope: AiAttemptRetryScope) -> Self {
        Self::Retry {
            scope,
            fallback_response: None,
        }
    }

    pub fn from_optional_response(response: Option<Response>) -> Self {
        match response {
            Some(response) => Self::Responded(response),
            None => Self::retry(AiAttemptRetryScope::Candidate),
        }
    }
}

#[async_trait]
pub trait AiAttemptLoopPort<Attempt>: Send + Sync
where
    Attempt: AiExecutionAttempt + Send + Sync + 'static,
{
    type Response: Send;
    type Exhaustion: Send;
    type Error: Send;

    async fn execute_attempt(
        &self,
        attempt: &Attempt,
    ) -> Result<AiAttemptExecutionOutcome<Self::Response>, Self::Error>;

    async fn should_skip_attempt(&self, _attempt: &Attempt) -> Result<bool, Self::Error> {
        Ok(false)
    }

    async fn record_attempt_started(&self, _attempt: &Attempt) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn record_attempt_failed(&self, _attempt: &Attempt) -> Result<(), Self::Error> {
        Ok(())
    }

    /// After `attempt` failed with candidate scope, return the next attempt on
    /// the same key, or `None` once the sticky-key budget is used up. Retries
    /// are derived here on demand so no attempt is materialized before it is
    /// actually needed.
    async fn next_same_key_retry(
        &self,
        _attempt: &Attempt,
    ) -> Result<Option<Attempt>, Self::Error> {
        Ok(None)
    }

    async fn mark_unused_attempts(&self, attempts: Vec<Attempt>) -> Result<(), Self::Error>;

    async fn build_exhaustion(
        &self,
        last_plan: aether_contracts::ExecutionPlan,
        last_report_context: Option<serde_json::Value>,
    ) -> Result<Self::Exhaustion, Self::Error>;
}

pub async fn run_ai_attempt_loop<Port, Attempt>(
    port: &Port,
    attempts: Vec<Attempt>,
) -> Result<AiAttemptLoopOutcome<Port::Response, Port::Exhaustion>, Port::Error>
where
    Port: AiAttemptLoopPort<Attempt>,
    Attempt: AiExecutionAttempt + Send + Sync + 'static,
{
    let mut remaining = attempts.into_iter();
    let mut pending_same_key_retry: Option<Attempt> = None;
    let mut last_attempted = None;
    let mut retry_filters: Vec<AiAttemptRetryFilter> = Vec::new();
    let mut fallback_response = None;

    loop {
        let Some(attempt) = pending_same_key_retry.take().or_else(|| remaining.next()) else {
            break;
        };
        if retry_filters.iter().any(|filter| filter.matches(&attempt))
            || port.should_skip_attempt(&attempt).await?
        {
            port.mark_unused_attempts(vec![attempt]).await?;
            continue;
        }
        port.record_attempt_started(&attempt).await?;
        let execution = match port.execute_attempt(&attempt).await {
            Ok(execution) => execution,
            Err(err) => {
                port.mark_unused_attempts(remaining.collect()).await?;
                return Err(err);
            }
        };
        match execution {
            AiAttemptExecutionOutcome::Responded(response) => {
                port.mark_unused_attempts(remaining.collect()).await?;
                return Ok(AiAttemptLoopOutcome::Responded(response));
            }
            AiAttemptExecutionOutcome::Retry {
                scope,
                fallback_response: attempt_fallback_response,
            } => {
                port.record_attempt_failed(&attempt).await?;
                if attempt_fallback_response.is_some() {
                    fallback_response = attempt_fallback_response;
                }
                if scope == AiAttemptRetryScope::Candidate {
                    pending_same_key_retry = port.next_same_key_retry(&attempt).await?;
                } else {
                    retry_filters.push(AiAttemptRetryFilter::new(&attempt, scope));
                }
            }
        }

        // Exhaustion diagnostics are only needed after an attempt fails. Keep
        // the common successful path free of a deep plan/report-context clone.
        last_attempted = Some((attempt.execution_plan().clone(), attempt.report_context()));
    }

    if let Some(response) = fallback_response {
        return Ok(AiAttemptLoopOutcome::Deferred(response));
    }

    let Some((last_plan, last_report_context)) = last_attempted else {
        return Ok(AiAttemptLoopOutcome::NoPath);
    };

    Ok(AiAttemptLoopOutcome::Exhausted(
        port.build_exhaustion(last_plan, last_report_context)
            .await?,
    ))
}

#[derive(Debug)]
struct AiAttemptRetryFilter {
    scope: AiAttemptRetryScope,
    provider_id: String,
    endpoint_id: String,
    key_id: String,
}

impl AiAttemptRetryFilter {
    fn new<Attempt: AiExecutionAttempt>(attempt: &Attempt, scope: AiAttemptRetryScope) -> Self {
        let plan = attempt.execution_plan();
        Self {
            scope,
            provider_id: plan.provider_id.clone(),
            endpoint_id: plan.endpoint_id.clone(),
            key_id: plan.key_id.clone(),
        }
    }

    fn matches<Attempt: AiExecutionAttempt>(&self, attempt: &Attempt) -> bool {
        let plan = attempt.execution_plan();
        match self.scope {
            AiAttemptRetryScope::Candidate => false,
            AiAttemptRetryScope::Credential => plan.key_id == self.key_id,
            AiAttemptRetryScope::Endpoint => plan.endpoint_id == self.endpoint_id,
            AiAttemptRetryScope::Provider => plan.provider_id == self.provider_id,
        }
    }
}

/// Clone `plan`/`report_context` for a same-key retry: only the candidate id
/// and retry index change, everything else (url, headers, body) is reused.
fn same_key_retry_parts(
    plan: &aether_contracts::ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    retry_index: u32,
    candidate_id: String,
) -> (aether_contracts::ExecutionPlan, Option<serde_json::Value>) {
    let mut plan = plan.clone();
    plan.candidate_id = Some(candidate_id.clone());
    let report_context = report_context.cloned().map(|mut value| {
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "candidate_id".to_string(),
                serde_json::Value::String(candidate_id),
            );
            object.insert(
                "retry_index".to_string(),
                serde_json::Value::Number(retry_index.into()),
            );
        }
        value
    });
    (plan, report_context)
}

impl AiExecutionAttempt for crate::dto::AiSyncAttempt {
    fn execution_plan(&self) -> &aether_contracts::ExecutionPlan {
        &self.plan
    }

    fn report_kind(&self) -> Option<String> {
        self.report_kind.clone()
    }

    fn report_context(&self) -> Option<serde_json::Value> {
        self.report_context.clone()
    }

    fn report_context_ref(&self) -> Option<&serde_json::Value> {
        self.report_context.as_ref()
    }

    fn with_same_key_retry(&self, retry_index: u32, candidate_id: String) -> Option<Self> {
        let (plan, report_context) = same_key_retry_parts(
            &self.plan,
            self.report_context.as_ref(),
            retry_index,
            candidate_id,
        );
        Some(Self {
            plan,
            report_kind: self.report_kind.clone(),
            report_context,
        })
    }
}

impl AiExecutionAttempt for crate::dto::AiStreamAttempt {
    fn execution_plan(&self) -> &aether_contracts::ExecutionPlan {
        &self.plan
    }

    fn report_kind(&self) -> Option<String> {
        self.report_kind.clone()
    }

    fn report_context(&self) -> Option<serde_json::Value> {
        self.report_context.clone()
    }

    fn report_context_ref(&self) -> Option<&serde_json::Value> {
        self.report_context.as_ref()
    }

    fn with_same_key_retry(&self, retry_index: u32, candidate_id: String) -> Option<Self> {
        let (plan, report_context) = same_key_retry_parts(
            &self.plan,
            self.report_context.as_ref(),
            retry_index,
            candidate_id,
        );
        Some(Self {
            plan,
            report_kind: self.report_kind.clone(),
            report_context,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::{
        run_ai_attempt_loop, AiAttemptExecutionOutcome, AiAttemptLoopPort, AiAttemptRetryScope,
        AiExecutionAttempt,
    };

    #[derive(Clone)]
    struct TestAttempt {
        id: &'static str,
        plan: aether_contracts::ExecutionPlan,
    }

    impl AiExecutionAttempt for TestAttempt {
        fn execution_plan(&self) -> &aether_contracts::ExecutionPlan {
            &self.plan
        }

        fn report_kind(&self) -> Option<String> {
            None
        }

        fn report_context(&self) -> Option<serde_json::Value> {
            None
        }
    }

    struct FailingPort {
        fail_on: &'static str,
        unused: Mutex<Vec<&'static str>>,
    }

    struct ScopedRetryPort {
        executed: Mutex<Vec<&'static str>>,
        unused: Mutex<Vec<&'static str>>,
    }

    #[async_trait]
    impl AiAttemptLoopPort<TestAttempt> for ScopedRetryPort {
        type Response = &'static str;
        type Exhaustion = ();
        type Error = &'static str;

        async fn execute_attempt(
            &self,
            attempt: &TestAttempt,
        ) -> Result<AiAttemptExecutionOutcome<Self::Response>, Self::Error> {
            self.executed
                .lock()
                .expect("executed attempts should lock")
                .push(attempt.id);
            Ok(match attempt.id {
                "endpoint-failure" => {
                    AiAttemptExecutionOutcome::retry(AiAttemptRetryScope::Endpoint)
                }
                "credential-failure" => {
                    AiAttemptExecutionOutcome::retry(AiAttemptRetryScope::Credential)
                }
                "provider-failure" => AiAttemptExecutionOutcome::Retry {
                    scope: AiAttemptRetryScope::Provider,
                    fallback_response: Some("provider-error"),
                },
                _ => AiAttemptExecutionOutcome::Responded(attempt.id),
            })
        }

        async fn mark_unused_attempts(
            &self,
            attempts: Vec<TestAttempt>,
        ) -> Result<(), Self::Error> {
            self.unused
                .lock()
                .expect("unused attempts should lock")
                .extend(attempts.into_iter().map(|attempt| attempt.id));
            Ok(())
        }

        async fn build_exhaustion(
            &self,
            _last_plan: aether_contracts::ExecutionPlan,
            _last_report_context: Option<serde_json::Value>,
        ) -> Result<Self::Exhaustion, Self::Error> {
            Ok(())
        }
    }

    #[async_trait]
    impl AiAttemptLoopPort<TestAttempt> for FailingPort {
        type Response = ();
        type Exhaustion = ();
        type Error = &'static str;

        async fn execute_attempt(
            &self,
            attempt: &TestAttempt,
        ) -> Result<AiAttemptExecutionOutcome<Self::Response>, Self::Error> {
            if attempt.id == self.fail_on {
                Err("attempt failed")
            } else {
                Ok(AiAttemptExecutionOutcome::retry(
                    AiAttemptRetryScope::Candidate,
                ))
            }
        }

        async fn mark_unused_attempts(
            &self,
            attempts: Vec<TestAttempt>,
        ) -> Result<(), Self::Error> {
            self.unused
                .lock()
                .expect("unused attempts should lock")
                .extend(attempts.into_iter().map(|attempt| attempt.id));
            Ok(())
        }

        async fn build_exhaustion(
            &self,
            _last_plan: aether_contracts::ExecutionPlan,
            _last_report_context: Option<serde_json::Value>,
        ) -> Result<Self::Exhaustion, Self::Error> {
            Ok(())
        }
    }

    fn attempt(id: &'static str) -> TestAttempt {
        TestAttempt {
            id,
            plan: aether_contracts::ExecutionPlan {
                request_id: format!("request-{id}"),
                candidate_id: Some(id.to_string()),
                provider_name: Some("provider".to_string()),
                provider_id: "provider-1".to_string(),
                endpoint_id: "endpoint-1".to_string(),
                key_id: "key-1".to_string(),
                method: "POST".to_string(),
                url: "https://example.test/v1/responses".to_string(),
                headers: BTreeMap::new(),
                content_type: Some("application/json".to_string()),
                content_encoding: None,
                body: aether_contracts::RequestBody::from_json(serde_json::json!({})),
                stream: false,
                client_api_format: "openai:responses".to_string(),
                provider_api_format: "openai:responses".to_string(),
                model_name: Some("gpt-5.6-sol".to_string()),
                proxy: None,
                transport_profile: None,
                timeouts: None,
            },
        }
    }

    fn routed_attempt(
        id: &'static str,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
    ) -> TestAttempt {
        let mut attempt = attempt(id);
        attempt.plan.provider_id = provider_id.to_string();
        attempt.plan.endpoint_id = endpoint_id.to_string();
        attempt.plan.key_id = key_id.to_string();
        attempt
    }

    #[tokio::test]
    async fn marks_unattempted_candidates_unused_when_execution_returns_error() {
        let port = FailingPort {
            fail_on: "candidate-2",
            unused: Mutex::new(Vec::new()),
        };

        let error = run_ai_attempt_loop(
            &port,
            vec![
                attempt("candidate-1"),
                attempt("candidate-2"),
                attempt("candidate-3"),
            ],
        )
        .await
        .expect_err("second attempt should fail");

        assert_eq!(error, "attempt failed");
        assert_eq!(
            *port.unused.lock().expect("unused attempts should lock"),
            vec!["candidate-3"]
        );
    }

    #[tokio::test]
    async fn retry_scopes_skip_matching_static_candidates() {
        let port = ScopedRetryPort {
            executed: Mutex::new(Vec::new()),
            unused: Mutex::new(Vec::new()),
        };
        let attempts = vec![
            routed_attempt("endpoint-failure", "provider-a", "endpoint-a", "key-a"),
            routed_attempt("same-endpoint", "provider-a", "endpoint-a", "key-b"),
            routed_attempt("credential-failure", "provider-a", "endpoint-b", "key-c"),
            routed_attempt("same-credential", "provider-a", "endpoint-c", "key-c"),
            routed_attempt("provider-failure", "provider-b", "endpoint-d", "key-d"),
            routed_attempt("same-provider", "provider-b", "endpoint-e", "key-e"),
            routed_attempt("success", "provider-c", "endpoint-f", "key-f"),
        ];

        let outcome = run_ai_attempt_loop(&port, attempts)
            .await
            .expect("scoped retry loop should succeed");

        assert!(matches!(
            outcome,
            super::AiAttemptLoopOutcome::Responded("success")
        ));
        assert_eq!(
            *port.executed.lock().expect("executed attempts should lock"),
            vec![
                "endpoint-failure",
                "credential-failure",
                "provider-failure",
                "success"
            ]
        );
        assert_eq!(
            *port.unused.lock().expect("unused attempts should lock"),
            vec!["same-endpoint", "same-credential", "same-provider"]
        );
    }

    #[tokio::test]
    async fn returns_preserved_upstream_response_after_candidates_exhaust() {
        let port = ScopedRetryPort {
            executed: Mutex::new(Vec::new()),
            unused: Mutex::new(Vec::new()),
        };
        let outcome = run_ai_attempt_loop(
            &port,
            vec![
                routed_attempt("provider-failure", "provider-a", "endpoint-a", "key-a"),
                routed_attempt("same-provider", "provider-a", "endpoint-b", "key-b"),
            ],
        )
        .await
        .expect("fallback response loop should succeed");

        assert!(matches!(
            outcome,
            super::AiAttemptLoopOutcome::Deferred("provider-error")
        ));
        assert_eq!(
            *port.unused.lock().expect("unused attempts should lock"),
            vec!["same-provider"]
        );
    }
}
