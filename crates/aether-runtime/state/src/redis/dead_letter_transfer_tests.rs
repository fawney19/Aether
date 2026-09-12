use super::*;

type TransferTestConnection = ::redis::aio::MultiplexedConnection;

const TRANSFER_GROUP: &str = "transfer-workers";
const TRANSFER_USER: &str = "transfer-worker";

async fn transfer_runtime(
    protocol: &str,
) -> Option<(TestRedisServer, RuntimeState, TransferTestConnection)> {
    let Some(server) = TestRedisServer::start().await else {
        eprintln!(
            "dead letter transfer {protocol} skipped: isolated Redis fixture unavailable; check AETHER_REDIS_SERVER_BIN"
        );
        return None;
    };
    let mut admin = redis_test_connection(&server.redis_url).await;
    ::redis::cmd("ACL")
        .arg("SETUSER")
        .arg(TRANSFER_USER)
        .arg("on")
        .arg(">transfer-test-password")
        .arg("~*")
        .arg("+@all")
        .query_async::<()>(&mut admin)
        .await
        .expect("transfer test user");
    let runtime = RuntimeState::redis_with_blocking_stream_lanes(
        RedisClientConfig {
            url: format!(
                "redis://{TRANSFER_USER}:transfer-test-password@127.0.0.1:{}/5?protocol={protocol}",
                server.port
            ),
            key_prefix: Some(format!("transfer-{protocol}")),
        },
        Some(5_000),
        Some(4),
    )
    .await
    .expect("authenticated transfer runtime");
    ::redis::cmd("SELECT")
        .arg(5)
        .query_async::<()>(&mut admin)
        .await
        .expect("transfer admin database");
    eprintln!(
        "dead letter transfer fixture ready: protocol={protocol} db=5 port={} authenticated=true",
        server.port
    );
    Some((server, runtime, admin))
}

fn transfer_source_fields() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "payload".to_string(),
            "malformed\r\n\"quoted\"\\\u{4e2d}\u{6587}\0".to_string(),
        ),
        ("legacy".to_string(), "retain every field".to_string()),
        (
            String::new(),
            "empty field name is valid Redis data".to_string(),
        ),
    ])
}

fn transfer_archive_fields(entry: &RuntimeQueueEntry) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "payload".to_string(),
            serde_json::json!({
                "entry_id": entry.id,
                "fields": entry.fields,
                "error": "invalid record\r\n\"details\"\\\u{4e2d}\u{6587}"
            })
            .to_string(),
        ),
        ("archive_version".to_string(), "1".to_string()),
    ])
}

async fn seed_transfer_entry(runtime: &RuntimeState, source: &str) -> RuntimeQueueEntry {
    RuntimeQueueStore::ensure_consumer_group(runtime, source, TRANSFER_GROUP, "0-0")
        .await
        .expect("source consumer group");
    let id = RuntimeQueueStore::append_fields_with_maxlen(
        runtime,
        source,
        &transfer_source_fields(),
        None,
    )
    .await
    .expect("source append");
    let mut entries =
        RuntimeQueueStore::read_group(runtime, source, TRANSFER_GROUP, "owner", 1, None)
            .await
            .expect("pending source entry");
    assert_eq!(entries.len(), 1);
    let entry = entries.pop().expect("one entry");
    assert_eq!(entry.id, id);
    assert_eq!(entry.fields, transfer_source_fields());
    entry
}

async fn transfer_entries(
    admin: &mut TransferTestConnection,
    stream: &str,
) -> Vec<RuntimeQueueEntry> {
    let rows = ::redis::cmd("XRANGE")
        .arg(stream)
        .arg("-")
        .arg("+")
        .query_async::<::redis::streams::StreamRangeReply>(admin)
        .await
        .expect("inspect transfer stream");
    rows.ids
        .into_iter()
        .map(|row| RuntimeQueueEntry {
            id: row.id,
            fields: row
                .map
                .into_iter()
                .map(|(field, value)| {
                    let value =
                        ::redis::from_redis_value::<String>(&value).expect("string field value");
                    (field, value)
                })
                .collect(),
        })
        .collect()
}

