#[derive(Debug, thiserror::Error)]
pub enum DataLayerError {
    #[error("invalid configuration: {0}")]
    InvalidConfiguration(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("postgres error: {0}")]
    Postgres(String),

    #[error("redis error: {0}")]
    Redis(String),

    #[error("sql error: {0}")]
    Sql(String),

    #[error("operation timed out: {0}")]
    TimedOut(String),

    #[error("unexpected database value: {0}")]
    UnexpectedValue(String),
}

impl DataLayerError {
    pub fn postgres(error: impl std::fmt::Display) -> Self {
        Self::Postgres(error.to_string())
    }

    pub fn redis(error: impl std::fmt::Display) -> Self {
        Self::Redis(error.to_string())
    }

    pub fn sql(error: impl std::fmt::Display) -> Self {
        Self::Sql(error.to_string())
    }
}
