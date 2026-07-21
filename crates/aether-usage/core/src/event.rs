use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UsageEventKind {
    Started,
    Streaming,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UsageEventError {
    #[error("usage event id cannot be empty")]
    MissingEventId,
    #[error("usage request id cannot be empty")]
    MissingRequestId,
    #[error("usage subject id cannot be empty")]
    MissingSubjectId,
}