async fn transfer_pending(
    admin: &mut TransferTestConnection,
    source: &str,
) -> Vec<(String, String, u64, u64)> {
    ::redis::cmd("XPENDING")
        .arg(source)
        .arg(TRANSFER_GROUP)
        .arg("-")
        .arg("+")
        .arg(16)
        .query_async(admin)
        .await
        .expect("inspect transfer pending entries")
}

async fn transfer_entry(
    runtime: &RuntimeState,
    source: &str,
    entry: &RuntimeQueueEntry,
    destination: &str,
    fields: &BTreeMap<String, String>,
) -> Result<RuntimeQueueTransferOutcome, DataLayerError> {
    RuntimeQueueStore::try_transfer_pending_to_stream(
        runtime,
        source,
        TRANSFER_GROUP,
        &entry.id,
        destination,
        fields,
    )
    .await
    .map(|outcome| outcome.expect("Redis implements atomic transfer"))
}

async fn assert_transfer_source_unchanged(
    admin: &mut TransferTestConnection,
    source: &str,
    entry: &RuntimeQueueEntry,
) {
    assert_eq!(
        transfer_entries(admin, source).await.as_slice(),
        std::slice::from_ref(entry)
    );
    let pending = transfer_pending(admin, source).await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0, entry.id);
    assert_eq!(pending[0].1, "owner");
}

async fn assert_transfer_completed(
    admin: &mut TransferTestConnection,
    source: &str,
    destination: &str,
    fields: &BTreeMap<String, String>,
) {
    assert!(transfer_entries(admin, source).await.is_empty());
    assert!(transfer_pending(admin, source).await.is_empty());
    let archived = transfer_entries(admin, destination).await;
    assert_eq!(archived.len(), 1);
    assert_eq!(&archived[0].fields, fields);
}

#[tokio::test]
async fn redis_dead_letter_transfer_concurrent_consumers_archive_exactly_once() {
    for protocol in ["resp2", "resp3"] {
        let Some((_server, runtime, mut admin)) = transfer_runtime(protocol).await else {
            return;
        };
        let source = "usage:{transfer}:concurrent";
        let destination = "usage:{transfer}:concurrent:dlq";
        let entry = seed_transfer_entry(&runtime, source).await;
        let fields = transfer_archive_fields(&entry);
        let barrier = Arc::new(tokio::sync::Barrier::new(16));
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let runtime = runtime.clone();
            let entry = entry.clone();
            let fields = fields.clone();
            let barrier = Arc::clone(&barrier);
            tasks.spawn(async move {
                barrier.wait().await;
                transfer_entry(&runtime, source, &entry, destination, &fields)
                    .await
                    .expect("concurrent transfer")
            });
        }
        let mut transferred_ids = Vec::new();
        let mut not_pending = 0;
        while let Some(result) = tasks.join_next().await {
            match result.expect("transfer task") {
                RuntimeQueueTransferOutcome::Transferred {
                    destination_id,
                    acked,
                    deleted,
                } => {
                    assert_eq!((acked, deleted), (1, 1));
                    transferred_ids.push(destination_id);
                }
                RuntimeQueueTransferOutcome::NotPending => not_pending += 1,
            }
        }
        assert_eq!(not_pending, 15);
        assert_eq!(transferred_ids.len(), 1);
        assert_transfer_completed(&mut admin, source, destination, &fields).await;
        assert_eq!(
            transfer_entries(&mut admin, destination).await[0].id,
            transferred_ids[0]
        );
        let diagnostics = runtime.redis_diagnostics().await.unwrap().unwrap();
        let lane = diagnostics
            .lanes
            .iter()
            .find(|lane| lane.lane == "blocking_stream")
            .expect("exclusive stream lane diagnostics");
        assert!(lane.command_count >= 16);
    }
}

