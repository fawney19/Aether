use super::{
    any, build_router_with_state, build_state_with_execution_runtime_override,
    encrypt_python_fernet_plaintext, hash_api_key, json, start_server, Arc, Body, GatewayDataState,
    HeaderValue, InMemoryAuthApiKeySnapshotRepository,
    InMemoryMinimalCandidateSelectionReadRepository, InMemoryProviderCatalogReadRepository,
    InMemoryRequestCandidateRepository, InMemoryUsageReadRepository, Json, Request,
    RequestCandidateReadRepository, RequestCandidateStatus, Response, Router, StatusCode,
    StoredAuthApiKeySnapshot, StoredMinimalCandidateSelectionRow, StoredProviderCatalogEndpoint,
    StoredProviderCatalogKey, StoredProviderCatalogProvider, StoredProviderModelMapping,
    UsageReadRepository, UsageRuntimeConfig, DEVELOPMENT_ENCRYPTION_KEY, TRACE_ID_HEADER,
};
use aether_data::repository::billing::InMemoryBillingReadRepository;
use aether_data::repository::wallet::{InMemoryWalletRepository, StoredWalletSnapshot};
use aether_data_contracts::repository::billing::StoredBillingModelContext;
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use serde_json::Value;
use tokio::task::JoinHandle;

const INPUT_PRICE_PER_1M: f64 = 3.0;
const OUTPUT_PRICE_PER_1M: f64 = 15.0;
const CACHE_CREATION_PRICE_PER_1M: f64 = 3.75;
const CACHE_READ_PRICE_PER_1M: f64 = 0.30;

fn run_async_test_on_large_stack<F>(name: &'static str, future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let handle = std::thread::Builder::new()
        .name(name.to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime should build")
                .block_on(future);
        })
        .expect("large-stack usage pricing test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

#[derive(Clone, Copy)]
struct ProviderSpec {
    provider_id: &'static str,
    provider_name: &'static str,
    api_format: &'static str,
    endpoint_id: &'static str,
    key_id: &'static str,
    model_id: &'static str,
    global_model_id: &'static str,
    global_model_name: &'static str,
    provider_model_name: &'static str,
    upstream_base_url: &'static str,
    upstream_secret: &'static str,
}

const OPENAI_SPEC: ProviderSpec = ProviderSpec {
    provider_id: "provider-openai-usage-pricing-1",
    provider_name: "openai",
    api_format: "openai:chat",
    endpoint_id: "endpoint-openai-usage-pricing-1",
    key_id: "key-openai-usage-pricing-1",
    model_id: "model-openai-usage-pricing-1",
    global_model_id: "global-model-openai-usage-pricing-1",
    global_model_name: "gpt-5",
    provider_model_name: "gpt-5-upstream",
    upstream_base_url: "https://api.openai.example",
    upstream_secret: "sk-upstream-openai-usage-pricing",
};

const CLAUDE_SPEC: ProviderSpec = ProviderSpec {
    provider_id: "provider-claude-usage-pricing-1",
    provider_name: "claude",
    api_format: "claude:messages",
    endpoint_id: "endpoint-claude-usage-pricing-1",
    key_id: "key-claude-usage-pricing-1",
    model_id: "model-claude-usage-pricing-1",
    global_model_id: "global-model-claude-usage-pricing-1",
    global_model_name: "claude-sonnet-4-5",
    provider_model_name: "claude-sonnet-4-5-upstream",
    upstream_base_url: "https://api.anthropic.example",
    upstream_secret: "sk-upstream-claude-usage-pricing",
};

const GEMINI_SPEC: ProviderSpec = ProviderSpec {
    provider_id: "provider-gemini-usage-pricing-1",
    provider_name: "gemini",
    api_format: "gemini:generate_content",
    endpoint_id: "endpoint-gemini-usage-pricing-1",
    key_id: "key-gemini-usage-pricing-1",
    model_id: "model-gemini-usage-pricing-1",
    global_model_id: "global-model-gemini-usage-pricing-1",
    global_model_name: "gemini-2.5-pro",
    provider_model_name: "gemini-2.5-pro-upstream",
    upstream_base_url: "https://generativelanguage.googleapis.com",
    upstream_secret: "sk-upstream-gemini-usage-pricing",
};

#[derive(Clone, Copy)]
struct ExpectedUsagePricing {
    input_tokens: u64,
    billed_input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_creation_ephemeral_5m_tokens: u64,
    cache_creation_ephemeral_1h_tokens: u64,
    cache_read_tokens: u64,
}

impl ExpectedUsagePricing {
    fn total_tokens(self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_creation_tokens)
            .saturating_add(self.cache_read_tokens)
    }

    fn cache_creation_uncategorized_tokens(self) -> u64 {
        self.cache_creation_tokens.saturating_sub(
            self.cache_creation_ephemeral_5m_tokens
                .saturating_add(self.cache_creation_ephemeral_1h_tokens),
        )
    }

    fn total_cost(self) -> f64 {
        quantize_cost(
            self.input_cost()
                + self.output_cost()
                + self.cache_creation_uncategorized_cost()
                + self.cache_creation_ephemeral_5m_cost()
                + self.cache_creation_ephemeral_1h_cost()
                + self.cache_read_cost(),
        )
    }

    fn input_cost(self) -> f64 {
        billed_cost(self.billed_input_tokens, INPUT_PRICE_PER_1M)
    }

    fn output_cost(self) -> f64 {
        billed_cost(self.output_tokens, OUTPUT_PRICE_PER_1M)
    }

    fn cache_creation_uncategorized_cost(self) -> f64 {
        billed_cost(
            self.cache_creation_uncategorized_tokens(),
            CACHE_CREATION_PRICE_PER_1M,
        )
    }

    fn cache_creation_ephemeral_5m_cost(self) -> f64 {
        billed_cost(
            self.cache_creation_ephemeral_5m_tokens,
            CACHE_CREATION_PRICE_PER_1M,
        )
    }

    fn cache_creation_ephemeral_1h_cost(self) -> f64 {
        billed_cost(
            self.cache_creation_ephemeral_1h_tokens,
            CACHE_CREATION_PRICE_PER_1M,
        )
    }

    fn cache_read_cost(self) -> f64 {
        billed_cost(self.cache_read_tokens, CACHE_READ_PRICE_PER_1M)
    }
}

