use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use aether_data::repository::settlement::InMemorySettlementRepository;
use aether_data_contracts::repository::settlement::{
    ReconcileUsagePolicyCostInput, StoredUsagePolicyCostReservation, StoredUsageSettlement,
    UsagePolicyCostReservationState,
};
use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UpsertUsageRecord};
use aether_data_contracts::DataLayerError;
use aether_runtime_state::RuntimeQueueStore;
use async_trait::async_trait;
use serde_json::json;

use super::sample_usage as base_usage;
use crate::settlement::{UsageSettlementInput, UsageSettlementWriter};
use crate::worker::write_event_record;
use crate::{
    UsageBillingEventEnricher, UsageEvent, UsageEventData, UsageEventType, UsageRecordWriter,
    UsageRuntime, UsageRuntimeAccess, UsageRuntimeConfig,
};

const RESERVATION_TOKEN: &str = "550e8400-e29b-41d4-a716-446655440000";

fn sample_usage() -> StoredRequestUsageAudit {
    let mut usage = base_usage();
    usage.request_metadata = Some(json!({"plan_usage_reservation_token": RESERVATION_TOKEN}));
    usage
}

fn runtime() -> UsageRuntime {
    UsageRuntime::new(UsageRuntimeConfig {
        enabled: true,
        ..Default::default()
    })
    .unwrap()
}

#[derive(Default)]
enum ReconcileResponse {
    #[default]
    Exact,
    Missing,
    Changed(fn(&mut StoredUsagePolicyCostReservation)),
    Error,
}

#[derive(Default)]
struct ReuseStore {
    repository: Option<InMemorySettlementRepository>,
    response: ReconcileResponse,
    stored_override: Option<StoredRequestUsageAudit>,
    fail_next_upsert: AtomicBool,
    upserts: AtomicUsize,
    reconciliations: Mutex<Vec<ReconcileUsagePolicyCostInput>>,
    settlements: Mutex<Vec<UsageSettlementInput>>,
}

#[async_trait]
impl UsageSettlementWriter for ReuseStore {
    fn has_usage_settlement_writer(&self) -> bool {
        true
    }

    async fn reconcile_usage_policy_cost(
        &self,
        input: ReconcileUsagePolicyCostInput,
    ) -> Result<Option<StoredUsagePolicyCostReservation>, DataLayerError> {
        input.validate()?;
        self.reconciliations.lock().unwrap().push(input.clone());
        tokio::task::yield_now().await;
        if let Some(repository) = self.repository.as_ref() {
            return aether_data_contracts::repository::settlement::SettlementWriteRepository::reconcile_usage_policy_cost(repository, input).await;
        }
        let mut stored = StoredUsagePolicyCostReservation {
            request_id: input.request_id,
            subject_id: input.subject_id,
            reservation_token: input.reservation_token,
            admitted_at_unix_secs: 100,
            reserved_cost_units: 100_000_000,
            actual_cost_units: Some(input.actual_cost_units),
            state: input.terminal_state,
            reservation_expires_at_unix_secs: 500,
            retain_until_unix_secs: 1_000,
            finalized_at_unix_secs: Some(input.finalized_at_unix_secs),
        };
        match self.response {
            ReconcileResponse::Exact => {}
            ReconcileResponse::Missing => return Ok(None),
            ReconcileResponse::Changed(change) => change(&mut stored),
            ReconcileResponse::Error => {
                return Err(DataLayerError::TimedOut("reconciliation".to_string()));
            }
        }
        Ok(Some(stored))
    }

    async fn settle_usage(
        &self,
        input: UsageSettlementInput,
    ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
        self.settlements.lock().unwrap().push(input.clone());
        if let Some(repository) = self.repository.as_ref() {
            return aether_data_contracts::repository::settlement::SettlementWriteRepository::settle_usage(repository, input).await;
        }
        Ok(None)
    }
}

