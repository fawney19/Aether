use serde_json::json;

pub(super) fn admin_provider_ops_action_response(
    status: &str,
    action_type: &str,
    data: serde_json::Value,
    message: Option<String>,
    response_time_ms: Option<u64>,
    cache_ttl_seconds: u64,
) -> serde_json::Value {
    json!({
        "status": status,
        "action_type": action_type,
        "data": data,
        "message": message,
        "executed_at": chrono::Utc::now()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "response_time_ms": response_time_ms,
        "cache_ttl_seconds": cache_ttl_seconds,
    })
}

pub(super) fn admin_provider_ops_action_error(
    status: &str,
    action_type: &str,
    message: impl Into<String>,
    response_time_ms: Option<u64>,
) -> serde_json::Value {
    admin_provider_ops_action_response(
        status,
        action_type,
        serde_json::Value::Null,
        Some(message.into()),
        response_time_ms,
        0,
    )
}

pub(super) fn admin_provider_ops_action_not_configured(
    action_type: &str,
    message: impl Into<String>,
) -> serde_json::Value {
    admin_provider_ops_action_error("not_configured", action_type, message, None)
}

pub(super) fn admin_provider_ops_action_not_supported(
    action_type: &str,
    message: impl Into<String>,
) -> serde_json::Value {
    admin_provider_ops_action_error("not_supported", action_type, message, None)
}