struct StartedGateway {
    gateway_url: String,
    usage_repository: Arc<InMemoryUsageReadRepository>,
    request_candidate_repository: Arc<InMemoryRequestCandidateRepository>,
    gateway_handle: JoinHandle<()>,
    execution_runtime_handle: JoinHandle<()>,
}

impl StartedGateway {
    fn shutdown(self) {
        self.gateway_handle.abort();
        self.execution_runtime_handle.abort();
    }
}

fn billed_cost(tokens: u64, price_per_1m: f64) -> f64 {
    quantize_cost(tokens as f64 * price_per_1m / 1_000_000.0)
}

fn quantize_cost(value: f64) -> f64 {
    let factor = 10_f64.powi(8);
    (value * factor).round() / factor
}

fn sample_auth_snapshot(
    spec: ProviderSpec,
    api_key_id: &str,
    user_id: &str,
) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        user_id.to_string(),
        "alice".to_string(),
        Some("alice@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(json!([spec.provider_name])),
        Some(json!([spec.api_format])),
        Some(json!([spec.global_model_name])),
        api_key_id.to_string(),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(60),
        Some(5),
        Some(4_102_444_800),
        Some(json!([spec.provider_name])),
        Some(json!([spec.api_format])),
        Some(json!([spec.global_model_name])),
    )
    .expect("auth snapshot should build")
}

fn sample_candidate_row(spec: ProviderSpec) -> StoredMinimalCandidateSelectionRow {
    let api_family = spec
        .api_format
        .split(':')
        .next()
        .unwrap_or_default()
        .to_string();
    let endpoint_kind = spec
        .api_format
        .split(':')
        .nth(1)
        .unwrap_or_default()
        .to_string();

    StoredMinimalCandidateSelectionRow {
        provider_id: spec.provider_id.to_string(),
        provider_name: spec.provider_name.to_string(),
        provider_type: "custom".to_string(),
        provider_priority: 10,
        provider_is_active: true,
        endpoint_id: spec.endpoint_id.to_string(),
        endpoint_api_format: spec.api_format.to_string(),
        endpoint_api_family: Some(api_family.clone()),
        endpoint_kind: Some(endpoint_kind),
        endpoint_is_active: true,
        key_id: spec.key_id.to_string(),
        key_name: "prod".to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec![spec.api_format.to_string()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 5,
        key_global_priority_by_format: Some(single_format_priority_map(spec.api_format)),
        model_id: spec.model_id.to_string(),
        global_model_id: spec.global_model_id.to_string(),
        global_model_name: spec.global_model_name.to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(true),
        model_provider_model_name: spec.provider_model_name.to_string(),
        model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
            name: spec.provider_model_name.to_string(),
            priority: 1,
            api_formats: Some(vec![spec.api_format.to_string()]),
        }]),
        model_supports_streaming: Some(true),
        model_is_active: true,
        model_is_available: true,
    }
}

fn sample_provider_catalog_provider(spec: ProviderSpec) -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        spec.provider_id.to_string(),
        spec.provider_name.to_string(),
        Some("https://example.com".to_string()),
        "custom".to_string(),
    )
    .expect("provider should build")
    .with_transport_fields(
        true,
        false,
        false,
        None,
        Some(2),
        None,
        Some(20.0),
        None,
        None,
    )
}

