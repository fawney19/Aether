use std::sync::{Arc, OnceLock};

use aether_data_contracts::repository::billing::nonnegative_usd_to_usage_policy_cost_units;
use aether_data_contracts::repository::settlement::{
    ReconcileUsagePolicyCostInput, StoredUsagePolicyCostReservation, StoredUsageSettlement,
    UsagePolicyCostReservationState, UsageSettlementInput,
};
use aether_data_contracts::repository::usage::PLAN_USAGE_RESERVATION_DEFERRED_METADATA_KEY;
use aether_data_contracts::repository::usage::{
    cancelled_request_fee_is_billable, StoredRequestUsageAudit,
};
use aether_data_contracts::{DataLayerError, DataLayerError::InvalidInput};
use async_trait::async_trait;

use crate::event::{UsageEvent, UsageEventType};
use crate::keyed_lock::KeyedAsyncLockPool;

#[async_trait]
pub trait UsageSettlementWriter: Send + Sync {
    fn has_usage_settlement_writer(&self) -> bool;

    async fn reconcile_usage_policy_cost(
        &self,
        _input: ReconcileUsagePolicyCostInput,
    ) -> Result<Option<StoredUsagePolicyCostReservation>, DataLayerError> {
        Ok(None)
    }

    async fn settle_usage(
        &self,
        input: UsageSettlementInput,
    ) -> Result<Option<StoredUsageSettlement>, DataLayerError>;
}

pub async fn reconcile_usage_policy_cost_for_event(
    writer: &dyn UsageSettlementWriter,
    event: &UsageEvent,
) -> Result<(), DataLayerError> {
    reconcile_usage_policy_cost_for_event_with_result(writer, event)
        .await
        .map(|_| ())
}

pub(crate) struct ReconciledUsagePolicyCost(ReconcileUsagePolicyCostInput);

pub(crate) async fn reconcile_usage_policy_cost_for_event_with_result(
    writer: &dyn UsageSettlementWriter,
    event: &UsageEvent,
) -> Result<Option<ReconciledUsagePolicyCost>, DataLayerError> {
    if !writer.has_usage_settlement_writer() {
        return Ok(None);
    }
    let terminal_state = match event.event_type {
        UsageEventType::Completed => UsagePolicyCostReservationState::Finalized,
        UsageEventType::Cancelled
            if cancelled_request_fee_is_billable(event.data.request_metadata.as_ref()) =>
        {
            UsagePolicyCostReservationState::Finalized
        }
        UsageEventType::Failed | UsageEventType::Cancelled => {
            UsagePolicyCostReservationState::Released
        }
        UsageEventType::Pending | UsageEventType::Streaming => return Ok(None),
    };
    if plan_usage_reservation_reconciliation_is_deferred(event.data.request_metadata.as_ref()) {
        return Ok(None);
    }
    let Some(subject_id) = event.data.user_id.as_deref().and_then(non_empty_trimmed) else {
        return Ok(None);
    };
    let Some(reservation_token) = event_usage_policy_reservation_token(event) else {
        return Ok(None);
    };
    let actual_cost_units = if terminal_state == UsagePolicyCostReservationState::Finalized {
        let actual_cost_usd = event.data.actual_total_cost_usd.ok_or_else(|| {
            InvalidInput(
                "completed usage event with a plan reservation token is missing actual cost"
                    .to_string(),
            )
        })?;
        nonnegative_usd_to_usage_policy_cost_units(finite_cost(actual_cost_usd)?.max(0.0))
            .ok_or_else(|| {
                InvalidInput("usage policy settlement cost exceeds the supported range".to_string())
            })?
    } else {
        0
    };

    let input = ReconcileUsagePolicyCostInput {
        request_id: event.request_id.clone(),
        subject_id: subject_id.to_string(),
        reservation_token: reservation_token.to_string(),
        actual_cost_units,
        terminal_state,
        finalized_at_unix_secs: event.timestamp_ms / 1_000,
    };
    let stored = writer.reconcile_usage_policy_cost(input.clone()).await?;
    // A successful call alone is insufficient: None or a different existing terminal
    // reservation must not suppress reconciliation of the subsequent stored usage row.
    let matches = stored.is_some_and(|stored| {
        stored.request_id == input.request_id
            && stored.subject_id == input.subject_id
            && stored.reservation_token == input.reservation_token
            && stored.actual_cost_units == Some(input.actual_cost_units)
            && stored.state == input.terminal_state
            && stored.finalized_at_unix_secs == Some(input.finalized_at_unix_secs)
    });
    Ok(matches.then_some(ReconciledUsagePolicyCost(input)))
}