#[async_trait]
impl UsageRecordWriter for ReuseStore {
    async fn upsert_usage_record(
        &self,
        record: UpsertUsageRecord,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        self.upserts.fetch_add(1, Ordering::Relaxed);
        if self.fail_next_upsert.swap(false, Ordering::Relaxed) {
            return Err(DataLayerError::TimedOut("upsert".to_string()));
        }
        if let Some(stored) = self.stored_override.as_ref() {
            return Ok(Some(stored.clone()));
        }
        let mut stored = sample_usage();
        stored.request_id = record.request_id;
        stored.user_id = record.user_id;
        stored.api_key_id = record.api_key_id;
        stored.provider_id = record.provider_id;
        stored.status = record.status;
        stored.billing_status = record.billing_status;
        stored.total_cost_usd = record.total_cost_usd.unwrap_or_default();
        stored.actual_total_cost_usd = record.actual_total_cost_usd.unwrap_or_default();
        stored.request_metadata = record.request_metadata;
        stored.updated_at_unix_secs = record.updated_at_unix_secs;
        stored.finalized_at_unix_secs = record.finalized_at_unix_secs;
        Ok(Some(stored))
    }
}

#[async_trait]
impl UsageBillingEventEnricher for ReuseStore {
    async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
        Ok(())
    }
}

impl UsageRuntimeAccess for ReuseStore {
    fn has_usage_writer(&self) -> bool {
        true
    }

    fn has_usage_worker_queue(&self) -> bool {
        false
    }

    fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
        None
    }
}

fn event() -> UsageEvent {
    let mut event = UsageEvent::new(
        UsageEventType::Completed,
        "req-1",
        UsageEventData {
            user_id: Some("user-1".to_string()),
            api_key_id: Some("key-1".to_string()),
            provider_name: "openai".to_string(),
            model: "gpt-5".to_string(),
            total_cost_usd: Some(1.25),
            actual_total_cost_usd: Some(0.75),
            request_metadata: Some(json!({"plan_usage_reservation_token": RESERVATION_TOKEN})),
            ..Default::default()
        },
    );
    event.timestamp_ms = 200_999;
    event
}

async fn write(store: &ReuseStore, event: UsageEvent, direct: bool) {
    if direct {
        runtime().record_terminal_event_direct(store, event).await;
    } else {
        write_event_record(store, &event).await.unwrap();
    }
}

#[tokio::test]
async fn worker_and_direct_writes_reuse_confirmed_reservation_and_still_settle_wallet() {
    for direct in [false, true] {
        let store = ReuseStore::default();
        write(&store, event(), direct).await;
        assert_eq!(store.upserts.load(Ordering::Relaxed), 1);
        let reconciliations = store.reconciliations.lock().unwrap();
        assert_eq!(reconciliations.len(), 1, "direct={direct}");
        assert_eq!(reconciliations[0].actual_cost_units, 75_000_000);
        assert_eq!(reconciliations[0].finalized_at_unix_secs, 200);
        let settlements = store.settlements.lock().unwrap();
        assert_eq!(settlements.len(), 1);
        assert_eq!(settlements[0].request_id, "req-1");
        assert_eq!(settlements[0].actual_total_cost_usd, 0.75);
    }
}

#[tokio::test]
async fn missing_or_different_reconciliation_results_keep_stored_usage_reconciliation() {
    let changes: [fn(&mut StoredUsagePolicyCostReservation); 9] = [
        |row| row.request_id = "other-request".to_string(),
        |row| row.subject_id = "other-user".to_string(),
        |row| row.reservation_token = "other-token".to_string(),
        |row| row.actual_cost_units = Some(1),
        |row| row.actual_cost_units = None,
        |row| row.state = UsagePolicyCostReservationState::Reserved,
        |row| row.state = UsagePolicyCostReservationState::Released,
        |row| row.finalized_at_unix_secs = Some(199),
        |row| row.finalized_at_unix_secs = None,
    ];
    for direct in [false, true] {
        for response in std::iter::once(ReconcileResponse::Missing)
            .chain(changes.into_iter().map(ReconcileResponse::Changed))
        {
            let store = ReuseStore {
                response,
                ..Default::default()
            };
            write(&store, event(), direct).await;
            assert_eq!(store.reconciliations.lock().unwrap().len(), 2);
            assert_eq!(store.settlements.lock().unwrap().len(), 1);
        }
    }
}