fn sample_provider_catalog_endpoint(spec: ProviderSpec) -> StoredProviderCatalogEndpoint {
    let api_family = spec
        .api_format
        .split(':')
        .next()
        .unwrap_or_default()
        .to_string();
    let endpoint_kind = spec
        .api_format
        .split(':')
        .nth(1)
        .unwrap_or_default()
        .to_string();

    StoredProviderCatalogEndpoint::new(
        spec.endpoint_id.to_string(),
        spec.provider_id.to_string(),
        spec.api_format.to_string(),
        Some(api_family),
        Some(endpoint_kind),
        true,
    )
    .expect("endpoint should build")
    .with_transport_fields(
        spec.upstream_base_url.to_string(),
        None,
        None,
        Some(2),
        None,
        None,
        None,
        None,
    )
    .expect("endpoint transport should build")
}

fn sample_provider_catalog_key(spec: ProviderSpec) -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        spec.key_id.to_string(),
        spec.provider_id.to_string(),
        "prod".to_string(),
        "api_key".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(json!([spec.api_format])),
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, spec.upstream_secret)
            .expect("api key should encrypt"),
        None,
        None,
        Some(single_format_priority_map(spec.api_format)),
        None,
        None,
        None,
        None,
    )
    .expect("key transport should build")
}

fn sample_billing_context(spec: ProviderSpec) -> StoredBillingModelContext {
    StoredBillingModelContext::new(
        spec.provider_id.to_string(),
        Some("pay_as_you_go".to_string()),
        Some(spec.key_id.to_string()),
        None,
        Some(60),
        spec.global_model_id.to_string(),
        spec.global_model_name.to_string(),
        None,
        None,
        Some(json!({
            "tiers": [{
                "up_to": null,
                "input_price_per_1m": INPUT_PRICE_PER_1M,
                "output_price_per_1m": OUTPUT_PRICE_PER_1M,
                "cache_creation_price_per_1m": CACHE_CREATION_PRICE_PER_1M,
                "cache_read_price_per_1m": CACHE_READ_PRICE_PER_1M
            }]
        })),
        Some(spec.model_id.to_string()),
        Some(spec.provider_model_name.to_string()),
        None,
        None,
        None,
    )
    .expect("billing context should build")
}

fn single_format_priority_map(api_format: &str) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(api_format.to_string(), json!(1));
    Value::Object(map)
}

fn sample_wallet_snapshot(user_id: &str) -> StoredWalletSnapshot {
    StoredWalletSnapshot::new(
        format!("wallet-{user_id}"),
        Some(user_id.to_string()),
        None,
        10.0,
        0.0,
        "finite".to_string(),
        "USD".to_string(),
        "active".to_string(),
        0.0,
        0.0,
        0.0,
        0.0,
        100,
    )
    .expect("wallet should build")
}

async fn start_local_billing_gateway(
    spec: ProviderSpec,
    client_api_key: &str,
    auth_api_key_id: &str,
    user_id: &str,
    execution_runtime: Router,
) -> StartedGateway {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(client_api_key)),
        sample_auth_snapshot(spec, auth_api_key_id, user_id),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_candidate_row(spec),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider_catalog_provider(spec)],
        vec![sample_provider_catalog_endpoint(spec)],
        vec![sample_provider_catalog_key(spec)],
    ));
    let billing_repository = Arc::new(InMemoryBillingReadRepository::seed(vec![
        sample_billing_context(spec),
    ]));
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![
        sample_wallet_snapshot(user_id),
    ]));

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let data_state = GatewayDataState::with_auth_candidate_selection_provider_catalog_request_candidates_usage_billing_and_wallet_for_tests(
        auth_repository,
        candidate_selection_repository,
        provider_catalog_repository,
        Arc::clone(&request_candidate_repository),
        Arc::clone(&usage_repository),
        billing_repository,
        wallet_repository,
        DEVELOPMENT_ENCRYPTION_KEY,
    );
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(data_state)
        .with_usage_runtime_for_tests(UsageRuntimeConfig {
            enabled: true,
            ..UsageRuntimeConfig::default()
        });
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    StartedGateway {
        gateway_url,
        usage_repository,
        request_candidate_repository,
        gateway_handle,
        execution_runtime_handle,
    }
}

