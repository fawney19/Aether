mod error;
mod frame;
mod plan;
mod result;
pub mod tunnel;
pub mod tunnel_security;
mod usage;

pub use error::{ExecutionError, ExecutionErrorKind, ExecutionPhase};
pub use frame::{StreamFrame, StreamFramePayload, StreamFrameType};
pub use plan::{
    ExecutionPlan, ExecutionTimeouts, ProxySnapshot, RequestBody, ResolvedTransportProfile,
    EXECUTION_REQUEST_ACCEPT_INVALID_CERTS_HEADER, EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER,
    EXECUTION_REQUEST_HTTP1_ONLY_HEADER, MAX_EXECUTION_REQUEST_TIMEOUT_MS,
    MAX_EXECUTION_REQUEST_TIMEOUT_SECS, MAX_EXECUTION_STREAM_FIRST_BYTE_TIMEOUT_MS,
    MAX_EXECUTION_STREAM_FIRST_BYTE_TIMEOUT_SECS, TRANSPORT_BACKEND_BROWSER_WREQ,
    TRANSPORT_BACKEND_HYPER_RUSTLS, TRANSPORT_BACKEND_REQWEST_RUSTLS, TRANSPORT_HTTP_MODE_AUTO,
    TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE, TRANSPORT_HTTP_MODE_HTTP1_ONLY,
    TRANSPORT_POOL_SCOPE_KEY,
};
pub use result::{ExecutionResult, ExecutionTelemetry, ResponseBody};
pub use usage::{
    ExecutionStreamTerminalSummary, StandardizedUsage, USAGE_SERVER_NOW_UNIX_MS_HEADER,
};
