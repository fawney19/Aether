use super::*;

use std::collections::BTreeSet;
use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::task::Poll;

type TestConnection = ::redis::aio::MultiplexedConnection;
type ReadResult = Result<Vec<RuntimeQueueEntry>, DataLayerError>;

const TEST_GROUP: &str = "receive-workers";
const OWNER_BLOCK_MS: u64 = 60_000;

fn receive_lane_count() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4)
        .clamp(4, 16)
}

async fn receive_runtime(
    protocol: &str,
    command_timeout_ms: u64,
) -> Option<(TestRedisServer, RuntimeState, TestConnection)> {
    let Some(server) = TestRedisServer::start().await else {
        eprintln!(
            "stream receive {protocol} skipped: isolated Redis fixture unavailable; check AETHER_REDIS_SERVER_BIN"
        );
        return None;
    };
    let mut admin = redis_test_connection(&server.redis_url).await;
    ::redis::cmd("ACL")
        .arg("SETUSER")
        .arg("stream-reader")
        .arg("on")
        .arg(">stream-test-password")
        .arg("~*")
        .arg("+@all")
        .query_async::<()>(&mut admin)
        .await
        .expect("test stream user");
    let runtime = RuntimeState::redis_with_blocking_stream_lanes(
        RedisClientConfig {
            url: format!(
                "redis://stream-reader:stream-test-password@127.0.0.1:{}/7?protocol={protocol}",
                server.port
            ),
            key_prefix: Some(format!("receive-{protocol}")),
        },
        Some(command_timeout_ms),
        Some(4),
    )
    .await
    .expect("authenticated receive runtime in database 7");
    ::redis::cmd("SELECT")
        .arg(7)
        .query_async::<()>(&mut admin)
        .await
        .expect("admin selects test database");
    eprintln!(
        "stream receive fixture ready: protocol={protocol} db=7 port={} authenticated=true",
        server.port
    );
    Some((server, runtime, admin))
}

async fn receive_group(runtime: &RuntimeState, stream: &str) {
    RuntimeQueueStore::ensure_consumer_group(runtime, stream, TEST_GROUP, "0-0")
        .await
        .expect("receive consumer group");
}

fn receive_fields(sequence: usize) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "payload".to_string(),
            format!("record-{sequence}\r\n\"quoted\"\\\u{4e2d}\u{6587}"),
        ),
        ("sequence".to_string(), sequence.to_string()),
        ("legacy_field".to_string(), "preserve exactly".to_string()),
    ])
}

async fn append_receive(runtime: &RuntimeState, stream: &str, sequence: usize) -> String {
    RuntimeQueueStore::append_fields_with_maxlen(runtime, stream, &receive_fields(sequence), None)
        .await
        .expect("append receive entry")
}

async fn client_rows(admin: &mut TestConnection) -> Vec<BTreeMap<String, String>> {
    let value = ::redis::cmd("CLIENT")
        .arg("LIST")
        .query_async::<String>(admin)
        .await
        .expect("Redis client list");
    value
        .lines()
        .map(|line| {
            line.split_whitespace()
                .filter_map(|field| field.split_once('='))
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect()
        })
        .collect()
}

fn blocked_rows(rows: &[BTreeMap<String, String>]) -> Vec<&BTreeMap<String, String>> {
    rows.iter()
        .filter(|row| row.get("flags").is_some_and(|flags| flags.contains('b')))
        .collect()
}

async fn wait_for_blocked(admin: &mut TestConnection, expected: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if blocked_rows(&client_rows(admin).await).len() == expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Redis blocked clients reach expected count");
}

fn spawn_owner(
    owners: &mut tokio::task::JoinSet<ReadResult>,
    runtime: &RuntimeState,
    stream: &'static str,
    index: usize,
) {
    let runtime = runtime.clone();
    owners.spawn(async move {
        RuntimeQueueStore::read_group(
            &runtime,
            stream,
            TEST_GROUP,
            &format!("owner-{index}"),
            1,
            Some(OWNER_BLOCK_MS),
        )
        .await
    });
}

async fn abort_owners(owners: &mut tokio::task::JoinSet<ReadResult>) {
    owners.abort_all();
    while let Some(result) = owners.join_next().await {
        assert!(result.expect_err("owner must be cancelled").is_cancelled());
    }
}