async fn wait_for_usage_status<T>(
    repository: &T,
    request_id: &str,
    expected_status: &str,
) -> StoredRequestUsageAudit
where
    T: UsageReadRepository + ?Sized,
{
    let mut stored = None;
    // Usage terminal events are written on a shared background runtime; under full-suite parallel
    // load they can lag noticeably behind the request/response assertion path.
    let timeout = std::time::Duration::from_secs(30);
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        stored = repository
            .find_by_request_id(request_id)
            .await
            .expect("usage lookup should succeed");
        if stored
            .as_ref()
            .is_some_and(|usage| usage.status == expected_status)
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            let observed = stored
                .as_ref()
                .map(|usage| usage.status.as_str())
                .unwrap_or("<missing>");
            panic!(
                "usage should reach status {expected_status} within {:?}, last observed status: {observed}",
                timeout
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    stored.expect("usage should be present once the expected status is observed")
}

fn standardized_usage_json(expected: ExpectedUsagePricing) -> Value {
    json!({
        "input_tokens": expected.input_tokens,
        "output_tokens": expected.output_tokens,
        "cache_creation_tokens": expected.cache_creation_tokens,
        "cache_creation_ephemeral_5m_tokens": expected.cache_creation_ephemeral_5m_tokens,
        "cache_creation_ephemeral_1h_tokens": expected.cache_creation_ephemeral_1h_tokens,
        "cache_read_tokens": expected.cache_read_tokens,
        "reasoning_tokens": 0,
        "cache_storage_token_hours": 0.0,
        "request_count": 1,
        "dimensions": {}
    })
}

fn build_stream_frames(
    chunks: &[&str],
    standardized_usage: Value,
    response_id: &str,
    model: &str,
    elapsed_ms: u64,
    ttfb_ms: u64,
) -> String {
    let mut telemetry = serde_json::Map::new();
    telemetry.insert("elapsed_ms".to_string(), json!(elapsed_ms));
    telemetry.insert("ttfb_ms".to_string(), json!(ttfb_ms));

    let mut frames = vec![json!({
        "type": "headers",
        "payload": {
            "kind": "headers",
            "status_code": 200,
            "headers": {"content-type": "text/event-stream"}
        }
    })];

    for chunk in chunks {
        frames.push(json!({
            "type": "data",
            "payload": {
                "kind": "data",
                "text": chunk
            }
        }));
    }

    frames.push(json!({
        "type": "telemetry",
        "payload": {
            "kind": "telemetry",
            "telemetry": Value::Object(telemetry)
        }
    }));
    frames.push(json!({
        "type": "eof",
        "payload": {
            "kind": "eof",
            "summary": {
                "standardized_usage": standardized_usage,
                "finish_reason": "stop",
                "response_id": response_id,
                "model": model,
                "observed_finish": true
            }
        }
    }));

    frames
        .into_iter()
        .map(|frame| serde_json::to_string(&frame).expect("frame should encode"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn billing_snapshot(stored_usage: &StoredRequestUsageAudit) -> &Value {
    stored_usage
        .request_metadata
        .as_ref()
        .and_then(|value| value.get("billing_snapshot"))
        .expect("billing snapshot should be present")
}

fn assert_cost_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected cost {expected}, got {actual}"
    );
}

fn assert_usage_and_pricing(
    stored_usage: &StoredRequestUsageAudit,
    expected: ExpectedUsagePricing,
    expected_response_time_ms: u64,
    expected_ttfb_ms: Option<u64>,
) {
    assert_eq!(stored_usage.status, "completed");
    assert_eq!(stored_usage.billing_status, "pending");
    assert_eq!(stored_usage.input_tokens, expected.input_tokens);
    assert_eq!(stored_usage.output_tokens, expected.output_tokens);
    assert_eq!(
        stored_usage.cache_creation_input_tokens,
        expected.cache_creation_tokens
    );
    assert_eq!(
        stored_usage.cache_creation_ephemeral_5m_input_tokens,
        expected.cache_creation_ephemeral_5m_tokens
    );
    assert_eq!(
        stored_usage.cache_creation_ephemeral_1h_input_tokens,
        expected.cache_creation_ephemeral_1h_tokens
    );
    assert_eq!(
        stored_usage.cache_read_input_tokens,
        expected.cache_read_tokens
    );
    assert_eq!(stored_usage.total_tokens, expected.total_tokens());
    assert_eq!(
        stored_usage.response_time_ms,
        Some(expected_response_time_ms)
    );
    assert_eq!(stored_usage.first_byte_time_ms, expected_ttfb_ms);
    assert_eq!(
        stored_usage.settlement_input_price_per_1m(),
        Some(INPUT_PRICE_PER_1M)
    );
    assert_eq!(
        stored_usage.settlement_output_price_per_1m(),
        Some(OUTPUT_PRICE_PER_1M)
    );
    assert_eq!(
        stored_usage.settlement_cache_creation_price_per_1m(),
        Some(CACHE_CREATION_PRICE_PER_1M)
    );
    assert_eq!(
        stored_usage.settlement_cache_read_price_per_1m(),
        Some(CACHE_READ_PRICE_PER_1M)
    );

    let snapshot = billing_snapshot(stored_usage);
    assert_eq!(
        snapshot.get("status").and_then(Value::as_str),
        Some("complete")
    );
    assert_eq!(
        snapshot
            .get("resolved_dimensions")
            .and_then(|value| value.get("input_tokens"))
            .and_then(Value::as_u64),
        Some(expected.billed_input_tokens)
    );
    assert_eq!(
        snapshot
            .get("resolved_dimensions")
            .and_then(|value| value.get("output_tokens"))
            .and_then(Value::as_u64),
        Some(expected.output_tokens)
    );
    assert_eq!(
        snapshot
            .get("resolved_dimensions")
            .and_then(|value| value.get("cache_creation_tokens"))
            .and_then(Value::as_u64),
        Some(expected.cache_creation_tokens)
    );
    assert_eq!(
        snapshot
            .get("resolved_dimensions")
            .and_then(|value| value.get("cache_creation_ephemeral_5m_tokens"))
            .and_then(Value::as_u64),
        Some(expected.cache_creation_ephemeral_5m_tokens)
    );
    assert_eq!(
        snapshot
            .get("resolved_dimensions")
            .and_then(|value| value.get("cache_creation_ephemeral_1h_tokens"))
            .and_then(Value::as_u64),
        Some(expected.cache_creation_ephemeral_1h_tokens)
    );
    assert_eq!(
        snapshot
            .get("resolved_dimensions")
            .and_then(|value| value.get("cache_creation_uncategorized_tokens"))
            .and_then(Value::as_u64),
        Some(expected.cache_creation_uncategorized_tokens())
    );
    assert_eq!(
        snapshot
            .get("resolved_dimensions")
            .and_then(|value| value.get("cache_read_tokens"))
            .and_then(Value::as_u64),
        Some(expected.cache_read_tokens)
    );
    assert_cost_close(stored_usage.total_cost_usd, expected.total_cost());
    assert_cost_close(stored_usage.actual_total_cost_usd, expected.total_cost());
    assert_cost_close(
        snapshot
            .get("total_cost")
            .and_then(Value::as_f64)
            .unwrap_or_default(),
        expected.total_cost(),
    );
    assert_cost_close(
        snapshot
            .get("cost_breakdown")
            .and_then(|value| value.get("input_cost"))
            .and_then(Value::as_f64)
            .unwrap_or_default(),
        expected.input_cost(),
    );
    assert_cost_close(
        snapshot
            .get("cost_breakdown")
            .and_then(|value| value.get("output_cost"))
            .and_then(Value::as_f64)
            .unwrap_or_default(),
        expected.output_cost(),
    );
    assert_cost_close(
        snapshot
            .get("cost_breakdown")
            .and_then(|value| value.get("cache_creation_uncategorized_cost"))
            .and_then(Value::as_f64)
            .unwrap_or_default(),
        expected.cache_creation_uncategorized_cost(),
    );
    assert_cost_close(
        snapshot
            .get("cost_breakdown")
            .and_then(|value| value.get("cache_creation_ephemeral_5m_cost"))
            .and_then(Value::as_f64)
            .unwrap_or_default(),
        expected.cache_creation_ephemeral_5m_cost(),
    );
    assert_cost_close(
        snapshot
            .get("cost_breakdown")
            .and_then(|value| value.get("cache_creation_ephemeral_1h_cost"))
            .and_then(Value::as_f64)
            .unwrap_or_default(),
        expected.cache_creation_ephemeral_1h_cost(),
    );
    assert_cost_close(
        snapshot
            .get("cost_breakdown")
            .and_then(|value| value.get("cache_read_cost"))
            .and_then(Value::as_f64)
            .unwrap_or_default(),
        expected.cache_read_cost(),
    );
}

async fn assert_candidate_success(
    repository: &InMemoryRequestCandidateRepository,
    request_id: &str,
) {
    let stored_candidates = repository
        .list_by_request_id(request_id)
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);
}

#[test]
fn gateway_records_openai_sync_usage_and_pricing_with_cache_tokens() {
    run_async_test_on_large_stack(
        "gateway_records_openai_sync_usage_and_pricing_with_cache_tokens",
        gateway_records_openai_sync_usage_and_pricing_with_cache_tokens_impl(),
    );
}

async fn gateway_records_openai_sync_usage_and_pricing_with_cache_tokens_impl() {
    let expected = ExpectedUsagePricing {
        input_tokens: 120,
        billed_input_tokens: 100,
        output_tokens: 40,
        cache_creation_tokens: 80,
        cache_creation_ephemeral_5m_tokens: 0,
        cache_creation_ephemeral_1h_tokens: 0,
        cache_read_tokens: 20,
    };
    let payload = json!({
        "request_id": "trace-openai-usage-pricing-sync-123",
        "status_code": 200,
        "headers": {
            "content-type": "application/json"
        },
        "body": {
            "json_body": {
                "id": "chatcmpl-openai-usage-pricing-sync-123",
                "object": "chat.completion",
                "model": OPENAI_SPEC.provider_model_name,
                "choices": [],
                "usage": {
                    "prompt_tokens": expected.input_tokens,
                    "completion_tokens": expected.output_tokens,
                    "cache_creation_input_tokens": expected.cache_creation_tokens,
                    "cache_read_input_tokens": expected.cache_read_tokens
                }
            }
        },
        "telemetry": {
            "elapsed_ms": 25
        }
    });
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |_request: Request| {
            let payload = payload.clone();
            async move { Json(payload) }
        }),
    );
    let gateway = start_local_billing_gateway(
        OPENAI_SPEC,
        "sk-client-openai-usage-pricing-sync",
        "api-key-openai-usage-pricing-sync-1",
        "user-openai-usage-pricing-sync-1",
        execution_runtime,
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", gateway.gateway_url))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-usage-pricing-sync",
        )
        .header(TRACE_ID_HEADER, "trace-openai-usage-pricing-sync-123")
        .body("{\"model\":\"gpt-5\",\"messages\":[]}")
        .send()
        .await
        .expect("request should succeed");

    let response_status = response.status();
    let response_text = response.text().await.expect("body should read");
    assert_eq!(response_status, StatusCode::OK, "{response_text}");
    let response_json: Value = serde_json::from_str(&response_text).expect("body should parse");
    assert_eq!(response_json["model"], OPENAI_SPEC.provider_model_name);

    let stored_usage = wait_for_usage_status(
        gateway.usage_repository.as_ref(),
        "trace-openai-usage-pricing-sync-123",
        "completed",
    )
    .await;
    assert!(!stored_usage.is_stream);
    assert_usage_and_pricing(&stored_usage, expected, 25, None);
    assert_candidate_success(
        gateway.request_candidate_repository.as_ref(),
        "trace-openai-usage-pricing-sync-123",
    )
    .await;

    gateway.shutdown();
}

