//! Responses WebSocket end-to-end coverage.
//!
//! Every test starts a protocol-aware mock upstream, seeds a throwaway SQLite
//! store, mounts the real gateway router, and drives the public
//! `/v1/responses` WebSocket the way a client would.
//!
//! The assertions deliberately reach back into the database. A turn settles its
//! billing row from a task that outlives the relay loop, so a client that saw
//! `response.completed` is not evidence that the turn was ever accounted for —
//! only the row is.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::CreateStandaloneApiKeyRecord;
use aether_data::repository::wallet::WalletLookupKey;
use aether_data::{
    DataBackends, DataLayerConfig, DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig,
};
use aether_data_contracts::repository::global_models::{
    CreateAdminGlobalModelRecord, UpsertAdminProviderModelRecord,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UsageAuditListQuery};
use aether_gateway::{build_router_with_state, AppState, GatewayDataConfig, UsageRuntimeConfig};
use aether_testkit::SpawnedServer;
use axum::extract::ws::{Message as AxumWsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, Uri};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::Digest;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type BoxError = Box<dyn std::error::Error>;
type ClientSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const CLIENT_API_KEY: &str = "sk-aether-responses-ws-e2e";
const PROVIDER_API_KEY: &str = "sk-upstream-responses-ws-e2e";
const PROVIDER_ID: &str = "provider-responses-ws-e2e";
const ENDPOINT_ID: &str = "endpoint-responses-ws-e2e";
const PROVIDER_KEY_ID: &str = "provider-key-responses-ws-e2e";
/// 透明重试的替代 key。只有配额重试用例会 seed 它。
const ALTERNATE_PROVIDER_KEY_ID: &str = "provider-key-responses-ws-e2e-alt";
const ALTERNATE_PROVIDER_API_KEY: &str = "sk-upstream-responses-ws-e2e-alt";
const GLOBAL_MODEL_ID: &str = "global-model-responses-ws-e2e";
const PROVIDER_MODEL_ID: &str = "provider-model-responses-ws-e2e";
const API_KEY_ID: &str = "api-key-responses-ws-e2e";
const PUBLIC_MODEL: &str = "gpt-responses-ws-e2e";
const UPSTREAM_MODEL: &str = "gpt-responses-ws-upstream";

/// 2100-01-01，保证 oauth 凭证在测试期间不会被判为过期。
const FAR_FUTURE_UNIX_SECS: u64 = 4_102_444_800;

const INPUT_TOKENS: u64 = 4;
const OUTPUT_TOKENS: u64 = 2;

/// Generous enough to absorb a loaded CI runner, short enough that a genuinely
/// lost row fails the test instead of hanging the job.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(30);
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The headline guarantee: a continuation stays on one physical upstream socket,
/// and both turns are billed independently.
#[tokio::test]
async fn continuation_reuses_one_upstream_connection_and_bills_both_turns() -> Result<(), BoxError>
{
    let harness = Harness::start(UpstreamBehavior::CompleteEveryTurn).await?;
    let mut client = harness.connect().await?;

    client
        .send(response_create(json!({"input": "first turn"})))
        .await?;
    let first = receive_event(&mut client, "response.completed").await?;
    assert_eq!(
        first.pointer("/response/id").and_then(Value::as_str),
        Some("resp-e2e-1")
    );

    client
        .send(response_create(json!({
            "previous_response_id": "resp-e2e-1",
            "input": "second turn"
        })))
        .await?;
    let second = receive_event(&mut client, "response.completed").await?;
    assert_eq!(
        second.pointer("/response/id").and_then(Value::as_str),
        Some("resp-e2e-2")
    );

    // The mock records each `response.create` before answering it, so both
    // turns completing means both are already on record.
    let upstream_events = harness.upstream.observed_events().await;
    assert_eq!(
        upstream_events.len(),
        2,
        "one upstream turn per client turn"
    );
    assert_eq!(
        harness.upstream.connections(),
        1,
        "the continuation must reuse the bound upstream socket"
    );
    for event in &upstream_events {
        assert_eq!(
            event.get("model").and_then(Value::as_str),
            Some(UPSTREAM_MODEL),
            "every turn is rewritten to the mapped provider model"
        );
    }
    assert_eq!(
        upstream_events[1]
            .get("previous_response_id")
            .and_then(Value::as_str),
        Some("resp-e2e-1"),
        "the continuation id survives provider body normalization"
    );
    assert_eq!(
        harness.upstream.authorization_headers().await,
        vec![Some(format!("Bearer {PROVIDER_API_KEY}"))],
        "the upstream is opened with the configured provider key"
    );

    let audits = harness
        .usage_audits_where(2, "billed turns", is_billed)
        .await?;
    assert_eq!(audits.len(), 2, "each response.create bills separately");
    for audit in &audits {
        assert!(
            audit.is_websocket(),
            "turns are recorded as WebSocket usage, metadata: {:?}",
            audit.request_metadata
        );
        assert_eq!(audit.model, PUBLIC_MODEL);
        assert_eq!(audit.input_tokens, INPUT_TOKENS);
        assert_eq!(audit.output_tokens, OUTPUT_TOKENS);
        assert_eq!(audit.total_tokens, INPUT_TOKENS + OUTPUT_TOKENS);
        assert_eq!(audit.status_code, Some(200));
    }
    assert_ne!(
        audits[0].request_id, audits[1].request_id,
        "each turn gets its own logical request identity"
    );

    Ok(())
}

#[tokio::test]
async fn persisted_previous_response_can_continue_on_a_new_client_connection(
) -> Result<(), BoxError> {
    let harness = Harness::start(UpstreamBehavior::CompleteEveryTurn).await?;

    let mut first_client = harness.connect().await?;
    first_client
        .send(response_create(json!({
            "store": true,
            "input": "first connection"
        })))
        .await?;
    let first = receive_event(&mut first_client, "response.completed").await?;
    assert_eq!(
        first.pointer("/response/id").and_then(Value::as_str),
        Some("resp-e2e-1")
    );
    first_client.close(None).await?;

    let mut second_client = harness.connect().await?;
    second_client
        .send(response_create(json!({
            "store": true,
            "previous_response_id": "resp-e2e-1",
            "input": "second connection"
        })))
        .await?;
    let second = receive_event(&mut second_client, "response.completed").await?;
    assert_eq!(
        second.pointer("/response/id").and_then(Value::as_str),
        Some("resp-e2e-2")
    );

    let upstream_events = harness.upstream.observed_events().await;
    assert_eq!(upstream_events.len(), 2);
    assert_eq!(harness.upstream.connections(), 2);
    assert_eq!(upstream_events[0]["store"], json!(true));
    assert_eq!(upstream_events[1]["store"], json!(true));
    assert_eq!(
        upstream_events[1]["previous_response_id"],
        json!("resp-e2e-1")
    );

    let audits = harness
        .usage_audits_where(2, "billed cross-connection turns", is_billed)
        .await?;
    assert_eq!(audits.iter().filter(|audit| is_billed(audit)).count(), 2);

    second_client.close(None).await?;
    Ok(())
}