async fn assert_receive_pending<F: Future + ?Sized>(mut future: Pin<&mut F>) {
    poll_fn(|context| {
        assert!(future.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
}

async fn pending_consumers(
    admin: &mut TestConnection,
    stream: &str,
) -> Vec<(String, String, u64, u64)> {
    ::redis::cmd("XPENDING")
        .arg(stream)
        .arg(TEST_GROUP)
        .arg("-")
        .arg("+")
        .arg(100)
        .query_async(admin)
        .await
        .expect("pending entry ownership")
}

#[tokio::test]
async fn redis_stream_receive_full_pool_waits_without_sending_and_cancellation_preserves_pel() {
    for protocol in ["resp2", "resp3"] {
        let Some((_server, runtime, mut admin)) = receive_runtime(protocol, 5_000).await else {
            return;
        };
        let stream = "receive:blocked";
        let fast_stream = "receive:nonblocking";
        receive_group(&runtime, stream).await;
        receive_group(&runtime, fast_stream).await;
        let lanes = receive_lane_count();
        let mut owners = tokio::task::JoinSet::new();
        for index in 0..lanes {
            spawn_owner(&mut owners, &runtime, stream, index);
        }
        wait_for_blocked(&mut admin, lanes).await;
        let before = client_rows(&mut admin).await;
        let owner_input_bytes = blocked_rows(&before)
            .into_iter()
            .map(|row| (row["id"].clone(), row.get("tot-net-in").cloned()))
            .collect::<BTreeMap<_, _>>();

        let mut waiter = Box::pin(RuntimeQueueStore::read_group(
            &runtime,
            stream,
            TEST_GROUP,
            "cancelled-waiter",
            1,
            Some(OWNER_BLOCK_MS),
        ));
        assert_receive_pending(waiter.as_mut()).await;

        // Complete unrelated round trips while the waiter remains polled and the owners block.
        tokio::time::timeout(Duration::from_secs(5), async {
            runtime.kv_set("receive-fast", "ready", None).await.unwrap();
            assert_eq!(
                runtime.kv_get("receive-fast").await.unwrap().as_deref(),
                Some("ready")
            );
            let expected_id = append_receive(&runtime, fast_stream, 7).await;
            let entries = RuntimeQueueStore::read_group(
                &runtime,
                fast_stream,
                TEST_GROUP,
                "nonblocking-reader",
                1,
                None,
            )
            .await
            .unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].id, expected_id);
            assert_eq!(entries[0].fields, receive_fields(7));
        })
        .await
        .expect("full blocking pool must not delay fast or nonblocking stream lanes");

        for _ in 0..3 {
            tokio::task::yield_now().await;
            let rows = client_rows(&mut admin).await;
            let blocked = blocked_rows(&rows);
            assert_eq!(blocked.len(), lanes);
            for row in blocked {
                assert_eq!(
                    row.get("tot-net-in"),
                    owner_input_bytes[&row["id"]].as_ref()
                );
                assert_eq!(row["qbuf"], "0", "waiter must not be sent behind a BLOCK");
            }
            assert_receive_pending(waiter.as_mut()).await;
        }
        drop(waiter);
        assert_eq!(blocked_rows(&client_rows(&mut admin).await).len(), lanes);
        assert!(pending_consumers(&mut admin, stream).await.is_empty());

        abort_owners(&mut owners).await;
        wait_for_blocked(&mut admin, 0).await;
        let expected_id = append_receive(&runtime, stream, 8).await;
        let entries = RuntimeQueueStore::read_group(
            &runtime,
            stream,
            TEST_GROUP,
            "replacement-reader",
            1,
            Some(100),
        )
        .await
        .expect("replacement blocking connection");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, expected_id);
        assert_eq!(entries[0].fields, receive_fields(8));
        let pending = pending_consumers(&mut admin, stream).await;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, expected_id);
        assert_eq!(pending[0].1, "replacement-reader");
        ::redis::cmd("SELECT")
            .arg(0)
            .query_async::<()>(&mut admin)
            .await
            .unwrap();
        let other_database_len = ::redis::cmd("XLEN")
            .arg(stream)
            .query_async::<u64>(&mut admin)
            .await
            .unwrap();
        assert_eq!(
            other_database_len, 0,
            "AUTH and SELECT must survive replacement connections"
        );
    }
}

