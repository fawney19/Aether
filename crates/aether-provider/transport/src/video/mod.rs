use std::collections::BTreeMap;
use std::fmt;

use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::video_tasks::StoredVideoTask;
use aether_video_tasks_core::{
    LocalVideoTaskSnapshot, LocalVideoTaskTransport, LocalVideoTaskTransportBridgeInput,
};
use async_trait::async_trait;
use serde_json::Value;

use super::auth::{
    build_passthrough_headers_with_auth, resolve_local_gemini_auth,
    resolve_local_openai_bearer_auth,
};
use super::network::{
    resolve_transport_execution_timeouts, resolve_transport_profile,
    resolve_transport_proxy_snapshot,
};
use super::policy::{
    local_gemini_transport_unsupported_reason_with_network,
    local_standard_transport_unsupported_reason_with_network,
};
use super::rules::{
    apply_local_body_rules_with_request_headers, apply_local_header_rules_with_request_headers,
};
use super::snapshot::GatewayProviderTransportSnapshot;
use super::url::{build_gemini_video_predict_long_running_url, build_passthrough_path_url};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderVideoCreateFamily {
    OpenAi,
    Gemini,
}

#[derive(Clone, Copy)]
pub struct ProviderVideoCreateHeadersInput<'a> {
    pub transport: &'a GatewayProviderTransportSnapshot,
    pub headers: &'a http::HeaderMap,
    pub auth_header: &'a str,
    pub auth_value: &'a str,
    pub header_rules: Option<&'a Value>,
    pub provider_request_body: &'a Value,
    pub original_request_body: &'a Value,
}

impl fmt::Debug for ProviderVideoCreateHeadersInput<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderVideoCreateHeadersInput")
            .field(
                "request_header_names",
                &self
                    .headers
                    .keys()
                    .map(|name| name.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("auth_header", &self.auth_header)
            .field("has_auth_value", &(!self.auth_value.is_empty()))
            .field("has_header_rules", &self.header_rules.is_some())
            .field(
                "provider_request_body_bytes",
                &serde_json::to_vec(self.provider_request_body)
                    .ok()
                    .map(|bytes| bytes.len()),
            )
            .field(
                "original_request_body_bytes",
                &serde_json::to_vec(self.original_request_body)
                    .ok()
                    .map(|bytes| bytes.len()),
            )
            .finish()
    }
}

#[async_trait]
pub trait VideoTaskTransportSnapshotLookup: Send + Sync {
    async fn read_video_task_provider_transport_snapshot(
        &self,
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
    ) -> Result<Option<GatewayProviderTransportSnapshot>, String>;

    async fn resolve_video_task_proxy(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<ProxySnapshot> {
        resolve_transport_proxy_snapshot(transport)
    }
}

pub fn resolve_local_video_task_transport(
    transport: &GatewayProviderTransportSnapshot,
    api_format: &str,
    model_name: Option<String>,
) -> Option<LocalVideoTaskTransport> {
    let api_format = api_format.trim();
    let (auth_header, auth_value) = match api_format {
        "openai:video" => {
            if local_standard_transport_unsupported_reason_with_network(transport, api_format)
                .is_some()
            {
                return None;
            }
            resolve_openai_compatible_video_auth(transport)?
        }
        "gemini:video" => {
            if local_gemini_transport_unsupported_reason_with_network(transport, api_format)
                .is_some()
            {
                return None;
            }
            resolve_local_gemini_auth(transport)?
        }
        _ => return None,
    };

    let mut resolved =
        LocalVideoTaskTransport::from_bridge_input(LocalVideoTaskTransportBridgeInput {
            upstream_base_url: crate::xai::resolved_xai_request_base_url(transport, api_format),
            provider_name: Some(transport.provider.name.clone()),
            provider_id: transport.provider.id.clone(),
            endpoint_id: transport.endpoint.id.clone(),
            key_id: transport.key.id.clone(),
            auth_header,
            auth_value,
            content_type: Some("application/json".to_string()),
            model_name,
            proxy: resolve_transport_proxy_snapshot(transport),
            transport_profile: resolve_transport_profile(transport),
            timeouts: resolve_transport_execution_timeouts(transport),
        });
    crate::xai::insert_cli_identity_headers_if_needed(transport, api_format, &mut resolved.headers);
    Some(resolved)
}

pub fn video_create_transport_unsupported_reason(
    transport: &GatewayProviderTransportSnapshot,
    family: ProviderVideoCreateFamily,
    api_format: &str,
) -> Option<&'static str> {
    match family {
        ProviderVideoCreateFamily::OpenAi => {
            local_standard_transport_unsupported_reason_with_network(transport, api_format)
        }
        ProviderVideoCreateFamily::Gemini => {
            local_gemini_transport_unsupported_reason_with_network(transport, api_format)
        }
    }
}