pub async fn settle_usage_if_needed(
    writer: &dyn UsageSettlementWriter,
    usage: &StoredRequestUsageAudit,
) -> Result<(), DataLayerError> {
    settle_usage_with_reconciled_cost(writer, usage, None).await
}

pub(crate) async fn settle_usage_with_reconciled_cost(
    writer: &dyn UsageSettlementWriter,
    usage: &StoredRequestUsageAudit,
    reconciled: Option<ReconciledUsagePolicyCost>,
) -> Result<(), DataLayerError> {
    if !writer.has_usage_settlement_writer() {
        return Ok(());
    }
    if !matches!(usage.status.as_str(), "completed" | "failed" | "cancelled") {
        return Ok(());
    }

    let finalized_at_unix_secs = usage
        .finalized_at_unix_secs
        .or(Some(usage.updated_at_unix_secs));
    let settlement_key = usage_settlement_lock_key_for_usage(usage);
    let settlement_lock = usage_settlement_lock(&settlement_key);
    let _guard = settlement_lock.lock().await;

    // Cost reservations are tied to a server-issued per-request token. Legacy usage rows do not
    // have that token, so they must continue through wallet settlement without touching a cost
    // reservation selected only by the client-visible request id.
    if !plan_usage_reservation_reconciliation_is_deferred(usage.request_metadata.as_ref()) {
        if let (Some(subject_id), Some(reservation_token)) = (
            usage.user_id.as_deref().and_then(non_empty_trimmed),
            usage_policy_reservation_token(usage),
        ) {
            let (terminal_state, actual_cost_units) = if usage.status == "completed"
                || (usage.status == "cancelled"
                    && cancelled_request_fee_is_billable(usage.request_metadata.as_ref()))
            {
                (
                    UsagePolicyCostReservationState::Finalized,
                    nonnegative_usd_to_usage_policy_cost_units(
                        finite_cost(usage.actual_total_cost_usd)?.max(0.0),
                    )
                    .ok_or_else(|| {
                        InvalidInput(
                            "usage policy settlement cost exceeds the supported range".to_string(),
                        )
                    })?,
                )
            } else {
                (UsagePolicyCostReservationState::Released, 0)
            };
            let input = ReconcileUsagePolicyCostInput {
                request_id: usage.request_id.clone(),
                subject_id: subject_id.to_string(),
                reservation_token: reservation_token.to_string(),
                actual_cost_units,
                terminal_state,
                finalized_at_unix_secs: finalized_at_unix_secs
                    .unwrap_or(usage.updated_at_unix_secs),
            };
            if !reconciled
                .as_ref()
                .is_some_and(|previous| previous.0 == input)
            {
                let _ = writer.reconcile_usage_policy_cost(input).await?;
            }
        }
    }

    if usage.billing_status != "pending"
        || (usage.status == "cancelled"
            && !cancelled_request_fee_is_billable(usage.request_metadata.as_ref()))
    {
        return Ok(());
    }
    let input = UsageSettlementInput {
        request_id: usage.request_id.clone(),
        user_id: usage.user_id.clone(),
        api_key_id: usage.api_key_id.clone(),
        api_key_is_standalone: usage_api_key_is_standalone(usage),
        provider_id: usage.provider_id.clone(),
        status: usage.status.clone(),
        billing_status: usage.billing_status.clone(),
        total_cost_usd: finite_cost(usage.total_cost_usd)?,
        actual_total_cost_usd: finite_cost(usage.actual_total_cost_usd)?,
        finalized_at_unix_secs,
    };
    let _ = writer.settle_usage(input).await?;
    Ok(())
}