#[tokio::test]
async fn redis_stream_receive_fast_consumer_reuses_free_lane_while_other_lanes_block() {
    let Some((_server, runtime, mut admin)) = receive_runtime("resp2", 2_000).await else {
        return;
    };
    let slow_stream = "receive:slow";
    let fast_stream = "receive:ready";
    receive_group(&runtime, slow_stream).await;
    receive_group(&runtime, fast_stream).await;
    let lanes = receive_lane_count();
    let mut owners = tokio::task::JoinSet::new();
    for index in 0..lanes - 1 {
        spawn_owner(&mut owners, &runtime, slow_stream, index);
    }
    wait_for_blocked(&mut admin, lanes - 1).await;

    let mut reader_connection_id = None;
    for sequence in 0..lanes * 2 {
        let expected_id = append_receive(&runtime, fast_stream, sequence).await;
        let entries = RuntimeQueueStore::read_group(
            &runtime,
            fast_stream,
            TEST_GROUP,
            "fast-reader",
            1,
            Some(100),
        )
        .await
        .expect("a free lane must remain reusable instead of rotating into a blocked lane");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, expected_id);
        assert_eq!(entries[0].fields, receive_fields(sequence));
        let rows = client_rows(&mut admin).await;
        let idle_readers = rows
            .iter()
            .filter(|row| {
                row.get("cmd").map(String::as_str) == Some("xreadgroup")
                    && row.get("flags").is_some_and(|flags| !flags.contains('b'))
            })
            .collect::<Vec<_>>();
        assert_eq!(idle_readers.len(), 1);
        let current_id = &idle_readers[0]["id"];
        if let Some(previous_id) = reader_connection_id.as_ref() {
            assert_eq!(
                current_id, previous_id,
                "successful reads must reuse the same free connection"
            );
        } else {
            reader_connection_id = Some(current_id.clone());
        }
    }
    assert_eq!(
        blocked_rows(&client_rows(&mut admin).await).len(),
        lanes - 1
    );
    abort_owners(&mut owners).await;
    wait_for_blocked(&mut admin, 0).await;
}

#[tokio::test]
async fn redis_stream_receive_timeout_discards_inflight_connection_before_reuse() {
    let Some((_server, runtime, mut admin)) = receive_runtime("resp3", 1_000).await else {
        return;
    };
    let stream = "receive:timeout";
    receive_group(&runtime, stream).await;
    let initial_connection_ids = client_rows(&mut admin)
        .await
        .into_iter()
        .map(|row| row["id"].clone())
        .collect::<BTreeSet<_>>();
    // XREADGROUP is a write command; pausing writes makes its network response exceed the
    // normal BLOCK-plus-grace timeout while read-only CLIENT diagnostics remain available.
    ::redis::cmd("CLIENT")
        .arg("PAUSE")
        .arg(30_000)
        .arg("WRITE")
        .query_async::<()>(&mut admin)
        .await
        .expect("pause test Redis writes");
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        RuntimeQueueStore::read_group(
            &runtime,
            stream,
            TEST_GROUP,
            "timed-out-reader",
            1,
            Some(100),
        ),
    )
    .await
    .expect("read reaches its configured command deadline");
    assert!(matches!(result, Err(DataLayerError::TimedOut(_))));
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let connection_ids = client_rows(&mut admin)
                .await
                .into_iter()
                .map(|row| row["id"].clone())
                .collect::<BTreeSet<_>>();
            if connection_ids.len() + 1 == initial_connection_ids.len()
                && connection_ids.is_subset(&initial_connection_ids)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed-out connection must disconnect while its old command is still paused");
    ::redis::cmd("CLIENT")
        .arg("UNPAUSE")
        .query_async::<()>(&mut admin)
        .await
        .unwrap();
    let expected_id = append_receive(&runtime, stream, 9).await;
    let entries =
        RuntimeQueueStore::read_group(&runtime, stream, TEST_GROUP, "after-timeout", 1, Some(100))
            .await
            .expect("replacement read after timeout");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, expected_id);
    assert_eq!(entries[0].fields, receive_fields(9));
    let pending = pending_consumers(&mut admin, stream).await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0, expected_id);
    assert_eq!(pending[0].1, "after-timeout");
}

async fn set_pending_idle(admin: &mut TestConnection, stream: &str, ids: &[String]) {
    let mut command = ::redis::cmd("XCLAIM");
    command
        .arg(stream)
        .arg(TEST_GROUP)
        .arg("initial-reader")
        .arg(0);
    for id in ids {
        command.arg(id);
    }
    command.arg("IDLE").arg(120_000).arg("JUSTID");
    let changed = command
        .query_async::<Vec<String>>(admin)
        .await
        .expect("set pending idle");
    assert_eq!(changed, ids);
}

