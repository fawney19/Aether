use aether_data_contracts::repository::candidates::StoredRequestCandidate;
use aether_data_contracts::repository::video_tasks::StoredVideoTask;
use serde_json::{Map, Value};

pub fn report_context_is_locally_actionable(report_context: Option<&Value>) -> bool {
    let Some(context) = report_context else {
        return false;
    };

    has_non_empty_str(context, "request_id")
        && (has_non_empty_str(context, "candidate_id")
            || has_u64(context, "candidate_index")
            || has_non_empty_str(context, "provider_id")
            || has_non_empty_str(context, "endpoint_id")
            || has_non_empty_str(context, "key_id"))
}

pub fn build_locally_actionable_report_context_from_request_candidate(
    context: &Value,
    candidate: &StoredRequestCandidate,
) -> Option<Value> {
    let mut object = context.as_object()?.clone();
    insert_missing_string_value(&mut object, "candidate_id", Some(candidate.id.as_str()));
    if !object.contains_key("candidate_index") {
        object.insert(
            "candidate_index".to_string(),
            Value::Number(candidate.candidate_index.into()),
        );
    }
    insert_missing_optional_string_value(
        &mut object,
        "provider_id",
        candidate.provider_id.as_deref(),
    );
    insert_missing_optional_string_value(
        &mut object,
        "endpoint_id",
        candidate.endpoint_id.as_deref(),
    );
    insert_missing_optional_string_value(&mut object, "key_id", candidate.key_id.as_deref());
    insert_missing_optional_string_value(&mut object, "user_id", candidate.user_id.as_deref());
    insert_missing_optional_string_value(
        &mut object,
        "api_key_id",
        candidate.api_key_id.as_deref(),
    );

    let resolved = Value::Object(object);
    report_context_is_locally_actionable(Some(&resolved)).then_some(resolved)
}

pub fn build_locally_actionable_report_context_from_video_task(
    context: &Value,
    task: &StoredVideoTask,
) -> Option<Value> {
    let mut object = context.as_object()?.clone();
    insert_missing_string_value(&mut object, "request_id", Some(task.request_id.as_str()));
    insert_missing_optional_string_value(&mut object, "provider_id", task.provider_id.as_deref());
    insert_missing_optional_string_value(&mut object, "endpoint_id", task.endpoint_id.as_deref());
    insert_missing_optional_string_value(&mut object, "key_id", task.key_id.as_deref());
    insert_missing_optional_string_value(&mut object, "user_id", task.user_id.as_deref());
    insert_missing_optional_string_value(&mut object, "api_key_id", task.api_key_id.as_deref());
    insert_missing_optional_string_value(
        &mut object,
        "client_api_format",
        task.client_api_format.as_deref(),
    );
    insert_missing_optional_string_value(
        &mut object,
        "provider_api_format",
        task.provider_api_format.as_deref(),
    );
    Some(Value::Object(object))
}

fn insert_missing_string_value(object: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if object.contains_key(key) {
        return;
    }
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    object.insert(key.to_string(), Value::String(value.to_string()));
}

fn insert_missing_optional_string_value(
    object: &mut Map<String, Value>,
    key: &str,
    value: Option<&str>,
) {
    insert_missing_string_value(object, key, value);
}

fn has_non_empty_str(value: &Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
}

fn has_u64(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_u64).is_some()
}

#[cfg(test)]
mod tests {
    use aether_data_contracts::repository::candidates::{
        RequestCandidateStatus, StoredRequestCandidate,
    };
    use aether_data_contracts::repository::video_tasks::{StoredVideoTask, VideoTaskStatus};
    use serde_json::{json, Value};

    use super::{
        build_locally_actionable_report_context_from_request_candidate,
        build_locally_actionable_report_context_from_video_task,
        report_context_is_locally_actionable,
    };

    fn sample_candidate() -> StoredRequestCandidate {
        StoredRequestCandidate {
            id: "cand-1".to_string(),
            request_id: "req-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            username: None,
            api_key_name: None,
            candidate_index: 0,
            retry_index: 0,
            provider_id: Some("provider-1".to_string()),
            endpoint_id: Some("endpoint-1".to_string()),
            key_id: Some("key-1".to_string()),
            status: RequestCandidateStatus::Pending,
            skip_reason: None,
            is_cached: false,
            status_code: None,
            error_type: None,
            error_message: None,
            latency_ms: None,
            concurrent_requests: None,
            extra_data: None,
            required_capabilities: None,
            created_at_unix_ms: 1,
            started_at_unix_ms: None,
            finished_at_unix_ms: None,
        }
    }

    fn sample_video_task() -> StoredVideoTask {
        StoredVideoTask {
            id: "task-1".to_string(),
            short_id: Some("short-1".to_string()),
            request_id: "req-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            username: None,
            api_key_name: None,
            external_task_id: Some("ext-1".to_string()),
            provider_id: Some("provider-1".to_string()),
            endpoint_id: Some("endpoint-1".to_string()),
            key_id: Some("key-1".to_string()),
            client_api_format: Some("openai:video".to_string()),
            provider_api_format: Some("openai:video".to_string()),
            format_converted: false,
            model: Some("sora".to_string()),
            prompt: None,
            original_request_body: None,
            duration_seconds: None,
            resolution: None,
            aspect_ratio: None,
            size: None,
            status: VideoTaskStatus::Submitted,
            progress_percent: 0,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: None,
            poll_count: 0,
            max_poll_count: 360,
            created_at_unix_ms: 1,
            submitted_at_unix_secs: Some(1),
            completed_at_unix_secs: None,
            updated_at_unix_secs: 1,
            error_code: None,
            error_message: None,
            video_url: None,
            request_metadata: None,
        }
    }

    #[test]
    fn detects_locally_actionable_report_context() {
        assert!(report_context_is_locally_actionable(Some(&json!({
            "request_id": "req-1",
            "provider_id": "provider-1"
        }))));
        assert!(!report_context_is_locally_actionable(Some(&json!({
            "request_id": "req-1"
        }))));
    }

    #[test]
    fn patches_locally_actionable_report_context_from_candidate() {
        let resolved = build_locally_actionable_report_context_from_request_candidate(
            &json!({"request_id": "req-1"}),
            &sample_candidate(),
        )
        .expect("candidate context should resolve");

        assert_eq!(
            resolved.get("candidate_id").and_then(Value::as_str),
            Some("cand-1")
        );
        assert_eq!(
            resolved.get("provider_id").and_then(Value::as_str),
            Some("provider-1")
        );
    }

    #[test]
    fn patches_locally_actionable_report_context_from_video_task() {
        let resolved = build_locally_actionable_report_context_from_video_task(
            &json!({"local_task_id": "task-1"}),
            &sample_video_task(),
        )
        .expect("video task context should resolve");

        assert_eq!(
            resolved.get("request_id").and_then(Value::as_str),
            Some("req-1")
        );
        assert_eq!(
            resolved.get("provider_api_format").and_then(Value::as_str),
            Some("openai:video")
        );
    }
}
