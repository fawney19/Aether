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
}

#[derive(Debug)]
pub enum AiAttemptLoopOutcome<Response, Exhaustion> {
    Responded(Response),
    Exhausted(Exhaustion),
    NoPath,
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
    ) -> Result<Option<Self::Response>, Self::Error>;

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
    let mut last_attempted = None;

    while let Some(attempt) = remaining.next() {
        let response = match port.execute_attempt(&attempt).await {
            Ok(response) => response,
            Err(err) => {
                port.mark_unused_attempts(remaining.collect()).await?;
                return Err(err);
            }
        };
        if let Some(response) = response {
            port.mark_unused_attempts(remaining.collect()).await?;
            return Ok(AiAttemptLoopOutcome::Responded(response));
        }

        // Exhaustion diagnostics are only needed after an attempt fails. Keep
        // the common successful path free of a deep plan/report-context clone.
        last_attempted = Some((attempt.execution_plan().clone(), attempt.report_context()));
    }

    let Some((last_plan, last_report_context)) = last_attempted else {
        return Ok(AiAttemptLoopOutcome::NoPath);
    };

    Ok(AiAttemptLoopOutcome::Exhausted(
        port.build_exhaustion(last_plan, last_report_context)
            .await?,
    ))
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
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::{run_ai_attempt_loop, AiAttemptLoopPort, AiExecutionAttempt};

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

    #[async_trait]
    impl AiAttemptLoopPort<TestAttempt> for FailingPort {
        type Response = ();
        type Exhaustion = ();
        type Error = &'static str;

        async fn execute_attempt(
            &self,
            attempt: &TestAttempt,
        ) -> Result<Option<Self::Response>, Self::Error> {
            if attempt.id == self.fail_on {
                Err("attempt failed")
            } else {
                Ok(None)
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
}
