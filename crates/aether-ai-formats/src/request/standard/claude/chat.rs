use crate::contracts::{CLAUDE_CHAT_STREAM_PLAN_KIND, CLAUDE_CHAT_SYNC_PLAN_KIND};
use crate::request::standard::family::{
    LocalStandardSourceFamily, LocalStandardSourceMode, LocalStandardSpec,
};

pub fn resolve_sync_spec(plan_kind: &str) -> Option<LocalStandardSpec> {
    match plan_kind {
        CLAUDE_CHAT_SYNC_PLAN_KIND => Some(LocalStandardSpec {
            api_format: "claude:messages",
            decision_kind: CLAUDE_CHAT_SYNC_PLAN_KIND,
            report_kind: "claude_chat_sync_finalize",
            family: LocalStandardSourceFamily::Standard,
            mode: LocalStandardSourceMode::Chat,
            require_streaming: false,
        }),
        _ => None,
    }
}

pub fn resolve_stream_spec(plan_kind: &str) -> Option<LocalStandardSpec> {
    match plan_kind {
        CLAUDE_CHAT_STREAM_PLAN_KIND => Some(LocalStandardSpec {
            api_format: "claude:messages",
            decision_kind: CLAUDE_CHAT_STREAM_PLAN_KIND,
            report_kind: "claude_chat_stream_success",
            family: LocalStandardSourceFamily::Standard,
            mode: LocalStandardSourceMode::Chat,
            require_streaming: true,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_stream_spec, resolve_sync_spec};

    #[test]
    fn resolves_claude_chat_sync_spec() {
        let spec = resolve_sync_spec("claude_chat_sync").expect("spec");
        assert_eq!(spec.api_format, "claude:messages");
        assert_eq!(spec.report_kind, "claude_chat_sync_finalize");
        assert!(!spec.require_streaming);
    }

    #[test]
    fn resolves_claude_chat_stream_spec() {
        let spec = resolve_stream_spec("claude_chat_stream").expect("spec");
        assert_eq!(spec.api_format, "claude:messages");
        assert_eq!(spec.report_kind, "claude_chat_stream_success");
        assert!(spec.require_streaming);
    }
}
