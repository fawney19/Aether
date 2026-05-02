use crate::contracts::{
    OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND, OPENAI_RESPONSES_COMPACT_STREAM_SUCCESS_REPORT_KIND,
    OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND, OPENAI_RESPONSES_COMPACT_SYNC_SUCCESS_REPORT_KIND,
    OPENAI_RESPONSES_STREAM_PLAN_KIND, OPENAI_RESPONSES_STREAM_SUCCESS_REPORT_KIND,
    OPENAI_RESPONSES_SYNC_PLAN_KIND, OPENAI_RESPONSES_SYNC_SUCCESS_REPORT_KIND,
};

#[derive(Debug, Clone, Copy)]
pub struct LocalOpenAiResponsesSpec {
    pub api_format: &'static str,
    pub decision_kind: &'static str,
    pub report_kind: &'static str,
    pub compact: bool,
    pub require_streaming: bool,
}

pub fn resolve_sync_spec(plan_kind: &str) -> Option<LocalOpenAiResponsesSpec> {
    match plan_kind {
        OPENAI_RESPONSES_SYNC_PLAN_KIND => Some(LocalOpenAiResponsesSpec {
            api_format: "openai:responses",
            decision_kind: OPENAI_RESPONSES_SYNC_PLAN_KIND,
            report_kind: OPENAI_RESPONSES_SYNC_SUCCESS_REPORT_KIND,
            compact: false,
            require_streaming: false,
        }),
        OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND => Some(LocalOpenAiResponsesSpec {
            api_format: "openai:responses:compact",
            decision_kind: OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND,
            report_kind: OPENAI_RESPONSES_COMPACT_SYNC_SUCCESS_REPORT_KIND,
            compact: true,
            require_streaming: false,
        }),
        _ => None,
    }
}

pub fn resolve_stream_spec(plan_kind: &str) -> Option<LocalOpenAiResponsesSpec> {
    match plan_kind {
        OPENAI_RESPONSES_STREAM_PLAN_KIND => Some(LocalOpenAiResponsesSpec {
            api_format: "openai:responses",
            decision_kind: OPENAI_RESPONSES_STREAM_PLAN_KIND,
            report_kind: OPENAI_RESPONSES_STREAM_SUCCESS_REPORT_KIND,
            compact: false,
            require_streaming: true,
        }),
        OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND => Some(LocalOpenAiResponsesSpec {
            api_format: "openai:responses:compact",
            decision_kind: OPENAI_RESPONSES_COMPACT_STREAM_PLAN_KIND,
            report_kind: OPENAI_RESPONSES_COMPACT_STREAM_SUCCESS_REPORT_KIND,
            compact: true,
            require_streaming: true,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_stream_spec, resolve_sync_spec};

    #[test]
    fn resolves_openai_responses_sync_spec() {
        let spec = resolve_sync_spec("openai_responses_sync").expect("spec");
        assert_eq!(spec.api_format, "openai:responses");
        assert_eq!(spec.report_kind, "openai_responses_sync_success");
        assert!(!spec.compact);
        assert!(!spec.require_streaming);
    }

    #[test]
    fn resolves_openai_responses_compact_stream_spec() {
        let spec = resolve_stream_spec("openai_responses_compact_stream").expect("spec");
        assert_eq!(spec.api_format, "openai:responses:compact");
        assert_eq!(spec.report_kind, "openai_responses_compact_stream_success");
        assert!(spec.compact);
        assert!(spec.require_streaming);
    }
}