#[tokio::test]
async fn unknown_future_request_and_response_fields_round_trip_opaquely() -> Result<(), BoxError> {
    let harness = Harness::start(UpstreamBehavior::FutureEventThenComplete).await?;
    let mut client = harness.connect().await?;

    client
        .send(response_create(json!({
            "input": "future-compatible",
            "future_request_capability": {
                "mode": "opaque",
                "revision": 7
            }
        })))
        .await?;

    let future = receive_event(&mut client, "response.future.capability").await?;
    assert_eq!(future["future_capability"]["enabled"], json!(true));
    assert_eq!(future["future_capability"]["revision"], json!(7));
    receive_event(&mut client, "response.completed").await?;

    let upstream_events = harness.upstream.observed_events().await;
    assert_eq!(upstream_events.len(), 1);
    assert_eq!(
        upstream_events[0]["future_request_capability"],
        json!({"mode": "opaque", "revision": 7})
    );

    client.close(None).await?;
    Ok(())
}

#[tokio::test]
async fn invalid_or_control_text_on_a_bound_socket_does_not_poison_the_next_valid_turn(
) -> Result<(), BoxError> {
    let harness = Harness::start(UpstreamBehavior::CompleteEveryTurn).await?;
    let mut client = harness.connect().await?;

    // Establish the provider connection first. Initial-frame validation has a
    // separate handshake policy; this regression targets client events read by
    // the live relay loop after the socket is fully bound.
    client
        .send(response_create(json!({"input": "bind the socket"})))
        .await?;
    receive_event(&mut client, "response.completed").await?;

    client
        .send(Message::Text("this is not json".into()))
        .await?;
    let invalid_json = receive_error_or_close(&mut client)
        .await?
        .ok_or("gateway closed after invalid JSON instead of returning an error")?;
    assert_eq!(invalid_json["status"], json!(400));
    assert_eq!(
        invalid_json.pointer("/error/code").and_then(Value::as_str),
        Some("invalid_response_create")
    );

    client
        .send(Message::Text(
            json!({"type": "response.cancel"}).to_string().into(),
        ))
        .await?;
    let unsupported_control = receive_error_or_close(&mut client)
        .await?
        .ok_or("gateway closed after a control event instead of returning an error")?;
    assert_eq!(unsupported_control["status"], json!(400));
    assert_eq!(
        unsupported_control
            .pointer("/error/code")
            .and_then(Value::as_str),
        Some("expected_response_create")
    );
    client
        .send(response_create(json!({"input": "valid after errors"})))
        .await?;
    let completed = receive_event(&mut client, "response.completed").await?;
    assert_eq!(
        completed
            .pointer("/response/status")
            .and_then(Value::as_str),
        Some("completed")
    );

    let upstream_events = harness.upstream.observed_events().await;
    assert_eq!(upstream_events.len(), 2);
    assert_eq!(upstream_events[0]["input"], json!("bind the socket"));
    assert_eq!(upstream_events[1]["input"], json!("valid after errors"));
    assert_eq!(
        harness.upstream.connections(),
        1,
        "client protocol errors must not tear down the bound provider socket"
    );
    let audits = harness
        .usage_audits_where(2, "valid turns surrounding client errors", is_billed)
        .await?;
    assert_eq!(audits.iter().filter(|audit| is_billed(audit)).count(), 2);

    client.close(None).await?;
    Ok(())
}

#[tokio::test]
async fn downstream_credentials_and_handshake_headers_do_not_reach_upstream() -> Result<(), BoxError>
{
    const QUERY_CREDENTIAL: &str = "downstream-query-secret";
    const COOKIE_CREDENTIAL: &str = "downstream-cookie-secret";
    const PROXY_CREDENTIAL: &str = "downstream-proxy-secret";
    const WEBSOCKET_FUTURE_VALUE: &str = "downstream-future-websocket-secret";
    const CONNECTION_SECRET: &str = "downstream-connection-secret";

    let harness = Harness::start(UpstreamBehavior::CompleteEveryTurn).await?;
    let mut request = format!(
        "{}?key={QUERY_CREDENTIAL}&client_version=0.145.2",
        harness.websocket_url
    )
    .into_client_request()?;
    request.headers_mut().insert(
        "authorization",
        http::HeaderValue::from_str(&format!("Bearer {CLIENT_API_KEY}"))?,
    );
    request.headers_mut().insert(
        "cookie",
        http::HeaderValue::from_str(&format!("session={COOKIE_CREDENTIAL}"))?,
    );
    request.headers_mut().insert(
        "proxy-authorization",
        http::HeaderValue::from_str(&format!("Bearer {PROXY_CREDENTIAL}"))?,
    );
    request.headers_mut().insert(
        "sec-websocket-future-capability",
        http::HeaderValue::from_static(WEBSOCKET_FUTURE_VALUE),
    );
    request.headers_mut().insert(
        "connection",
        http::HeaderValue::from_static("Upgrade, x-downstream-connection-secret"),
    );
    request.headers_mut().insert(
        "x-downstream-connection-secret",
        http::HeaderValue::from_static(CONNECTION_SECRET),
    );

    let mut client = harness.connect_request(request).await?;
    client
        .send(response_create(json!({"input": "sanitize handshake"})))
        .await?;
    receive_event(&mut client, "response.completed").await?;

    let handshakes = harness.upstream.observed_handshakes().await;
    assert_eq!(handshakes.len(), 1);
    let handshake = &handshakes[0];
    assert_eq!(
        handshake.request_target, "/v1/responses?client_version=0.145.2",
        "benign query state must survive while the downstream credential is removed"
    );

    let expected_provider_authorization = format!("Bearer {PROVIDER_API_KEY}");
    assert_eq!(
        handshake
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok()),
        Some(expected_provider_authorization.as_str()),
        "provider authentication must be generated independently"
    );
    for removed in [
        "cookie",
        "proxy-authorization",
        "sec-websocket-future-capability",
        "x-downstream-connection-secret",
    ] {
        assert!(
            handshake.headers.get(removed).is_none(),
            "downstream header {removed} reached the upstream handshake"
        );
    }
    for (name, value) in &handshake.headers {
        let value = value.to_str().unwrap_or_default();
        for secret in [
            CLIENT_API_KEY,
            QUERY_CREDENTIAL,
            COOKIE_CREDENTIAL,
            PROXY_CREDENTIAL,
            WEBSOCKET_FUTURE_VALUE,
            CONNECTION_SECRET,
        ] {
            assert!(
                !value.contains(secret),
                "downstream secret leaked through upstream header {name}: {value}"
            );
        }
    }

    client.close(None).await?;
    Ok(())
}

