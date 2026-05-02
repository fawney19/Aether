use thiserror::Error;

#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("unsupported oauth provider: {0}")]
    UnsupportedProvider(String),
    #[error("invalid oauth request: {0}")]
    InvalidRequest(String),
    #[error("oauth state is invalid or expired")]
    InvalidState,
    #[error("oauth provider returned HTTP {status_code}: {body_excerpt}")]
    HttpStatus {
        status_code: u16,
        body_excerpt: String,
    },
    #[error("oauth provider returned invalid response: {0}")]
    InvalidResponse(String),
    #[error("oauth transport failed: {0}")]
    Transport(String),
    #[error("oauth storage failed: {0}")]
    Storage(String),
    #[error("oauth encryption failed")]
    EncryptionUnavailable,
}

impl OAuthError {
    pub fn invalid_request(detail: impl Into<String>) -> Self {
        Self::InvalidRequest(detail.into())
    }

    pub fn invalid_response(detail: impl Into<String>) -> Self {
        Self::InvalidResponse(detail.into())
    }

    pub fn transport(detail: impl Into<String>) -> Self {
        Self::Transport(detail.into())
    }
}
