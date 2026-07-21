#[derive(Debug, thiserror::Error)]
pub enum RuntimeBootstrapError {
    #[error("failed to initialize tracing: {0}")]
    Tracing(String),
}