fn plan_usage_reservation_reconciliation_is_deferred(metadata: Option<&serde_json::Value>) -> bool {
    metadata
        .and_then(serde_json::Value::as_object)
        .and_then(|metadata| metadata.get(PLAN_USAGE_RESERVATION_DEFERRED_METADATA_KEY))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn usage_settlement_lock(key: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<KeyedAsyncLockPool> = OnceLock::new();
    LOCKS.get_or_init(KeyedAsyncLockPool::default).lock_for(key)
}

fn usage_settlement_lock_key_for_usage(usage: &StoredRequestUsageAudit) -> String {
    if usage_api_key_is_standalone(usage) {
        if let Some(api_key_id) = usage.api_key_id.as_deref().and_then(non_empty_trimmed) {
            return format!("api-key:{api_key_id}");
        }
    }
    if let Some(user_id) = usage.user_id.as_deref().and_then(non_empty_trimmed) {
        return format!("user:{user_id}");
    }
    if let Some(api_key_id) = usage.api_key_id.as_deref().and_then(non_empty_trimmed) {
        return format!("api-key:{api_key_id}");
    }
    format!("request:{}", usage.request_id.trim())
}

fn non_empty_trimmed(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn usage_api_key_is_standalone(usage: &StoredRequestUsageAudit) -> bool {
    usage
        .request_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("api_key_is_standalone"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn usage_policy_reservation_token(usage: &StoredRequestUsageAudit) -> Option<&str> {
    usage
        .request_metadata
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|metadata| metadata.get("plan_usage_reservation_token"))
        .and_then(serde_json::Value::as_str)
        .and_then(non_empty_trimmed)
}

fn event_usage_policy_reservation_token(event: &UsageEvent) -> Option<&str> {
    event
        .data
        .request_metadata
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|metadata| metadata.get("plan_usage_reservation_token"))
        .and_then(serde_json::Value::as_str)
        .and_then(non_empty_trimmed)
}

fn finite_cost(value: f64) -> Result<f64, DataLayerError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(InvalidInput(
            "wallet settlement cost must be finite".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    mod reconciliation_reuse {
        include!("settlement_reuse_tests.rs");
    }

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    use super::{
        reconcile_usage_policy_cost_for_event, settle_usage_if_needed, UsageSettlementWriter,
    };
    use aether_data_contracts::repository::settlement::{
        ReconcileUsagePolicyCostInput, StoredUsagePolicyCostReservation,
        UsagePolicyCostReservationState, UsageSettlementInput,
    };
    use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
    use async_trait::async_trait;
    use serde_json::json;

    use crate::{UsageEvent, UsageEventData, UsageEventType};

    #[derive(Default)]
    struct TestSettlementWriter {
        has_writer: bool,
        inputs: Mutex<Vec<UsageSettlementInput>>,
        reconciliations: Mutex<Vec<ReconcileUsagePolicyCostInput>>,
    }

    #[derive(Default)]
    struct SlowSettlementWriter {
        active: AtomicUsize,
        max_active: AtomicUsize,
        inputs: Mutex<Vec<UsageSettlementInput>>,
    }

    #[async_trait]
    impl UsageSettlementWriter for TestSettlementWriter {
        fn has_usage_settlement_writer(&self) -> bool {
            self.has_writer
        }

        async fn reconcile_usage_policy_cost(
            &self,
            input: ReconcileUsagePolicyCostInput,
        ) -> Result<Option<StoredUsagePolicyCostReservation>, aether_data_contracts::DataLayerError>
        {
            self.reconciliations
                .lock()
                .expect("reconciliation inputs lock")
                .push(input);
            Ok(None)
        }

        async fn settle_usage(
            &self,
            input: UsageSettlementInput,
        ) -> Result<
            Option<aether_data_contracts::repository::settlement::StoredUsageSettlement>,
            aether_data_contracts::DataLayerError,
        > {
            self.inputs
                .lock()
                .expect("settlement inputs lock")
                .push(input);
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for SlowSettlementWriter {
        fn has_usage_settlement_writer(&self) -> bool {
            true
        }

        async fn settle_usage(
            &self,
            input: UsageSettlementInput,
        ) -> Result<
            Option<aether_data_contracts::repository::settlement::StoredUsageSettlement>,
            aether_data_contracts::DataLayerError,
        > {
            let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
            self.max_active.fetch_max(active, Ordering::AcqRel);
            tokio::time::sleep(Duration::from_millis(30)).await;
            self.inputs
                .lock()
                .expect("settlement inputs lock")
                .push(input);
            self.active.fetch_sub(1, Ordering::AcqRel);
            Ok(None)
        }
    }

    fn sample_usage() -> StoredRequestUsageAudit {
        let mut usage = StoredRequestUsageAudit::new(
            "usage-1".to_string(),
            "req-1".to_string(),
            Some("user-1".to_string()),
            Some("key-1".to_string()),
            None,
            None,
            "openai".to_string(),
            "gpt-5".to_string(),
            None,
            Some("provider-1".to_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            false,
            0,
            0,
            0,
            1.25,
            0.75,
            Some(200),
            None,
            None,
            None,
            None,
            "completed".to_string(),
            "pending".to_string(),
            100,
            200,
            None,
        )
        .expect("usage should build");
        usage.request_metadata = Some(json!({
            "plan_usage_reservation_token": "token-1"
        }));
        usage
    }

    #[tokio::test]
    async fn settles_pending_terminal_usage() {
        let writer = TestSettlementWriter {
            has_writer: true,
            ..Default::default()
        };
        let usage = sample_usage();

        settle_usage_if_needed(&writer, &usage)
            .await
            .expect("settlement should succeed");

        let inputs = writer.inputs.lock().expect("settlement inputs lock");
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].request_id, "req-1");
        assert_eq!(inputs[0].status, "completed");
        assert_eq!(inputs[0].billing_status, "pending");
        assert_eq!(inputs[0].finalized_at_unix_secs, Some(200));
        assert_eq!(inputs[0].total_cost_usd, 1.25);
        assert_eq!(inputs[0].actual_total_cost_usd, 0.75);
        assert!(!inputs[0].api_key_is_standalone);
        drop(inputs);
        let reconciliations = writer
            .reconciliations
            .lock()
            .expect("reconciliation inputs lock");
        assert_eq!(reconciliations.len(), 1);
        assert_eq!(reconciliations[0].actual_cost_units, 75_000_000);
        assert_eq!(reconciliations[0].reservation_token, "token-1");
        assert_eq!(
            reconciliations[0].terminal_state,
            UsagePolicyCostReservationState::Finalized
        );
    }

    #[tokio::test]
    async fn releases_pending_cancelled_usage_without_wallet_settlement() {
        let writer = TestSettlementWriter {
            has_writer: true,
            ..Default::default()
        };
        let mut usage = sample_usage();
        usage.status = "cancelled".to_string();
        usage.status_code = Some(499);

        settle_usage_if_needed(&writer, &usage)
            .await
            .expect("skipped settlement should succeed");

        let inputs = writer.inputs.lock().expect("settlement inputs lock");
        assert!(inputs.is_empty());
        drop(inputs);
        let reconciliations = writer
            .reconciliations
            .lock()
            .expect("reconciliation inputs lock");
        assert_eq!(reconciliations.len(), 1);
        assert_eq!(reconciliations[0].actual_cost_units, 0);
        assert_eq!(
            reconciliations[0].terminal_state,
            UsagePolicyCostReservationState::Released
        );
    }

    #[tokio::test]
    async fn cancelled_request_fee_settles_wallet_and_finalizes_cost_reservation() {
        let writer = TestSettlementWriter {
            has_writer: true,
            ..Default::default()
        };
        let mut usage = sample_usage();
        usage.status = "cancelled".to_string();
        usage.status_code = Some(499);
        usage.request_metadata.as_mut().unwrap()["cancelled_request_fee"] = json!(true);
        settle_usage_if_needed(&writer, &usage).await.unwrap();
        let inputs = writer.inputs.lock().unwrap();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].status, "cancelled");
        assert_eq!(inputs[0].actual_total_cost_usd, usage.actual_total_cost_usd);
        let reconciliations = writer.reconciliations.lock().unwrap();
        assert_eq!(reconciliations.len(), 1);
        assert_eq!(
            reconciliations[0].terminal_state,
            UsagePolicyCostReservationState::Finalized
        );
        assert_eq!(reconciliations[0].actual_cost_units, 75_000_000);
    }

    #[tokio::test]
    async fn cancelled_request_fee_event_finalizes_cost_reservation() {
        let writer = TestSettlementWriter {
            has_writer: true,
            ..Default::default()
        };
        let event = UsageEvent::new(
            UsageEventType::Cancelled,
            "req-cancelled-fee",
            UsageEventData {
                user_id: Some("user-1".to_string()),
                actual_total_cost_usd: Some(0.01),
                request_metadata: Some(json!({
                    "cancelled_request_fee": true,
                    "plan_usage_reservation_token": "server-token"
                })),
                ..Default::default()
            },
        );
        reconcile_usage_policy_cost_for_event(&writer, &event)
            .await
            .unwrap();
        let reconciliations = writer.reconciliations.lock().unwrap();
        assert_eq!(reconciliations.len(), 1);
        assert_eq!(
            reconciliations[0].terminal_state,
            UsagePolicyCostReservationState::Finalized
        );
        assert_eq!(reconciliations[0].actual_cost_units, 1_000_000);
    }

    #[tokio::test]
    async fn releases_failed_usage_before_void_settlement() {
        let writer = TestSettlementWriter {
            has_writer: true,
            ..Default::default()
        };
        let mut usage = sample_usage();
        usage.status = "failed".to_string();

        settle_usage_if_needed(&writer, &usage)
            .await
            .expect("failed usage should settle");

        assert_eq!(
            writer.inputs.lock().expect("settlement inputs lock").len(),
            1
        );
        let reconciliations = writer
            .reconciliations
            .lock()
            .expect("reconciliation inputs lock");
        assert_eq!(
            reconciliations[0].terminal_state,
            UsagePolicyCostReservationState::Released
        );
        assert_eq!(reconciliations[0].actual_cost_units, 0);
    }

    #[tokio::test]
    async fn skips_cost_reconciliation_for_legacy_usage_without_token() {
        let writer = TestSettlementWriter {
            has_writer: true,
            ..Default::default()
        };
        let mut usage = sample_usage();
        usage.request_metadata = None;

        settle_usage_if_needed(&writer, &usage)
            .await
            .expect("legacy usage should still settle its wallet charge");

        assert_eq!(
            writer
                .reconciliations
                .lock()
                .expect("reconciliation lock")
                .len(),
            0
        );
        assert_eq!(
            writer.inputs.lock().expect("settlement inputs lock").len(),
            1
        );
    }

    #[tokio::test]
    async fn ignores_blank_reservation_token_metadata() {
        let writer = TestSettlementWriter {
            has_writer: true,
            ..Default::default()
        };
        let mut usage = sample_usage();
        usage.request_metadata = Some(json!({
            "plan_usage_reservation_token": "  "
        }));

        settle_usage_if_needed(&writer, &usage)
            .await
            .expect("blank token should be treated as legacy usage");

        assert!(writer
            .reconciliations
            .lock()
            .expect("reconciliation lock")
            .is_empty());
        assert_eq!(
            writer.inputs.lock().expect("settlement inputs lock").len(),
            1
        );
    }

    #[tokio::test]
    async fn event_reconciliation_requires_enriched_cost_and_preserves_server_token() {
        let writer = TestSettlementWriter {
            has_writer: true,
            ..Default::default()
        };
        let mut event = UsageEvent::new(
            UsageEventType::Completed,
            "shared-trace",
            UsageEventData {
                user_id: Some("user-1".to_string()),
                provider_name: "openai".to_string(),
                model: "gpt-5".to_string(),
                request_metadata: Some(json!({
                    "plan_usage_reservation_token": "server-token"
                })),
                ..UsageEventData::default()
            },
        );
        assert!(matches!(
            reconcile_usage_policy_cost_for_event(&writer, &event).await,
            Err(aether_data_contracts::DataLayerError::InvalidInput(_))
        ));
        assert!(writer
            .reconciliations
            .lock()
            .expect("reconciliations lock")
            .is_empty());

        event.data.actual_total_cost_usd = Some(1.25);
        reconcile_usage_policy_cost_for_event(&writer, &event)
            .await
            .expect("enriched terminal event should reconcile");
        let reconciliations = writer.reconciliations.lock().expect("reconciliations lock");
        assert_eq!(reconciliations.len(), 1);
        assert_eq!(reconciliations[0].request_id, "shared-trace");
        assert_eq!(reconciliations[0].reservation_token, "server-token");
        assert_eq!(reconciliations[0].actual_cost_units, 125_000_000);
    }

    #[tokio::test]
    async fn deferred_event_keeps_cost_reservation_without_requiring_actual_cost() {
        let writer = TestSettlementWriter {
            has_writer: true,
            ..Default::default()
        };
        let event = UsageEvent::new(
            UsageEventType::Completed,
            "possibly-sent-request",
            UsageEventData {
                user_id: Some("user-1".to_string()),
                provider_name: "openai".to_string(),
                model: "gpt-5".to_string(),
                request_metadata: Some(json!({
                    "plan_usage_reservation_token": "server-token",
                    "plan_usage_reservation_deferred": true
                })),
                ..UsageEventData::default()
            },
        );

        reconcile_usage_policy_cost_for_event(&writer, &event)
            .await
            .expect("deferred reconciliation should not require unknown actual cost");

        assert!(writer
            .reconciliations
            .lock()
            .expect("reconciliations lock")
            .is_empty());
    }

    #[tokio::test]
    async fn deferred_stored_usage_skips_cost_reconcile_but_still_settles_wallet() {
        let writer = TestSettlementWriter {
            has_writer: true,
            ..Default::default()
        };
        let mut usage = sample_usage();
        usage.request_metadata = Some(json!({
            "plan_usage_reservation_token": "server-token",
            "plan_usage_reservation_deferred": true
        }));

        settle_usage_if_needed(&writer, &usage)
            .await
            .expect("wallet settlement should continue");

        assert!(writer
            .reconciliations
            .lock()
            .expect("reconciliations lock")
            .is_empty());
        assert_eq!(
            writer.inputs.lock().expect("settlement inputs lock").len(),
            1
        );
    }

    #[test]
    fn deferred_metadata_requires_a_boolean_true() {
        assert!(super::plan_usage_reservation_reconciliation_is_deferred(
            Some(&json!({"plan_usage_reservation_deferred": true}))
        ));
        assert!(!super::plan_usage_reservation_reconciliation_is_deferred(
            Some(&json!({"plan_usage_reservation_deferred": "true"}))
        ));
    }

    #[tokio::test]
    async fn propagates_standalone_key_flag_from_usage_metadata() {
        let writer = TestSettlementWriter {
            has_writer: true,
            ..Default::default()
        };
        let mut usage = sample_usage();
        usage.request_metadata = Some(json!({ "api_key_is_standalone": true }));

        settle_usage_if_needed(&writer, &usage)
            .await
            .expect("settlement should succeed");

        let inputs = writer.inputs.lock().expect("settlement inputs lock");
        assert_eq!(inputs.len(), 1);
        assert!(inputs[0].api_key_is_standalone);
    }

    #[tokio::test]
    async fn skips_when_usage_is_not_pending_or_terminal() {
        let writer = TestSettlementWriter {
            has_writer: true,
            ..Default::default()
        };
        let mut usage = sample_usage();
        usage.billing_status = "settled".to_string();
        usage.status = "streaming".to_string();

        settle_usage_if_needed(&writer, &usage)
            .await
            .expect("skipped settlement should succeed");

        let inputs = writer.inputs.lock().expect("settlement inputs lock");
        assert!(inputs.is_empty());
    }

    #[tokio::test]
    async fn rejects_non_finite_costs_before_writing() {
        let writer = TestSettlementWriter {
            has_writer: true,
            ..Default::default()
        };
        let mut usage = sample_usage();
        usage.total_cost_usd = f64::NAN;

        let err = settle_usage_if_needed(&writer, &usage)
            .await
            .expect_err("non-finite costs should be rejected");

        assert!(matches!(
            err,
            aether_data_contracts::DataLayerError::InvalidInput(_)
        ));
        let inputs = writer.inputs.lock().expect("settlement inputs lock");
        assert!(inputs.is_empty());
    }

    #[tokio::test]
    async fn serializes_settlements_for_same_billing_subject() {
        let writer = SlowSettlementWriter::default();
        let mut first = sample_usage();
        first.request_id = "req-same-subject-1".to_string();
        let mut second = sample_usage();
        second.request_id = "req-same-subject-2".to_string();

        tokio::try_join!(
            settle_usage_if_needed(&writer, &first),
            settle_usage_if_needed(&writer, &second)
        )
        .expect("settlements should succeed");

        assert_eq!(writer.max_active.load(Ordering::Acquire), 1);
        assert_eq!(writer.inputs.lock().expect("inputs lock").len(), 2);
    }
}
