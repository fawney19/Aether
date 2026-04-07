mod body;
mod follow_up;
mod gemini;
mod openai;
mod path;
mod read_side;
mod service;
mod snapshot;
mod store;
mod store_backend;
mod store_registry;
mod sync;
mod transport;
mod transport_domain;
mod types;
mod util;

pub use body::{
    context_text, context_u64, request_body_string, request_body_text, request_body_u32,
};
pub use follow_up::{
    build_video_follow_up_report_context, resolve_follow_up_auth, VideoFollowUpReportContextInput,
};
pub use gemini::map_gemini_stored_task_to_read_response;
pub use openai::map_openai_stored_task_to_read_response;
pub use path::{
    build_local_sync_finalize_request_path, current_unix_timestamp_secs,
    extract_gemini_short_id_from_cancel_path, extract_gemini_short_id_from_path,
    extract_openai_task_id_from_cancel_path, extract_openai_task_id_from_content_path,
    extract_openai_task_id_from_path, extract_openai_task_id_from_remix_path,
    generate_local_short_id, local_status_from_stored, resolve_local_video_registry_mutation,
    resolve_video_task_hydration_lookup_key, resolve_video_task_read_lookup_key,
    resolve_video_task_report_lookup, VideoTaskReportLookup,
};
pub use read_side::{read_data_backed_video_task_response, StoredVideoTaskReadSide};
pub use service::VideoTaskService;
pub use store::VideoTaskStore;
pub use store_backend::{FileVideoTaskStore, InMemoryVideoTaskStore};
pub use store_registry::VideoTaskRegistry;
pub use sync::{
    build_internal_finalize_video_plan, build_local_sync_finalize_read_response,
    resolve_local_sync_error_background_report_kind,
    resolve_local_sync_success_background_report_kind,
};
pub use transport::{
    gemini_metadata_video_url, map_openai_task_status, parse_video_content_variant,
};
pub use types::{
    GeminiVideoTaskSeed, LocalVideoTaskContentAction, LocalVideoTaskFollowUpPlan,
    LocalVideoTaskPersistence, LocalVideoTaskProjectionTarget, LocalVideoTaskReadRefreshPlan,
    LocalVideoTaskReadResponse, LocalVideoTaskRegistryMutation, LocalVideoTaskSeed,
    LocalVideoTaskSnapshot, LocalVideoTaskStatus, LocalVideoTaskSuccessPlan,
    LocalVideoTaskTransport, LocalVideoTaskTransportBridgeInput, OpenAiVideoTaskSeed,
    VideoTaskSyncReportMode, VideoTaskTruthSourceMode, DEFAULT_VIDEO_TASK_MAX_POLL_COUNT,
    DEFAULT_VIDEO_TASK_POLL_INTERVAL_SECONDS,
};
pub use util::non_empty_owned;
