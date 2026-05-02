use std::collections::BTreeMap;

use aether_contracts::{ExecutionPlan, ExecutionResult, ProxySnapshot, RequestBody};
use aether_provider_transport::antigravity::{
    build_antigravity_static_identity_headers, resolve_local_antigravity_request_auth,
    AntigravityRequestAuthSupport, ANTIGRAVITY_REQUEST_USER_AGENT,
};
use aether_provider_transport::auth::{
    ensure_upstream_auth_header, resolve_local_gemini_auth, resolve_local_openai_bearer_auth,
    resolve_local_standard_auth,
};
use aether_provider_transport::vertex::resolve_local_vertex_api_key_query_auth;
use aether_provider_transport::{
    apply_local_header_rules, resolve_transport_execution_timeouts, resolve_transport_tls_profile,
    GatewayProviderTransportSnapshot, LocalResolvedOAuthRequestAuth,
};
use async_trait::async_trait;
use serde_json::json;

use crate::build_models_fetch_url;

const OPENAI_RESPONSES_USER_AGENT: &str = "openai-codex/1.0";
const CLAUDE_CLI_USER_AGENT: &str = "claude-code/1.0.1";
const GEMINI_CLI_USER_AGENT: &str = "GeminiCLI/0.1.5 (Windows; AMD64)";
const CLAUDE_VERSION_HEADER: &str = "2023-06-01";
const ANTIGRAVITY_FETCH_PROVIDER_API_FORMAT: &str = "antigravity:fetch_available_models";
const GEMINI_CLI_LOAD_CODE_ASSIST_PROVIDER_API_FORMAT: &str = "gemini_cli:load_code_assist";

const BROWSER_FINGERPRINT_HEADERS: &[(&str, &str)] = &[
    (
        "user-agent",
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.7339.249 Electron/38.7.0 Safari/537.36",
    ),
    ("accept", "application/json"),
    ("accept-encoding", "gzip, deflate, br"),
    ("accept-language", "zh-CN"),
    ("sec-ch-ua", "\"Not=A?Brand\";v=\"24\", \"Chromium\";v=\"140\""),
    ("sec-ch-ua-mobile", "?0"),
    ("sec-ch-ua-platform", "\"macOS\""),
    ("sec-fetch-site", "cross-site"),
    ("sec-fetch-mode", "cors"),
    ("sec-fetch-dest", "empty"),
];

