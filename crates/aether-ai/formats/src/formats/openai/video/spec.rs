use crate::contracts::OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND;
use crate::formats::shared::video::{LocalVideoCreateFamily, LocalVideoCreateSpec};

pub fn resolve_sync_spec(plan_kind: &str) -> Option<LocalVideoCreateSpec> {
    match plan_kind {
        OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND => Some(LocalVideoCreateSpec {
            api_format: "openai:video",
            decision_kind: OPENAI_VIDEO_CREATE_SYNC_PLAN_KIND,
            report_kind: "openai_video_create_sync_finalize",
            family: LocalVideoCreateFamily::OpenAi,
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
}