#[tokio::test]
async fn redis_dead_letter_transfer_retry_after_ignored_success_does_not_archive_again() {
    let Some((_server, runtime, mut admin)) = transfer_runtime("resp2").await else {
        return;
    };
    let source = "usage:{transfer}:retry";
    let destination = "usage:{transfer}:retry:dlq";
    let entry = seed_transfer_entry(&runtime, source).await;
    let fields = transfer_archive_fields(&entry);
    // Commit the operation, but discard its result as a caller missing the reply would.
    let _ = transfer_entry(&runtime, source, &entry, destination, &fields)
        .await
        .expect("first transfer commits");
    let changed_fields = BTreeMap::from([("payload".to_string(), "retry value".to_string())]);
    assert_eq!(
        transfer_entry(&runtime, source, &entry, destination, &changed_fields)
            .await
            .expect("retry is successful"),
        RuntimeQueueTransferOutcome::NotPending
    );
    assert_transfer_completed(&mut admin, source, destination, &fields).await;
}

#[tokio::test]
async fn redis_dead_letter_transfer_acl_preflight_prevents_partial_writes() {
    let Some((_server, runtime, mut admin)) = transfer_runtime("resp3").await else {
        return;
    };
    for forbidden in ["XADD", "XACK", "XDEL"] {
        let source = format!("usage:{{transfer}}:acl-{forbidden}");
        let destination = format!("{source}:dlq");
        let entry = seed_transfer_entry(&runtime, &source).await;
        let fields = transfer_archive_fields(&entry);
        ::redis::cmd("ACL")
            .arg("SETUSER")
            .arg(TRANSFER_USER)
            .arg(format!("-{forbidden}"))
            .query_async::<()>(&mut admin)
            .await
            .expect("deny one write command");
        let error = transfer_entry(&runtime, &source, &entry, &destination, &fields)
            .await
            .expect_err("denied write must fail before archiving");
        assert!(
            error
                .to_string()
                .contains(&format!("requires {forbidden} permission")),
            "expected the {forbidden} preflight error, got {error}"
        );
        assert_transfer_source_unchanged(&mut admin, &source, &entry).await;
        assert!(transfer_entries(&mut admin, &destination).await.is_empty());
        ::redis::cmd("ACL")
            .arg("SETUSER")
            .arg(TRANSFER_USER)
            .arg(format!("+{forbidden}"))
            .query_async::<()>(&mut admin)
            .await
            .expect("restore one write command");
        assert!(matches!(
            transfer_entry(&runtime, &source, &entry, &destination, &fields)
                .await
                .expect("retry after restoring permission"),
            RuntimeQueueTransferOutcome::Transferred {
                acked: 1,
                deleted: 1,
                ..
            }
        ));
        assert_eq!(
            transfer_entry(&runtime, &source, &entry, &destination, &fields)
                .await
                .expect("idempotent retry"),
            RuntimeQueueTransferOutcome::NotPending
        );
        assert_transfer_completed(&mut admin, &source, &destination, &fields).await;
    }
}

#[tokio::test]
async fn redis_dead_letter_transfer_invalid_state_preserves_source_until_repaired() {
    let Some((_server, runtime, mut admin)) = transfer_runtime("resp2").await else {
        return;
    };
    let source = "usage:{transfer}:invalid";
    let destination = "usage:{transfer}:invalid:dlq";
    let entry = seed_transfer_entry(&runtime, source).await;
    let fields = transfer_archive_fields(&entry);
    ::redis::cmd("SET")
        .arg(destination)
        .arg("existing non-stream data")
        .query_async::<()>(&mut admin)
        .await
        .expect("wrong-type destination");
    let error = transfer_entry(&runtime, source, &entry, destination, &fields)
        .await
        .expect_err("wrong type must not acknowledge source");
    assert!(error.to_string().contains("WRONGTYPE"));
    assert_transfer_source_unchanged(&mut admin, source, &entry).await;
    assert_eq!(
        ::redis::cmd("GET")
            .arg(destination)
            .query_async::<String>(&mut admin)
            .await
            .unwrap(),
        "existing non-stream data"
    );
    ::redis::cmd("DEL")
        .arg(destination)
        .query_async::<usize>(&mut admin)
        .await
        .expect("repair destination type");

    let error = RuntimeQueueStore::try_transfer_pending_to_stream(
        &runtime,
        source,
        "missing-group",
        &entry.id,
        destination,
        &fields,
    )
    .await
    .expect_err("missing group must fail before archiving");
    assert!(error.to_string().contains("NOGROUP"));
    for invalid_id in [
        "",
        "-",
        "+",
        "1",
        "01-0",
        "1-00",
        "(1-0",
        "1-+0",
        "18446744073709551616-0",
    ] {
        assert!(matches!(
            RuntimeQueueStore::try_transfer_pending_to_stream(
                &runtime,
                source,
                TRANSFER_GROUP,
                invalid_id,
                destination,
                &fields,
            )
            .await,
            Err(DataLayerError::InvalidInput(_))
        ));
    }
    assert!(matches!(
        transfer_entry(&runtime, source, &entry, source, &fields).await,
        Err(DataLayerError::InvalidInput(_))
    ));
    assert!(matches!(
        transfer_entry(&runtime, source, &entry, destination, &BTreeMap::new()).await,
        Err(DataLayerError::InvalidInput(_))
    ));
    assert_transfer_source_unchanged(&mut admin, source, &entry).await;
    assert!(transfer_entries(&mut admin, destination).await.is_empty());
    assert!(matches!(
        transfer_entry(&runtime, source, &entry, destination, &fields)
            .await
            .expect("valid transfer after failed attempts"),
        RuntimeQueueTransferOutcome::Transferred {
            acked: 1,
            deleted: 1,
            ..
        }
    ));
    assert_transfer_completed(&mut admin, source, destination, &fields).await;
}

