use std::time::Duration;

use aether_scheduler_core::{
    build_scheduler_affinity_cache_key_for_api_key_id, SchedulerAffinityTarget,
};

use super::state::SchedulerRuntimeState;

pub(crate) const SCHEDULER_AFFINITY_TTL: Duration = Duration::from_secs(300);

pub(crate) fn read_cached_scheduler_affinity_target(
    state: &(impl SchedulerRuntimeState + ?Sized),
    api_key_id: &str,
    api_format: &str,
    global_model_name: &str,
) -> Option<SchedulerAffinityTarget> {
    let cache_key = build_scheduler_affinity_cache_key_for_api_key_id(
        api_key_id,
        api_format,
        global_model_name,
    )?;
    state
        .read_cached_scheduler_affinity_target(&cache_key, SCHEDULER_AFFINITY_TTL)
        .or_else(|| {
            openai_responses_affinity_alias(api_format)
                .and_then(|alias| {
                    legacy_raw_scheduler_affinity_cache_key(api_key_id, alias, global_model_name)
                })
                .and_then(|alias_cache_key| {
                    state.read_cached_scheduler_affinity_target(
                        &alias_cache_key,
                        SCHEDULER_AFFINITY_TTL,
                    )
                })
        })
}

fn openai_responses_affinity_alias(api_format: &str) -> Option<&'static str> {
    match api_format.trim().to_ascii_lowercase().as_str() {
        "openai:responses" => Some("openai:cli"),
        "openai:responses:compact" => Some("openai:compact"),
        "openai:cli" => Some("openai:responses"),
        "openai:compact" => Some("openai:responses:compact"),
        _ => None,
    }
}

fn legacy_raw_scheduler_affinity_cache_key(
    api_key_id: &str,
    api_format: &str,
    global_model_name: &str,
) -> Option<String> {
    let api_key_id = api_key_id.trim();
    let api_format = api_format.trim().to_ascii_lowercase();
    let global_model_name = global_model_name.trim();
    if api_key_id.is_empty() || api_format.is_empty() || global_model_name.is_empty() {
        return None;
    }
    Some(format!(
        "scheduler_affinity:{api_key_id}:{api_format}:{global_model_name}"
    ))
}
