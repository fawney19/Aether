use aether_data_contracts::repository::settlement::{StoredUsageSettlement, UsageSettlementInput};
use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
use aether_data_contracts::{DataLayerError, DataLayerError::InvalidInput};
use async_trait::async_trait;

#[async_trait]
pub trait UsageSettlementWriter: Send + Sync {
    fn has_usage_settlement_writer(&self) -> bool;

    async fn settle_usage(
        &self,
        input: UsageSettlementInput,
    ) -> Result<Option<StoredUsageSettlement>, DataLayerError>;
}

pub async fn settle_usage_if_needed(
    writer: &dyn UsageSettlementWriter,
    usage: &StoredRequestUsageAudit,
) -> Result<(), DataLayerError> {
    if !writer.has_usage_settlement_writer() || usage.billing_status != "pending" {
        return Ok(());
    }
    if !matches!(usage.status.as_str(), "completed" | "failed" | "cancelled") {
        return Ok(());
    }

    let finalized_at_unix_secs = usage
        .finalized_at_unix_secs
        .or(Some(usage.updated_at_unix_secs));
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

fn usage_api_key_is_standalone(usage: &StoredRequestUsageAudit) -> bool {
    usage
        .request_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("api_key_is_standalone"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
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
    use std::sync::Mutex;

    use super::{settle_usage_if_needed, UsageSettlementWriter};
    use aether_data_contracts::repository::settlement::UsageSettlementInput;
    use aether_data_contracts::repository::usage::StoredRequestUsageAudit;
    use async_trait::async_trait;
    use serde_json::json;

    #[derive(Default)]
    struct TestSettlementWriter {
        has_writer: bool,
        inputs: Mutex<Vec<UsageSettlementInput>>,
    }

    #[async_trait]
    impl UsageSettlementWriter for TestSettlementWriter {
        fn has_usage_settlement_writer(&self) -> bool {
            self.has_writer
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

    fn sample_usage() -> StoredRequestUsageAudit {
        StoredRequestUsageAudit::new(
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
        .expect("usage should build")
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
}
