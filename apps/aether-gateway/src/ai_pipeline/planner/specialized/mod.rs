//! Non-matrix AI surfaces such as files and video.

mod files;
mod image;
mod video;

pub(crate) use self::files::{
    build_local_gemini_files_stream_plan_and_reports_for_kind,
    build_local_gemini_files_sync_plan_and_reports_for_kind,
    maybe_build_stream_local_gemini_files_decision_payload,
    maybe_build_sync_local_gemini_files_decision_payload,
};
pub(crate) use self::image::{
    build_local_image_stream_plan_and_reports_for_kind,
    build_local_image_sync_plan_and_reports_for_kind, is_openai_image_stream_request,
    maybe_build_stream_local_image_decision_payload, maybe_build_sync_local_image_decision_payload,
};
pub(crate) use self::video::{
    build_local_video_sync_plan_and_reports_for_kind, maybe_build_sync_local_video_decision_payload,
};