#[tokio::test]
async fn redis_stream_receive_reclaim_pages_advance_past_fresh_prefix_and_deleted_entries() {
    for protocol in ["resp2", "resp3"] {
        let Some((_server, runtime, mut admin)) = receive_runtime(protocol, 5_000).await else {
            return;
        };
        let stream = "receive:reclaim-pages";
        receive_group(&runtime, stream).await;
        let mut ids = Vec::new();
        for sequence in 0..28 {
            ids.push(append_receive(&runtime, stream, sequence).await);
        }
        let entries =
            RuntimeQueueStore::read_group(&runtime, stream, TEST_GROUP, "initial-reader", 28, None)
                .await
                .expect("seed pending entries");
        assert_eq!(entries.len(), ids.len());
        set_pending_idle(&mut admin, stream, &ids[25..]).await;
        assert_eq!(
            RuntimeQueueStore::delete(&runtime, stream, &ids[26..27])
                .await
                .unwrap(),
            1
        );
        let config = RuntimeQueueReclaimConfig {
            min_idle_ms: 60_000,
            count: 2,
        };
        let first = RuntimeQueueStore::claim_stale_page(
            &runtime,
            stream,
            TEST_GROUP,
            "reclaim-reader",
            "0-0",
            config,
        )
        .await
        .expect("first reclaim page");
        assert!(
            first.entries.is_empty(),
            "fresh prefix exceeds COUNT * 10 scan budget"
        );
        assert!(first.deleted_ids.is_empty());
        assert_ne!(
            first.next_start_id, "0-0",
            "empty page must preserve continuation"
        );
        let mut cursor = first.next_start_id;
        let mut reclaimed = BTreeMap::new();
        let mut deleted = BTreeSet::new();
        for _ in 0..8 {
            let page = RuntimeQueueStore::claim_stale_page(
                &runtime,
                stream,
                TEST_GROUP,
                "reclaim-reader",
                &cursor,
                config,
            )
            .await
            .expect("continued reclaim page");
            assert!(page.entries.len() <= config.count);
            for entry in page.entries {
                assert!(reclaimed.insert(entry.id, entry.fields).is_none());
            }
            deleted.extend(page.deleted_ids);
            cursor = page.next_start_id;
            if cursor == "0-0" {
                break;
            }
        }
        assert_eq!(cursor, "0-0", "scan must eventually wrap");
        assert_eq!(
            reclaimed,
            BTreeMap::from([
                (ids[25].clone(), receive_fields(25)),
                (ids[27].clone(), receive_fields(27))
            ])
        );
        assert_eq!(deleted, BTreeSet::from([ids[26].clone()]));
        let pending = pending_consumers(&mut admin, stream).await;
        assert_eq!(pending.len(), 27);
        assert!(!pending.iter().any(|entry| entry.0 == ids[26]));
        assert!(pending
            .iter()
            .filter(|entry| entry.1 == "reclaim-reader")
            .all(|entry| entry.0 == ids[25] || entry.0 == ids[27]));

        set_pending_idle(&mut admin, stream, &ids[..1]).await;
        let restarted = RuntimeQueueStore::claim_stale_page(
            &runtime,
            stream,
            TEST_GROUP,
            "rescan-reader",
            "0-0",
            config,
        )
        .await
        .expect("restart scan after cursor wraps");
        assert_eq!(restarted.entries.len(), 1);
        assert_eq!(restarted.entries[0].id, ids[0]);
        assert_eq!(restarted.entries[0].fields, receive_fields(0));
        assert!(restarted.deleted_ids.is_empty());
    }
}

