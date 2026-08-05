use crate::contracts::DOUBAO_VIDEO_CREATE_SYNC_PLAN_KIND;
use crate::formats::shared::video::{LocalVideoCreateFamily, LocalVideoCreateSpec};

pub fn resolve_sync_spec(plan_kind: &str) -> Option<LocalVideoCreateSpec> {
    match plan_kind {
        DOUBAO_VIDEO_CREATE_SYNC_PLAN_KIND => Some(LocalVideoCreateSpec {
            api_format: "doubao:video",
            decision_kind: DOUBAO_VIDEO_CREATE_SYNC_PLAN_KIND,
            report_kind: "doubao_video_create_sync_finalize",
            family: LocalVideoCreateFamily::Doubao,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_sync_spec, LocalVideoCreateFamily};

    #[test]
    fn resolves_doubao_video_create_spec() {
        let spec = resolve_sync_spec("doubao_video_create_sync").expect("spec");
        assert_eq!(spec.api_format, "doubao:video");
        assert_eq!(spec.family, LocalVideoCreateFamily::Doubao);
        assert_eq!(spec.report_kind, "doubao_video_create_sync_finalize");
    }
}
