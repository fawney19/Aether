#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalStandardSourceFamily {
    Standard,
    Gemini,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalStandardSourceMode {
    Chat,
    Cli,
    Embedding,
}

#[derive(Debug, Clone, Copy)]
pub struct LocalStandardSpec {
    pub api_format: &'static str,
    pub decision_kind: &'static str,
    pub report_kind: &'static str,
    pub family: LocalStandardSourceFamily,
    pub mode: LocalStandardSourceMode,
    pub require_streaming: bool,
}