async fn interrupted_reclaim_preserves_pending(protocol: &str, cancel: bool) {
    let command_timeout_ms = if cancel { 10_000 } else { 1_000 };
    let Some((_server, runtime, mut admin)) = receive_runtime(protocol, command_timeout_ms).await
    else {
        return;
    };
    let stream = "receive:interrupted-reclaim";
    receive_group(&runtime, stream).await;
    let mut ids = Vec::new();
    for sequence in 0..3 {
        ids.push(append_receive(&runtime, stream, sequence).await);
    }
    assert_eq!(
        RuntimeQueueStore::read_group(&runtime, stream, TEST_GROUP, "initial-reader", 3, None,)
            .await
            .unwrap()
            .len(),
        3
    );
    set_pending_idle(&mut admin, stream, &ids).await;
    RuntimeQueueStore::delete(&runtime, stream, &ids[1..2])
        .await
        .unwrap();
    let before = client_rows(&mut admin).await;
    let initial_ids = before
        .iter()
        .map(|row| row["id"].clone())
        .collect::<BTreeSet<_>>();
    let input_bytes = before
        .iter()
        .filter(|row| row.get("user").map(String::as_str) == Some("stream-reader"))
        .map(|row| {
            (
                row["id"].clone(),
                row.get("tot-net-in")
                    .and_then(|value| value.parse::<u64>().ok()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    ::redis::cmd("CLIENT")
        .arg("PAUSE")
        .arg(30_000)
        .arg("WRITE")
        .query_async::<()>(&mut admin)
        .await
        .unwrap();
    let config = RuntimeQueueReclaimConfig {
        min_idle_ms: 60_000,
        count: 1,
    };
    let claim_runtime = runtime.clone();
    let claim = tokio::spawn(async move {
        RuntimeQueueStore::claim_stale_page(
            &claim_runtime,
            stream,
            TEST_GROUP,
            "interrupted-reader",
            "0-0",
            config,
        )
        .await
    });
    if cancel {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let sent = client_rows(&mut admin).await.iter().any(|row| {
                    let Some(previous) = input_bytes.get(&row["id"]) else {
                        return false;
                    };
                    match (
                        previous,
                        row.get("tot-net-in")
                            .and_then(|value| value.parse::<u64>().ok()),
                    ) {
                        (Some(previous), Some(current)) => current > *previous,
                        _ => row.get("cmd").map(String::as_str) == Some("xautoclaim"),
                    }
                });
                if sent {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("claim reaches Redis before caller cancellation");
        claim.abort();
        assert!(claim.await.expect_err("cancelled reclaim").is_cancelled());
    } else {
        let result = tokio::time::timeout(Duration::from_secs(10), claim)
            .await
            .expect("reclaim command deadline")
            .expect("reclaim task");
        assert!(matches!(result, Err(DataLayerError::TimedOut(_))));
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let remaining = client_rows(&mut admin)
                .await
                .into_iter()
                .map(|row| row["id"].clone())
                .collect::<BTreeSet<_>>();
            if remaining.len() + 1 == initial_ids.len() && remaining.is_subset(&initial_ids) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("interrupted claim closes its socket before writes resume");
    let pending = pending_consumers(&mut admin, stream).await;
    assert_eq!(pending.len(), 3);
    assert!(pending.iter().all(|entry| entry.1 == "initial-reader"));
    ::redis::cmd("CLIENT")
        .arg("UNPAUSE")
        .query_async::<()>(&mut admin)
        .await
        .unwrap();

    let mut cursor = "0-0".to_string();
    let mut recovered = BTreeMap::new();
    let mut deleted = BTreeSet::new();
    let mut successful_claims = 0;
    for _ in 0..5 {
        let page = RuntimeQueueStore::claim_stale_page(
            &runtime,
            stream,
            TEST_GROUP,
            "recovery-reader",
            &cursor,
            config,
        )
        .await
        .expect("reclaim recovers after interrupted connection");
        successful_claims += 1;
        for entry in page.entries {
            assert!(recovered.insert(entry.id, entry.fields).is_none());
        }
        deleted.extend(page.deleted_ids);
        cursor = page.next_start_id;
        if cursor == "0-0" {
            break;
        }
    }
    assert_eq!(cursor, "0-0");
    assert_eq!(
        recovered,
        BTreeMap::from([
            (ids[0].clone(), receive_fields(0)),
            (ids[2].clone(), receive_fields(2))
        ])
    );
    assert_eq!(deleted, BTreeSet::from([ids[1].clone()]));
    let pending = pending_consumers(&mut admin, stream).await;
    assert_eq!(pending.len(), 2);
    assert!(pending.iter().all(|entry| entry.1 == "recovery-reader"));
    let diagnostics = runtime.redis_diagnostics().await.unwrap().unwrap();
    let lane = diagnostics
        .lanes
        .iter()
        .find(|lane| lane.lane == "blocking_stream")
        .unwrap();
    assert_eq!(lane.command_timeouts, u64::from(!cancel));
    assert!(lane.command_count >= successful_claims);
}

#[tokio::test]
async fn redis_stream_receive_reclaim_cancellation_closes_connection_and_preserves_pending() {
    interrupted_reclaim_preserves_pending("resp2", true).await;
}

#[tokio::test]
async fn redis_stream_receive_reclaim_timeout_closes_connection_and_preserves_pending() {
    interrupted_reclaim_preserves_pending("resp3", false).await;
}

#[tokio::test]
async fn redis_stream_receive_reclaim_waits_for_read_lease_and_continues_after_release() {
    let Some((_server, runtime, mut admin)) = receive_runtime("resp2", 1_000).await else {
        return;
    };
    let blocked_stream = "receive:reclaim-pool-blocked";
    let pending_stream = "receive:reclaim-pool-pending";
    receive_group(&runtime, blocked_stream).await;
    receive_group(&runtime, pending_stream).await;
    let ids = vec![
        append_receive(&runtime, pending_stream, 0).await,
        append_receive(&runtime, pending_stream, 1).await,
    ];
    assert_eq!(
        RuntimeQueueStore::read_group(
            &runtime,
            pending_stream,
            TEST_GROUP,
            "initial-reader",
            2,
            None
        )
        .await
        .unwrap()
        .len(),
        2
    );
    set_pending_idle(&mut admin, pending_stream, &ids).await;
    let lanes = receive_lane_count();
    let mut owners = tokio::task::JoinSet::new();
    for index in 0..lanes - 1 {
        spawn_owner(&mut owners, &runtime, blocked_stream, index);
    }
    let owner_runtime = runtime.clone();
    let release_owner = owners.spawn(async move {
        RuntimeQueueStore::read_group(
            &owner_runtime,
            blocked_stream,
            TEST_GROUP,
            "released-owner",
            1,
            Some(OWNER_BLOCK_MS),
        )
        .await
    });
    wait_for_blocked(&mut admin, lanes).await;
    let config = RuntimeQueueReclaimConfig {
        min_idle_ms: 60_000,
        count: 1,
    };
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        RuntimeQueueStore::claim_stale_page(
            &runtime,
            pending_stream,
            TEST_GROUP,
            "timed-out-waiter",
            "0-0",
            config,
        ),
    )
    .await
    .expect("checkout uses the original command deadline");
    assert!(matches!(result, Err(DataLayerError::TimedOut(_))));
    assert_eq!(blocked_rows(&client_rows(&mut admin).await).len(), lanes);
    assert!(pending_consumers(&mut admin, pending_stream)
        .await
        .iter()
        .all(|entry| entry.1 == "initial-reader"));
    let mut cancelled_claim = Box::pin(RuntimeQueueStore::claim_stale_page(
        &runtime,
        pending_stream,
        TEST_GROUP,
        "cancelled-waiter",
        "0-0",
        config,
    ));
    assert_receive_pending(cancelled_claim.as_mut()).await;
    drop(cancelled_claim);
    assert_eq!(blocked_rows(&client_rows(&mut admin).await).len(), lanes);

    let mut claim = Box::pin(RuntimeQueueStore::claim_stale_page(
        &runtime,
        pending_stream,
        TEST_GROUP,
        "after-release",
        "0-0",
        config,
    ));
    assert_receive_pending(claim.as_mut()).await;
    release_owner.abort();
    assert!(owners
        .join_next()
        .await
        .unwrap()
        .expect_err("released owner cancelled")
        .is_cancelled());
    let first = tokio::time::timeout(Duration::from_secs(5), claim)
        .await
        .expect("claim receives released pool capacity")
        .expect("claim succeeds after read releases lease");
    assert_eq!(first.entries.len(), 1);
    assert_eq!(first.entries[0].id, ids[0]);
    assert_eq!(first.entries[0].fields, receive_fields(0));
    assert_ne!(first.next_start_id, "0-0");

    let next_id = append_receive(&runtime, pending_stream, 2).await;
    let next_read = RuntimeQueueStore::read_group(
        &runtime,
        pending_stream,
        TEST_GROUP,
        "read-after-claim",
        1,
        Some(100),
    )
    .await
    .expect("completed claim returns its lease for the next read");
    assert_eq!(next_read.len(), 1);
    assert_eq!(next_read[0].id, next_id);
    let second = RuntimeQueueStore::claim_stale_page(
        &runtime,
        pending_stream,
        TEST_GROUP,
        "after-release",
        &first.next_start_id,
        config,
    )
    .await
    .expect("completed read returns its lease for the next claim");
    assert_eq!(second.entries.len(), 1);
    assert_eq!(second.entries[0].id, ids[1]);
    assert_eq!(second.entries[0].fields, receive_fields(1));
    assert_eq!(
        blocked_rows(&client_rows(&mut admin).await).len(),
        lanes - 1
    );
    abort_owners(&mut owners).await;
    wait_for_blocked(&mut admin, 0).await;
}