#[test]
fn gateway_records_openai_stream_usage_and_pricing_with_cache_tokens() {
    run_async_test_on_large_stack(
        "gateway_records_openai_stream_usage_and_pricing_with_cache_tokens",
        gateway_records_openai_stream_usage_and_pricing_with_cache_tokens_impl(),
    );
}

async fn gateway_records_openai_stream_usage_and_pricing_with_cache_tokens_impl() {
    let expected = ExpectedUsagePricing {
        input_tokens: 240,
        billed_input_tokens: 200,
        output_tokens: 60,
        cache_creation_tokens: 80,
        cache_creation_ephemeral_5m_tokens: 0,
        cache_creation_ephemeral_1h_tokens: 0,
        cache_read_tokens: 40,
    };
    let stream_body = [
        "data: {\"id\":\"chatcmpl-openai-usage-pricing-stream-123\",\"choices\":[]}\n\n",
        "data: [DONE]\n\n",
    ];
    let frames = build_stream_frames(
        &stream_body,
        standardized_usage_json(expected),
        "chatcmpl-openai-usage-pricing-stream-123",
        OPENAI_SPEC.provider_model_name,
        31,
        11,
    );
    let execution_runtime = Router::new().route(
        "/v1/execute/stream",
        any(move |_request: Request| {
            let frames = frames.clone();
            async move {
                let mut response = Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(frames))
                    .expect("response should build");
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/x-ndjson"),
                );
                response
            }
        }),
    );
    let gateway = start_local_billing_gateway(
        OPENAI_SPEC,
        "sk-client-openai-usage-pricing-stream",
        "api-key-openai-usage-pricing-stream-1",
        "user-openai-usage-pricing-stream-1",
        execution_runtime,
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", gateway.gateway_url))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-usage-pricing-stream",
        )
        .header(TRACE_ID_HEADER, "trace-openai-usage-pricing-stream-123")
        .body("{\"model\":\"gpt-5\",\"messages\":[],\"stream\":true}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.text().await.expect("stream body should read"),
        stream_body.concat()
    );

    let stored_usage = wait_for_usage_status(
        gateway.usage_repository.as_ref(),
        "trace-openai-usage-pricing-stream-123",
        "completed",
    )
    .await;
    assert!(stored_usage.is_stream);
    assert_usage_and_pricing(&stored_usage, expected, 31, Some(11));
    assert_candidate_success(
        gateway.request_candidate_repository.as_ref(),
        "trace-openai-usage-pricing-stream-123",
    )
    .await;

    gateway.shutdown();
}