#[tokio::test]
async fn disabling_the_downstream_key_is_enforced_on_the_next_turn_of_the_same_socket(
) -> Result<(), BoxError> {
    let harness = Harness::start(UpstreamBehavior::CompleteEveryTurn).await?;
    let mut client = harness.connect().await?;

    client
        .send(response_create(json!({"input": "before key disable"})))
        .await?;
    receive_event(&mut client, "response.completed").await?;

    // Mutate through a repository handle that is independent of the gateway
    // state. The next turn must perform a strong refresh instead of trusting
    // the Upgrade-time or ordinary cached auth snapshot.
    harness.set_client_api_key_active(false).await?;

    client
        .send(response_create(json!({"input": "after key disable"})))
        .await?;
    let rejection = receive_error_or_close(&mut client)
        .await?
        .ok_or("gateway closed without reporting the live API-key rejection")?;
    assert_eq!(rejection["status"], json!(401));
    assert_eq!(
        rejection.pointer("/error/code").and_then(Value::as_str),
        Some("gateway_request_not_allowed")
    );

    let upstream_events = harness.upstream.observed_events().await;
    assert_eq!(
        upstream_events.len(),
        1,
        "the disabled key's second turn must not reach the provider"
    );
    assert_eq!(upstream_events[0]["input"], json!("before key disable"));
    assert_eq!(harness.upstream.connections(), 1);

    let audits = harness
        .usage_audits_where(1, "the turn completed before key disable", is_billed)
        .await?;
    assert_eq!(
        audits.len(),
        1,
        "a control-plane rejection must not create a provider attempt row"
    );

    client.close(None).await?;
    Ok(())
}

/// A client that walks away before the provider produced anything must settle
/// as a void row: nothing was produced, so nothing is billed.
///
/// This is the path with no protocol event to announce it: the relay loop owns
/// the turn, and losing the client is an exit the upstream never reports.
///
/// The mirror case — the provider *did* reach a terminal event and only the last
/// hop to the client failed — is billed instead. That one cannot be pinned here:
/// it depends on the relay loop's `select!` observing the upstream terminal frame
/// before it observes the closed client socket, which is a race by construction.
/// It is covered deterministically by the relay-level unit tests
/// `a_provider_terminal_that_reaches_a_closed_client_socket_is_still_billed` and
/// `a_closed_client_socket_before_any_terminal_still_voids_the_bill`.
#[tokio::test]
async fn client_disconnect_before_any_provider_output_settles_a_void_row() -> Result<(), BoxError> {
    let harness = Harness::start(UpstreamBehavior::StallAfterCreated).await?;
    let mut client = harness.connect().await?;

    client
        .send(response_create(json!({"input": "abandoned turn"})))
        .await?;
    // Leave only once the turn is genuinely in flight upstream, so this covers
    // an interrupted turn rather than racing turn start.
    receive_event(&mut client, "response.created").await?;
    drop(client);

    let audits = harness
        .usage_audits_where(1, "settled turns", |audit| !is_pending(audit))
        .await?;
    assert_eq!(audits.len(), 1, "the abandoned turn is still accounted for");
    let audit = &audits[0];
    assert_eq!(audit.model, PUBLIC_MODEL);
    assert!(
        !is_pending(audit),
        "an abandoned turn must not be left pending: {audit:?}"
    );
    // The provider never emitted a terminal event, so this row stays void.
    // Only a reached provider terminal survives a client delivery failure.
    assert!(
        !is_billed(audit),
        "a turn with no provider output must not be billed: {audit:?}"
    );
    assert_eq!(
        audit.status, "cancelled",
        "a client that left before any provider output settles as cancelled: {audit:?}"
    );
    assert_eq!(audit.status_code, Some(499));

    Ok(())
}

/// An upstream that dies mid-turn must surface an error and still settle.
#[tokio::test]
async fn upstream_drop_mid_turn_reports_an_error_and_settles_the_usage_row() -> Result<(), BoxError>
{
    let harness = Harness::start(UpstreamBehavior::CloseAfterCreated).await?;
    let mut client = harness.connect().await?;

    client
        .send(response_create(json!({"input": "doomed turn"})))
        .await?;
    let error = receive_error_or_close(&mut client)
        .await?
        .ok_or("gateway closed without telling the client why")?;
    assert_eq!(error.get("type").and_then(Value::as_str), Some("error"));

    let audits = harness
        .usage_audits_where(1, "settled turns", |audit| !is_pending(audit))
        .await?;
    assert_eq!(audits.len(), 1, "the failed turn is still accounted for");
    let audit = &audits[0];
    assert!(
        !is_pending(audit),
        "a failed turn must not be left pending: {audit:?}"
    );

    Ok(())
}

