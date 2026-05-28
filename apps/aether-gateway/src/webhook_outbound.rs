use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use aether_data::repository::webhook_notifications::{
    ClaimWebhookDeliveriesRequest, CreateWebhookDeliveryRecord, RecordWebhookDeliveryAttemptRecord,
    RescheduleWebhookDeliveryRecord, StoredWebhookDelivery, StoredWebhookEndpoint,
    UpdateWebhookEndpointTestResultRecord, UpsertWebhookEndpointRecord, WebhookDeliveryListQuery,
    WebhookEndpointListQuery, WebhookSecretUpdate,
};
use axum::body::Bytes;
use axum::http::{self, HeaderMap, HeaderName, HeaderValue, StatusCode};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::net::lookup_host;
use tracing::{debug, warn};
use url::Url;
use uuid::Uuid;

use crate::admin_api::{AdminAppState, AdminRequestContext};
use crate::handlers::shared::{
    decrypt_catalog_secret_with_fallbacks, encrypt_catalog_secret_with_fallbacks,
    unix_secs_to_rfc3339,
};
use crate::{AppState, GatewayError};

const GLOBAL_ENABLED_CONFIG_KEY: &str = "module.webhook_outbound.enabled";
const DEFAULT_TIMEOUT_MS: u32 = 5_000;
const MIN_TIMEOUT_MS: u32 = 1_000;
const MAX_TIMEOUT_MS: u32 = 30_000;
const DEFAULT_MAX_ATTEMPTS: u32 = 5;
const MAX_MAX_ATTEMPTS: u32 = 11;
const DEFAULT_INITIAL_RETRY_DELAY_SECONDS: u32 = 60;
const DEFAULT_MAX_RETRY_DELAY_SECONDS: u32 = 3_600;
const RESPONSE_EXCERPT_LIMIT: usize = 2_048;
const WORKER_INTERVAL: Duration = Duration::from_secs(1);
const WORKER_BATCH_SIZE: usize = 20;
const LOCK_TTL: Duration = Duration::from_secs(60);
const EVENT_FILTER_LIMIT: usize = 64;

pub(crate) const OUTBOUND_WEBHOOK_EVENTS: &[&str] = &[
    "user.registered",
    "wallet.recharged",
    "api_key.created",
    "risk_control.hit",
    "provider.error",
    "balance.low",
    "webhook.test",
];

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OutboundWebhookEvent {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) event_type: String,
    pub(crate) occurred_at_unix_secs: u64,
    #[serde(default)]
    pub(crate) data: Value,
}

impl OutboundWebhookEvent {
    pub(crate) fn new(event_type: impl Into<String>, data: Value) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            event_type: event_type.into(),
            occurred_at_unix_secs: now_unix_secs(),
            data,
        }
    }
}

#[derive(Debug, Clone)]
struct OutboundWebhookEndpoint {
    id: String,
    name: String,
    url: String,
    enabled: bool,
    events: Vec<String>,
    secret: Option<String>,
    timeout_ms: u32,
    max_attempts: u32,
    initial_retry_delay_seconds: u32,
    max_retry_delay_seconds: u32,
    allow_insecure_http: bool,
    allow_private_network: bool,
}

#[derive(Debug, Clone)]
struct OutboundWebhookDeliveryRecord {
    id: String,
    endpoint_id: String,
    endpoint_name: String,
    event_id: String,
    event_type: String,
    event: OutboundWebhookEvent,
    attempt_count: u32,
    last_attempt_at_unix_secs: Option<u64>,
}

