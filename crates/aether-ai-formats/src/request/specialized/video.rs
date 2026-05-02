use crate::contracts::{GEMINI_VIDEO_CREATE_SYNC_PLAN_KIND, OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalVideoCreateFamily {
    OpenAi,
    Gemini,
}

#[derive(Debug, Clone, Copy)]
pub struct LocalVideoCreateSpec {
    pub api_format: &'static str,
    pub decision_kind: &'static str,
    pub report_kind: &'static str,
    pub family: LocalVideoCreateFamily,
}

pub fn resolve_sync_spec(plan_kind: &str) -> Option<LocalVideoCreateSpec> {
    match plan_kind {
        OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND => Some(LocalVideoCreateSpec {
            api_format: "openai:video",
            decision_kind: OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND,
            report_kind: "openai_video_create_sync_finalize",
            family: LocalVideoCreateFamily::OpenAi,
        }),
        GEMINI_VIDEO_CREATE_SYNC_PLAN_KIND => Some(LocalVideoCreateSpec {
            api_format: "gemini:video",
            decision_kind: GEMINI_VIDEO_CREATE_SYNC_PLAN_KIND,
            report_kind: "gemini_video_create_sync_finalize",
            family: LocalVideoCreateFamily::Gemini,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_sync_spec, LocalVideoCreateFamily};

    #[test]
    fn resolves_openai_video_create_spec() {
        let spec = resolve_sync_spec("openai_video_create_sync").expect("spec");
        assert_eq!(spec.api_format, "openai:video");
        assert_eq!(spec.family, LocalVideoCreateFamily::OpenAi);
        assert_eq!(spec.report_kind, "openai_video_create_sync_finalize");
    }

    #[test]
    fn resolves_gemini_video_create_spec() {
        let spec = resolve_sync_spec("gemini_video_create_sync").expect("spec");
        assert_eq!(spec.api_format, "gemini:video");
        assert_eq!(spec.family, LocalVideoCreateFamily::Gemini);
        assert_eq!(spec.report_kind, "gemini_video_create_sync_finalize");
    }
}