#[tokio::test]
async fn redis_dead_letter_transfer_archives_retained_fields_after_source_trim() {
    for protocol in ["resp2", "resp3"] {
        let Some((_server, runtime, mut admin)) = transfer_runtime(protocol).await else {
            return;
        };
        let source = "usage:{transfer}:trimmed";
        let destination = "usage:{transfer}:trimmed:dlq";
        let entry = seed_transfer_entry(&runtime, source).await;
        let fields = transfer_archive_fields(&entry);
        let trimmed = ::redis::cmd("XTRIM")
            .arg(source)
            .arg("MAXLEN")
            .arg(0)
            .query_async::<usize>(&mut admin)
            .await
            .expect("trim source body while retaining PEL");
        assert_eq!(trimmed, 1);
        assert!(transfer_entries(&mut admin, source).await.is_empty());
        assert_eq!(transfer_pending(&mut admin, source).await[0].0, entry.id);
        assert!(matches!(
            transfer_entry(&runtime, source, &entry, destination, &fields)
                .await
                .expect("pending body remains recoverable from caller fields"),
            RuntimeQueueTransferOutcome::Transferred {
                acked: 1,
                deleted: 0,
                ..
            }
        ));
        assert_eq!(
            transfer_entry(&runtime, source, &entry, destination, &fields)
                .await
                .expect("trimmed entry retry"),
            RuntimeQueueTransferOutcome::NotPending
        );
        assert_transfer_completed(&mut admin, source, destination, &fields).await;
    }
}

#[tokio::test]
async fn redis_dead_letter_transfer_preserves_ids_larger_than_lua_integer_precision() {
    let Some((_server, runtime, mut admin)) = transfer_runtime("resp3").await else {
        return;
    };
    let source = "usage:{transfer}:large-id";
    let destination = "usage:{transfer}:large-id:dlq";
    let entry_id = "9007199254740993-18446744073709551614";
    RuntimeQueueStore::ensure_consumer_group(&runtime, source, TRANSFER_GROUP, "0-0")
        .await
        .expect("large-ID source group");
    ::redis::cmd("XADD")
        .arg(source)
        .arg(entry_id)
        .arg("payload")
        .arg("retained value")
        .query_async::<String>(&mut admin)
        .await
        .expect("large stream ID");
    let mut entries =
        RuntimeQueueStore::read_group(&runtime, source, TRANSFER_GROUP, "owner", 1, None)
            .await
            .expect("read large-ID entry");
    assert_eq!(entries.len(), 1);
    let entry = entries.pop().unwrap();
    assert_eq!(entry.id, entry_id);
    let fields = transfer_archive_fields(&entry);
    assert!(matches!(
        transfer_entry(&runtime, source, &entry, destination, &fields)
            .await
            .expect("transfer exact large ID"),
        RuntimeQueueTransferOutcome::Transferred {
            acked: 1,
            deleted: 1,
            ..
        }
    ));
    assert_transfer_completed(&mut admin, source, destination, &fields).await;
}
