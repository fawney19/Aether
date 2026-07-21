use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AiSyncDecisionStep {
    VideoTaskFollowUp,
    LocalVideo,
    LocalImage,
    LocalOpenAiChat,
    LocalOpenAiResponses,
    LocalStandardFamily,
    LocalSameFormatProvider,
    LocalGeminiFiles,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiStreamDecisionStep {
    LocalVideoContent,
    LocalImage,
    LocalOpenAiChat,
    LocalOpenAiResponses,
    LocalStandardFamily,
    LocalSameFormatProvider,
    LocalGeminiFiles,
}

#[async_trait]
pub trait AiSyncDecisionPathPort: Send + Sync {
    type Decision: Send;
    type Error: Send;

    fn sync_decision_step_enabled(&self, step: AiSyncDecisionStep) -> bool;

    async fn build_sync_decision_step(
        &self,
        step: AiSyncDecisionStep,
    ) -> Result<Option<Self::Decision>, Self::Error>;
}

#[async_trait]
pub trait AiStreamDecisionPathPort: Send + Sync {
    type Decision: Send;
    type Error: Send;

    async fn build_stream_decision_step(
        &self,
        step: AiStreamDecisionStep,
    ) -> Result<Option<Self::Decision>, Self::Error>;
}

pub async fn run_ai_sync_decision_path<Port>(
    port: &Port,
) -> Result<Option<Port::Decision>, Port::Error>
where
    Port: AiSyncDecisionPathPort,
{
    for step in [
        AiSyncDecisionStep::VideoTaskFollowUp,
        AiSyncDecisionStep::LocalVideo,
        AiSyncDecisionStep::LocalImage,
        AiSyncDecisionStep::LocalOpenAiChat,
        AiSyncDecisionStep::LocalOpenAiResponses,
        AiSyncDecisionStep::LocalStandardFamily,
        AiSyncDecisionStep::LocalSameFormatProvider,
        AiSyncDecisionStep::LocalGeminiFiles,
    ] {
        if !port.sync_decision_step_enabled(step) {
            continue;
        }
        if let Some(decision) = port.build_sync_decision_step(step).await? {
            return Ok(Some(decision));
        }
    }

    Ok(None)
}

pub async fn run_ai_stream_decision_path<Port>(
    port: &Port,
) -> Result<Option<Port::Decision>, Port::Error>
where
    Port: AiStreamDecisionPathPort,
{
    for step in [
        AiStreamDecisionStep::LocalVideoContent,
        AiStreamDecisionStep::LocalImage,
        AiStreamDecisionStep::LocalOpenAiChat,
        AiStreamDecisionStep::LocalOpenAiResponses,
        AiStreamDecisionStep::LocalStandardFamily,
        AiStreamDecisionStep::LocalSameFormatProvider,
        AiStreamDecisionStep::LocalGeminiFiles,
    ] {
        if let Some(decision) = port.build_stream_decision_step(step).await? {
            return Ok(Some(decision));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, VecDeque};
    use std::sync::Mutex;

    #[derive(Default)]
    struct TestSyncDecisionPort {
        disabled: BTreeSet<AiSyncDecisionStep>,
        outcomes: Mutex<VecDeque<Option<&'static str>>>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl AiSyncDecisionPathPort for TestSyncDecisionPort {
        type Decision = &'static str;
        type Error = std::convert::Infallible;

        fn sync_decision_step_enabled(&self, step: AiSyncDecisionStep) -> bool {
            !self.disabled.contains(&step)
        }

        async fn build_sync_decision_step(
            &self,
            step: AiSyncDecisionStep,
        ) -> Result<Option<Self::Decision>, Self::Error> {
            self.calls.lock().unwrap().push(format!("{step:?}"));
            Ok(self.outcomes.lock().unwrap().pop_front().unwrap_or(None))
        }
    }

    #[derive(Default)]
    struct TestStreamDecisionPort {
        outcomes: Mutex<VecDeque<Option<&'static str>>>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl AiStreamDecisionPathPort for TestStreamDecisionPort {
        type Decision = &'static str;
        type Error = std::convert::Infallible;

        async fn build_stream_decision_step(
            &self,
            step: AiStreamDecisionStep,
        ) -> Result<Option<Self::Decision>, Self::Error> {
            self.calls.lock().unwrap().push(format!("{step:?}"));
            Ok(self.outcomes.lock().unwrap().pop_front().unwrap_or(None))
        }
    }

    #[tokio::test]
    async fn sync_decision_path_runs_steps_in_serving_order() {
        let port = TestSyncDecisionPort::default();

        let decision = run_ai_sync_decision_path(&port).await.unwrap();

        assert_eq!(decision, None);
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
            ]
        );
    }

    #[tokio::test]
    async fn sync_decision_path_skips_disabled_steps_and_stops_at_first_decision() {
        let port = TestSyncDecisionPort {
            disabled: BTreeSet::from([AiSyncDecisionStep::LocalGeminiFiles]),
            outcomes: Mutex::new(VecDeque::from([
                None,
                None,
                Some("image_decision"),
                Some("should_not_run"),
            ])),
            calls: Mutex::default(),
        };

        let decision = run_ai_sync_decision_path(&port).await.unwrap();

        assert_eq!(decision, Some("image_decision"));
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            ["VideoTaskFollowUp", "LocalVideo", "LocalImage"]
        );
    }

    #[tokio::test]
    async fn stream_decision_path_stops_at_first_decision() {
        let port = TestStreamDecisionPort {
            outcomes: Mutex::new(VecDeque::from([
                None,
                None,
                Some("chat_decision"),
                Some("should_not_run"),
            ])),
            calls: Mutex::default(),
        };

        let decision = run_ai_stream_decision_path(&port).await.unwrap();

        assert_eq!(decision, Some("chat_decision"));
        assert_eq!(
            port.calls.lock().unwrap().as_slice(),
            ["LocalVideoContent", "LocalImage", "LocalOpenAiChat"]
        );
    }
}
