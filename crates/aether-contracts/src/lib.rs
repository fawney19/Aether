mod error;
mod frame;
mod plan;
mod result;
pub mod tunnel;

pub use error::{ExecutionError, ExecutionErrorKind, ExecutionPhase};
pub use frame::{StreamFrame, StreamFramePayload, StreamFrameType};
pub use plan::{
    ExecutionPlan, ExecutionTimeouts, ProxySnapshot, RequestBody,
    EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER, EXECUTION_REQUEST_HTTP1_ONLY_HEADER,
};
pub use result::{ExecutionResult, ExecutionTelemetry, ResponseBody};