#[tokio::test]
async fn changed_stored_usage_is_reconciled_using_its_own_identity_cost_and_terminal_state() {
    let changes: [fn(&mut StoredRequestUsageAudit); 6] = [
        |row| row.request_id = "other-request".to_string(),
        |row| row.user_id = Some("other-user".to_string()),
        |row| {
            row.request_metadata.as_mut().unwrap()["plan_usage_reservation_token"] =
                json!("other-token")
        },
        |row| row.actual_total_cost_usd = 0.25,
        |row| row.status = "failed".to_string(),
        |row| row.finalized_at_unix_secs = Some(199),
    ];
    for direct in [false, true] {
        for change in changes {
            let mut stored = sample_usage();
            change(&mut stored);
            let store = ReuseStore {
                stored_override: Some(stored.clone()),
                ..Default::default()
            };
            write(&store, event(), direct).await;
            let reconciliations = store.reconciliations.lock().unwrap();
            assert_eq!(reconciliations.len(), 2);
            assert_eq!(reconciliations[1].request_id, stored.request_id);
            assert_eq!(
                reconciliations[1].subject_id,
                stored.user_id.as_ref().unwrap().as_str()
            );
            assert_eq!(
                reconciliations[1].reservation_token,
                stored.request_metadata.as_ref().unwrap()["plan_usage_reservation_token"]
                    .as_str()
                    .unwrap()
            );
            assert_ne!(reconciliations[0], reconciliations[1]);
            let settlements = store.settlements.lock().unwrap();
            assert_eq!(settlements.len(), 1);
            assert_eq!(
                settlements[0].actual_total_cost_usd,
                stored.actual_total_cost_usd
            );
            assert_eq!(settlements[0].status, stored.status);
        }
    }
}

#[tokio::test]
async fn cancellation_release_billable_cancellation_and_zero_cost_preserve_settlement_rules() {
    for direct in [false, true] {
        for (event_type, billable_cancel, cost, terminal_state, wallets) in [
            (
                UsageEventType::Failed,
                false,
                0.0,
                UsagePolicyCostReservationState::Released,
                0,
            ),
            (
                UsageEventType::Cancelled,
                false,
                0.75,
                UsagePolicyCostReservationState::Released,
                0,
            ),
            (
                UsageEventType::Cancelled,
                true,
                0.75,
                UsagePolicyCostReservationState::Finalized,
                1,
            ),
            (
                UsageEventType::Completed,
                false,
                0.0,
                UsagePolicyCostReservationState::Finalized,
                1,
            ),
        ] {
            let store = ReuseStore::default();
            let mut event = event();
            event.event_type = event_type;
            event.data.actual_total_cost_usd = Some(cost);
            event.data.request_metadata.as_mut().unwrap()["cancelled_request_fee"] =
                json!(billable_cancel);
            write(&store, event, direct).await;
            let reconciliations = store.reconciliations.lock().unwrap();
            assert_eq!(reconciliations.len(), 1);
            assert_eq!(reconciliations[0].terminal_state, terminal_state);
            assert_eq!(
                reconciliations[0].actual_cost_units,
                if billable_cancel { 75_000_000 } else { 0 }
            );
            assert_eq!(store.settlements.lock().unwrap().len(), wallets);
        }
    }
}

