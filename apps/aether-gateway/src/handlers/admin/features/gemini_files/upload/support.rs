use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;

pub(super) fn admin_gemini_files_query_key_ids(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Vec<String> {
    let _ = state;
    let mut key_ids = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let Some(raw) = query_param_value(request_context.query_string(), "key_ids") else {
        return key_ids;
    };
    for key_id in raw.split(',') {
        let trimmed = key_id.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }
        key_ids.push(trimmed.to_string());
    }
    key_ids
}