/// 供应商配额耗尽后的透明重试：客户端不该看到 429，两个 attempt 都要结算。
///
/// 第一个 attempt 拿到 Codex 的 `usage_limit_reached`，网关换到第二把 key 重开一条
/// 上游连接重放同一个 `response.create`。C6 之前，重试的规划发生在旧 attempt 结算
/// 之前：规划读到的是旧 attempt 还没投射的 health / adaptive / pool 状态，而且旧
/// attempt 的 pool key lease 还被它自己占着。
///
/// 顺序本身在这里无法确定性断言（结算与规划都在同一个任务里、DB 里看不到先后），
/// 由 lifecycle 的单测确定性覆盖；这个用例保证整条路径真的能跑通，并且两个
/// attempt 都留下了终态记账行。
#[tokio::test]
async fn provider_quota_exhaustion_transparently_retries_onto_another_key() -> Result<(), BoxError>
{
    let harness = Harness::start_with_fixture(
        UpstreamBehavior::QuotaExhaustedThenComplete,
        ProviderFixture::CodexKeyPair,
    )
    .await?;
    let mut client = harness.connect().await?;

    client
        .send(response_create(json!({"input": "retry after quota"})))
        .await?;

    // 客户端只应该看到重试之后那次成功的响应，看不到 429。
    let completed = receive_event(&mut client, "response.completed").await?;
    assert_eq!(
        completed
            .pointer("/response/status")
            .and_then(Value::as_str),
        Some("completed")
    );

    // 上游被连了两次：配额耗尽的那条 + 重试用的那条。
    assert_eq!(
        harness.upstream.connections(),
        2,
        "the transparent retry must open a second upstream connection"
    );
    let observed = harness.upstream.observed_events().await;
    assert_eq!(
        observed.len(),
        2,
        "the same response.create must be replayed once"
    );

    // 两把不同的 key 被用过：重试不能落回那把已经耗尽的 key。
    let authorizations = harness.upstream.authorization_headers().await;
    assert_eq!(authorizations.len(), 2);
    assert_ne!(
        authorizations[0], authorizations[1],
        "the retry must not reuse the exhausted key: {authorizations:?}"
    );

    // 两个 attempt 各自留下一条终态行：配额失败的那条 + 成功计费的那条。
    let audits = harness
        .usage_audits_where(2, "settled attempts", |audit| !is_pending(audit))
        .await?;
    let settled = audits
        .iter()
        .filter(|audit| !is_pending(audit))
        .collect::<Vec<_>>();
    assert_eq!(
        settled.len(),
        2,
        "both attempts must reach a terminal accounting row: {:?}",
        audits
            .iter()
            .map(|audit| (audit.status.clone(), audit.status_code, audit.total_tokens))
            .collect::<Vec<_>>()
    );
    assert!(
        settled.iter().any(|audit| audit.status_code == Some(429)),
        "the exhausted attempt keeps its own 429 row: {:?}",
        settled
            .iter()
            .map(|audit| (audit.status.clone(), audit.status_code))
            .collect::<Vec<_>>()
    );

    let billed = harness
        .usage_audits_where(1, "the billed retry attempt", is_billed)
        .await?;
    let retry = billed
        .iter()
        .find(|audit| is_billed(audit))
        .ok_or("the successful retry attempt must be billed")?;
    assert_eq!(
        retry.total_tokens,
        INPUT_TOKENS + OUTPUT_TOKENS,
        "the retry attempt is billed for what it actually consumed"
    );

    client.close(None).await?;
    Ok(())
}

/// 脱敏的另一半：请求侧把真实 PII 换成占位符发给上游，响应侧必须在推给客户端之前
/// 换回真实值。
///
/// 上游把收到的 `input` 原样回显，所以它回来的就是占位符——这一条同时钉住了两个
/// 方向：上游不能看到原文，客户端不能看到占位符。
#[tokio::test]
async fn redacted_pii_is_restored_before_the_client_sees_a_provider_frame() -> Result<(), BoxError>
{
    const CLIENT_EMAIL: &str = "responses.ws.pii@example.com";

    let harness = Harness::start_with_pii_redaction(UpstreamBehavior::EchoInputBack).await?;
    let mut client = harness.connect().await?;

    client
        .send(response_create(
            json!({"input": format!("my mail is {CLIENT_EMAIL}")}),
        ))
        .await?;

    let delta = receive_event(&mut client, "response.output_text.delta").await?;
    let delta_text = delta
        .get("delta")
        .and_then(Value::as_str)
        .ok_or("the provider delta must carry text")?;
    assert!(
        delta_text.contains(CLIENT_EMAIL),
        "the client must receive the restored value: {delta_text}"
    );
    assert!(
        !delta_text.contains("<AETHER:"),
        "no redaction placeholder may reach the client: {delta_text}"
    );

    // 请求侧仍然成立：上游只看到占位符，看不到原文。
    let upstream_events = harness.upstream.observed_events().await;
    assert_eq!(upstream_events.len(), 1);
    let upstream_input = upstream_events[0]
        .get("input")
        .and_then(Value::as_str)
        .ok_or("the upstream request must carry input")?;
    assert!(
        !upstream_input.contains(CLIENT_EMAIL),
        "the upstream must never see the raw PII: {upstream_input}"
    );
    assert!(
        upstream_input.contains("<AETHER:EMAIL:"),
        "the upstream must see the placeholder: {upstream_input}"
    );

    // 还原只发生在最后一跳：终态照常到达，计费不受影响。
    let completed = receive_event(&mut client, "response.completed").await?;
    assert_eq!(
        completed
            .pointer("/response/status")
            .and_then(Value::as_str),
        Some("completed")
    );
    let audits = harness
        .usage_audits_where(1, "the billed redacted turn", is_billed)
        .await?;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0].total_tokens, INPUT_TOKENS + OUTPUT_TOKENS);

    client.close(None).await?;
    Ok(())
}

fn is_pending(audit: &StoredRequestUsageAudit) -> bool {
    audit.status.eq_ignore_ascii_case("pending")
}

/// A turn that finished accounting: settled, and carrying what it consumed.
fn is_billed(audit: &StoredRequestUsageAudit) -> bool {
    !is_pending(audit) && audit.total_tokens > 0
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A live gateway wired to a mock Responses WebSocket upstream over a throwaway
/// SQLite store.
struct Harness {
    database: TemporarySqlite,
    upstream: Arc<MockUpstreamState>,
    websocket_url: String,
    _upstream_server: SpawnedServer,
    _gateway_server: SpawnedServer,
}

/// 供应商夹具形态。
///
/// 透明配额重试只有 Codex adapter 会开启（`retry_current_turn: true` 只从
/// codex.rs 出），而且重试要有第二把 key 可挑，否则规划直接判无可用供应商。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderFixture {
    /// 单个 openai 类型供应商、单把 key。
    SingleOpenAiKey,
    /// codex 类型供应商 + 两把 key：第一把配额耗尽后重试落到第二把。
    CodexKeyPair,
}

impl ProviderFixture {
    const fn provider_type(self) -> &'static str {
        match self {
            Self::SingleOpenAiKey => "openai",
            Self::CodexKeyPair => "codex",
        }
    }

    const fn has_alternate_key(self) -> bool {
        matches!(self, Self::CodexKeyPair)
    }
}

/// 这条用例要不要打开 chat PII 脱敏模块。
///
/// 默认关闭：其余用例都靠原文 body 断言上游看到了什么，打开脱敏会把断言目标换成
/// 占位符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PiiRedaction {
    Disabled,
    Enabled,
}