pub fn resolve_video_create_auth(
    transport: &GatewayProviderTransportSnapshot,
    family: ProviderVideoCreateFamily,
) -> Option<(String, String)> {
    match family {
        ProviderVideoCreateFamily::OpenAi => resolve_openai_compatible_video_auth(transport),
        ProviderVideoCreateFamily::Gemini => resolve_local_gemini_auth(transport),
    }
}

pub fn build_video_create_request_body(
    body_json: &Value,
    family: ProviderVideoCreateFamily,
    mapped_model: &str,
    body_rules: Option<&Value>,
    request_headers: Option<&http::HeaderMap>,
) -> Option<Value> {
    let mut provider_request_body = match family {
        ProviderVideoCreateFamily::OpenAi => {
            let mut provider_request_body = body_json.as_object().cloned().unwrap_or_default();
            provider_request_body
                .insert("model".to_string(), Value::String(mapped_model.to_string()));
            Value::Object(provider_request_body)
        }
        ProviderVideoCreateFamily::Gemini => body_json.clone(),
    };
    if !apply_local_body_rules_with_request_headers(
        &mut provider_request_body,
        body_rules,
        Some(body_json),
        request_headers,
    ) {
        return None;
    }
    Some(provider_request_body)
}

fn resolve_openai_compatible_video_auth(
    transport: &GatewayProviderTransportSnapshot,
) -> Option<(String, String)> {
    resolve_local_openai_bearer_auth(transport).or_else(|| {
        crate::generic_oauth::resolve_local_generic_oauth_transport_authorization(transport)
            .map(|value| ("authorization".to_string(), value))
    })
}

pub fn build_video_create_upstream_url(
    transport: &GatewayProviderTransportSnapshot,
    request_path: &str,
    request_query: Option<&str>,
    mapped_model: &str,
    family: ProviderVideoCreateFamily,
) -> Option<String> {
    let custom_path = transport
        .endpoint
        .custom_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(path) = custom_path {
        let blocked_keys = match family {
            ProviderVideoCreateFamily::OpenAi => &[][..],
            ProviderVideoCreateFamily::Gemini => &["key"][..],
        };
        return build_passthrough_path_url(
            &crate::xai::resolved_xai_request_base_url(
                transport,
                match family {
                    ProviderVideoCreateFamily::OpenAi => "openai:video",
                    ProviderVideoCreateFamily::Gemini => "gemini:video",
                },
            ),
            path,
            request_query,
            blocked_keys,
        );
    }

    match family {
        ProviderVideoCreateFamily::OpenAi => build_passthrough_path_url(
            &crate::xai::resolved_xai_request_base_url(transport, "openai:video"),
            if crate::xai::is_xai_provider_transport(transport)
                && matches!(request_path, "/v1/videos" | "/openai/v1/videos")
            {
                "/videos/generations"
            } else {
                openai_video_api_root_request_path(request_path)
            },
            request_query,
            &[],
        ),
        ProviderVideoCreateFamily::Gemini => build_gemini_video_predict_long_running_url(
            &transport.endpoint.base_url,
            mapped_model,
            request_query,
        ),
    }
}

fn openai_video_api_root_request_path(request_path: &str) -> &str {
    let request_path = request_path.strip_prefix("/openai").unwrap_or(request_path);
    if request_path.starts_with("/v1/") {
        &request_path[3..]
    } else {
        request_path
    }
}

