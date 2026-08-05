use crate::control::GatewayControlAuthContext;
use aether_contracts::{ExecutionPlan, ExecutionTimeouts, ProxySnapshot, RequestBody};

pub(crate) use self::service::VideoTaskService;
pub use self::types::VideoTaskTruthSourceMode;
pub(crate) use self::types::{
    GeminiVideoTaskSeed, LocalVideoTaskContentAction, LocalVideoTaskFollowUpPlan,
    LocalVideoTaskPersistence, LocalVideoTaskReadRefreshPlan, LocalVideoTaskReadResponse,
    LocalVideoTaskRegistryMutation, LocalVideoTaskSeed, LocalVideoTaskSnapshot,
    LocalVideoTaskStatus, LocalVideoTaskSuccessPlan, LocalVideoTaskTransport, OpenAiVideoTaskSeed,
    VideoTaskSyncReportMode,
};
pub(crate) use aether_video_tasks_core::{
    build_internal_finalize_video_plan, build_local_sync_finalize_read_response,
    build_local_sync_finalize_request_path, resolve_local_sync_error_background_report_kind,
    resolve_local_sync_success_background_report_kind,
};

pub(crate) use self::helpers::{
    extract_doubao_task_id_from_content_path, extract_doubao_task_id_from_path,
    extract_gemini_short_id_from_cancel_path, extract_gemini_short_id_from_path,
    extract_openai_task_id_from_cancel_path, extract_openai_task_id_from_content_path,
    extract_openai_task_id_from_path, extract_openai_task_id_from_remix_path,
    resolve_video_task_hydration_lookup_key, resolve_video_task_read_lookup_key,
    resolve_video_task_report_lookup, VideoTaskReportLookup,
};

use self::helpers::resolve_local_video_registry_mutation;
use self::store::{FileVideoTaskStore, InMemoryVideoTaskStore, VideoTaskStore};
use self::types::LocalVideoTaskProjectionTarget;

mod helpers;
mod service;
mod store;
mod types;

#[cfg(test)]
mod tests;