impl PiiRedaction {
    const fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl Harness {
    async fn start(behavior: UpstreamBehavior) -> Result<Self, BoxError> {
        Self::start_with(
            behavior,
            ProviderFixture::SingleOpenAiKey,
            PiiRedaction::Disabled,
        )
        .await
    }

    async fn start_with_fixture(
        behavior: UpstreamBehavior,
        fixture: ProviderFixture,
    ) -> Result<Self, BoxError> {
        Self::start_with(behavior, fixture, PiiRedaction::Disabled).await
    }

    async fn start_with_pii_redaction(behavior: UpstreamBehavior) -> Result<Self, BoxError> {
        Self::start_with(
            behavior,
            ProviderFixture::SingleOpenAiKey,
            PiiRedaction::Enabled,
        )
        .await
    }

    async fn start_with(
        behavior: UpstreamBehavior,
        fixture: ProviderFixture,
        redaction: PiiRedaction,
    ) -> Result<Self, BoxError> {
        let upstream = Arc::new(MockUpstreamState::new(behavior));
        let upstream_server =
            SpawnedServer::start(mock_upstream_router(Arc::clone(&upstream))).await?;

        let database = TemporarySqlite::new();
        prepare_and_seed_database(
            &database.config,
            upstream_server.base_url(),
            fixture,
            redaction,
        )
        .await?;

        let data_config = GatewayDataConfig::from_database_config(database.config.clone())
            .with_encryption_key(DEVELOPMENT_ENCRYPTION_KEY);
        let state = AppState::new()?
            .with_data_config_and_background_isolation(data_config, false)?
            // The usage runtime defaults to disabled, which silently turns every
            // terminal usage write into a no-op. Without this the suite could
            // not observe billing at all. Queueing stays off so the terminal
            // write lands through the in-process path instead of Redis.
            .with_usage_runtime_config(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            })?;
        let gateway_server = SpawnedServer::start(build_router_with_state(state)).await?;
        let websocket_url = format!(
            "{}/v1/responses",
            gateway_server.base_url().replacen("http://", "ws://", 1)
        );

        Ok(Self {
            database,
            upstream,
            websocket_url,
            _upstream_server: upstream_server,
            _gateway_server: gateway_server,
        })
    }

    async fn connect(&self) -> Result<ClientSocket, BoxError> {
        let mut request = self.websocket_url.clone().into_client_request()?;
        request.headers_mut().insert(
            "authorization",
            http::HeaderValue::from_str(&format!("Bearer {CLIENT_API_KEY}"))?,
        );
        self.connect_request(request).await
    }

    async fn connect_request(&self, request: http::Request<()>) -> Result<ClientSocket, BoxError> {
        let (socket, response) =
            tokio::time::timeout(RECEIVE_TIMEOUT, tokio_tungstenite::connect_async(request))
                .await
                .map_err(|_| "timed out connecting to the gateway WebSocket")??;
        if response.status() != http::StatusCode::SWITCHING_PROTOCOLS {
            return Err(
                format!("unexpected gateway handshake status: {}", response.status()).into(),
            );
        }
        Ok(socket)
    }

