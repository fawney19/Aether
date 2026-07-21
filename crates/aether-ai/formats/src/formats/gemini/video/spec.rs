use crate::contracts::GEMINI_VIDEO_CREATE_SYNC_PLAN_KIND;
use crate::formats::shared::video::{LocalVideoCreateFamily, LocalVideoCreateSpec};

pub fn resolve_sync_spec(plan_kind: &str) -> Option<LocalVideoCreateSpec> {
    match plan_kind {
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
    fn resolves_gemini_video_create_spec() {
        let spec = resolve_sync_spec("gemini_video_create_sync").expect("spec");
        assert_eq!(spec.api_format, "gemini:video");
        assert_eq!(spec.family, LocalVideoCreateFamily::Gemini);
        assert_eq!(spec.report_kind, "gemini_video_create_sync_finalize");
    }
}