#[derive(Debug, Clone)]
struct DeliveryAttemptOutcome {
    success: bool,
    retryable: bool,
    http_status: Option<u16>,
    error: Option<String>,
    response_excerpt: Option<String>,
    duration_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct OutboundWebhookEnqueueSummary {
    pub(crate) queued: usize,
    pub(crate) skipped: usize,
    pub(crate) deliveries: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct AdminWebhookEndpointUpsertRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    subscribed_events: Option<Vec<String>>,
    #[serde(default)]
    timeout_ms: Option<u32>,
    #[serde(default)]
    max_retries: Option<u32>,
    #[serde(default)]
    secret: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct AdminWebhookTestDeliveryRequest {
    #[serde(default)]
    event_type: Option<String>,
    #[serde(default)]
    payload: Option<Value>,
    #[serde(default)]
    data: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct LegacyOutboundWebhookConfigUpdate {
    #[serde(default)]
    enabled: Option<bool>,
}

pub(crate) fn spawn_outbound_webhook_worker(
    state: AppState,
) -> Option<tokio::task::JoinHandle<()>> {
    Some(tokio::spawn(async move {
        let mut interval = tokio::time::interval(WORKER_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        loop {
            interval.tick().await;
            match process_due_outbound_webhooks(&state, WORKER_BATCH_SIZE).await {
                Ok(processed) if processed > 0 => debug!(
                    event_name = "outbound_webhook_worker_processed",
                    log_type = "ops",
                    processed,
                    "outbound webhook worker processed due deliveries"
                ),
                Ok(_) => {}
                Err(err) => warn!(
                    event_name = "outbound_webhook_worker_failed",
                    log_type = "ops",
                    error = %err.into_message(),
                    "outbound webhook worker failed"
                ),
            }
        }
    }))
}

pub(crate) async fn enqueue_outbound_webhook_event(
    state: &AppState,
    event: OutboundWebhookEvent,
) -> Result<OutboundWebhookEnqueueSummary, GatewayError> {
    if !webhook_outbound_enabled(state).await? {
        return Ok(OutboundWebhookEnqueueSummary {
            queued: 0,
            skipped: 0,
            deliveries: Vec::new(),
        });
    }

    let endpoints = runtime_endpoints(state).await?;
    let mut queued = 0usize;
    let mut skipped = 0usize;
    let mut deliveries = Vec::new();
    for endpoint in endpoints.iter().filter(|endpoint| endpoint.enabled) {
        if !endpoint_accepts_event(endpoint, &event.event_type) {
            skipped += 1;
            continue;
        }
        if endpoint
            .secret
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            skipped += 1;
            continue;
        }
        let delivery = create_webhook_delivery_for_endpoint(state, endpoint, &event, false).await?;
        if let Some(delivery) = delivery {
            queued += 1;
            deliveries.push(delivery.id);
        }
    }

    Ok(OutboundWebhookEnqueueSummary {
        queued,
        skipped,
        deliveries,
    })
}

pub(crate) async fn enqueue_outbound_webhook_event_best_effort(
    state: &AppState,
    event_type: impl Into<String>,
    data: Value,
) {
    let event_type = event_type.into();
    let event = OutboundWebhookEvent::new(event_type.clone(), data);
    match enqueue_outbound_webhook_event(state, event).await {
        Ok(summary) if summary.queued > 0 => debug!(
            event_name = "outbound_webhook_event_queued",
            log_type = "event",
            event_type = %event_type,
            queued = summary.queued,
            skipped = summary.skipped,
            "outbound webhook event queued"
        ),
        Ok(_) => {}
        Err(err) => warn!(
            event_name = "outbound_webhook_event_enqueue_failed",
            log_type = "ops",
            event_type = %event_type,
            error = %err.into_message(),
            "failed to enqueue outbound webhook event"
        ),
    }
}

pub(crate) fn spawn_outbound_webhook_event_best_effort(
    state: AppState,
    event_type: impl Into<String> + Send + 'static,
    data: Value,
) {
    let event_type = event_type.into();
    tokio::spawn(async move {
        enqueue_outbound_webhook_event_best_effort(&state, event_type, data).await;
    });
}

pub(crate) async fn build_admin_outbound_webhook_config_payload(
    state: &AdminAppState<'_>,
) -> Result<Value, GatewayError> {
    Ok(json!({
        "enabled": webhook_outbound_enabled(state.app()).await?,
        "available_events": OUTBOUND_WEBHOOK_EVENTS,
    }))
}

pub(crate) async fn apply_admin_outbound_webhook_config_update(
    state: &AdminAppState<'_>,
    request_body: Option<&Bytes>,
) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
    let request = match parse_json_request::<LegacyOutboundWebhookConfigUpdate>(request_body) {
        Ok(request) => request,
        Err(response) => return Ok(Err(response)),
    };
    if let Some(enabled) = request.enabled {
        state
            .app()
            .upsert_system_config_json_value(
                GLOBAL_ENABLED_CONFIG_KEY,
                &json!(enabled),
                Some("Outbound webhook global enabled switch"),
            )
            .await?;
    }
    Ok(Ok(build_admin_outbound_webhook_config_payload(state).await?))
}

pub(crate) async fn build_admin_outbound_webhook_endpoints_payload(
    state: &AdminAppState<'_>,
) -> Result<Value, GatewayError> {
    let endpoints = state
        .app()
        .data
        .list_webhook_endpoints(&WebhookEndpointListQuery::default())
        .await
        .map_err(map_data_layer_error)?;
    let delivery_summary = load_endpoint_delivery_summary(state.app()).await?;
    let items = endpoints
        .iter()
        .map(|endpoint| {
            stored_endpoint_to_admin_payload(endpoint, delivery_summary.get(&endpoint.id))
        })
        .collect::<Vec<_>>();
    Ok(json!({ "items": items }))
}

pub(crate) async fn create_admin_outbound_webhook_endpoint_payload(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
    let request = match parse_json_request::<AdminWebhookEndpointUpsertRequest>(request_body) {
        Ok(request) => request,
        Err(response) => return Ok(Err(response)),
    };
    let record = match build_endpoint_upsert_record(
        state.app(),
        request,
        None,
        admin_actor_id(request_context),
    ) {
        Ok(record) => record,
        Err(message) => return Ok(Err(bad_request(message))),
    };
    let endpoint = state
        .app()
        .data
        .upsert_webhook_endpoint(&record)
        .await
        .map_err(map_data_layer_error)?
        .ok_or_else(|| {
            GatewayError::Internal("webhook endpoint repository is unavailable".into())
        })?;
    Ok(Ok(stored_endpoint_to_admin_payload(&endpoint, None)))
}

pub(crate) async fn update_admin_outbound_webhook_endpoint_payload(
    state: &AdminAppState<'_>,
    endpoint_id: &str,
    request_body: Option<&Bytes>,
) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
    if !is_valid_id(endpoint_id) {
        return Ok(Err(bad_request("invalid endpoint id")));
    }
    let Some(existing) = state
        .app()
        .data
        .find_webhook_endpoint(endpoint_id)
        .await
        .map_err(map_data_layer_error)?
    else {
        return Ok(Err((
            StatusCode::NOT_FOUND,
            json!({ "detail": "webhook endpoint not found" }),
        )));
    };
    let request = match parse_json_request::<AdminWebhookEndpointUpsertRequest>(request_body) {
        Ok(request) => request,
        Err(response) => return Ok(Err(response)),
    };
    let record = match build_endpoint_upsert_record(state.app(), request, Some(&existing), None) {
        Ok(record) => record,
        Err(message) => return Ok(Err(bad_request(message))),
    };
    let endpoint = state
        .app()
        .data
        .upsert_webhook_endpoint(&record)
        .await
        .map_err(map_data_layer_error)?
        .ok_or_else(|| {
            GatewayError::Internal("webhook endpoint repository is unavailable".into())
        })?;
    let delivery_summary = load_endpoint_delivery_summary(state.app()).await?;
    Ok(Ok(stored_endpoint_to_admin_payload(
        &endpoint,
        delivery_summary.get(&endpoint.id),
    )))
}

pub(crate) async fn delete_admin_outbound_webhook_endpoint_payload(
    state: &AdminAppState<'_>,
    endpoint_id: &str,
) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
    if !is_valid_id(endpoint_id) {
        return Ok(Err(bad_request("invalid endpoint id")));
    }
    let deleted = state
        .app()
        .data
        .delete_webhook_endpoint(endpoint_id)
        .await
        .map_err(map_data_layer_error)?;
    if !deleted {
        return Ok(Err((
            StatusCode::NOT_FOUND,
            json!({ "detail": "webhook endpoint not found" }),
        )));
    }
    Ok(Ok(json!({ "deleted": true })))
}

pub(crate) async fn build_admin_outbound_webhook_deliveries_payload(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Value, GatewayError> {
    let limit = query_usize(request_context.query_string(), "limit")
        .unwrap_or(50)
        .clamp(1, 200);
    let status_filter =
        query_param(request_context.query_string(), "status").filter(|value| value != "all");
    let endpoint_filter =
        query_param(request_context.query_string(), "endpoint_id").filter(|value| value != "all");
    let event_filter =
        query_param(request_context.query_string(), "event_type").filter(|value| value != "all");
    let page = state
        .app()
        .data
        .list_webhook_deliveries(&WebhookDeliveryListQuery {
            endpoint_id: endpoint_filter,
            status: status_filter,
            event_type: event_filter,
            is_test: None,
            offset: 0,
            limit,
        })
        .await
        .map_err(map_data_layer_error)?;
    let endpoint_map = load_endpoint_map(state.app()).await?;
    let items = page
        .items
        .iter()
        .map(|delivery| {
            stored_delivery_to_admin_payload(delivery, endpoint_map.get(&delivery.endpoint_id))
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "items": items,
        "total": page.total,
    }))
}

