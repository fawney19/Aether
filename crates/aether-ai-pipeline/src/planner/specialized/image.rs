use crate::contracts::{OPENAI_IMAGE_STREAM_PLAN_KIND, OPENAI_IMAGE_SYNC_PLAN_KIND};

#[derive(Debug, Clone, Copy)]
pub struct LocalOpenAiImageSpec {
    pub api_format: &'static str,
    pub decision_kind: &'static str,
    pub report_kind: &'static str,
    pub require_streaming: bool,
}

pub fn resolve_sync_spec(plan_kind: &str) -> Option<LocalOpenAiImageSpec> {
    match plan_kind {
        OPENAI_IMAGE_SYNC_PLAN_KIND => Some(LocalOpenAiImageSpec {
            api_format: "openai:image",
            decision_kind: OPENAI_IMAGE_SYNC_PLAN_KIND,
            report_kind: "openai_image_sync_finalize",
            require_streaming: false,
        }),
        _ => None,
    }
}

pub fn resolve_stream_spec(plan_kind: &str) -> Option<LocalOpenAiImageSpec> {
    match plan_kind {
        OPENAI_IMAGE_STREAM_PLAN_KIND => Some(LocalOpenAiImageSpec {
            api_format: "openai:image",
            decision_kind: OPENAI_IMAGE_STREAM_PLAN_KIND,
            report_kind: "openai_image_stream_success",
            require_streaming: true,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_stream_spec, resolve_sync_spec};

    #[test]
    fn resolves_openai_image_sync_spec() {
        let spec = resolve_sync_spec("openai_image_sync").expect("spec");
        assert_eq!(spec.api_format, "openai:image");
        assert_eq!(spec.report_kind, "openai_image_sync_finalize");
        assert!(!spec.require_streaming);
    }

    #[test]
    fn resolves_openai_image_stream_spec() {
        let spec = resolve_stream_spec("openai_image_stream").expect("spec");
        assert_eq!(spec.api_format, "openai:image");
        assert_eq!(spec.report_kind, "openai_image_stream_success");
        assert!(spec.require_streaming);
    }
}
