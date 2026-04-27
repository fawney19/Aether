use crate::contracts::{
    CLAUDE_CHAT_SYNC_PLAN_KIND, CLAUDE_CLI_SYNC_PLAN_KIND, GEMINI_CHAT_SYNC_PLAN_KIND,
    GEMINI_CLI_SYNC_PLAN_KIND, OPENAI_CHAT_SYNC_PLAN_KIND, OPENAI_IMAGE_STREAM_PLAN_KIND,
    OPENAI_IMAGE_SYNC_PLAN_KIND, OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND,
    OPENAI_RESPONSES_SYNC_PLAN_KIND,
};

pub const OPENAI_CHAT_SYNC_FINALIZE_REPORT_KIND: &str = "openai_chat_sync_finalize";
pub const CLAUDE_CHAT_SYNC_FINALIZE_REPORT_KIND: &str = "claude_chat_sync_finalize";
pub const GEMINI_CHAT_SYNC_FINALIZE_REPORT_KIND: &str = "gemini_chat_sync_finalize";
pub const OPENAI_RESPONSES_SYNC_FINALIZE_REPORT_KIND: &str = "openai_responses_sync_finalize";
pub const OPENAI_RESPONSES_COMPACT_SYNC_FINALIZE_REPORT_KIND: &str =
    "openai_responses_compact_sync_finalize";
pub const OPENAI_IMAGE_SYNC_FINALIZE_REPORT_KIND: &str = "openai_image_sync_finalize";
pub const CLAUDE_CLI_SYNC_FINALIZE_REPORT_KIND: &str = "claude_cli_sync_finalize";
pub const GEMINI_CLI_SYNC_FINALIZE_REPORT_KIND: &str = "gemini_cli_sync_finalize";
pub const OPENAI_VIDEO_CREATE_SYNC_FINALIZE_REPORT_KIND: &str = "openai_video_create_sync_finalize";
pub const GEMINI_VIDEO_CREATE_SYNC_FINALIZE_REPORT_KIND: &str = "gemini_video_create_sync_finalize";
const LEGACY_OPENAI_CLI_SYNC_FINALIZE_REPORT_KIND: &str = "openai_cli_sync_finalize";
const LEGACY_OPENAI_COMPACT_SYNC_FINALIZE_REPORT_KIND: &str = "openai_compact_sync_finalize";

pub const OPENAI_CHAT_SYNC_SUCCESS_REPORT_KIND: &str = "openai_chat_sync_success";
pub const CLAUDE_CHAT_SYNC_SUCCESS_REPORT_KIND: &str = "claude_chat_sync_success";
pub const GEMINI_CHAT_SYNC_SUCCESS_REPORT_KIND: &str = "gemini_chat_sync_success";
pub const OPENAI_RESPONSES_SYNC_SUCCESS_REPORT_KIND: &str = "openai_responses_sync_success";
pub const OPENAI_RESPONSES_COMPACT_SYNC_SUCCESS_REPORT_KIND: &str =
    "openai_responses_compact_sync_success";
pub const OPENAI_IMAGE_SYNC_SUCCESS_REPORT_KIND: &str = "openai_image_sync_success";
pub const CLAUDE_CLI_SYNC_SUCCESS_REPORT_KIND: &str = "claude_cli_sync_success";
pub const GEMINI_CLI_SYNC_SUCCESS_REPORT_KIND: &str = "gemini_cli_sync_success";

pub const OPENAI_CHAT_STREAM_SUCCESS_REPORT_KIND: &str = "openai_chat_stream_success";
pub const CLAUDE_CHAT_STREAM_SUCCESS_REPORT_KIND: &str = "claude_chat_stream_success";
pub const GEMINI_CHAT_STREAM_SUCCESS_REPORT_KIND: &str = "gemini_chat_stream_success";
pub const OPENAI_RESPONSES_STREAM_SUCCESS_REPORT_KIND: &str = "openai_responses_stream_success";
pub const OPENAI_RESPONSES_COMPACT_STREAM_SUCCESS_REPORT_KIND: &str =
    "openai_responses_compact_stream_success";