pub(crate) async fn build_admin_outbound_webhook_test_payload(
    state: &AdminAppState<'_>,
    request_body: Option<&Bytes>,
) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
    let request =
        parse_json_request::<AdminWebhookTestDeliveryRequest>(request_body).unwrap_or_default();
    let event_type = request
        .event_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("webhook.test")
        .to_ascii_lowercase();
    if !is_valid_event_filter(&event_type) || event_type == "*" {
        return Ok(Err(bad_request("invalid event_type")));
    }
    let endpoints = state
        .app()
        .data
        .list_webhook_endpoints(&WebhookEndpointListQuery {
            enabled: Some(true),
            event_type: Some(event_type.clone()),
        })
        .await
        .map_err(map_data_layer_error)?;
    if endpoints.is_empty() {
        return Ok(Ok(json!({
            "success": false,
            "message": "no enabled endpoint matches the test event",
            "reports": [],
        })));
    }

    let mut reports = Vec::new();
    for endpoint in endpoints {
        let result = send_test_delivery_to_endpoint(
            state.app(),
            &endpoint,
            AdminWebhookTestDeliveryRequest {
                event_type: Some(event_type.clone()),
                payload: request.payload.clone().or(request.data.clone()),
                data: None,
            },
        )
        .await?;
        match result {
            Ok(report) => reports.push(report),
            Err((_status, payload)) => reports.push(payload),
        }
    }
    let success = reports
        .iter()
        .any(|report| report.get("status").and_then(Value::as_str) == Some("succeeded"));
    Ok(Ok(json!({
        "success": success,
        "message": if success { "test webhook delivered" } else { "test webhook failed" },
        "reports": reports,
    })))
}

pub(crate) async fn build_admin_outbound_webhook_endpoint_test_payload(
    state: &AdminAppState<'_>,
    endpoint_id: &str,
    request_body: Option<&Bytes>,
) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
    if !is_valid_id(endpoint_id) {
        return Ok(Err(bad_request("invalid endpoint id")));
    }
    let Some(endpoint) = state
        .app()
        .data
        .find_webhook_endpoint(endpoint_id)
        .await
        .map_err(map_data_layer_error)?
    else {
        return Ok(Err((
            StatusCode::NOT_FOUND,
            json!({ "detail": "webhook endpoint not found" }),
        )));
    };
    let request = match parse_json_request::<AdminWebhookTestDeliveryRequest>(request_body) {
        Ok(request) => request,
        Err(response) => return Ok(Err(response)),
    };
    send_test_delivery_to_endpoint(state.app(), &endpoint, request).await
}

pub(crate) async fn retry_outbound_webhook_delivery_payload(
    state: &AdminAppState<'_>,
    delivery_id: &str,
) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
    if !is_valid_id(delivery_id) {
        return Ok(Err(bad_request("invalid delivery id")));
    }
    let Some(record) = state
        .app()
        .data
        .find_webhook_delivery(delivery_id)
        .await
        .map_err(map_data_layer_error)?
    else {
        return Ok(Err((
            StatusCode::NOT_FOUND,
            json!({ "detail": "delivery not found" }),
        )));
    };
    if record.status == "delivering" {
        return Ok(Err((
            StatusCode::CONFLICT,
            json!({ "detail": "delivery is already in progress" }),
        )));
    }
    let now = now_unix_secs();
    let updated = state
        .app()
        .data
        .reschedule_webhook_delivery(&RescheduleWebhookDeliveryRecord {
            delivery_id: delivery_id.to_string(),
            status: "retrying".to_string(),
            next_attempt_at_unix_secs: now,
            updated_at_unix_secs: now,
        })
        .await
        .map_err(map_data_layer_error)?;
    if !updated {
        return Ok(Err((
            StatusCode::CONFLICT,
            json!({ "detail": "delivery retry could not be queued" }),
        )));
    }
    let delivery = state
        .app()
        .data
        .find_webhook_delivery(delivery_id)
        .await
        .map_err(map_data_layer_error)?
        .unwrap_or(record);
    let endpoint_map = load_endpoint_map(state.app()).await?;
    Ok(Ok(stored_delivery_to_admin_payload(
        &delivery,
        endpoint_map.get(&delivery.endpoint_id),
    )))
}

async fn process_due_outbound_webhooks(
    state: &AppState,
    batch_size: usize,
) -> Result<usize, GatewayError> {
    if !webhook_outbound_enabled(state).await? {
        return Ok(0);
    }
    let now = now_unix_secs();
    let endpoints = runtime_endpoints(state)
        .await?
        .into_iter()
        .map(|endpoint| (endpoint.id.clone(), endpoint))
        .collect::<BTreeMap<_, _>>();
    if endpoints.is_empty() {
        return Ok(0);
    }
    let claimed = state
        .data
        .claim_due_webhook_deliveries(&ClaimWebhookDeliveriesRequest {
            owner: format!("gateway-{}", std::process::id()),
            now_unix_secs: now,
            lease_seconds: LOCK_TTL.as_secs(),
            limit: batch_size.max(1),
        })
        .await
        .map_err(map_data_layer_error)?;

    let mut processed = 0usize;
    for delivery in claimed {
        match process_single_due_delivery(state, &endpoints, delivery, now).await {
            Ok(true) => processed += 1,
            Ok(false) => {}
            Err(err) => warn!(
                event_name = "outbound_webhook_delivery_process_failed",
                log_type = "ops",
                error = %err.into_message(),
                "failed to process outbound webhook delivery"
            ),
        }
    }
    Ok(processed)
}

async fn process_single_due_delivery(
    state: &AppState,
    endpoints: &BTreeMap<String, OutboundWebhookEndpoint>,
    delivery: StoredWebhookDelivery,
    now: u64,
) -> Result<bool, GatewayError> {
    let Some(endpoint) = endpoints.get(&delivery.endpoint_id) else {
        record_delivery_failure_without_http(
            state,
            &delivery,
            "cancelled",
            "endpoint_missing: webhook endpoint no longer exists",
            now,
        )
        .await?;
        return Ok(false);
    };
    if !endpoint.enabled {
        record_delivery_failure_without_http(
            state,
            &delivery,
            "cancelled",
            "endpoint_disabled: webhook endpoint is disabled",
            now,
        )
        .await?;
        return Ok(false);
    }
    if !endpoint_accepts_event(endpoint, &delivery.event_type) {
        record_delivery_failure_without_http(
            state,
            &delivery,
            "cancelled",
            "event_filter_mismatch: webhook endpoint no longer subscribes to this event",
            now,
        )
        .await?;
        return Ok(false);
    }

    let record = delivery_record_from_stored(endpoint, &delivery, now);
    let outcome = deliver_outbound_webhook(endpoint, &record).await;
    record_delivery_attempt_outcome(state, endpoint, &delivery, outcome, now).await?;
    Ok(true)
}