#[async_trait]
pub trait ModelFetchTransportRuntime: Send + Sync {
    async fn resolve_local_oauth_request_auth(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Result<Option<LocalResolvedOAuthRequestAuth>, String>;

    async fn resolve_model_fetch_proxy(
        &self,
        transport: &GatewayProviderTransportSnapshot,
    ) -> Option<ProxySnapshot>;

    async fn execute_model_fetch_execution_plan(
        &self,
        plan: &ExecutionPlan,
    ) -> Result<ExecutionResult, String>;
}

pub async fn build_models_fetch_execution_plan(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
) -> Result<ExecutionPlan, String> {
    build_standard_models_fetch_execution_plan(runtime, transport, None).await
}

struct ModelFetchExecutionPlanRequest {
    method: String,
    url: String,
    headers: BTreeMap<String, String>,
    content_type: Option<String>,
    body: RequestBody,
    client_api_format: String,
    provider_api_format: String,
    model_name: Option<String>,
}

pub async fn build_standard_models_fetch_execution_plan(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
    after_id: Option<&str>,
) -> Result<ExecutionPlan, String> {
    let api_format = transport.endpoint.api_format.trim().to_ascii_lowercase();
    let provider_api_format = api_format.clone();
    let mut headers = standard_models_fetch_headers(&api_format, &transport.provider.provider_type);
    let mut protected_headers = Vec::<String>::new();

    if api_format.starts_with("openai:") || api_format.starts_with("claude:") {
        let (auth_header_name, auth_header_value) =
            resolve_standard_header_auth(runtime, transport)
                .await?
                .ok_or_else(|| {
                    "Rust models fetch auth resolution is not supported for this key".to_string()
                })?;
        insert_non_empty_auth_header(
            &mut headers,
            &mut protected_headers,
            &auth_header_name,
            &auth_header_value,
        );
        headers = apply_fetch_header_rules(transport, headers, &protected_headers)?;
        ensure_upstream_auth_header(&mut headers, &auth_header_name, &auth_header_value);
    } else {
        headers = apply_fetch_header_rules(transport, headers, &protected_headers)?;
    }

    let upstream_url = build_standard_models_fetch_url(transport, after_id)?;
    build_execution_plan(
        runtime,
        transport,
        ModelFetchExecutionPlanRequest {
            method: "GET".to_string(),
            url: upstream_url,
            headers,
            content_type: None,
            body: RequestBody {
                json_body: None,
                body_bytes_b64: None,
                body_ref: None,
            },
            client_api_format: provider_api_format.clone(),
            provider_api_format,
            model_name: Some("models".to_string()),
        },
    )
    .await
}

pub async fn build_antigravity_fetch_available_models_plan(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
    base_url: &str,
    project_id: &str,
) -> Result<ExecutionPlan, String> {
    let authorization = resolve_oauth_header_auth(runtime, transport)
        .await?
        .ok_or_else(|| "Antigravity fetch requires OAuth authorization header".to_string())?;
    let identity_auth = match resolve_local_antigravity_request_auth(transport) {
        AntigravityRequestAuthSupport::Supported(auth) => auth,
        AntigravityRequestAuthSupport::Unsupported(reason) => {
            return Err(format!(
                "Antigravity fetch auth resolution is not supported: {reason:?}"
            ))
        }
    };

    let mut headers = build_antigravity_static_identity_headers(&identity_auth);
    headers.insert(authorization.0.clone(), authorization.1.clone());
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("accept".to_string(), "application/json".to_string());
    headers
        .entry("user-agent".to_string())
        .or_insert_with(|| ANTIGRAVITY_REQUEST_USER_AGENT.to_string());
    let protected_headers = vec![authorization.0];
    headers = apply_fetch_header_rules(transport, headers, &protected_headers)?;

    let url = format!(
        "{}{}",
        base_url.trim_end_matches('/'),
        "/v1internal:fetchAvailableModels"
    );
    build_execution_plan(
        runtime,
        transport,
        ModelFetchExecutionPlanRequest {
            method: "POST".to_string(),
            url,
            headers,
            content_type: Some("application/json".to_string()),
            body: RequestBody::from_json(json!({ "project": project_id })),
            client_api_format: "gemini:generate_content".to_string(),
            provider_api_format: ANTIGRAVITY_FETCH_PROVIDER_API_FORMAT.to_string(),
            model_name: Some("fetchAvailableModels".to_string()),
        },
    )
    .await
}

pub async fn build_gemini_cli_load_code_assist_plan(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
) -> Result<ExecutionPlan, String> {
    let authorization = resolve_bearer_or_oauth_header_auth(runtime, transport)
        .await?
        .filter(|(_, value)| !value.trim().is_empty())
        .ok_or_else(|| "GeminiCLI loadCodeAssist requires bearer or OAuth auth".to_string())?;

    let mut headers = BTreeMap::from([
        ("user-agent".to_string(), GEMINI_CLI_USER_AGENT.to_string()),
        ("accept-encoding".to_string(), "identity".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
    ]);
    let mut protected_headers = Vec::new();
    insert_non_empty_auth_header(
        &mut headers,
        &mut protected_headers,
        &authorization.0,
        &authorization.1,
    );
    headers = apply_fetch_header_rules(transport, headers, &protected_headers)?;

    build_execution_plan(
        runtime,
        transport,
        ModelFetchExecutionPlanRequest {
            method: "POST".to_string(),
            url: "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist".to_string(),
            headers,
            content_type: Some("application/json".to_string()),
            body: RequestBody::from_json(json!({
                "metadata": {
                    "ideType": "ANTIGRAVITY",
                    "platform": "PLATFORM_UNSPECIFIED",
                    "pluginType": "GEMINI",
                }
            })),
            client_api_format: "gemini:generate_content".to_string(),
            provider_api_format: GEMINI_CLI_LOAD_CODE_ASSIST_PROVIDER_API_FORMAT.to_string(),
            model_name: Some("loadCodeAssist".to_string()),
        },
    )
    .await
}

pub async fn build_vertex_models_fetch_execution_plan(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
    url: &str,
    api_format: &str,
    auth_header: Option<(String, String)>,
) -> Result<ExecutionPlan, String> {
    let mut headers = standard_models_fetch_headers(api_format, &transport.provider.provider_type);
    let mut protected_headers = Vec::<String>::new();
    if let Some((name, value)) = auth_header {
        insert_non_empty_auth_header(&mut headers, &mut protected_headers, &name, &value);
        headers = apply_fetch_header_rules(transport, headers, &protected_headers)?;
        ensure_upstream_auth_header(&mut headers, &name, &value);
    } else {
        headers = apply_fetch_header_rules(transport, headers, &protected_headers)?;
    }

    build_execution_plan(
        runtime,
        transport,
        ModelFetchExecutionPlanRequest {
            method: "GET".to_string(),
            url: url.trim().to_string(),
            headers,
            content_type: None,
            body: RequestBody {
                json_body: None,
                body_bytes_b64: None,
                body_ref: None,
            },
            client_api_format: api_format.to_string(),
            provider_api_format: api_format.to_string(),
            model_name: Some("models".to_string()),
        },
    )
    .await
}

async fn build_execution_plan(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
    request: ModelFetchExecutionPlanRequest,
) -> Result<ExecutionPlan, String> {
    let ModelFetchExecutionPlanRequest {
        method,
        url,
        headers,
        content_type,
        body,
        client_api_format,
        provider_api_format,
        model_name,
    } = request;

    Ok(ExecutionPlan {
        request_id: format!(
            "req-model-fetch-{}-{}",
            transport.key.id,
            provider_api_format.replace(':', "-")
        ),
        candidate_id: None,
        provider_name: Some(transport.provider.name.clone()),
        provider_id: transport.provider.id.clone(),
        endpoint_id: transport.endpoint.id.clone(),
        key_id: transport.key.id.clone(),
        method,
        url,
        headers,
        content_type,
        content_encoding: None,
        body,
        stream: false,
        client_api_format,
        provider_api_format,
        model_name,
        proxy: runtime.resolve_model_fetch_proxy(transport).await,
        tls_profile: resolve_transport_tls_profile(transport),
        timeouts: resolve_transport_execution_timeouts(transport),
    })
}

async fn resolve_standard_header_auth(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
) -> Result<Option<(String, String)>, String> {
    if transport.key.auth_type.trim().eq_ignore_ascii_case("oauth")
        || transport.key.auth_type.trim().eq_ignore_ascii_case("kiro")
    {
        return resolve_oauth_header_auth(runtime, transport).await;
    }

    let api_format = transport.endpoint.api_format.trim().to_ascii_lowercase();
    if api_format.starts_with("openai:") {
        return Ok(resolve_local_openai_bearer_auth(transport));
    }
    if api_format.starts_with("claude:") {
        return Ok(resolve_local_standard_auth(transport));
    }
    Ok(None)
}

async fn resolve_oauth_header_auth(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
) -> Result<Option<(String, String)>, String> {
    match runtime.resolve_local_oauth_request_auth(transport).await {
        Ok(Some(LocalResolvedOAuthRequestAuth::Header { name, value })) => Ok(Some((name, value))),
        Ok(Some(LocalResolvedOAuthRequestAuth::Kiro(_))) => Ok(None),
        Ok(None) => Ok(None),
        Err(err) => Err(err),
    }
}

async fn resolve_bearer_or_oauth_header_auth(
    runtime: &(impl ModelFetchTransportRuntime + ?Sized),
    transport: &GatewayProviderTransportSnapshot,
) -> Result<Option<(String, String)>, String> {
    if let Some(auth) = resolve_oauth_header_auth(runtime, transport).await? {
        return Ok(Some(auth));
    }

    if let Some((name, value)) = resolve_local_openai_bearer_auth(transport) {
        return Ok(Some((name, value)));
    }

    if transport
        .key
        .auth_type
        .trim()
        .eq_ignore_ascii_case("bearer")
    {
        let secret = transport.key.decrypted_api_key.trim();
        if !secret.is_empty() {
            return Ok(Some((
                "authorization".to_string(),
                format!("Bearer {secret}"),
            )));
        }
    }

    Ok(None)
}

fn apply_fetch_header_rules(
    transport: &GatewayProviderTransportSnapshot,
    mut headers: BTreeMap<String, String>,
    protected_headers: &[String],
) -> Result<BTreeMap<String, String>, String> {
    let protected = protected_headers
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    if !apply_local_header_rules(
        &mut headers,
        transport.endpoint.header_rules.as_ref(),
        &protected,
        &json!({}),
        None,
    ) {
        return Err("Endpoint header_rules application failed".to_string());
    }
    Ok(headers)
}

fn standard_models_fetch_headers(
    api_format: &str,
    provider_type: &str,
) -> BTreeMap<String, String> {
    let api_format = aether_ai_formats::normalize_api_format_alias(api_format);
    let provider_type = provider_type.trim().to_ascii_lowercase();
    match api_format.as_str() {
        "openai:responses" | "openai:responses:compact" => BTreeMap::from([(
            "user-agent".to_string(),
            OPENAI_RESPONSES_USER_AGENT.to_string(),
        )]),
        "claude:messages" => {
            let mut headers = BTreeMap::from([(
                "anthropic-version".to_string(),
                CLAUDE_VERSION_HEADER.to_string(),
            )]);
            if matches!(provider_type.as_str(), "claude_code" | "kiro") {
                headers.insert("user-agent".to_string(), CLAUDE_CLI_USER_AGENT.to_string());
            }
            headers
        }
        "gemini:generate_content" => {
            let mut headers = BROWSER_FINGERPRINT_HEADERS
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<BTreeMap<_, _>>();
            if provider_type == "gemini_cli" {
                headers.insert("user-agent".to_string(), GEMINI_CLI_USER_AGENT.to_string());
            }
            headers
        }
        _ => BTreeMap::new(),
    }
}

fn build_standard_models_fetch_url(
    transport: &GatewayProviderTransportSnapshot,
    after_id: Option<&str>,
) -> Result<String, String> {
    let api_format = transport.endpoint.api_format.trim().to_ascii_lowercase();
    if api_format.starts_with("gemini:") {
        let secret = resolve_local_vertex_api_key_query_auth(transport)
            .map(|auth| auth.value)
            .or_else(|| {
                resolve_local_gemini_auth(transport).and_then(|(name, value)| {
                    name.eq_ignore_ascii_case("x-goog-api-key").then_some(value)
                })
            })
            .or_else(|| {
                let secret = transport.key.decrypted_api_key.trim();
                (!secret.is_empty()).then_some(secret.to_string())
            })
            .ok_or_else(|| "Gemini models fetch requires an API key".to_string())?;

        let (url, _) = build_models_fetch_url(
            &transport.provider.provider_type,
            &transport.endpoint.api_format,
            &transport.endpoint.base_url,
        )
        .ok_or_else(|| "Rust models fetch does not support this provider format yet".to_string())?;
        return Ok(append_query_param(url, "key", &secret));
    }

    let (mut url, _) = build_models_fetch_url(
        &transport.provider.provider_type,
        &transport.endpoint.api_format,
        &transport.endpoint.base_url,
    )
    .ok_or_else(|| "Rust models fetch does not support this provider format yet".to_string())?;

    if api_format.starts_with("claude:") {
        url = append_query_param(url, "limit", "100");
        if let Some(after_id) = after_id.map(str::trim).filter(|value| !value.is_empty()) {
            url = append_query_param(url, "after_id", after_id);
        }
    }

    Ok(url)
}

fn append_query_param(mut url: String, key: &str, value: &str) -> String {
    if key.trim().is_empty() || value.trim().is_empty() {
        return url;
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    url.push(separator);
    url.push_str(key.trim());
    url.push('=');
    url.push_str(value.trim());
    url
}

fn insert_non_empty_auth_header(
    headers: &mut BTreeMap<String, String>,
    protected_headers: &mut Vec<String>,
    name: &str,
    value: &str,
) {
    let name = name.trim();
    let value = value.trim();
    if name.is_empty() || value.is_empty() {
        return;
    }

    protected_headers.push(name.to_string());
    headers.insert(name.to_string(), value.to_string());
}

#[cfg(test)]
mod tests {
    use aether_contracts::{ExecutionPlan, ExecutionResult, ProxySnapshot};
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use async_trait::async_trait;
    use serde_json::json;

    use super::{
        build_antigravity_fetch_available_models_plan, build_gemini_cli_load_code_assist_plan,
        build_models_fetch_execution_plan, build_standard_models_fetch_execution_plan,
        build_vertex_models_fetch_execution_plan, ModelFetchTransportRuntime,
    };

    struct TestRuntime {
        oauth_auth: Option<aether_provider_transport::LocalResolvedOAuthRequestAuth>,
        proxy: Option<ProxySnapshot>,
    }

    #[async_trait]
    impl ModelFetchTransportRuntime for TestRuntime {
        async fn resolve_local_oauth_request_auth(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
        ) -> Result<Option<aether_provider_transport::LocalResolvedOAuthRequestAuth>, String>
        {
            Ok(self.oauth_auth.clone())
        }

        async fn resolve_model_fetch_proxy(
            &self,
            _transport: &GatewayProviderTransportSnapshot,
        ) -> Option<ProxySnapshot> {
            self.proxy.clone()
        }

        async fn execute_model_fetch_execution_plan(
            &self,
            _plan: &ExecutionPlan,
        ) -> Result<ExecutionResult, String> {
            unreachable!("tests only validate plan construction")
        }
    }

    fn sample_transport(
        provider_type: &str,
        api_format: &str,
        auth_type: &str,
    ) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "Provider One".to_string(),
                provider_type: provider_type.to_string(),
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
                api_formats: Some(vec![api_format.to_string()]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,

                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: None,
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "secret".to_string(),
                decrypted_auth_config: Some(
                    r#"{"project_id":"project-1","client_version":"1.2.3","session_id":"sess-1"}"#
                        .to_string(),
                ),
            },
        }
    }

    #[tokio::test]
    async fn builds_openai_responses_models_fetch_plan_with_codex_user_agent() {
        let runtime = TestRuntime {
            oauth_auth: None,
            proxy: None,
        };
        let mut transport = sample_transport("openai", "openai:responses", "api_key");
        transport.key.decrypted_auth_config = None;
        let plan = build_models_fetch_execution_plan(&runtime, &transport)
            .await
            .expect("plan");

        assert_eq!(plan.url, "https://example.com/v1/models");
        assert_eq!(
            plan.headers.get("user-agent").map(String::as_str),
            Some("openai-codex/1.0")
        );
        assert_eq!(
            plan.headers.get("authorization").map(String::as_str),
            Some("Bearer secret")
        );
    }

    #[tokio::test]
    async fn builds_openai_responses_compact_models_fetch_plan_with_bearer_authorization() {
        let runtime = TestRuntime {
            oauth_auth: None,
            proxy: None,
        };
        let mut transport = sample_transport("openai", "openai:responses:compact", "api_key");
        transport.key.decrypted_auth_config = None;
        let plan = build_models_fetch_execution_plan(&runtime, &transport)
            .await
            .expect("plan");

        assert_eq!(plan.url, "https://example.com/v1/models");
        assert_eq!(
            plan.headers.get("authorization").map(String::as_str),
            Some("Bearer secret")
        );
    }

    #[tokio::test]
    async fn builds_claude_models_fetch_plan_with_pagination() {
        let runtime = TestRuntime {
            oauth_auth: None,
            proxy: None,
        };
        let mut transport = sample_transport("custom", "claude:messages", "api_key");
        transport.key.decrypted_auth_config = None;
        let plan =
            build_standard_models_fetch_execution_plan(&runtime, &transport, Some("cursor-1"))
                .await
                .expect("plan");

        assert_eq!(
            plan.url,
            "https://example.com/v1/models?limit=100&after_id=cursor-1"
        );
        assert_eq!(
            plan.headers.get("anthropic-version").map(String::as_str),
            Some("2023-06-01")
        );
        assert_eq!(
            plan.headers.get("x-api-key").map(String::as_str),
            Some("secret")
        );
    }

    #[tokio::test]
    async fn builds_gemini_models_fetch_plan_with_browser_headers_and_query_auth() {
        let runtime = TestRuntime {
            oauth_auth: None,
            proxy: None,
        };
        let mut transport = sample_transport("custom", "gemini:generate_content", "api_key");
        transport.key.decrypted_auth_config = None;
        let plan = build_models_fetch_execution_plan(&runtime, &transport)
            .await
            .expect("plan");

        assert_eq!(plan.url, "https://example.com/v1beta/models?key=secret");
        assert!(plan.headers.contains_key("sec-ch-ua"));
        assert_eq!(
            plan.headers.get("accept-language").map(String::as_str),
            Some("zh-CN")
        );
    }

    #[tokio::test]
    async fn builds_antigravity_fetch_available_models_plan() {
        let runtime = TestRuntime {
            oauth_auth: Some(
                aether_provider_transport::LocalResolvedOAuthRequestAuth::Header {
                    name: "authorization".to_string(),
                    value: "Bearer oauth-token".to_string(),
                },
            ),
            proxy: None,
        };
        let transport = sample_transport("antigravity", "gemini:generate_content", "oauth");
        let plan = build_antigravity_fetch_available_models_plan(
            &runtime,
            &transport,
            "https://daily-cloudcode-pa.sandbox.googleapis.com",
            "project-1",
        )
        .await
        .expect("plan");

        assert_eq!(plan.method, "POST");
        assert_eq!(
            plan.url,
            "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:fetchAvailableModels"
        );
        assert_eq!(
            plan.provider_api_format,
            "antigravity:fetch_available_models"
        );
        assert_eq!(
            plan.body
                .json_body
                .as_ref()
                .and_then(|value| value.get("project")),
            Some(&json!("project-1"))
        );
    }

    #[tokio::test]
    async fn builds_gemini_cli_load_code_assist_plan() {
        let runtime = TestRuntime {
            oauth_auth: Some(
                aether_provider_transport::LocalResolvedOAuthRequestAuth::Header {
                    name: "authorization".to_string(),
                    value: "Bearer oauth-token".to_string(),
                },
            ),
            proxy: None,
        };
        let transport = sample_transport("gemini_cli", "gemini:generate_content", "oauth");
        let plan = build_gemini_cli_load_code_assist_plan(&runtime, &transport)
            .await
            .expect("plan");

        assert_eq!(plan.method, "POST");
        assert_eq!(
            plan.url,
            "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist"
        );
        assert_eq!(
            plan.headers.get("user-agent").map(String::as_str),
            Some("GeminiCLI/0.1.5 (Windows; AMD64)")
        );
    }

    #[tokio::test]
    async fn builds_vertex_models_fetch_plan_with_auth_override() {
        let runtime = TestRuntime {
            oauth_auth: None,
            proxy: None,
        };
        let mut transport = sample_transport("vertex_ai", "claude:messages", "api_key");
        transport.key.decrypted_auth_config = None;
        let plan = build_vertex_models_fetch_execution_plan(
            &runtime,
            &transport,
            "https://aiplatform.googleapis.com/v1/publishers/google/models?key=secret",
            "gemini:generate_content",
            None,
        )
        .await
        .expect("plan");

        assert_eq!(
            plan.url,
            "https://aiplatform.googleapis.com/v1/publishers/google/models?key=secret"
        );
        assert!(plan.headers.contains_key("sec-ch-ua"));
    }
}