pub const OPENAI_IMAGE_STREAM_SUCCESS_REPORT_KIND: &str = "openai_image_stream_success";
pub const CLAUDE_CLI_STREAM_SUCCESS_REPORT_KIND: &str = "claude_cli_stream_success";
pub const GEMINI_CLI_STREAM_SUCCESS_REPORT_KIND: &str = "gemini_cli_stream_success";

pub const OPENAI_CHAT_SYNC_ERROR_REPORT_KIND: &str = "openai_chat_sync_error";
pub const CLAUDE_CHAT_SYNC_ERROR_REPORT_KIND: &str = "claude_chat_sync_error";
pub const GEMINI_CHAT_SYNC_ERROR_REPORT_KIND: &str = "gemini_chat_sync_error";
pub const OPENAI_RESPONSES_SYNC_ERROR_REPORT_KIND: &str = "openai_responses_sync_error";
pub const OPENAI_RESPONSES_COMPACT_SYNC_ERROR_REPORT_KIND: &str =
    "openai_responses_compact_sync_error";
pub const CLAUDE_CLI_SYNC_ERROR_REPORT_KIND: &str = "claude_cli_sync_error";
pub const GEMINI_CLI_SYNC_ERROR_REPORT_KIND: &str = "gemini_cli_sync_error";

pub fn implicit_sync_finalize_report_kind(plan_kind: &str) -> Option<&'static str> {
    match plan_kind {
        OPENAI_CHAT_SYNC_PLAN_KIND => Some(OPENAI_CHAT_SYNC_FINALIZE_REPORT_KIND),
        CLAUDE_CHAT_SYNC_PLAN_KIND => Some(CLAUDE_CHAT_SYNC_FINALIZE_REPORT_KIND),
        GEMINI_CHAT_SYNC_PLAN_KIND => Some(GEMINI_CHAT_SYNC_FINALIZE_REPORT_KIND),
        OPENAI_RESPONSES_SYNC_PLAN_KIND => Some(OPENAI_RESPONSES_SYNC_FINALIZE_REPORT_KIND),
        OPENAI_RESPONSES_COMPACT_SYNC_PLAN_KIND => {
            Some(OPENAI_RESPONSES_COMPACT_SYNC_FINALIZE_REPORT_KIND)
        }
        OPENAI_IMAGE_SYNC_PLAN_KIND => Some(OPENAI_IMAGE_SYNC_FINALIZE_REPORT_KIND),
        CLAUDE_CLI_SYNC_PLAN_KIND => Some(CLAUDE_CLI_SYNC_FINALIZE_REPORT_KIND),
        GEMINI_CLI_SYNC_PLAN_KIND => Some(GEMINI_CLI_SYNC_FINALIZE_REPORT_KIND),
        _ => None,
    }
}

pub fn core_error_default_client_api_format(report_kind: &str) -> Option<&'static str> {
    match report_kind {
        OPENAI_CHAT_SYNC_FINALIZE_REPORT_KIND => Some("openai:chat"),
        CLAUDE_CHAT_SYNC_FINALIZE_REPORT_KIND => Some("claude:chat"),
        GEMINI_CHAT_SYNC_FINALIZE_REPORT_KIND => Some("gemini:chat"),
        OPENAI_RESPONSES_SYNC_FINALIZE_REPORT_KIND => Some("openai:responses"),
        OPENAI_RESPONSES_COMPACT_SYNC_FINALIZE_REPORT_KIND => Some("openai:responses:compact"),
        LEGACY_OPENAI_CLI_SYNC_FINALIZE_REPORT_KIND => Some("openai:responses"),
        LEGACY_OPENAI_COMPACT_SYNC_FINALIZE_REPORT_KIND => Some("openai:responses:compact"),
        OPENAI_IMAGE_SYNC_FINALIZE_REPORT_KIND => Some("openai:image"),
        CLAUDE_CLI_SYNC_FINALIZE_REPORT_KIND => Some("claude:cli"),
        GEMINI_CLI_SYNC_FINALIZE_REPORT_KIND => Some("gemini:cli"),
        _ => None,
    }
}