#[test]
fn gateway_records_claude_sync_usage_and_pricing_with_cache_breakdown() {
    run_async_test_on_large_stack(
        "gateway_records_claude_sync_usage_and_pricing_with_cache_breakdown",
        gateway_records_claude_sync_usage_and_pricing_with_cache_breakdown_impl(),
    );
}

async fn gateway_records_claude_sync_usage_and_pricing_with_cache_breakdown_impl() {
    let expected = ExpectedUsagePricing {
        input_tokens: 50,
        billed_input_tokens: 50,
        output_tokens: 10,
        cache_creation_tokens: 20,
        cache_creation_ephemeral_5m_tokens: 8,
        cache_creation_ephemeral_1h_tokens: 12,
        cache_read_tokens: 10,
    };
    let payload = json!({
        "request_id": "trace-claude-usage-pricing-sync-123",
        "status_code": 200,
        "headers": {
            "content-type": "application/json"
        },
        "body": {
            "json_body": {
                "id": "msg-claude-usage-pricing-sync-123",
                "type": "message",
                "model": CLAUDE_SPEC.provider_model_name,
                "role": "assistant",
                "content": [],
                "usage": {
                    "input_tokens": expected.input_tokens,
                    "output_tokens": expected.output_tokens,
                    "cache_creation_input_tokens": expected.cache_creation_tokens,
                    "cache_creation": {
                        "ephemeral_5m_input_tokens": expected.cache_creation_ephemeral_5m_tokens,
                        "ephemeral_1h_input_tokens": expected.cache_creation_ephemeral_1h_tokens
                    },
                    "cache_read_input_tokens": expected.cache_read_tokens
                }
            }
        },
        "telemetry": {
            "elapsed_ms": 29
        }
    });
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |_request: Request| {
            let payload = payload.clone();
            async move { Json(payload) }
        }),
    );
    let gateway = start_local_billing_gateway(
        CLAUDE_SPEC,
        "sk-client-claude-usage-pricing-sync",
        "api-key-claude-usage-pricing-sync-1",
        "user-claude-usage-pricing-sync-1",
        execution_runtime,
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/messages", gateway.gateway_url))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header("x-api-key", "sk-client-claude-usage-pricing-sync")
        .header("anthropic-version", "2023-06-01")
        .header(
            "anthropic-beta",
            "prompt-caching-2024-07-31,context-1m-2025-08-07",
        )
        .header(TRACE_ID_HEADER, "trace-claude-usage-pricing-sync-123")
        .body("{\"model\":\"claude-sonnet-4-5\",\"messages\":[]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let response_json: Value = response.json().await.expect("body should parse");
    assert_eq!(response_json["model"], CLAUDE_SPEC.provider_model_name);

    let stored_usage = wait_for_usage_status(
        gateway.usage_repository.as_ref(),
        "trace-claude-usage-pricing-sync-123",
        "completed",
    )
    .await;
    assert!(!stored_usage.is_stream);
    assert_usage_and_pricing(&stored_usage, expected, 29, None);
    assert_candidate_success(
        gateway.request_candidate_repository.as_ref(),
        "trace-claude-usage-pricing-sync-123",
    )
    .await;

    gateway.shutdown();
}

#[test]
fn gateway_records_claude_stream_usage_and_pricing_with_cache_breakdown() {
    run_async_test_on_large_stack(
        "gateway_records_claude_stream_usage_and_pricing_with_cache_breakdown",
        gateway_records_claude_stream_usage_and_pricing_with_cache_breakdown_impl(),
    );
}

async fn gateway_records_claude_stream_usage_and_pricing_with_cache_breakdown_impl() {
    let expected = ExpectedUsagePricing {
        input_tokens: 90,
        billed_input_tokens: 90,
        output_tokens: 30,
        cache_creation_tokens: 24,
        cache_creation_ephemeral_5m_tokens: 4,
        cache_creation_ephemeral_1h_tokens: 20,
        cache_read_tokens: 10,
    };
    let stream_body = [
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg-claude-usage-pricing-stream-123\",\"type\":\"message\",\"model\":\"claude-sonnet-4-5-upstream\",\"role\":\"assistant\",\"content\":[]}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ];
    let frames = build_stream_frames(
        &stream_body,
        standardized_usage_json(expected),
        "msg-claude-usage-pricing-stream-123",
        CLAUDE_SPEC.provider_model_name,
        37,
        13,
    );
    let execution_runtime = Router::new().route(
        "/v1/execute/stream",
        any(move |_request: Request| {
            let frames = frames.clone();
            async move {
                let mut response = Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(frames))
                    .expect("response should build");
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/x-ndjson"),
                );
                response
            }
        }),
    );
    let gateway = start_local_billing_gateway(
        CLAUDE_SPEC,
        "sk-client-claude-usage-pricing-stream",
        "api-key-claude-usage-pricing-stream-1",
        "user-claude-usage-pricing-stream-1",
        execution_runtime,
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/messages", gateway.gateway_url))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header("x-api-key", "sk-client-claude-usage-pricing-stream")
        .header("anthropic-version", "2023-06-01")
        .header(
            "anthropic-beta",
            "prompt-caching-2024-07-31,context-1m-2025-08-07",
        )
        .header(TRACE_ID_HEADER, "trace-claude-usage-pricing-stream-123")
        .body("{\"model\":\"claude-sonnet-4-5\",\"messages\":[],\"stream\":true}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.text().await.expect("stream body should read"),
        stream_body.concat()
    );

    let stored_usage = wait_for_usage_status(
        gateway.usage_repository.as_ref(),
        "trace-claude-usage-pricing-stream-123",
        "completed",
    )
    .await;
    assert!(stored_usage.is_stream);
    assert_usage_and_pricing(&stored_usage, expected, 37, Some(13));
    assert_candidate_success(
        gateway.request_candidate_repository.as_ref(),
        "trace-claude-usage-pricing-stream-123",
    )
    .await;

    gateway.shutdown();
}

#[test]
fn gateway_records_gemini_sync_usage_and_pricing_with_cache_read_tokens() {
    run_async_test_on_large_stack(
        "gateway_records_gemini_sync_usage_and_pricing_with_cache_read_tokens",
        gateway_records_gemini_sync_usage_and_pricing_with_cache_read_tokens_impl(),
    );
}

async fn gateway_records_gemini_sync_usage_and_pricing_with_cache_read_tokens_impl() {
    let expected = ExpectedUsagePricing {
        input_tokens: 70,
        billed_input_tokens: 60,
        output_tokens: 20,
        cache_creation_tokens: 0,
        cache_creation_ephemeral_5m_tokens: 0,
        cache_creation_ephemeral_1h_tokens: 0,
        cache_read_tokens: 10,
    };
    let payload = json!({
        "request_id": "trace-gemini-usage-pricing-sync-123",
        "status_code": 200,
        "headers": {
            "content-type": "application/json"
        },
        "body": {
            "json_body": {
                "candidates": [],
                "usageMetadata": {
                    "promptTokenCount": expected.input_tokens,
                    "candidatesTokenCount": expected.output_tokens,
                    "cachedContentTokenCount": expected.cache_read_tokens
                }
            }
        },
        "telemetry": {
            "elapsed_ms": 27
        }
    });
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |_request: Request| {
            let payload = payload.clone();
            async move { Json(payload) }
        }),
    );
    let gateway = start_local_billing_gateway(
        GEMINI_SPEC,
        "client-gemini-usage-pricing-sync-key",
        "api-key-gemini-usage-pricing-sync-1",
        "user-gemini-usage-pricing-sync-1",
        execution_runtime,
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/v1beta/models/gemini-2.5-pro:generateContent?key=client-gemini-usage-pricing-sync-key&alt=sse",
            gateway.gateway_url
        ))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(TRACE_ID_HEADER, "trace-gemini-usage-pricing-sync-123")
        .body("{\"contents\":[]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let response_json: Value = response.json().await.expect("body should parse");
    assert_eq!(
        response_json["usageMetadata"]["cachedContentTokenCount"],
        expected.cache_read_tokens
    );

    let stored_usage = wait_for_usage_status(
        gateway.usage_repository.as_ref(),
        "trace-gemini-usage-pricing-sync-123",
        "completed",
    )
    .await;
    assert!(!stored_usage.is_stream);
    assert_usage_and_pricing(&stored_usage, expected, 27, None);
    assert_candidate_success(
        gateway.request_candidate_repository.as_ref(),
        "trace-gemini-usage-pricing-sync-123",
    )
    .await;

    gateway.shutdown();
}

