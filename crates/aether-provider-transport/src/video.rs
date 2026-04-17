use aether_data_contracts::repository::video_tasks::StoredVideoTask;
use aether_video_tasks_core::{
    LocalVideoTaskSnapshot, LocalVideoTaskTransport, LocalVideoTaskTransportBridgeInput,
};
use async_trait::async_trait;

use super::auth::{resolve_local_gemini_auth, resolve_local_openai_bearer_auth};
use super::network::resolve_transport_execution_timeouts;
use super::policy::{supports_local_gemini_transport, supports_local_standard_transport};
use super::snapshot::GatewayProviderTransportSnapshot;

#[async_trait]
pub trait VideoTaskTransportSnapshotLookup: Send + Sync {
    async fn read_video_task_provider_transport_snapshot(
        &self,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
    ) -> Result<Option<GatewayProviderTransportSnapshot>, String>;
}

pub fn resolve_local_video_task_transport(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
    model_name: Option<String>,
) -> Option<LocalVideoTaskTransport> {
    let api_format = api_format.trim();
    let (auth_header, auth_value) = match api_format {
        "openai:video" => {
            if !supports_local_standard_transport(transport, api_format) {
                return None;
            }
            resolve_local_openai_bearer_auth(transport)?
        }
        "gemini:video" => {
            if !supports_local_gemini_transport(transport, api_format) {
                return None;
            }
            resolve_local_gemini_auth(transport)?
        }
        _ => return None,
    };

    Some(LocalVideoTaskTransport::from_bridge_input(
        LocalVideoTaskTransportBridgeInput {
            upstream_base_url: transport.endpoint.base_url.clone(),
            provider_name: Some(transport.provider.name.clone()),
            provider_id: transport.provider.id.clone(),
            endpoint_id: transport.endpoint.id.clone(),
            key_id: transport.key.id.clone(),
            auth_header,
            auth_value,
            content_type: Some("application/json".to_string()),
            model_name,
            proxy: None,
            tls_profile: None,
            timeouts: resolve_transport_execution_timeouts(transport),
        },
    ))
}

pub async fn reconstruct_local_video_task_snapshot(
    lookup: &dyn VideoTaskTransportSnapshotLookup,
    task: &StoredVideoTask,
) -> Result<Option<LocalVideoTaskSnapshot>, String> {
    let provider_api_format = task
        .provider_api_format
        .as_deref()
        .unwrap_or_default()
        .trim();
    if !matches!(provider_api_format, "openai:video" | "gemini:video") {
        return Ok(None);
    }

    let Some(provider_id) = task.provider_id.as_deref() else {
        return Ok(None);
    };
    let Some(endpoint_id) = task.endpoint_id.as_deref() else {
        return Ok(None);
    };
    let Some(key_id) = task.key_id.as_deref() else {
        return Ok(None);
    };

    let Some(transport) = lookup
        .read_video_task_provider_transport_snapshot(provider_id, endpoint_id, key_id)
        .await?
    else {
        return Ok(None);
    };

    let Some(local_transport) =
        resolve_local_video_task_transport(&transport, provider_api_format, task.model.clone())
    else {
        return Ok(None);
    };

    Ok(LocalVideoTaskSnapshot::from_stored_task_with_transport(
        task,
        local_transport,
    ))
}

#[cfg(test)]
mod tests {
    use aether_data_contracts::repository::video_tasks::{StoredVideoTask, VideoTaskStatus};
    use aether_video_tasks_core::LocalVideoTaskSnapshot;
    use async_trait::async_trait;
    use serde_json::json;

    use super::{
        reconstruct_local_video_task_snapshot, resolve_local_video_task_transport,
        VideoTaskTransportSnapshotLookup,
    };
    use crate::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };

    fn sample_transport(api_format: &str, auth_type: &str) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Provider One".to_string(),
                provider_type: "openai".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: false,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: Some(30.0),
                stream_first_byte_timeout_secs: Some(5.0),
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: api_format.to_string(),
                api_family: None,
                endpoint_kind: None,
                is_active: true,
                base_url: "https://example.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: auth_type.to_string(),
                is_active: true,
                api_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "secret".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    fn sample_stored_video_task() -> StoredVideoTask {
        StoredVideoTask {
            id: "task-1".to_string(),
            short_id: Some("short-1".to_string()),
            request_id: "request-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            username: Some("user".to_string()),
            api_key_name: Some("key".to_string()),
            external_task_id: Some("upstream-task-1".to_string()),
            provider_id: Some("provider-1".to_string()),
            endpoint_id: Some("endpoint-1".to_string()),
            key_id: Some("key-1".to_string()),
            client_api_format: Some("openai:video".to_string()),
            provider_api_format: Some("openai:video".to_string()),
            format_converted: false,
            model: Some("sora".to_string()),
            prompt: Some("generate".to_string()),
            original_request_body: Some(json!({"prompt": "generate"})),
            duration_seconds: None,
            resolution: None,
            aspect_ratio: None,
            size: Some("1024x1024".to_string()),
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

    struct TestLookup(Option<GatewayProviderTransportSnapshot>);

    #[async_trait]
    impl VideoTaskTransportSnapshotLookup for TestLookup {
        async fn read_video_task_provider_transport_snapshot(
            &self,
            _provider_id: &str,
            _endpoint_id: &str,
            _key_id: &str,
        ) -> Result<Option<GatewayProviderTransportSnapshot>, String> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn resolves_openai_video_transport() {
        let transport = resolve_local_video_task_transport(
            &sample_transport("openai:video", "api_key"),
            "openai:video",
            Some("sora".to_string()),
        )
        .expect("transport");

        assert_eq!(
            transport.headers.get("authorization").map(String::as_str),
            Some("Bearer secret")
        );
        assert_eq!(transport.model_name.as_deref(), Some("sora"));
        assert_eq!(transport.provider_id, "provider-1");
    }

    #[test]
    fn resolves_gemini_video_transport() {
        let transport = resolve_local_video_task_transport(
            &sample_transport("gemini:video", "api_key"),
            "gemini:video",
            Some("veo".to_string()),
        )
        .expect("transport");

        assert_eq!(
            transport.headers.get("x-goog-api-key").map(String::as_str),
            Some("secret")
        );
        assert_eq!(transport.model_name.as_deref(), Some("veo"));
        assert_eq!(transport.endpoint_id, "endpoint-1");
    }

    #[test]
    fn rejects_mismatched_video_transport_format() {
        let transport = sample_transport("openai:chat", "bearer");
        assert!(resolve_local_video_task_transport(&transport, "openai:video", None).is_none());
    }

    #[tokio::test]
    async fn reconstructs_openai_video_snapshot_via_lookup_trait() {
        let lookup = TestLookup(Some(sample_transport("openai:video", "bearer")));
        let snapshot = reconstruct_local_video_task_snapshot(&lookup, &sample_stored_video_task())
            .await
            .expect("lookup should succeed")
            .expect("snapshot");

        match snapshot {
            LocalVideoTaskSnapshot::OpenAi(seed) => {
                assert_eq!(seed.transport.provider_id, "provider-1");
                assert_eq!(seed.transport.model_name.as_deref(), Some("sora"));
            }
            LocalVideoTaskSnapshot::Gemini(_) => panic!("expected openai snapshot"),
        }
    }
}