#[tokio::test]
async fn reconciliation_failure_stops_both_writes_before_upsert_and_wallet_settlement() {
    for direct in [false, true] {
        let store = ReuseStore {
            response: ReconcileResponse::Error,
            ..Default::default()
        };
        if direct {
            write(&store, event(), true).await;
        } else {
            assert!(write_event_record(&store, &event()).await.is_err());
        }
        assert_eq!(store.reconciliations.lock().unwrap().len(), 1);
        assert_eq!(store.upserts.load(Ordering::Relaxed), 0);
        assert!(store.settlements.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn retry_after_upsert_failure_reconciles_again_before_settling() {
    for direct in [false, true] {
        let store = ReuseStore {
            fail_next_upsert: AtomicBool::new(true),
            ..Default::default()
        };
        if direct {
            write(&store, event(), true).await;
        } else {
            assert!(write_event_record(&store, &event()).await.is_err());
        }
        assert!(store.settlements.lock().unwrap().is_empty());
        write(&store, event(), direct).await;
        assert_eq!(store.reconciliations.lock().unwrap().len(), 2);
        assert_eq!(store.upserts.load(Ordering::Relaxed), 2);
        assert_eq!(store.settlements.lock().unwrap().len(), 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_same_user_worker_and_direct_writes_each_reconcile_once() {
    const REQUESTS: usize = 256;
    let store = Arc::new(ReuseStore::default());
    let runtime = runtime();
    let barrier = Arc::new(tokio::sync::Barrier::new(REQUESTS));
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..REQUESTS {
        let store = store.clone();
        let runtime = runtime.clone();
        let barrier = barrier.clone();
        tasks.spawn(async move {
            let mut event = event();
            event.request_id = format!("reuse-concurrent-{index}");
            event.data.request_metadata.as_mut().unwrap()["plan_usage_reservation_token"] =
                json!(format!("550e8400-e29b-41d4-a716-{index:012x}"));
            barrier.wait().await;
            if index % 2 == 0 {
                runtime
                    .record_terminal_event_direct(store.as_ref(), event)
                    .await;
            } else {
                write_event_record(store.as_ref(), &event).await.unwrap();
            }
        });
    }
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
    })
    .await
    .unwrap();
    let reconciliations = store.reconciliations.lock().unwrap();
    assert_eq!(reconciliations.len(), REQUESTS);
    let unique_tokens: std::collections::HashSet<_> = reconciliations
        .iter()
        .map(|input| &input.reservation_token)
        .collect();
    assert_eq!(unique_tokens.len(), REQUESTS);
    assert_eq!(store.upserts.load(Ordering::Relaxed), REQUESTS);
    let settlements = store.settlements.lock().unwrap();
    assert_eq!(settlements.len(), REQUESTS);
    let unique_requests: std::collections::HashSet<_> =
        settlements.iter().map(|input| &input.request_id).collect();
    assert_eq!(unique_requests.len(), REQUESTS);
    assert_eq!(
        settlements
            .iter()
            .map(|input| input.actual_total_cost_usd)
            .sum::<f64>(),
        REQUESTS as f64 * 0.75
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_duplicate_delivery_debits_real_memory_wallet_only_once() {
    use aether_data::repository::wallet::{
        InMemoryWalletRepository, StoredWalletSnapshot, WalletLookupKey, WalletReadRepository,
    };
    use aether_data_contracts::repository::settlement::{
        ReserveUsagePolicyCostInput, ReserveUsagePolicyCostOutcome, SettlementWriteRepository,
        UsagePolicyCostWindow,
    };

    let wallet = StoredWalletSnapshot::new(
        "wallet-1".to_string(),
        Some("user-1".to_string()),
        None,
        10.0,
        2.0,
        "finite".to_string(),
        "USD".to_string(),
        "active".to_string(),
        0.0,
        0.0,
        0.0,
        0.0,
        100,
    )
    .unwrap();
    let wallets = Arc::new(InMemoryWalletRepository::seed([wallet]));
    let repository = InMemorySettlementRepository::from_wallet_repository(wallets.clone());
    let reservation = ReserveUsagePolicyCostInput {
        request_id: "req-1".to_string(),
        subject_id: "user-1".to_string(),
        reservation_token: RESERVATION_TOKEN.to_string(),
        admitted_at_unix_secs: 100,
        reserved_cost_units: 100_000_000,
        reservation_expires_at_unix_secs: 500,
        retain_until_unix_secs: 1_000,
        windows: vec![UsagePolicyCostWindow {
            window_id: "window-1".to_string(),
            starts_at_unix_secs: 0,
            ends_at_unix_secs: 1_000,
            limit_cost_units: 1_000_000_000,
        }],
    };
    assert!(matches!(
        repository
            .reserve_usage_policy_cost(reservation.clone())
            .await
            .unwrap(),
        ReserveUsagePolicyCostOutcome::Allowed { .. }
    ));
    let store = Arc::new(ReuseStore {
        repository: Some(repository),
        ..Default::default()
    });
    let runtime = runtime();
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..32 {
        let store = store.clone();
        let runtime = runtime.clone();
        tasks.spawn(async move {
            if index % 2 == 0 {
                write_event_record(store.as_ref(), &event()).await.unwrap();
            } else {
                runtime
                    .record_terminal_event_direct(store.as_ref(), event())
                    .await;
            }
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
    assert_eq!(store.reconciliations.lock().unwrap().len(), 32);
    assert_eq!(store.settlements.lock().unwrap().len(), 32);
    let wallet = wallets
        .find(WalletLookupKey::UserId("user-1"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wallet.balance + wallet.gift_balance, 11.25);
    assert_eq!(wallet.total_consumed, 0.75);
    assert!(matches!(
        store
            .repository
            .as_ref()
            .unwrap()
            .reserve_usage_policy_cost(reservation)
            .await
            .unwrap(),
        ReserveUsagePolicyCostOutcome::AlreadyTerminal {
            state: UsagePolicyCostReservationState::Finalized
        }
    ));
}