pub fn build_video_create_headers(
    input: ProviderVideoCreateHeadersInput<'_>,
) -> Option<BTreeMap<String, String>> {
    let mut provider_request_headers = build_passthrough_headers_with_auth(
        input.headers,
        input.auth_header,
        input.auth_value,
        &BTreeMap::new(),
    );
    crate::xai::insert_cli_identity_headers_if_needed(
        input.transport,
        "openai:video",
        &mut provider_request_headers,
    );
    if !apply_local_header_rules_with_request_headers(
        &mut provider_request_headers,
        input.header_rules,
        &[input.auth_header, "content-type"],
        input.provider_request_body,
        Some(input.original_request_body),
        Some(input.headers),
    ) {
        return None;
    }
    let declared_connection_headers =
        super::headers::declared_connection_header_names(input.headers, &BTreeMap::new());
    super::headers::remove_declared_connection_headers(
        &mut provider_request_headers,
        &declared_connection_headers,
    );
    Some(provider_request_headers)
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

    let Some(mut local_transport) =
        resolve_local_video_task_transport(&transport, provider_api_format, task.model.clone())
    else {
        return Ok(None);
    };

    // Resolve deployment-managed nodes, system defaults and tunnel affinity just as
    // creation does; serialized task metadata intentionally contains no credentials.
    local_transport.proxy = lookup.resolve_video_task_proxy(&transport).await;

    let mut snapshot =
        LocalVideoTaskSnapshot::from_stored_task_with_transport(task, local_transport);
    if let Some(LocalVideoTaskSnapshot::OpenAi(seed)) = &mut snapshot {
        seed.xai_provider = crate::xai::is_xai_provider_transport(&transport);
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use aether_data_contracts::repository::video_tasks::{StoredVideoTask, VideoTaskStatus};
    use aether_video_tasks_core::LocalVideoTaskSnapshot;
    use async_trait::async_trait;
    use serde_json::json;

    use super::{
        build_video_create_headers, build_video_create_request_body,
        build_video_create_upstream_url, reconstruct_local_video_task_snapshot,
        resolve_local_video_task_transport, ProviderVideoCreateFamily,
        ProviderVideoCreateHeadersInput, VideoTaskTransportSnapshotLookup,
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
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,

                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                upstream_metadata: None,
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

    #[tokio::test]
    async fn reconstructs_video_with_configured_proxy_and_profile() {
        let mut transport = sample_transport("openai:video", "oauth");
        transport.provider.provider_type = "xai".into();
        transport.endpoint.base_url = "https://cli-chat-proxy.grok.com/v1".into();
        transport.provider.proxy = Some(json!({"enabled":true,"url":"http://127.0.0.1:9876"}));
        transport.provider.config = Some(json!({"fingerprint":{"transport_profile":{
            "profile_id":"test-video","backend":"reqwest_rustls","http_mode":"auto","pool_scope":"key"
        }}}));
        transport.key.decrypted_auth_config = Some(r#"{"using_api":false}"#.into());
        let lookup = TestLookup(Some(transport));
        let snapshot = reconstruct_local_video_task_snapshot(&lookup, &sample_stored_video_task())
            .await
            .unwrap()
            .expect("proxied video must resume after restart");
        let LocalVideoTaskSnapshot::OpenAi(seed) = snapshot else {
            panic!("expected OpenAI video")
        };
        assert!(seed.xai_provider);
        assert_eq!(
            seed.transport.proxy.as_ref().unwrap().url.as_deref(),
            Some("http://127.0.0.1:9876/")
        );
        assert_eq!(
            seed.transport
                .transport_profile
                .as_ref()
                .unwrap()
                .profile_id,
            "test-video"
        );
        assert_eq!(
            seed.transport
                .headers
                .get("x-xai-token-auth")
                .map(String::as_str),
            Some("xai-grok-cli")
        );
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
    fn builds_openai_video_create_request_body_with_mapped_model() {
        let body = build_video_create_request_body(
            &json!({"prompt": "make a clip", "model": "client-model"}),
            ProviderVideoCreateFamily::OpenAi,
            "upstream-video-model",
            None,
            None,
        )
        .expect("body should build");

        assert_eq!(body.get("prompt"), Some(&json!("make a clip")));
        assert_eq!(body.get("model"), Some(&json!("upstream-video-model")));
    }

    #[test]
    fn builds_openai_video_create_url_from_api_root_base() {
        let mut transport = sample_transport("openai:video", "bearer");
        transport.endpoint.base_url = "https://api.openai.example/v1".to_string();
        let url = build_video_create_upstream_url(
            &transport,
            "/v1/videos",
            Some("trace=1"),
            "sora-upstream",
            ProviderVideoCreateFamily::OpenAi,
        )
        .expect("url should build");

        assert_eq!(url, "https://api.openai.example/v1/videos?trace=1");
    }

    #[test]
    fn xai_video_create_paths_preserve_auth_hosts_and_custom_endpoints() {
        for (auth, base) in [
            ("oauth", "https://cli-chat-proxy.grok.com/v1"),
            ("api_key", "https://api.x.ai/v1"),
        ] {
            let mut transport = sample_transport("openai:video", auth);
            transport.provider.provider_type = "xai".into();
            transport.endpoint.base_url = "https://cli-chat-proxy.grok.com/v1".into();
            transport.key.decrypted_auth_config =
                (auth == "oauth").then(|| r#"{"using_api":false}"#.into());
            for path in ["/v1/videos", "/openai/v1/videos", "/v1/videos/generations"] {
                assert_eq!(
                    build_video_create_upstream_url(
                        &transport,
                        path,
                        Some("trace=1"),
                        "grok-imagine-video",
                        ProviderVideoCreateFamily::OpenAi
                    )
                    .unwrap(),
                    format!("{base}/videos/generations?trace=1")
                );
            }
            transport.endpoint.base_url = "https://gateway.example/prefix/v1".into();
            assert_eq!(
                build_video_create_upstream_url(
                    &transport,
                    "/openai/v1/videos",
                    None,
                    "grok-imagine-video",
                    ProviderVideoCreateFamily::OpenAi
                )
                .unwrap(),
                "https://gateway.example/prefix/v1/videos/generations"
            );
            transport.endpoint.custom_path = Some("/custom/videos/generations".into());
            let url = build_video_create_upstream_url(
                &transport,
                "/openai/v1/videos",
                None,
                "grok-imagine-video",
                ProviderVideoCreateFamily::OpenAi,
            )
            .unwrap();
            assert!(url.ends_with("/custom/videos/generations"), "{url}");
        }
        let transport = sample_transport("openai:video", "api_key");
        assert_eq!(
            build_video_create_upstream_url(
                &transport,
                "/openai/v1/videos",
                None,
                "sora",
                ProviderVideoCreateFamily::OpenAi
            ),
            build_video_create_upstream_url(
                &transport,
                "/v1/videos",
                None,
                "sora",
                ProviderVideoCreateFamily::OpenAi
            )
        );
    }

    #[test]
    fn xai_oauth_video_uses_cli_proxy() {
        let mut transport = sample_transport("openai:video", "oauth");
        transport.provider.provider_type = "xai".to_string();
        transport.endpoint.base_url = "https://cli-chat-proxy.grok.com/v1".to_string();
        transport.key.decrypted_auth_config =
            Some(r#"{"refresh_token":"rt","using_api":false}"#.to_string());
        let url = build_video_create_upstream_url(
            &transport,
            "/v1/videos/generations",
            None,
            "grok-imagine-video",
            ProviderVideoCreateFamily::OpenAi,
        )
        .expect("url should build");

        assert_eq!(url, "https://cli-chat-proxy.grok.com/v1/videos/generations");
        let headers = build_video_create_headers(ProviderVideoCreateHeadersInput {
            transport: &transport,
            headers: &http::HeaderMap::new(),
            auth_header: "authorization",
            auth_value: "Bearer test-token",
            header_rules: None,
            provider_request_body: &json!({"prompt": "A cat"}),
            original_request_body: &json!({"prompt": "A cat"}),
        })
        .unwrap();
        assert_eq!(
            headers.get("x-xai-token-auth").map(String::as_str),
            Some("xai-grok-cli")
        );

        let reconstructed = super::resolve_local_video_task_transport(
            &transport,
            "openai:video",
            Some("grok-imagine-video".into()),
        )
        .unwrap();
        assert_eq!(
            reconstructed.upstream_base_url,
            "https://cli-chat-proxy.grok.com/v1"
        );
        assert_eq!(
            reconstructed.headers.get("x-xai-token-auth"),
            headers.get("x-xai-token-auth")
        );
    }

    #[test]
    fn builds_gemini_video_create_url_and_removes_client_key_query() {
        let transport = sample_transport("gemini:video", "api_key");
        let url = build_video_create_upstream_url(
            &transport,
            "/v1beta/models/client-model:predictLongRunning",
            Some("key=client-key&trace=1"),
            "veo-upstream",
            ProviderVideoCreateFamily::Gemini,
        )
        .expect("url should build");

        assert_eq!(
            url,
            "https://example.com/v1beta/models/veo-upstream:predictLongRunning?trace=1"
        );
    }

    #[test]
    fn builds_video_create_headers_with_auth_and_rules() {
        let provider_request_body = json!({"prompt": "make a clip"});
        let original_request_body = provider_request_body.clone();
        let headers = build_video_create_headers(ProviderVideoCreateHeadersInput {
            transport: &sample_transport("openai:video", "bearer"),
            headers: &http::HeaderMap::new(),
            auth_header: "authorization",
            auth_value: "Bearer secret",
            header_rules: Some(&json!([
                {"action":"set","key":"x-provider-tag","value":"video"}
            ])),
            provider_request_body: &provider_request_body,
            original_request_body: &original_request_body,
        })
        .expect("headers should build");

        assert_eq!(
            headers.get("authorization").map(String::as_str),
            Some("Bearer secret")
        );
        assert_eq!(
            headers.get("x-provider-tag").map(String::as_str),
            Some("video")
        );
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