async fn send_test_delivery_to_endpoint(
    state: &AppState,
    endpoint: &StoredWebhookEndpoint,
    request: AdminWebhookTestDeliveryRequest,
) -> Result<Result<Value, (http::StatusCode, Value)>, GatewayError> {
    let event_type = request
        .event_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("webhook.test")
        .to_ascii_lowercase();
    if !is_valid_event_filter(&event_type) || event_type == "*" {
        return Ok(Err(bad_request("invalid event_type")));
    }
    let runtime_endpoint = match stored_endpoint_to_runtime(state, endpoint) {
        Ok(endpoint) => endpoint,
        Err(message) => return Ok(Err(bad_request(message))),
    };
    if runtime_endpoint
        .secret
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        return Ok(Err(bad_request(
            "enabled webhook endpoint requires a secret",
        )));
    }
    let payload = request.payload.or(request.data).unwrap_or_else(|| {
        json!({
            "source": "admin_test",
            "message": "Aether outbound webhook test event"
        })
    });
    let event = OutboundWebhookEvent::new(event_type, payload);
    let Some(delivery) =
        create_webhook_delivery_for_endpoint(state, &runtime_endpoint, &event, true).await?
    else {
        return Ok(Err((
            StatusCode::CONFLICT,
            json!({ "detail": "test delivery could not be created" }),
        )));
    };
    let now = now_unix_secs();
    let record = delivery_record_from_stored(&runtime_endpoint, &delivery, now);
    let outcome = deliver_outbound_webhook(&runtime_endpoint, &record).await;
    let final_status =
        record_delivery_attempt_outcome(state, &runtime_endpoint, &delivery, outcome, now).await?;
    let delivery = state
        .data
        .find_webhook_delivery(&delivery.id)
        .await
        .map_err(map_data_layer_error)?
        .unwrap_or(delivery);
    state
        .data
        .update_webhook_endpoint_test_result(&UpdateWebhookEndpointTestResultRecord {
            endpoint_id: endpoint.id.clone(),
            delivery_id: Some(delivery.id.clone()),
            status: final_status,
            error_message: delivery.last_error.clone(),
            tested_at_unix_secs: now_unix_secs(),
        })
        .await
        .map_err(map_data_layer_error)?;
    Ok(Ok(stored_delivery_to_admin_payload(
        &delivery,
        Some(endpoint),
    )))
}

async fn create_webhook_delivery_for_endpoint(
    state: &AppState,
    endpoint: &OutboundWebhookEndpoint,
    event: &OutboundWebhookEvent,
    is_test: bool,
) -> Result<Option<StoredWebhookDelivery>, GatewayError> {
    state
        .data
        .create_webhook_delivery(&CreateWebhookDeliveryRecord {
            id: None,
            endpoint_id: endpoint.id.clone(),
            event_id: event.id.clone(),
            event_type: event.event_type.clone(),
            is_test,
            payload_json: event_to_delivery_payload(event),
            max_attempts: endpoint.max_attempts.clamp(1, MAX_MAX_ATTEMPTS),
            next_attempt_at_unix_secs: now_unix_secs(),
        })
        .await
        .map_err(map_data_layer_error)
}

async fn record_delivery_failure_without_http(
    state: &AppState,
    delivery: &StoredWebhookDelivery,
    delivery_status: &str,
    error: &str,
    now: u64,
) -> Result<(), GatewayError> {
    state
        .data
        .record_webhook_delivery_attempt(&RecordWebhookDeliveryAttemptRecord {
            id: None,
            delivery_id: delivery.id.clone(),
            attempt_index: delivery.attempt_count.saturating_add(1),
            status: "failed".to_string(),
            http_status: None,
            error_message: Some(error.to_string()),
            response_excerpt: None,
            started_at_unix_secs: now,
            completed_at_unix_secs: Some(now),
            duration_ms: Some(0),
            delivery_status: delivery_status.to_string(),
            next_attempt_at_unix_secs: None,
            delivered_at_unix_secs: None,
            failed_at_unix_secs: Some(now),
        })
        .await
        .map_err(map_data_layer_error)?;
    Ok(())
}

async fn record_delivery_attempt_outcome(
    state: &AppState,
    endpoint: &OutboundWebhookEndpoint,
    delivery: &StoredWebhookDelivery,
    outcome: DeliveryAttemptOutcome,
    started_at: u64,
) -> Result<String, GatewayError> {
    let now = now_unix_secs();
    let attempt_index = delivery.attempt_count.saturating_add(1);
    let retryable = outcome.retryable && attempt_index < delivery.max_attempts.max(1);
    let (delivery_status, next_attempt_at, delivered_at, failed_at) = if outcome.success {
        ("succeeded".to_string(), None, Some(now), None)
    } else if retryable {
        (
            "retrying".to_string(),
            Some(now.saturating_add(retry_delay_seconds_for_endpoint(endpoint, attempt_index))),
            None,
            None,
        )
    } else {
        ("dead".to_string(), None, None, Some(now))
    };
    state
        .data
        .record_webhook_delivery_attempt(&RecordWebhookDeliveryAttemptRecord {
            id: None,
            delivery_id: delivery.id.clone(),
            attempt_index,
            status: if outcome.success {
                "succeeded"
            } else {
                "failed"
            }
            .to_string(),
            http_status: outcome.http_status.map(i32::from),
            error_message: outcome.error,
            response_excerpt: outcome.response_excerpt,
            started_at_unix_secs: started_at,
            completed_at_unix_secs: Some(now),
            duration_ms: Some(u32::try_from(outcome.duration_ms).unwrap_or(u32::MAX)),
            delivery_status: delivery_status.clone(),
            next_attempt_at_unix_secs: next_attempt_at,
            delivered_at_unix_secs: delivered_at,
            failed_at_unix_secs: failed_at,
        })
        .await
        .map_err(map_data_layer_error)?;
    Ok(delivery_status)
}