    /// Waits until `expected` usage rows satisfy `settled`.
    ///
    /// A row is created `Pending` at turn start and reaches its final shape
    /// through several independent writes, so "no longer pending" does not imply
    /// "finished": a row can briefly read as completed with zero tokens and no
    /// WebSocket metadata before the terminal write lands. Each caller waits for
    /// the specific end state it is about to assert.
    async fn usage_audits_where(
        &self,
        expected: usize,
        what: &str,
        settled: impl Fn(&StoredRequestUsageAudit) -> bool,
    ) -> Result<Vec<StoredRequestUsageAudit>, BoxError> {
        let deadline = tokio::time::Instant::now() + SETTLE_TIMEOUT;
        loop {
            let audits = self.usage_audits().await?;
            if audits.iter().filter(|audit| settled(audit)).count() >= expected {
                return Ok(audits);
            }
            if tokio::time::Instant::now() >= deadline {
                let observed = audits
                    .iter()
                    .map(|audit| {
                        format!(
                            "{} status={} code={:?} tokens={} websocket={}",
                            audit.request_id,
                            audit.status,
                            audit.status_code,
                            audit.total_tokens,
                            audit.is_websocket()
                        )
                    })
                    .collect::<Vec<_>>();
                return Err(format!(
                    "timed out waiting for {expected} {what}; observed {}: {observed:?}",
                    audits.len()
                )
                .into());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Reads the persisted audit rows, oldest first.
    ///
    /// Opens its own handle per call rather than holding one for the lifetime of
    /// the harness: the gateway keeps its own pool on the same SQLite file for
    /// the whole test, and an idle second pool only adds contention.
    async fn usage_audits(&self) -> Result<Vec<StoredRequestUsageAudit>, BoxError> {
        let backends = DataBackends::from_config(DataLayerConfig::from_database(
            self.database.config.clone(),
        ))?;
        let audits = backends
            .read()
            .usage()
            .ok_or("usage reader unavailable")?
            .list_usage_audits(&UsageAuditListQuery {
                limit: Some(50),
                newest_first: false,
                ..UsageAuditListQuery::default()
            })
            .await?;
        drop(backends);
        Ok(audits)
    }

    async fn set_client_api_key_active(&self, is_active: bool) -> Result<(), BoxError> {
        let backends = DataBackends::from_config(DataLayerConfig::from_database(
            self.database.config.clone(),
        ))?;
        backends
            .write()
            .auth_api_keys()
            .ok_or("auth API key writer unavailable")?
            .set_standalone_api_key_active(API_KEY_ID, is_active)
            .await?
            .ok_or("the E2E client API key disappeared before its live-control update")?;
        drop(backends);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Client protocol helpers
// ---------------------------------------------------------------------------

/// Builds a `response.create` frame for the seeded public model.
fn response_create(fields: Value) -> Message {
    let mut event = json!({"type": "response.create", "model": PUBLIC_MODEL});
    let object = event
        .as_object_mut()
        .expect("the literal above is an object");
    for (key, value) in fields
        .as_object()
        .expect("response.create fields must be an object")
    {
        object.insert(key.clone(), value.clone());
    }
    Message::Text(event.to_string().into())
}

/// Reads frames until `expected_type` arrives, failing fast on a gateway error.
async fn receive_event<S>(
    socket: &mut WebSocketStream<S>,
    expected_type: &str,
) -> Result<Value, BoxError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    tokio::time::timeout(RECEIVE_TIMEOUT, async {
        loop {
            let message = socket
                .next()
                .await
                .ok_or("gateway WebSocket closed before the expected event")??;
            match message {
                Message::Text(text) => {
                    let event: Value = serde_json::from_str(text.as_ref())?;
                    match event.get("type").and_then(Value::as_str) {
                        Some("error") => {
                            return Err(format!("gateway returned an error event: {event}").into())
                        }
                        Some(event_type) if event_type == expected_type => return Ok(event),
                        _ => {}
                    }
                }
                Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
                Message::Close(frame) => {
                    return Err(format!("gateway closed before {expected_type}: {frame:?}").into())
                }
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| format!("timed out waiting for {expected_type}"))?
}

/// Drains the socket until the gateway reports an error or hangs up.
///
/// Returns the error event when one arrives, `None` when the gateway closed
/// without explaining itself.
async fn receive_error_or_close<S>(
    socket: &mut WebSocketStream<S>,
) -> Result<Option<Value>, BoxError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    tokio::time::timeout(RECEIVE_TIMEOUT, async {
        loop {
            let Some(message) = socket.next().await else {
                return Ok(None);
            };
            match message? {
                Message::Text(text) => {
                    let event: Value = serde_json::from_str(text.as_ref())?;
                    if event.get("type").and_then(Value::as_str) == Some("error") {
                        return Ok(Some(event));
                    }
                }
                Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
                Message::Close(_) => return Ok(None),
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| "timed out waiting for a gateway error or close")?
}

// ---------------------------------------------------------------------------
// Mock upstream
// ---------------------------------------------------------------------------

/// How the mock upstream answers a `response.create`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpstreamBehavior {
    /// Announce, stream one delta, and complete — the ordinary turn.
    CompleteEveryTurn,
    /// Announce the response and then go quiet, leaving the turn in flight.
    StallAfterCreated,
    /// Announce the response and then hang up mid-turn.
    CloseAfterCreated,
    /// 第一轮只回一个 Codex 配额耗尽错误，之后的每一轮正常完成。
    ///
    /// 第一轮刻意不发 `response.created`：任何标准 `response.*` 事件都会让
    /// codex adapter 把这一轮判成 replay-unsafe，透明重试就不会发生。
    QuotaExhaustedThenComplete,
    /// 把收到的 `input` 原样回显成一个 delta，再正常完成。
    ///
    /// 上游看到的是脱敏后的 body，所以回显出来的就是占位符——正是响应侧还原要处理
    /// 的形状。
    EchoInputBack,
    /// Emit an event type and fields the gateway does not know, then complete.
    FutureEventThenComplete,
}

#[derive(Debug)]
struct MockUpstreamState {
    behavior: UpstreamBehavior,
    connections: AtomicUsize,
    events: Mutex<Vec<Value>>,
    authorization_headers: Mutex<Vec<Option<String>>>,
    handshakes: Mutex<Vec<ObservedUpstreamHandshake>>,
}

#[derive(Debug, Clone)]
struct ObservedUpstreamHandshake {
    request_target: String,
    headers: HeaderMap,
}

impl MockUpstreamState {
    fn new(behavior: UpstreamBehavior) -> Self {
        Self {
            behavior,
            connections: AtomicUsize::new(0),
            events: Mutex::new(Vec::new()),
            authorization_headers: Mutex::new(Vec::new()),
            handshakes: Mutex::new(Vec::new()),
        }
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::Acquire)
    }

    async fn observed_events(&self) -> Vec<Value> {
        self.events.lock().await.clone()
    }

    async fn authorization_headers(&self) -> Vec<Option<String>> {
        self.authorization_headers.lock().await.clone()
    }

    async fn observed_handshakes(&self) -> Vec<ObservedUpstreamHandshake> {
        self.handshakes.lock().await.clone()
    }
}

fn mock_upstream_router(state: Arc<MockUpstreamState>) -> Router {
    Router::new()
        .route("/v1/responses", get(mock_responses_websocket))
        .with_state(state)
}

async fn mock_responses_websocket(
    State(state): State<Arc<MockUpstreamState>>,
    uri: Uri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let authorization = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let handshake = ObservedUpstreamHandshake {
        request_target: uri.to_string(),
        headers,
    };
    ws.on_upgrade(move |socket| run_mock_upstream(socket, state, authorization, handshake))
}

async fn run_mock_upstream(
    mut socket: WebSocket,
    state: Arc<MockUpstreamState>,
    authorization: Option<String>,
    handshake: ObservedUpstreamHandshake,
) {
    state.connections.fetch_add(1, Ordering::AcqRel);
    state.authorization_headers.lock().await.push(authorization);
    state.handshakes.lock().await.push(handshake);
    while let Some(message) = socket.recv().await {
        let Ok(message) = message else {
            break;
        };
        match message {
            AxumWsMessage::Text(text) => {
                let Ok(event) = serde_json::from_str::<Value>(text.as_str()) else {
                    break;
                };
                if event.get("type").and_then(Value::as_str) != Some("response.create") {
                    continue;
                }
                let echoed_input = event
                    .get("input")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let turn = {
                    let mut events = state.events.lock().await;
                    events.push(event);
                    events.len()
                };
                let response_id = format!("resp-e2e-{turn}");
                match state.behavior {
                    UpstreamBehavior::CompleteEveryTurn => {
                        if send_mock_turn(&mut socket, &response_id).await.is_err() {
                            break;
                        }
                    }
                    UpstreamBehavior::StallAfterCreated => {
                        if send_mock_created(&mut socket, &response_id).await.is_err() {
                            break;
                        }
                    }
                    UpstreamBehavior::CloseAfterCreated => {
                        let _ = send_mock_created(&mut socket, &response_id).await;
                        break;
                    }
                    UpstreamBehavior::QuotaExhaustedThenComplete => {
                        if turn == 1 {
                            let _ =
                                send_mock_event(&mut socket, codex_quota_exhausted_error()).await;
                            break;
                        }
                        if send_mock_turn(&mut socket, &response_id).await.is_err() {
                            break;
                        }
                    }
                    UpstreamBehavior::EchoInputBack => {
                        if send_mock_turn_with_delta(&mut socket, &response_id, &echoed_input)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    UpstreamBehavior::FutureEventThenComplete => {
                        if send_mock_event(
                            &mut socket,
                            json!({
                                "type": "response.future.capability",
                                "response_id": response_id,
                                "future_capability": {
                                    "enabled": true,
                                    "revision": 7
                                }
                            }),
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                        if send_mock_turn(&mut socket, &response_id).await.is_err() {
                            break;
                        }
                    }
                }
            }
            AxumWsMessage::Ping(payload) => {
                if socket.send(AxumWsMessage::Pong(payload)).await.is_err() {
                    break;
                }
            }
            AxumWsMessage::Close(_) => break,
            _ => {}
        }
    }
}

/// Codex 的账户级配额耗尽信号。
///
/// `status_code: 429` + `error.type: usage_limit_reached` 是 adapter 识别
/// 「配额耗尽、可透明重试」的最小载荷：解析出的元数据被强制标上
/// `limit_reached: true`，于是 drain 指令带着 `retry_current_turn: true` 下来。
fn codex_quota_exhausted_error() -> Value {
    json!({
        "type": "error",
        "status_code": 429,
        "error": {
            "type": "usage_limit_reached",
            "message": "You have hit your usage limit",
            "plan_type": "plus",
            "resets_in_seconds": 3_600
        }
    })
}

async fn send_mock_created(socket: &mut WebSocket, response_id: &str) -> Result<(), axum::Error> {
    send_mock_event(
        socket,
        json!({
            "type": "response.created",
            "response": {
                "id": response_id,
                "status": "in_progress",
                "model": UPSTREAM_MODEL
            }
        }),
    )
    .await
}

async fn send_mock_turn(socket: &mut WebSocket, response_id: &str) -> Result<(), axum::Error> {
    send_mock_turn_with_delta(socket, response_id, "hello").await
}

async fn send_mock_turn_with_delta(
    socket: &mut WebSocket,
    response_id: &str,
    delta: &str,
) -> Result<(), axum::Error> {
    send_mock_created(socket, response_id).await?;
    send_mock_event(
        socket,
        json!({
            "type": "response.output_text.delta",
            "response_id": response_id,
            "delta": delta
        }),
    )
    .await?;
    send_mock_event(
        socket,
        json!({
            "type": "response.completed",
            "response": {
                "id": response_id,
                "status": "completed",
                "model": UPSTREAM_MODEL,
                "output": [],
                "usage": {
                    "input_tokens": INPUT_TOKENS,
                    "output_tokens": OUTPUT_TOKENS,
                    "total_tokens": INPUT_TOKENS + OUTPUT_TOKENS
                }
            }
        }),
    )
    .await
}

async fn send_mock_event(socket: &mut WebSocket, event: Value) -> Result<(), axum::Error> {
    socket
        .send(AxumWsMessage::Text(event.to_string().into()))
        .await
}

// ---------------------------------------------------------------------------
// Seeded data store
// ---------------------------------------------------------------------------

struct TemporarySqlite {
    directory: PathBuf,
    config: SqlDatabaseConfig,
}

impl TemporarySqlite {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "aether-responses-ws-e2e-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let database_path = directory.join("aether.db");
        Self {
            directory,
            config: SqlDatabaseConfig {
                driver: DatabaseDriver::Sqlite,
                url: format!("sqlite://{}", database_path.display()),
                pool: SqlPoolConfig {
                    min_connections: 1,
                    max_connections: 4,
                    acquire_timeout_ms: 5_000,
                    idle_timeout_ms: 30_000,
                    max_lifetime_ms: 300_000,
                    statement_cache_capacity: 64,
                    require_ssl: false,
                },
            },
        }
    }
}

impl Drop for TemporarySqlite {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

async fn prepare_and_seed_database(
    database: &SqlDatabaseConfig,
    upstream_base_url: &str,
    fixture: ProviderFixture,
    redaction: PiiRedaction,
) -> Result<(), BoxError> {
    let backends = DataBackends::from_config(DataLayerConfig::from_database(database.clone()))?;
    let pending = backends
        .prepare_database_for_startup()
        .await?
        .unwrap_or_default();
    if !pending.is_empty() {
        backends.run_database_migrations().await?;
    }

    seed_provider_catalog(&backends, upstream_base_url, fixture).await?;
    seed_models(&backends).await?;
    let user_id = seed_user(&backends).await?;
    seed_client_api_key(&backends, &user_id).await?;
    if redaction.is_enabled() {
        seed_chat_pii_redaction(&backends).await?;
    }

    let candidates = backends
        .read()
        .minimal_candidate_selection()
        .ok_or("candidate selection reader unavailable")?
        .list_for_exact_api_format_and_requested_model("openai:responses", PUBLIC_MODEL)
        .await?;
    if !candidates.iter().any(|candidate| {
        candidate.provider_id == PROVIDER_ID
            && candidate.endpoint_id == ENDPOINT_ID
            && candidate.key_id == PROVIDER_KEY_ID
    }) {
        return Err("seeded Responses WebSocket candidate is not visible".into());
    }
    drop(backends);
    Ok(())
}

async fn seed_provider_catalog(
    backends: &DataBackends,
    upstream_base_url: &str,
    fixture: ProviderFixture,
) -> Result<(), BoxError> {
    let writer = backends
        .write()
        .provider_catalog()
        .ok_or("provider catalog writer unavailable")?;
    writer
        .create_provider(
            &StoredProviderCatalogProvider::new(
                PROVIDER_ID.to_string(),
                "Responses WebSocket E2E".to_string(),
                None,
                fixture.provider_type().to_string(),
            )?
            .with_transport_fields(
                true,
                false,
                false,
                None,
                Some(0),
                None,
                Some(30.0),
                Some(10.0),
                Some(json!({"responses_websocket": {"enabled": true}})),
            ),
            None,
        )
        .await?;
    writer
        .create_endpoint(
            &StoredProviderCatalogEndpoint::new(
                ENDPOINT_ID.to_string(),
                PROVIDER_ID.to_string(),
                "openai:responses".to_string(),
                Some("openai".to_string()),
                Some("responses".to_string()),
                true,
            )?
            .with_transport_fields(
                upstream_base_url.trim_end_matches('/').to_string(),
                None,
                None,
                Some(0),
                Some("/v1/responses".to_string()),
                None,
                None,
                None,
            )?,
        )
        .await?;
    writer
        .create_key(&catalog_key(PROVIDER_KEY_ID, PROVIDER_API_KEY, fixture)?)
        .await?;
    if fixture.has_alternate_key() {
        writer
            .create_key(&catalog_key(
                ALTERNATE_PROVIDER_KEY_ID,
                ALTERNATE_PROVIDER_API_KEY,
                fixture,
            )?)
            .await?;
    }
    Ok(())
}

/// 一把健康、可服务本用例模型的 key。
///
/// codex 类型的候选要求 `auth_type = oauth`（见 candidate_selection 的
/// provider_type 约束），所以配额重试夹具走 oauth，凭证是一份未过期的
/// access_token。
fn catalog_key(
    key_id: &str,
    secret: &str,
    fixture: ProviderFixture,
) -> Result<StoredProviderCatalogKey, BoxError> {
    let oauth = fixture.has_alternate_key();
    let auth_type = if oauth { "oauth" } else { "api_key" };
    let auth_config = if oauth {
        Some(encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            &json!({
                "access_token": secret,
                "refresh_token": format!("{secret}-refresh"),
                "account_id": format!("{key_id}-account"),
                "expires_at": FAR_FUTURE_UNIX_SECS,
            })
            .to_string(),
        )?)
    } else {
        None
    };
    Ok(StoredProviderCatalogKey::new(
        key_id.to_string(),
        PROVIDER_ID.to_string(),
        "Responses WebSocket E2E".to_string(),
        auth_type.to_string(),
        Some(json!({"streaming": true})),
        true,
    )?
    .with_transport_fields(
        Some(json!(["openai:responses"])),
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, secret)?,
        auth_config,
        None,
        Some(json!({"openai:responses": 1})),
        Some(json!([PUBLIC_MODEL, UPSTREAM_MODEL])),
        None,
        None,
        None,
    )?
    .with_health_fields(
        Some(json!({"openai:responses": {"status": "healthy"}})),
        Some(json!({"openai:responses": {"state": "closed"}})),
    ))
}

async fn seed_models(backends: &DataBackends) -> Result<(), BoxError> {
    let writer = backends
        .write()
        .global_models()
        .ok_or("global model writer unavailable")?;
    writer
        .create_admin_global_model(&CreateAdminGlobalModelRecord::new(
            GLOBAL_MODEL_ID.to_string(),
            PUBLIC_MODEL.to_string(),
            PUBLIC_MODEL.to_string(),
            true,
            Some(0.0),
            None,
            Some(json!({"streaming": true, "chat": true})),
            Some(json!({"model_mappings": [UPSTREAM_MODEL]})),
        )?)
        .await?;
    writer
        .create_admin_provider_model(&UpsertAdminProviderModelRecord::new(
            PROVIDER_MODEL_ID.to_string(),
            PROVIDER_ID.to_string(),
            GLOBAL_MODEL_ID.to_string(),
            UPSTREAM_MODEL.to_string(),
            Some(json!([{
                "name": UPSTREAM_MODEL,
                "priority": 0,
                "api_formats": ["openai:responses"],
                "endpoint_ids": [ENDPOINT_ID]
            }])),
            Some(0.0),
            None,
            Some(false),
            Some(false),
            Some(true),
            Some(false),
            Some(false),
            true,
            true,
            Some(json!({"responses_websocket_e2e": true})),
        )?)
        .await?;
    Ok(())
}

async fn seed_user(backends: &DataBackends) -> Result<String, BoxError> {
    let users = backends.read().users().ok_or("user reader unavailable")?;
    let user = users
        .create_local_auth_user_with_settings(
            Some("responses-ws-e2e@example.test".to_string()),
            true,
            "responses-ws-e2e".to_string(),
            "disabled-password".to_string(),
            "user".to_string(),
            Some(vec![PROVIDER_ID.to_string()]),
            Some(vec!["openai:responses".to_string()]),
            Some(vec![PUBLIC_MODEL.to_string()]),
            None,
        )
        .await?
        .ok_or("failed to create E2E user")?;
    let wallets = backends
        .read()
        .wallets()
        .ok_or("wallet reader unavailable")?;
    wallets
        .initialize_auth_user_wallet(&user.id, 0.0, true)
        .await?;
    Ok(user.id)
}

async fn seed_client_api_key(backends: &DataBackends, user_id: &str) -> Result<(), BoxError> {
    backends
        .write()
        .auth_api_keys()
        .ok_or("auth API key writer unavailable")?
        .create_standalone_api_key(CreateStandaloneApiKeyRecord {
            user_id: user_id.to_string(),
            api_key_id: API_KEY_ID.to_string(),
            key_hash: sha256_hex(CLIENT_API_KEY),
            key_encrypted: Some(CLIENT_API_KEY.to_string()),
            name: Some("Responses WebSocket E2E".to_string()),
            allowed_providers: Some(vec![PROVIDER_ID.to_string()]),
            allowed_provider_key_ids: None,
            allowed_api_formats: Some(vec!["openai:responses".to_string()]),
            allowed_models: Some(vec![PUBLIC_MODEL.to_string()]),
            ip_rules: None,
            rate_limit: Some(0),
            concurrent_limit: None,
            force_capabilities: None,
            is_active: true,
            expires_at_unix_secs: None,
            auto_delete_on_expiry: false,
            total_requests: 0,
            total_tokens: 0,
            total_cost_usd: 0.0,
        })
        .await?;
    backends
        .read()
        .wallets()
        .ok_or("wallet reader unavailable")?
        .initialize_auth_api_key_wallet(API_KEY_ID, 0.0, true)
        .await?;
    if backends
        .read()
        .wallets()
        .ok_or("wallet reader unavailable")?
        .find(WalletLookupKey::ApiKeyId(API_KEY_ID))
        .await?
        .is_none()
    {
        return Err("failed to initialize E2E API key wallet".into());
    }
    Ok(())
}

/// 打开 chat PII 脱敏：系统模块开关 + 这把 client key 的 feature 开关。
///
/// 规则集刻意不写：缺省即内置规则（含 email 规则），和生产上「只打开开关」的最小
/// 配置一致。
async fn seed_chat_pii_redaction(backends: &DataBackends) -> Result<(), BoxError> {
    backends
        .upsert_system_config_entry("module.chat_pii_redaction.enabled", &json!(true), None)
        .await?;
    backends
        .write()
        .auth_api_keys()
        .ok_or("auth API key writer unavailable")?
        .set_standalone_api_key_feature_settings(
            API_KEY_ID,
            Some(json!({"chat_pii_redaction": {"enabled": true}})),
        )
        .await?
        .ok_or("failed to enable chat PII redaction on the E2E API key")?;
    Ok(())
}

fn sha256_hex(value: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}
