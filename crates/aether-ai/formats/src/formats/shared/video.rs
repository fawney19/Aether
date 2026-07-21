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
    crate::formats::openai::video::spec::resolve_sync_spec(plan_kind)
        .or_else(|| crate::formats::gemini::video::spec::resolve_sync_spec(plan_kind))
}

#[cfg(test)]
mod tests {
    use super::{resolve_sync_spec, LocalVideoCreateFamily};

    #[test]
    fn resolves_openai_and_gemini_video_create_specs() {
        let openai = resolve_sync_spec("openai_video_create_sync").expect("openai spec");
        assert_eq!(openai.api_format, "openai:video");
        assert_eq!(openai.family, LocalVideoCreateFamily::OpenAi);

        let gemini = resolve_sync_spec("gemini_video_create_sync").expect("gemini spec");
        assert_eq!(gemini.api_format, "gemini:video");
        assert_eq!(gemini.family, LocalVideoCreateFamily::Gemini);
    }
}