pub fn core_error_background_report_kind(report_kind: &str) -> Option<&'static str> {
    match report_kind {
        OPENAI_CHAT_SYNC_FINALIZE_REPORT_KIND => Some(OPENAI_CHAT_SYNC_ERROR_REPORT_KIND),
        CLAUDE_CHAT_SYNC_FINALIZE_REPORT_KIND => Some(CLAUDE_CHAT_SYNC_ERROR_REPORT_KIND),
        GEMINI_CHAT_SYNC_FINALIZE_REPORT_KIND => Some(GEMINI_CHAT_SYNC_ERROR_REPORT_KIND),
        OPENAI_RESPONSES_SYNC_FINALIZE_REPORT_KIND => Some(OPENAI_RESPONSES_SYNC_ERROR_REPORT_KIND),
        OPENAI_RESPONSES_COMPACT_SYNC_FINALIZE_REPORT_KIND => {
            Some(OPENAI_RESPONSES_COMPACT_SYNC_ERROR_REPORT_KIND)
        }
        LEGACY_OPENAI_CLI_SYNC_FINALIZE_REPORT_KIND => {
            Some(OPENAI_RESPONSES_SYNC_ERROR_REPORT_KIND)
        }
        LEGACY_OPENAI_COMPACT_SYNC_FINALIZE_REPORT_KIND => {
            Some(OPENAI_RESPONSES_COMPACT_SYNC_ERROR_REPORT_KIND)
        }
        CLAUDE_CLI_SYNC_FINALIZE_REPORT_KIND => Some(CLAUDE_CLI_SYNC_ERROR_REPORT_KIND),
        GEMINI_CLI_SYNC_FINALIZE_REPORT_KIND => Some(GEMINI_CLI_SYNC_ERROR_REPORT_KIND),
        _ => None,
    }
}

pub fn core_success_background_report_kind(report_kind: &str) -> Option<&'static str> {
    match report_kind {
        OPENAI_CHAT_SYNC_FINALIZE_REPORT_KIND => Some(OPENAI_CHAT_SYNC_SUCCESS_REPORT_KIND),
        CLAUDE_CHAT_SYNC_FINALIZE_REPORT_KIND => Some(CLAUDE_CHAT_SYNC_SUCCESS_REPORT_KIND),
        GEMINI_CHAT_SYNC_FINALIZE_REPORT_KIND => Some(GEMINI_CHAT_SYNC_SUCCESS_REPORT_KIND),
        OPENAI_IMAGE_SYNC_FINALIZE_REPORT_KIND => Some(OPENAI_IMAGE_SYNC_SUCCESS_REPORT_KIND),
        OPENAI_RESPONSES_SYNC_FINALIZE_REPORT_KIND => {
            Some(OPENAI_RESPONSES_SYNC_SUCCESS_REPORT_KIND)
        }
        OPENAI_RESPONSES_COMPACT_SYNC_FINALIZE_REPORT_KIND => {
            Some(OPENAI_RESPONSES_COMPACT_SYNC_SUCCESS_REPORT_KIND)
        }
        LEGACY_OPENAI_CLI_SYNC_FINALIZE_REPORT_KIND => {
            Some(OPENAI_RESPONSES_SYNC_SUCCESS_REPORT_KIND)
        }
        LEGACY_OPENAI_COMPACT_SYNC_FINALIZE_REPORT_KIND => {
            Some(OPENAI_RESPONSES_COMPACT_SYNC_SUCCESS_REPORT_KIND)
        }
        CLAUDE_CLI_SYNC_FINALIZE_REPORT_KIND => Some(CLAUDE_CLI_SYNC_SUCCESS_REPORT_KIND),
        GEMINI_CLI_SYNC_FINALIZE_REPORT_KIND => Some(GEMINI_CLI_SYNC_SUCCESS_REPORT_KIND),
        _ => None,
    }
}

pub fn implicit_stream_success_report_kind(plan_kind: &str) -> Option<&'static str> {
    match plan_kind {
        OPENAI_IMAGE_STREAM_PLAN_KIND => Some(OPENAI_IMAGE_STREAM_SUCCESS_REPORT_KIND),
        _ => None,
    }
}