async fn deliver_outbound_webhook(
    endpoint: &OutboundWebhookEndpoint,
    record: &OutboundWebhookDeliveryRecord,
) -> DeliveryAttemptOutcome {
    let started = Instant::now();
    let parsed_url = match validate_webhook_url_shape(endpoint) {
        Ok(url) => url,
        Err(error) => return failed_outcome(started, false, None, error, None),
    };
    let resolved = match resolve_and_validate_endpoint(&parsed_url, endpoint).await {
        Ok(addrs) => addrs,
        Err(error) => return failed_outcome(started, false, None, error, None),
    };
    let body = match delivery_body(record) {
        Ok(body) => body,
        Err(error) => return failed_outcome(started, false, None, error, None),
    };
    let headers = match build_delivery_headers(endpoint, record, &body) {
        Ok(headers) => headers,
        Err(error) => return failed_outcome(started, false, None, error, None),
    };
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(u64::from(
            endpoint.timeout_ms.clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS),
        )))
        .resolve_to_addrs(parsed_url.host_str().unwrap_or_default(), &resolved)
        .build();
    let client = match client {
        Ok(client) => client,
        Err(error) => return failed_outcome(started, false, None, error.to_string(), None),
    };
    let response = client
        .post(parsed_url)
        .headers(headers)
        .body(body)
        .send()
        .await;
    let response = match response {
        Ok(response) => response,
        Err(error) => return failed_outcome(started, true, None, error.to_string(), None),
    };
    let status = response.status();
    let status_code = status.as_u16();
    let text = response.text().await.unwrap_or_default();
    let excerpt = truncate_for_log(&text, RESPONSE_EXCERPT_LIMIT);
    if status.is_success() {
        DeliveryAttemptOutcome {
            success: true,
            retryable: false,
            http_status: Some(status_code),
            error: None,
            response_excerpt: (!excerpt.is_empty()).then_some(excerpt),
            duration_ms: elapsed_ms(started),
        }
    } else {
        failed_outcome(
            started,
            http_status_is_retryable(status),
            Some(status_code),
            format!("webhook endpoint returned HTTP {status_code}"),
            (!excerpt.is_empty()).then_some(excerpt),
        )
    }
}

fn failed_outcome(
    started: Instant,
    retryable: bool,
    http_status: Option<u16>,
    error: String,
    response_excerpt: Option<String>,
) -> DeliveryAttemptOutcome {
    DeliveryAttemptOutcome {
        success: false,
        retryable,
        http_status,
        error: Some(error),
        response_excerpt,
        duration_ms: elapsed_ms(started),
    }
}

async fn webhook_outbound_enabled(state: &AppState) -> Result<bool, GatewayError> {
    Ok(state
        .read_system_config_json_value(GLOBAL_ENABLED_CONFIG_KEY)
        .await?
        .and_then(|value| value.as_bool())
        .unwrap_or(true))
}

async fn runtime_endpoints(state: &AppState) -> Result<Vec<OutboundWebhookEndpoint>, GatewayError> {
    let endpoints = state
        .data
        .list_webhook_endpoints(&WebhookEndpointListQuery::default())
        .await
        .map_err(map_data_layer_error)?;
    Ok(endpoints
        .iter()
        .filter_map(
            |endpoint| match stored_endpoint_to_runtime(state, endpoint) {
                Ok(endpoint) => Some(endpoint),
                Err(error) => {
                    warn!(
                        event_name = "outbound_webhook_endpoint_load_failed",
                        log_type = "ops",
                        error = %error,
                        "failed to load outbound webhook endpoint"
                    );
                    None
                }
            },
        )
        .collect())
}

async fn load_endpoint_map(
    state: &AppState,
) -> Result<BTreeMap<String, StoredWebhookEndpoint>, GatewayError> {
    let endpoints = state
        .data
        .list_webhook_endpoints(&WebhookEndpointListQuery::default())
        .await
        .map_err(map_data_layer_error)?;
    Ok(endpoints
        .into_iter()
        .map(|endpoint| (endpoint.id.clone(), endpoint))
        .collect())
}

#[derive(Debug, Clone, Default)]
struct EndpointDeliverySummary {
    last_delivery_at_unix_secs: Option<u64>,
    last_delivery_status: Option<String>,
    failure_count: u64,
}

async fn load_endpoint_delivery_summary(
    state: &AppState,
) -> Result<BTreeMap<String, EndpointDeliverySummary>, GatewayError> {
    let page = state
        .data
        .list_webhook_deliveries(&WebhookDeliveryListQuery {
            endpoint_id: None,
            status: None,
            event_type: None,
            is_test: None,
            offset: 0,
            limit: 1_000,
        })
        .await
        .map_err(map_data_layer_error)?;
    let mut summaries = BTreeMap::<String, EndpointDeliverySummary>::new();
    for delivery in page.items {
        let summary = summaries.entry(delivery.endpoint_id.clone()).or_default();
        let seen_at = delivery
            .delivered_at_unix_secs
            .or(delivery.last_attempt_at_unix_secs)
            .unwrap_or(delivery.updated_at_unix_secs);
        if summary
            .last_delivery_at_unix_secs
            .is_none_or(|current| seen_at > current)
        {
            summary.last_delivery_at_unix_secs = Some(seen_at);
            summary.last_delivery_status = Some(delivery.status.clone());
        }
        if matches!(delivery.status.as_str(), "failed" | "dead") {
            summary.failure_count = summary.failure_count.saturating_add(1);
        }
    }
    Ok(summaries)
}

fn stored_endpoint_to_admin_payload(
    endpoint: &StoredWebhookEndpoint,
    summary: Option<&EndpointDeliverySummary>,
) -> Value {
    json!({
        "id": endpoint.id,
        "name": endpoint.name,
        "url": endpoint.url,
        "enabled": endpoint.enabled,
        "subscribed_events": endpoint_events(endpoint),
        "secret_set": endpoint.secret_encrypted.as_deref().is_some_and(|value| !value.trim().is_empty()),
        "timeout_ms": endpoint.timeout_ms,
        "max_retries": endpoint.max_attempts.saturating_sub(1),
        "created_at": unix_secs_to_rfc3339(endpoint.created_at_unix_secs),
        "updated_at": unix_secs_to_rfc3339(endpoint.updated_at_unix_secs),
        "last_delivery_at": summary
            .and_then(|summary| summary.last_delivery_at_unix_secs)
            .and_then(unix_secs_to_rfc3339),
        "last_delivery_status": summary.and_then(|summary| summary.last_delivery_status.clone()),
        "failure_count": summary.map(|summary| summary.failure_count).unwrap_or(0),
        "last_test_delivery_id": endpoint.last_test_delivery_id,
        "last_test_status": endpoint.last_test_status,
        "last_test_error": endpoint.last_test_error,
        "last_test_at": endpoint.last_test_at_unix_secs.and_then(unix_secs_to_rfc3339),
    })
}

