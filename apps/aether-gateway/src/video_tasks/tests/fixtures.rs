use std::collections::BTreeMap;

use serde_json::json;

use super::{
    ExecutionPlan, ExecutionTimeouts, GatewayControlAuthContext, LocalVideoTaskPersistence,
    LocalVideoTaskTransport, ProxySnapshot, RequestBody,
};

pub(super) fn sample_transport(
    base_url: &str,
    provider_api_format: &str,
) -> LocalVideoTaskTransport {
    let url = match provider_api_format {
        "openai:video" => format!("{base_url}/v1/videos"),
        "gemini:video" => {
            format!("{base_url}/v1beta/models/veo-3:predictLongRunning")
        }
        _ => panic!("unsupported provider api format"),
    };
    LocalVideoTaskTransport::from_plan(&sample_plan(&url, provider_api_format))
        .expect("transport should build")
}

pub(super) fn sample_persistence(provider_api_format: &str) -> LocalVideoTaskPersistence {
    LocalVideoTaskPersistence {
        request_id: "request-123".to_string(),
        username: Some("user".to_string()),
        api_key_name: Some("primary".to_string()),
        client_api_format: provider_api_format.to_string(),
        provider_api_format: provider_api_format.to_string(),
        original_request_body: json!({
            "prompt": "hello",
            "seconds": 4,
            "resolution": "720p",
            "aspect_ratio": "16:9",
            "size": "1280x720"
        }),
        format_converted: false,
    }
}

pub(super) fn sample_auth_context() -> GatewayControlAuthContext {
    GatewayControlAuthContext {
        user_id: "user-123".to_string(),
        api_key_id: "key-123".to_string(),
        username: None,
        api_key_name: None,
        balance_remaining: None,
        access_allowed: true,
        user_rate_limit: None,
        api_key_rate_limit: None,
        api_key_is_standalone: false,
        admin_bypass_limits: false,
        local_rejection: None,
        allowed_models: None,
        ip_rules: None,
    }
}

pub(super) fn sample_plan(url: &str, provider_api_format: &str) -> ExecutionPlan {
    ExecutionPlan {
        request_id: "req-123".to_string(),
        candidate_id: None,
        provider_name: Some(
            provider_api_format
                .split(':')
                .next()
                .expect("provider name should exist")
                .to_string(),
        ),
        provider_id: "provider-123".to_string(),
        endpoint_id: "endpoint-123".to_string(),
        key_id: "key-123".to_string(),
        method: "POST".to_string(),
        url: url.to_string(),
        headers: BTreeMap::from([(
            "authorization".to_string(),
            "Bearer upstream-key".to_string(),
        )]),
        content_type: Some("application/json".to_string()),
        content_encoding: None,
        body: RequestBody::from_json(json!({})),
        stream: false,
        client_api_format: provider_api_format.to_string(),
        provider_api_format: provider_api_format.to_string(),
        model_name: Some(
            if provider_api_format == "gemini:video" {
                "veo-3"
            } else {
                "sora-2"
            }
            .to_string(),
        ),
        proxy: Some(ProxySnapshot {
            enabled: Some(false),
            mode: Some("direct".to_string()),
            node_id: None,
            label: None,
            url: None,
            extra: None,
        }),
        transport_profile: None,
        timeouts: Some(ExecutionTimeouts {
            connect_ms: Some(10_000),
            read_ms: Some(30_000),
            first_byte_ms: None,
            write_ms: Some(30_000),
            pool_ms: Some(10_000),
            total_ms: Some(300_000),
        }),
    }
}
