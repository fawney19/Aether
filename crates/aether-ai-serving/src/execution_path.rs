use async_trait::async_trait;

#[derive(Debug)]
pub enum AiServingExecutionOutcome<Response, Exhaustion> {
    Responded(Response),
    Exhausted(Exhaustion),
    NoPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiPlanFallbackReason {
    RemoteDecisionMiss,
    SchedulerDecisionUnsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiSyncExecutionStep {
    VideoTaskFollowUp,
    LocalVideo,
    LocalImage,
    LocalOpenAiChat,
    LocalOpenAiResponses,
    LocalStandardFamily,
    LocalSameFormatProvider,
    LocalGeminiFiles,
    RemoteDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiStreamExecutionStep {
    LocalVideoContent,
    LocalImage,
    LocalOpenAiChat,
    LocalOpenAiResponses,
    LocalStandardFamily,
    LocalSameFormatProvider,
    LocalGeminiFiles,
    RemoteDecision,
}

#[async_trait]
pub trait AiSyncExecutionPathPort: Send + Sync {
    type Response: Send;
    type Exhaustion: Send;
    type Error: Send;

    fn scheduler_decision_supported(&self) -> bool;

    async fn execute_sync_step(
        &self,
        step: AiSyncExecutionStep,
    ) -> Result<AiServingExecutionOutcome<Self::Response, Self::Exhaustion>, Self::Error>;

    async fn execute_sync_plan_fallback(
        &self,
        reason: AiPlanFallbackReason,
    ) -> Result<AiServingExecutionOutcome<Self::Response, Self::Exhaustion>, Self::Error>;
}

#[async_trait]
pub trait AiStreamExecutionPathPort: Send + Sync {
    type Response: Send;
    type Exhaustion: Send;
    type Error: Send;

    fn scheduler_decision_supported(&self) -> bool;

    async fn execute_stream_step(
        &self,
        step: AiStreamExecutionStep,
    ) -> Result<AiServingExecutionOutcome<Self::Response, Self::Exhaustion>, Self::Error>;

    async fn execute_stream_plan_fallback(
        &self,
        reason: AiPlanFallbackReason,
    ) -> Result<AiServingExecutionOutcome<Self::Response, Self::Exhaustion>, Self::Error>;
}

pub async fn run_ai_sync_execution_path<Port>(
    port: &Port,
) -> Result<AiServingExecutionOutcome<Port::Response, Port::Exhaustion>, Port::Error>
where
    Port: AiSyncExecutionPathPort,
{
    let mut exhausted = None;

    if let Some(response) =
        absorb_sync_step(port, AiSyncExecutionStep::VideoTaskFollowUp, &mut exhausted).await?
    {
        return Ok(response);
    }

    if port.scheduler_decision_supported() {
        for step in [
            AiSyncExecutionStep::LocalVideo,
            AiSyncExecutionStep::LocalImage,
            AiSyncExecutionStep::LocalOpenAiChat,
            AiSyncExecutionStep::LocalOpenAiResponses,
            AiSyncExecutionStep::LocalStandardFamily,
            AiSyncExecutionStep::LocalSameFormatProvider,
            AiSyncExecutionStep::LocalGeminiFiles,
            AiSyncExecutionStep::RemoteDecision,
        ] {
            if let Some(response) = absorb_sync_step(port, step, &mut exhausted).await? {
                return Ok(response);
            }
        }
    }

    let fallback_reason = if port.scheduler_decision_supported() {
        AiPlanFallbackReason::RemoteDecisionMiss
    } else {
        AiPlanFallbackReason::SchedulerDecisionUnsupported
    };
    match port.execute_sync_plan_fallback(fallback_reason).await? {
        AiServingExecutionOutcome::Responded(response) => {
            Ok(AiServingExecutionOutcome::Responded(response))
        }
        AiServingExecutionOutcome::Exhausted(outcome) => {
            Ok(AiServingExecutionOutcome::Exhausted(outcome))
        }
        AiServingExecutionOutcome::NoPath => Ok(exhausted
            .map(AiServingExecutionOutcome::Exhausted)
            .unwrap_or(AiServingExecutionOutcome::NoPath)),
    }
}

pub async fn run_ai_stream_execution_path<Port>(
    port: &Port,
) -> Result<AiServingExecutionOutcome<Port::Response, Port::Exhaustion>, Port::Error>
where
    Port: AiStreamExecutionPathPort,
{
    let mut exhausted = None;

    if let Some(response) = absorb_stream_step(
        port,
        AiStreamExecutionStep::LocalVideoContent,
        &mut exhausted,
    )
    .await?
    {
        return Ok(response);
    }

    if port.scheduler_decision_supported() {
        for step in [
            AiStreamExecutionStep::LocalImage,
            AiStreamExecutionStep::LocalOpenAiChat,
            AiStreamExecutionStep::LocalOpenAiResponses,
            AiStreamExecutionStep::LocalStandardFamily,
            AiStreamExecutionStep::LocalSameFormatProvider,
            AiStreamExecutionStep::LocalGeminiFiles,
            AiStreamExecutionStep::RemoteDecision,
        ] {
            if let Some(response) = absorb_stream_step(port, step, &mut exhausted).await? {
                return Ok(response);
            }
        }
    }

    let fallback_reason = if port.scheduler_decision_supported() {
        AiPlanFallbackReason::RemoteDecisionMiss
    } else {
        AiPlanFallbackReason::SchedulerDecisionUnsupported
    };
    match port.execute_stream_plan_fallback(fallback_reason).await? {
        AiServingExecutionOutcome::Responded(response) => {
            Ok(AiServingExecutionOutcome::Responded(response))
        }
        AiServingExecutionOutcome::Exhausted(outcome) => {
            Ok(AiServingExecutionOutcome::Exhausted(outcome))
        }
        AiServingExecutionOutcome::NoPath => Ok(exhausted
            .map(AiServingExecutionOutcome::Exhausted)
            .unwrap_or(AiServingExecutionOutcome::NoPath)),
    }
}

async fn absorb_sync_step<Port>(
    port: &Port,
    step: AiSyncExecutionStep,
    exhausted: &mut Option<Port::Exhaustion>,
) -> Result<Option<AiServingExecutionOutcome<Port::Response, Port::Exhaustion>>, Port::Error>
where
    Port: AiSyncExecutionPathPort,
{
    match port.execute_sync_step(step).await? {
        AiServingExecutionOutcome::Responded(response) => {
            Ok(Some(AiServingExecutionOutcome::Responded(response)))
        }
        AiServingExecutionOutcome::Exhausted(outcome) => {
            *exhausted = Some(outcome);
            Ok(None)
        }
        AiServingExecutionOutcome::NoPath => Ok(None),
    }
}

async fn absorb_stream_step<Port>(
    port: &Port,
    step: AiStreamExecutionStep,
    exhausted: &mut Option<Port::Exhaustion>,
) -> Result<Option<AiServingExecutionOutcome<Port::Response, Port::Exhaustion>>, Port::Error>
where
    Port: AiStreamExecutionPathPort,
{
    match port.execute_stream_step(step).await? {
        AiServingExecutionOutcome::Responded(response) => {
            Ok(Some(AiServingExecutionOutcome::Responded(response)))
        }
        AiServingExecutionOutcome::Exhausted(outcome) => {
            *exhausted = Some(outcome);
            Ok(None)
        }
        AiServingExecutionOutcome::NoPath => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[derive(Default)]
    struct TestSyncPort {
        scheduler_supported: bool,
        outcomes: Mutex<VecDeque<AiServingExecutionOutcome<&'static str, &'static str>>>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl AiSyncExecutionPathPort for TestSyncPort {
        type Response = &'static str;
        type Exhaustion = &'static str;
        type Error = std::convert::Infallible;

        fn scheduler_decision_supported(&self) -> bool {
            self.scheduler_supported
        }

        async fn execute_sync_step(
            &self,
            step: AiSyncExecutionStep,
        ) -> Result<AiServingExecutionOutcome<Self::Response, Self::Exhaustion>, Self::Error>
        {
            self.calls.lock().unwrap().push(format!("{step:?}"));
            Ok(self
                .outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(AiServingExecutionOutcome::NoPath))
        }

        async fn execute_sync_plan_fallback(
            &self,
            reason: AiPlanFallbackReason,
        ) -> Result<AiServingExecutionOutcome<Self::Response, Self::Exhaustion>, Self::Error>
        {
            self.calls
                .lock()
                .unwrap()
                .push(format!("Fallback:{reason:?}"));
            Ok(self
                .outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(AiServingExecutionOutcome::NoPath))
        }
    }

    #[derive(Default)]
    struct TestStreamPort {
        scheduler_supported: bool,
        outcomes: Mutex<VecDeque<AiServingExecutionOutcome<&'static str, &'static str>>>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl AiStreamExecutionPathPort for TestStreamPort {
        type Response = &'static str;
        type Exhaustion = &'static str;
        type Error = std::convert::Infallible;

        fn scheduler_decision_supported(&self) -> bool {
            self.scheduler_supported
        }

        async fn execute_stream_step(
            &self,
            step: AiStreamExecutionStep,
        ) -> Result<AiServingExecutionOutcome<Self::Response, Self::Exhaustion>, Self::Error>
        {
            self.calls.lock().unwrap().push(format!("{step:?}"));
            Ok(self
                .outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(AiServingExecutionOutcome::NoPath))
        }

        async fn execute_stream_plan_fallback(
            &self,
            reason: AiPlanFallbackReason,
        ) -> Result<AiServingExecutionOutcome<Self::Response, Self::Exhaustion>, Self::Error>
        {
            self.calls
                .lock()
                .unwrap()
                .push(format!("Fallback:{reason:?}"));
            Ok(self
                .outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(AiServingExecutionOutcome::NoPath))
        }
    }

    #[tokio::test]
    async fn sync_path_runs_scheduler_steps_before_remote_and_fallback() {
        let port = TestSyncPort {
            scheduler_supported: true,
            ..TestSyncPort::default()
        };

        let outcome = run_ai_sync_execution_path(&port).await.unwrap();

        assert!(matches!(outcome, AiServingExecutionOutcome::NoPath));
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            [
                "VideoTaskFollowUp",
                "LocalVideo",
                "LocalImage",
                "LocalOpenAiChat",
                "LocalOpenAiResponses",
                "LocalStandardFamily",
                "LocalSameFormatProvider",
                "LocalGeminiFiles",
                "RemoteDecision",
                "Fallback:RemoteDecisionMiss",
            ]
        );
    }

    #[tokio::test]
    async fn sync_path_returns_last_exhaustion_when_fallback_has_no_path() {
        let port = TestSyncPort {
            scheduler_supported: true,
            outcomes: Mutex::new(VecDeque::from([
                AiServingExecutionOutcome::NoPath,
                AiServingExecutionOutcome::Exhausted("local_video_exhausted"),
            ])),
            calls: Mutex::default(),
        };

        let outcome = run_ai_sync_execution_path(&port).await.unwrap();

        assert!(matches!(
            outcome,
            AiServingExecutionOutcome::Exhausted("local_video_exhausted")
        ));
    }

    #[tokio::test]
    async fn stream_path_skips_scheduler_steps_when_unsupported() {
        let port = TestStreamPort {
            scheduler_supported: false,
            ..TestStreamPort::default()
        };

        let outcome = run_ai_stream_execution_path(&port).await.unwrap();

        assert!(matches!(outcome, AiServingExecutionOutcome::NoPath));
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            ["LocalVideoContent", "Fallback:SchedulerDecisionUnsupported",]
        );
    }

    #[tokio::test]
    async fn stream_path_stops_at_first_response() {
        let port = TestStreamPort {
            scheduler_supported: true,
            outcomes: Mutex::new(VecDeque::from([
                AiServingExecutionOutcome::NoPath,
                AiServingExecutionOutcome::Responded("image_response"),
            ])),
            calls: Mutex::default(),
        };

        let outcome = run_ai_stream_execution_path(&port).await.unwrap();

        assert!(matches!(
            outcome,
            AiServingExecutionOutcome::Responded("image_response")
        ));
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            ["LocalVideoContent", "LocalImage"]
        );
    }
}