fn stored_delivery_to_admin_payload(
    delivery: &StoredWebhookDelivery,
    endpoint: Option<&StoredWebhookEndpoint>,
) -> Value {
    json!({
        "id": delivery.id,
        "endpoint_id": delivery.endpoint_id,
        "endpoint_name": endpoint.map(|endpoint| endpoint.name.clone()).unwrap_or_else(|| delivery.endpoint_id.clone()),
        "event_type": delivery.event_type,
        "status": delivery.status,
        "attempt_count": delivery.attempt_count,
        "max_attempts": delivery.max_attempts,
        "status_code": delivery.last_http_status,
        "duration_ms": null,
        "created_at": unix_secs_to_rfc3339(delivery.created_at_unix_secs),
        "next_retry_at": if matches!(delivery.status.as_str(), "pending" | "retrying") {
            unix_secs_to_rfc3339(delivery.next_attempt_at_unix_secs)
        } else {
            None
        },
        "delivered_at": delivery.delivered_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "last_error": delivery.last_error,
        "response_excerpt": delivery.response_excerpt,
        "request_id": delivery.event_id,
    })
}

fn stored_endpoint_to_runtime(
    state: &AppState,
    endpoint: &StoredWebhookEndpoint,
) -> Result<OutboundWebhookEndpoint, String> {
    let secret = endpoint.secret_encrypted.as_deref().and_then(|secret| {
        decrypt_catalog_secret_with_fallbacks(state.encryption_key(), secret)
            .or_else(|| Some(secret.to_string()))
    });
    Ok(OutboundWebhookEndpoint {
        id: endpoint.id.clone(),
        name: endpoint.name.clone(),
        url: endpoint.url.clone(),
        enabled: endpoint.enabled,
        events: endpoint_events(endpoint),
        secret,
        timeout_ms: endpoint.timeout_ms.clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS),
        max_attempts: endpoint.max_attempts.clamp(1, MAX_MAX_ATTEMPTS),
        initial_retry_delay_seconds: endpoint
            .initial_retry_delay_seconds
            .clamp(1, DEFAULT_MAX_RETRY_DELAY_SECONDS),
        max_retry_delay_seconds: endpoint
            .max_retry_delay_seconds
            .max(endpoint.initial_retry_delay_seconds.max(1))
            .min(DEFAULT_MAX_RETRY_DELAY_SECONDS),
        allow_insecure_http: false,
        allow_private_network: false,
    })
}

fn endpoint_events(endpoint: &StoredWebhookEndpoint) -> Vec<String> {
    match endpoint.event_filter_json.as_ref() {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        Some(Value::String(value)) if !value.trim().is_empty() => {
            vec![value.trim().to_ascii_lowercase()]
        }
        _ => vec!["*".to_string()],
    }
}

fn build_endpoint_upsert_record(
    state: &AppState,
    request: AdminWebhookEndpointUpsertRequest,
    existing: Option<&StoredWebhookEndpoint>,
    created_by: Option<String>,
) -> Result<UpsertWebhookEndpointRecord, String> {
    let name = request
        .name
        .or_else(|| existing.map(|endpoint| endpoint.name.clone()))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "endpoint name is required".to_string())?;
    if name.len() > 128 {
        return Err("endpoint name is too long".to_string());
    }
    let url = request
        .url
        .or_else(|| existing.map(|endpoint| endpoint.url.clone()))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "endpoint url is required".to_string())?;
    let enabled = request
        .enabled
        .unwrap_or_else(|| existing.map(|endpoint| endpoint.enabled).unwrap_or(true));
    let events = match request.subscribed_events {
        Some(events) => normalize_event_filters(Some(&json!(events)))?,
        None => existing
            .map(endpoint_events)
            .unwrap_or_else(|| vec!["*".to_string()]),
    };
    if events.is_empty() {
        return Err("at least one webhook event must be selected".to_string());
    }
    let timeout_ms = request
        .timeout_ms
        .or_else(|| existing.map(|endpoint| endpoint.timeout_ms))
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS);
    let max_retries = request
        .max_retries
        .unwrap_or_else(|| {
            existing
                .map(|endpoint| endpoint.max_attempts.saturating_sub(1))
                .unwrap_or(DEFAULT_MAX_ATTEMPTS.saturating_sub(1))
        })
        .min(10);
    let max_attempts = max_retries.saturating_add(1).clamp(1, MAX_MAX_ATTEMPTS);
    let secret_update = match request.secret.map(|value| value.trim().to_string()) {
        Some(secret) if !secret.is_empty() => WebhookSecretUpdate::Set(
            encrypt_catalog_secret_with_fallbacks(state, &secret).unwrap_or(secret),
        ),
        _ if existing.is_some() => WebhookSecretUpdate::Preserve,
        _ => WebhookSecretUpdate::Clear,
    };
    let has_secret = matches!(secret_update, WebhookSecretUpdate::Set(_))
        || existing
            .and_then(|endpoint| endpoint.secret_encrypted.as_deref())
            .is_some_and(|value| !value.trim().is_empty());
    if enabled && !has_secret {
        return Err("enabled webhook endpoint requires a secret".to_string());
    }
    let runtime_endpoint = OutboundWebhookEndpoint {
        id: existing
            .map(|endpoint| endpoint.id.clone())
            .unwrap_or_default(),
        name: name.clone(),
        url: url.clone(),
        enabled,
        events: events.clone(),
        secret: None,
        timeout_ms,
        max_attempts,
        initial_retry_delay_seconds: existing
            .map(|endpoint| endpoint.initial_retry_delay_seconds)
            .unwrap_or(DEFAULT_INITIAL_RETRY_DELAY_SECONDS),
        max_retry_delay_seconds: existing
            .map(|endpoint| endpoint.max_retry_delay_seconds)
            .unwrap_or(DEFAULT_MAX_RETRY_DELAY_SECONDS),
        allow_insecure_http: false,
        allow_private_network: false,
    };
    validate_webhook_url_shape(&runtime_endpoint)?;

    Ok(UpsertWebhookEndpointRecord {
        id: existing.map(|endpoint| endpoint.id.clone()),
        name,
        url,
        secret_encrypted: secret_update,
        enabled,
        event_filter_json: Some(json!(events)),
        timeout_ms,
        max_attempts,
        initial_retry_delay_seconds: runtime_endpoint.initial_retry_delay_seconds,
        max_retry_delay_seconds: runtime_endpoint.max_retry_delay_seconds,
        created_by,
    })
}

