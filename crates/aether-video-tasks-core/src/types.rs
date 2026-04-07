use std::collections::BTreeMap;

use aether_contracts::{ExecutionPlan, ExecutionTimeouts, ProxySnapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_VIDEO_TASK_POLL_INTERVAL_SECONDS: u32 = 10;
pub const DEFAULT_VIDEO_TASK_MAX_POLL_COUNT: u32 = 360;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoTaskSyncReportMode {
    InlineSync,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoTaskTruthSourceMode {
    #[default]
    PythonSyncReport,
    RustAuthoritative,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalVideoTaskSuccessPlan {
    pub seed: LocalVideoTaskSeed,
    pub report_mode: VideoTaskSyncReportMode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalVideoTaskFollowUpPlan {
    pub plan: ExecutionPlan,
    pub report_kind: Option<String>,
    pub report_context: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalVideoTaskReadRefreshPlan {
    pub plan: ExecutionPlan,
    pub projection_target: LocalVideoTaskProjectionTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LocalVideoTaskContentAction {
    Immediate { status_code: u16, body_json: Value },
    StreamPlan(Box<ExecutionPlan>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalVideoTaskProjectionTarget {
    OpenAi { task_id: String },
    Gemini { short_id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LocalVideoTaskSnapshot {
    OpenAi(OpenAiVideoTaskSeed),
    Gemini(GeminiVideoTaskSeed),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalVideoTaskStatus {
    Submitted,
    Queued,
    Processing,
    Completed,
    Failed,
    Cancelled,
    Expired,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalVideoTaskReadResponse {
    pub status_code: u16,
    pub body_json: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalVideoTaskRegistryMutation {
    OpenAiCancelled { task_id: String },
    OpenAiDeleted { task_id: String },
    GeminiCancelled { short_id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LocalVideoTaskSeed {
    OpenAiCreate(OpenAiVideoTaskSeed),
    OpenAiRemix(OpenAiVideoTaskSeed),
    GeminiCreate(GeminiVideoTaskSeed),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalVideoTaskTransport {
    pub upstream_base_url: String,
    pub provider_name: Option<String>,
    pub provider_id: String,
    pub endpoint_id: String,
    pub key_id: String,
    pub headers: BTreeMap<String, String>,
    pub content_type: Option<String>,
    pub model_name: Option<String>,
    pub proxy: Option<ProxySnapshot>,
    pub tls_profile: Option<String>,
    pub timeouts: Option<ExecutionTimeouts>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalVideoTaskTransportBridgeInput {
    pub upstream_base_url: String,
    pub provider_name: Option<String>,
    pub provider_id: String,
    pub endpoint_id: String,
    pub key_id: String,
    pub auth_header: String,
    pub auth_value: String,
    pub content_type: Option<String>,
    pub model_name: Option<String>,
    pub proxy: Option<ProxySnapshot>,
    pub tls_profile: Option<String>,
    pub timeouts: Option<ExecutionTimeouts>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalVideoTaskPersistence {
    pub request_id: String,
    pub username: Option<String>,
    pub api_key_name: Option<String>,
    pub client_api_format: String,
    pub provider_api_format: String,
    pub original_request_body: Value,
    pub format_converted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenAiVideoTaskSeed {
    pub local_task_id: String,
    pub upstream_task_id: String,
    pub created_at_unix_secs: u64,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub size: Option<String>,
    pub seconds: Option<String>,
    pub remixed_from_video_id: Option<String>,
    pub status: LocalVideoTaskStatus,
    pub progress_percent: u16,
    pub completed_at_unix_secs: Option<u64>,
    pub expires_at_unix_secs: Option<u64>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub video_url: Option<String>,
    pub persistence: LocalVideoTaskPersistence,
    pub transport: LocalVideoTaskTransport,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeminiVideoTaskSeed {
    pub local_short_id: String,
    pub upstream_operation_name: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub model: String,
    pub status: LocalVideoTaskStatus,
    pub progress_percent: u16,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub metadata: Value,
    pub persistence: LocalVideoTaskPersistence,
    pub transport: LocalVideoTaskTransport,
}
