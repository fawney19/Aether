use crate::contracts::{
    GEMINI_FILES_DELETE_PLAN_KIND, GEMINI_FILES_DOWNLOAD_PLAN_KIND, GEMINI_FILES_GET_PLAN_KIND,
    GEMINI_FILES_LIST_PLAN_KIND, GEMINI_FILES_UPLOAD_PLAN_KIND,
};

#[derive(Debug, Clone, Copy)]
pub struct LocalGeminiFilesSpec {
    pub decision_kind: &'static str,
    pub report_kind: Option<&'static str>,
    pub require_streaming: bool,
}

pub fn resolve_sync_spec(plan_kind: &str) -> Option<LocalGeminiFilesSpec> {
    match plan_kind {
        GEMINI_FILES_UPLOAD_PLAN_KIND => Some(LocalGeminiFilesSpec {
            decision_kind: GEMINI_FILES_UPLOAD_PLAN_KIND,
            report_kind: Some("gemini_files_store_mapping"),
            require_streaming: false,
        }),
        GEMINI_FILES_LIST_PLAN_KIND => Some(LocalGeminiFilesSpec {
            decision_kind: GEMINI_FILES_LIST_PLAN_KIND,
            report_kind: Some("gemini_files_store_mapping"),
            require_streaming: false,
        }),
        GEMINI_FILES_GET_PLAN_KIND => Some(LocalGeminiFilesSpec {
            decision_kind: GEMINI_FILES_GET_PLAN_KIND,
            report_kind: Some("gemini_files_store_mapping"),
            require_streaming: false,
        }),
        GEMINI_FILES_DELETE_PLAN_KIND => Some(LocalGeminiFilesSpec {
            decision_kind: GEMINI_FILES_DELETE_PLAN_KIND,
            report_kind: Some("gemini_files_delete_mapping"),
            require_streaming: false,
        }),
        _ => None,
    }
}

pub fn resolve_stream_spec(plan_kind: &str) -> Option<LocalGeminiFilesSpec> {
    match plan_kind {
        GEMINI_FILES_DOWNLOAD_PLAN_KIND => Some(LocalGeminiFilesSpec {
            decision_kind: GEMINI_FILES_DOWNLOAD_PLAN_KIND,
            report_kind: None,
            require_streaming: true,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_stream_spec, resolve_sync_spec};

    #[test]
    fn resolves_sync_gemini_files_specs() {
        let spec = resolve_sync_spec("gemini_files_upload").expect("spec");
        assert_eq!(spec.decision_kind, "gemini_files_upload");
        assert_eq!(spec.report_kind, Some("gemini_files_store_mapping"));
        assert!(!spec.require_streaming);
    }

    #[test]
    fn resolves_stream_gemini_files_spec() {
        let spec = resolve_stream_spec("gemini_files_download").expect("spec");
        assert_eq!(spec.decision_kind, "gemini_files_download");
        assert_eq!(spec.report_kind, None);
        assert!(spec.require_streaming);
    }
}