fn delivery_record_from_stored(
    endpoint: &OutboundWebhookEndpoint,
    delivery: &StoredWebhookDelivery,
    now: u64,
) -> OutboundWebhookDeliveryRecord {
    OutboundWebhookDeliveryRecord {
        id: delivery.id.clone(),
        endpoint_id: delivery.endpoint_id.clone(),
        endpoint_name: endpoint.name.clone(),
        event_id: delivery.event_id.clone(),
        event_type: delivery.event_type.clone(),
        event: event_from_delivery(delivery),
        attempt_count: delivery.attempt_count.saturating_add(1),
        last_attempt_at_unix_secs: Some(now),
    }
}

fn event_to_delivery_payload(event: &OutboundWebhookEvent) -> Value {
    json!({
        "id": event.id,
        "type": event.event_type,
        "occurred_at_unix_secs": event.occurred_at_unix_secs,
        "data": event.data,
    })
}

fn event_from_delivery(delivery: &StoredWebhookDelivery) -> OutboundWebhookEvent {
    let object = delivery.payload_json.as_object();
    OutboundWebhookEvent {
        id: object
            .and_then(|object| object.get("id"))
            .and_then(Value::as_str)
            .unwrap_or(&delivery.event_id)
            .to_string(),
        event_type: object
            .and_then(|object| object.get("type"))
            .and_then(Value::as_str)
            .unwrap_or(&delivery.event_type)
            .to_string(),
        occurred_at_unix_secs: object
            .and_then(|object| object.get("occurred_at_unix_secs"))
            .and_then(Value::as_u64)
            .unwrap_or(delivery.created_at_unix_secs),
        data: object
            .and_then(|object| object.get("data"))
            .cloned()
            .unwrap_or_else(|| delivery.payload_json.clone()),
    }
}

fn delivery_body(record: &OutboundWebhookDeliveryRecord) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&json!({
        "delivery_id": record.id,
        "event_id": record.event_id,
        "event_type": record.event_type,
        "occurred_at_unix_secs": record.event.occurred_at_unix_secs,
        "endpoint_id": record.endpoint_id,
        "attempt": record.attempt_count,
        "data": record.event.data,
    }))
    .map_err(|err| err.to_string())
}

fn build_delivery_headers(
    endpoint: &OutboundWebhookEndpoint,
    record: &OutboundWebhookDeliveryRecord,
    body: &[u8],
) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    insert_static_header(&mut headers, http::header::CONTENT_TYPE, "application/json")?;
    insert_header(&mut headers, "aether-webhook-delivery-id", &record.id)?;
    insert_header(&mut headers, "aether-webhook-event", &record.event_type)?;
    insert_header(
        &mut headers,
        "aether-webhook-timestamp",
        &record
            .last_attempt_at_unix_secs
            .unwrap_or_else(now_unix_secs)
            .to_string(),
    )?;
    insert_header(
        &mut headers,
        "aether-webhook-attempt",
        &record.attempt_count.to_string(),
    )?;
    insert_header(
        &mut headers,
        "aether-webhook-endpoint-id",
        &record.endpoint_id,
    )?;
    let Some(secret) = endpoint
        .secret
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Err("webhook endpoint secret is missing".to_string());
    };
    let timestamp = record
        .last_attempt_at_unix_secs
        .unwrap_or_else(now_unix_secs);
    let signature = compute_signature_hex(secret, timestamp, &record.id, body)?;
    insert_header(
        &mut headers,
        "aether-webhook-signature",
        &format!("t={timestamp},v1={signature}"),
    )?;
    Ok(headers)
}

fn insert_static_header(
    headers: &mut HeaderMap,
    name: http::header::HeaderName,
    value: &str,
) -> Result<(), String> {
    let value = HeaderValue::from_str(value).map_err(|err| err.to_string())?;
    headers.insert(name, value);
    Ok(())
}

fn insert_header(headers: &mut HeaderMap, name: &'static str, value: &str) -> Result<(), String> {
    let name = HeaderName::from_static(name);
    let value = HeaderValue::from_str(value).map_err(|err| err.to_string())?;
    headers.insert(name, value);
    Ok(())
}

fn compute_signature_hex(
    secret: &str,
    timestamp: u64,
    delivery_id: &str,
    body: &[u8],
) -> Result<String, String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|err| err.to_string())?;
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(delivery_id.as_bytes());
    mac.update(b".");
    mac.update(body);
    Ok(hex_lower(mac.finalize().into_bytes().as_slice()))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn validate_webhook_url_shape(endpoint: &OutboundWebhookEndpoint) -> Result<Url, String> {
    let url = Url::parse(endpoint.url.trim()).map_err(|_| "webhook url is invalid".to_string())?;
    match url.scheme() {
        "https" => {}
        "http" if endpoint.allow_insecure_http => {}
        "http" => return Err("webhook url must use https".to_string()),
        _ => return Err("webhook url must use http or https".to_string()),
    }
    if url.host_str().is_none() {
        return Err("webhook url host is required".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("webhook url must not include credentials".to_string());
    }
    if url.fragment().is_some() {
        return Err("webhook url must not include a fragment".to_string());
    }
    let host = url.host_str().unwrap_or_default();
    if !endpoint.allow_private_network && host_is_local_name(host) {
        return Err("webhook url host is not allowed".to_string());
    }
    if !endpoint.allow_private_network {
        if let Ok(ip) = host.parse::<IpAddr>() {
            if is_disallowed_outbound_ip(ip) {
                return Err("webhook url ip is not allowed".to_string());
            }
        }
    }
    Ok(url)
}

async fn resolve_and_validate_endpoint(
    url: &Url,
    endpoint: &OutboundWebhookEndpoint,
) -> Result<Vec<SocketAddr>, String> {
    let Some(host) = url.host_str() else {
        return Err("webhook url host is required".to_string());
    };
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "webhook url port is invalid".to_string())?;
    let addrs = lookup_host((host, port))
        .await
        .map_err(|err| format!("webhook url DNS lookup failed: {err}"))?
        .collect::<Vec<_>>();
    if addrs.is_empty() {
        return Err("webhook url DNS lookup returned no addresses".to_string());
    }
    if !endpoint.allow_private_network {
        for addr in &addrs {
            if is_disallowed_outbound_ip(addr.ip()) {
                return Err("webhook url resolved to a disallowed address".to_string());
            }
        }
    }
    Ok(addrs)
}

fn is_disallowed_outbound_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_broadcast()
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_disallowed_outbound_ip(IpAddr::V4(mapped));
            }
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
        }
    }
}