#[test]
fn gateway_records_gemini_stream_usage_and_pricing_with_cache_read_tokens() {
    run_async_test_on_large_stack(
        "gateway_records_gemini_stream_usage_and_pricing_with_cache_read_tokens",
        gateway_records_gemini_stream_usage_and_pricing_with_cache_read_tokens_impl(),
    );
}

async fn gateway_records_gemini_stream_usage_and_pricing_with_cache_read_tokens_impl() {
    let expected = ExpectedUsagePricing {
        input_tokens: 110,
        billed_input_tokens: 80,
        output_tokens: 25,
        cache_creation_tokens: 0,
        cache_creation_ephemeral_5m_tokens: 0,
        cache_creation_ephemeral_1h_tokens: 0,
        cache_read_tokens: 30,
    };
    let stream_body = ["data: {\"candidates\":[]}\n\n"];
    let frames = build_stream_frames(
        &stream_body,
        standardized_usage_json(expected),
        "gemini-usage-pricing-stream-123",
        GEMINI_SPEC.provider_model_name,
        33,
        17,
    );
    let execution_runtime = Router::new().route(
        "/v1/execute/stream",
        any(move |_request: Request| {
            let frames = frames.clone();
            async move {
                let mut response = Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(frames))
                    .expect("response should build");
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/x-ndjson"),
                );
                response
            }
        }),
    );
    let gateway = start_local_billing_gateway(
        GEMINI_SPEC,
        "client-gemini-usage-pricing-stream-key",
        "api-key-gemini-usage-pricing-stream-1",
        "user-gemini-usage-pricing-stream-1",
        execution_runtime,
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/v1beta/models/gemini-2.5-pro:streamGenerateContent?key=client-gemini-usage-pricing-stream-key",
            gateway.gateway_url
        ))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(TRACE_ID_HEADER, "trace-gemini-usage-pricing-stream-123")
        .body("{\"contents\":[]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.text().await.expect("stream body should read"),
        stream_body.concat()
    );

    let stored_usage = wait_for_usage_status(
        gateway.usage_repository.as_ref(),
        "trace-gemini-usage-pricing-stream-123",
        "completed",
    )
    .await;
    assert!(stored_usage.is_stream);
    assert_usage_and_pricing(&stored_usage, expected, 33, Some(17));
    assert_candidate_success(
        gateway.request_candidate_repository.as_ref(),
        "trace-gemini-usage-pricing-stream-123",
    )
    .await;

    gateway.shutdown();
}
