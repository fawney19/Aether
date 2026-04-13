mod association_sync;
mod config;
mod logic;
mod strategy;
mod transport;

pub use association_sync::{
    sync_provider_model_whitelist_associations, ModelFetchAssociationStore,
};
pub use config::{
    model_fetch_interval_minutes, model_fetch_startup_delay_seconds, model_fetch_startup_enabled,
};
pub use logic::{
    aggregate_models_for_cache, apply_model_filters, build_models_fetch_url,
    endpoint_supports_rust_models_fetch, extract_error_message, json_string_list,
    merge_upstream_metadata, parse_models_response, parse_models_response_page,
    preset_models_for_provider, provider_type_uses_preset_models, select_models_fetch_endpoint,
    selected_models_fetch_endpoints, ModelFetchRunSummary, ModelsFetchPage, ModelsFetchSuccess,
};
pub use strategy::{fetch_models_from_transports, ModelsFetchOutcome};
pub use transport::{
    build_antigravity_fetch_available_models_plan, build_gemini_cli_load_code_assist_plan,
    build_models_fetch_execution_plan, build_standard_models_fetch_execution_plan,
    build_vertex_models_fetch_execution_plan, ModelFetchTransportRuntime,
};