fn host_is_local_name(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost") || host == "metadata.google.internal"
}

fn endpoint_accepts_event(endpoint: &OutboundWebhookEndpoint, event_type: &str) -> bool {
    endpoint.events.is_empty()
        || endpoint
            .events
            .iter()
            .any(|filter| filter == "*" || filter.eq_ignore_ascii_case(event_type.trim()))
}

fn normalize_event_filters(value: Option<&Value>) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(vec!["*".to_string()]);
    };
    let mut filters = Vec::new();
    match value {
        Value::Null => filters.push("*".to_string()),
        Value::String(raw) => {
            for item in raw.split(',') {
                push_event_filter(&mut filters, item)?;
            }
        }
        Value::Array(items) => {
            if items.len() > EVENT_FILTER_LIMIT {
                return Err("too many webhook event filters".to_string());
            }
            for item in items {
                let Some(raw) = item.as_str() else {
                    return Err("webhook event filters must be strings".to_string());
                };
                push_event_filter(&mut filters, raw)?;
            }
        }
        _ => return Err("webhook events must be an array or string".to_string()),
    }
    if filters.is_empty() {
        filters.push("*".to_string());
    }
    filters.sort();
    filters.dedup();
    Ok(filters)
}

fn push_event_filter(filters: &mut Vec<String>, raw: &str) -> Result<(), String> {
    let value = raw.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Ok(());
    }
    if !is_valid_event_filter(&value) {
        return Err("invalid webhook event filter".to_string());
    }
    filters.push(value);
    Ok(())
}

fn is_valid_event_filter(value: &str) -> bool {
    if value == "*" {
        return true;
    }
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
        && value
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn is_valid_id(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn parse_json_request<T: for<'de> Deserialize<'de>>(
    request_body: Option<&Bytes>,
) -> Result<T, (http::StatusCode, Value)> {
    let Some(body) = request_body.filter(|body| !body.is_empty()) else {
        return Err(bad_request("request body is required"));
    };
    serde_json::from_slice::<T>(body).map_err(|_| bad_request("request body must be valid JSON"))
}

fn admin_actor_id(request_context: &AdminRequestContext<'_>) -> Option<String> {
    request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.clone())
        .filter(|value| !value.trim().is_empty())
}

fn retry_delay_seconds_for_endpoint(endpoint: &OutboundWebhookEndpoint, attempt_count: u32) -> u64 {
    let base = u64::from(endpoint.initial_retry_delay_seconds.max(1));
    let max = u64::from(
        endpoint
            .max_retry_delay_seconds
            .max(endpoint.initial_retry_delay_seconds.max(1)),
    );
    let exp = attempt_count.saturating_sub(1).min(7);
    base.saturating_mul(2_u64.saturating_pow(exp)).min(max)
}

fn http_status_is_retryable(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

fn truncate_for_log(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn now_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn query_param(query_string: Option<&str>, name: &str) -> Option<String> {
    let query = query_string.filter(|value| !value.is_empty())?;
    url::form_urlencoded::parse(query.as_bytes())
        .find_map(|(key, value)| (key == name && !value.is_empty()).then(|| value.into_owned()))
}

fn query_usize(query_string: Option<&str>, name: &str) -> Option<usize> {
    query_param(query_string, name).and_then(|value| value.parse::<usize>().ok())
}

fn bad_request(detail: impl Into<String>) -> (http::StatusCode, Value) {
    (
        http::StatusCode::BAD_REQUEST,
        json!({ "detail": detail.into() }),
    )
}

fn map_data_layer_error(err: aether_data::DataLayerError) -> GatewayError {
    GatewayError::Internal(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_uses_timestamp_delivery_id_and_body() {
        let sig = compute_signature_hex(
            "secret",
            1_700_000_000,
            "delivery-1",
            br#"{"hello":"world"}"#,
        )
        .expect("signature should build");

        assert_eq!(
            sig,
            "87f5ba8278f40b5f91ad5b54623861244bf80cc88980af22ea5cdce14c368629"
        );
    }

    #[test]
    fn url_validation_rejects_localhost_by_default() {
        let endpoint = OutboundWebhookEndpoint {
            id: "test".to_string(),
            name: "Test".to_string(),
            url: "http://localhost:3000/hook".to_string(),
            enabled: true,
            events: vec!["*".to_string()],
            secret: Some("secret".to_string()),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            initial_retry_delay_seconds: DEFAULT_INITIAL_RETRY_DELAY_SECONDS,
            max_retry_delay_seconds: DEFAULT_MAX_RETRY_DELAY_SECONDS,
            allow_insecure_http: true,
            allow_private_network: false,
        };

        assert!(validate_webhook_url_shape(&endpoint).is_err());
    }

    #[test]
    fn url_validation_requires_https_unless_explicitly_allowed() {
        let endpoint = OutboundWebhookEndpoint {
            id: "test".to_string(),
            name: "Test".to_string(),
            url: "http://example.com/hook".to_string(),
            enabled: true,
            events: vec!["*".to_string()],
            secret: Some("secret".to_string()),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            initial_retry_delay_seconds: DEFAULT_INITIAL_RETRY_DELAY_SECONDS,
            max_retry_delay_seconds: DEFAULT_MAX_RETRY_DELAY_SECONDS,
            allow_insecure_http: false,
            allow_private_network: false,
        };

        assert!(validate_webhook_url_shape(&endpoint).is_err());
    }

    #[test]
    fn event_filters_default_to_all_and_dedupe() {
        assert_eq!(
            normalize_event_filters(None).expect("filters should normalize"),
            vec!["*".to_string()]
        );
        assert_eq!(
            normalize_event_filters(Some(&json!(["provider.error", "provider.error"])))
                .expect("filters should normalize"),
            vec!["provider.error".to_string()]
        );
    }

    #[test]
    fn retry_delay_is_bounded_exponential() {
        let endpoint = OutboundWebhookEndpoint {
            id: "test".to_string(),
            name: "Test".to_string(),
            url: "https://example.com/hook".to_string(),
            enabled: true,
            events: vec!["*".to_string()],
            secret: Some("secret".to_string()),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            initial_retry_delay_seconds: 30,
            max_retry_delay_seconds: 3_600,
            allow_insecure_http: false,
            allow_private_network: false,
        };
        assert_eq!(retry_delay_seconds_for_endpoint(&endpoint, 1), 30);
        assert_eq!(retry_delay_seconds_for_endpoint(&endpoint, 2), 60);
        assert_eq!(retry_delay_seconds_for_endpoint(&endpoint, 10), 3_600);
    }
}
