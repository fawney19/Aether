use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use axum::body::{to_bytes, Body, Bytes};
use axum::response::Response;
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use http::header::{HeaderName, HeaderValue};
use http::StatusCode;
use serde_json::json;

use crate::constants::{
    CONTROL_EXECUTED_HEADER, CONTROL_EXECUTE_FALLBACK_HEADER, DEPENDENCY_REASON_HEADER,
    EXECUTION_PATH_EXECUTION_RUNTIME_STREAM, EXECUTION_PATH_EXECUTION_RUNTIME_SYNC,
    EXECUTION_PATH_HEADER, EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS,
    LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER, TRACE_ID_HEADER,
};

use super::{
    build_router, build_router_with_execution_runtime_override, build_router_with_state,
    build_state_with_execution_runtime_override, next_non_keepalive_chunk, start_server,
    strip_sse_keepalive_comments, wait_until, AppState, FrontdoorCorsConfig,
    FrontdoorUserRpmConfig, GatewayFallbackMetricKind, GatewayFallbackReason, UsageRuntimeConfig,
    VideoTaskTruthSourceMode,
};

mod control_execute;
mod fallback;
mod finalize_local;
mod finalize_local_cli;
mod finalize_local_provider;
mod grok_oauth;
mod lifecycle;
mod stream;
mod stream_cli;
mod stream_provider;
mod stream_provider_gemini;
mod sync;
