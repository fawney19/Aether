use crate::contracts::{
    OPENAI_EMBEDDING_SYNC_FINALIZE_REPORT_KIND, OPENAI_EMBEDDING_SYNC_PLAN_KIND,
};
use crate::formats::shared::family::{
    LocalStandardSourceFamily, LocalStandardSourceMode, LocalStandardSpec,
};

pub fn resolve_sync_spec(plan_kind: &str) -> Option<LocalStandardSpec> {
    match plan_kind {
        OPENAI_EMBEDDING_SYNC_PLAN_KIND => Some(LocalStandardSpec {
            api_format: "openai:embedding",
            decision_kind: OPENAI_EMBEDDING_SYNC_PLAN_KIND,
            report_kind: OPENAI_EMBEDDING_SYNC_FINALIZE_REPORT_KIND,
            family: LocalStandardSourceFamily::Standard,
            mode: LocalStandardSourceMode::Embedding,
            require_streaming: false,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_sync_spec;
    use crate::formats::shared::family::LocalStandardSourceMode;

    #[test]
    fn resolves_openai_embedding_sync_standard_spec() {
        let spec = resolve_sync_spec("openai_embedding_sync").expect("spec");
        assert_eq!(spec.api_format, "openai:embedding");
        assert_eq!(spec.report_kind, "openai_embedding_sync_finalize");
        assert_eq!(spec.mode, LocalStandardSourceMode::Embedding);
        assert!(!spec.require_streaming);
    }
}
