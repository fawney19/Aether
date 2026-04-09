mod error;
mod registry;
pub mod request;
pub mod response;

pub use error::{
    build_core_error_body_for_client_format, core_error_background_report_kind,
    core_error_default_client_api_format, core_success_background_report_kind,
    is_core_error_finalize_kind, LocalCoreSyncErrorKind,
};
pub use registry::{
    request_conversion_direct_auth, request_conversion_kind,
    request_conversion_transport_supported, sync_chat_response_conversion_kind,
    sync_cli_response_conversion_kind, RequestConversionKind, SyncChatResponseConversionKind,
    SyncCliResponseConversionKind,
};
