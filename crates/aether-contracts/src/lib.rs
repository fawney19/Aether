mod error;
mod frame;
pub mod internal_gateway;
mod plan;
mod result;
pub mod tunnel;
pub mod tunnel_security;
mod usage;

pub use error::{ExecutionError, ExecutionErrorKind, ExecutionPhase};
pub use frame::{StreamFrame, StreamFramePayload, StreamFrameType};
pub use plan::{
    redact_url_for_debug, ExecutionPlan, ExecutionResponseBodyMode, ExecutionTimeouts,
    ProxySnapshot, RequestBody, ResolvedTransportProfile,
    EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER, EXECUTION_REQUEST_HTTP1_ONLY_HEADER,
    EXECUTION_RESPONSE_BODY_MODE_HEADER, MAX_EXECUTION_REQUEST_TIMEOUT_MS,
    MAX_EXECUTION_REQUEST_TIMEOUT_SECS, MAX_EXECUTION_STREAM_FIRST_BYTE_TIMEOUT_MS,
    MAX_EXECUTION_STREAM_FIRST_BYTE_TIMEOUT_SECS, PROXY_NODE_TUNNEL_GENERATION_EXTRA_KEY,
    TRANSPORT_BACKEND_BROWSER_WREQ, TRANSPORT_BACKEND_HYPER_RUSTLS,
    TRANSPORT_BACKEND_REQWEST_RUSTLS, TRANSPORT_HTTP_MODE_AUTO,
    TRANSPORT_HTTP_MODE_H2C_PRIOR_KNOWLEDGE, TRANSPORT_HTTP_MODE_HTTP1_ONLY,
    TRANSPORT_POOL_SCOPE_KEY,
};
pub use result::{ExecutionResponseObservation, ExecutionResult, ExecutionTelemetry, ResponseBody};
pub use usage::{
    ExecutionStreamTerminalSummary, StandardizedUsage, USAGE_SERVER_NOW_UNIX_MS_HEADER,
};
