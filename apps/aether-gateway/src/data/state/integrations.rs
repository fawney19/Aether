use aether_billing::enrich_usage_event_with_billing;
use aether_billing::BillingModelContextLookup;
use aether_data::redis::RedisStreamRunner;
use aether_data::repository::audit::RequestAuditReader;
use aether_data::repository::auth::{
    AuthApiKeyLookupKey, ResolvedAuthApiKeySnapshotReader, StoredAuthApiKeySnapshot,
};
use aether_data::DataLayerError;
use aether_data_contracts::repository::billing::StoredBillingModelContext;
use aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow;
use aether_data_contracts::repository::candidates::DecisionTrace;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::settlement::{StoredUsageSettlement, UsageSettlementInput};
use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UpsertUsageRecord};
use aether_data_contracts::repository::video_tasks::{StoredVideoTask, VideoTaskLookupKey};
use aether_usage_runtime::{
    UsageBillingEventEnricher, UsageBodyCapturePolicy, UsageEvent, UsageRecordWriter,
    UsageRequestRecordLevel, UsageRuntimeAccess, UsageSettlementWriter,
    DEFAULT_USAGE_REQUEST_BODY_CAPTURE_LIMIT_BYTES,
    DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES,
};
use aether_video_tasks_core::StoredVideoTaskReadSide;
use async_trait::async_trait;
use serde_json::Value;

use super::GatewayDataState;
use crate::data::candidate_selection::MinimalCandidateSelectionRowSource;
use crate::provider_transport::ProviderTransportSnapshotSource;

const REQUEST_RECORD_LEVEL_KEY: &str = "request_record_level";
const LEGACY_REQUEST_LOG_LEVEL_KEY: &str = "request_log_level";
const MAX_REQUEST_BODY_SIZE_KEY: &str = "max_request_body_size";
const MAX_RESPONSE_BODY_SIZE_KEY: &str = "max_response_body_size";

fn usage_request_record_level_from_value(value: Option<&Value>) -> UsageRequestRecordLevel {
    let Some(value) = value.and_then(Value::as_str).map(str::trim) else {
        return UsageRequestRecordLevel::Full;
    };

    if value.eq_ignore_ascii_case("basic")
        || value.eq_ignore_ascii_case("base")
        || value.eq_ignore_ascii_case("headers")
        || value.eq_ignore_ascii_case("minimal")
        || value.eq_ignore_ascii_case("none")
    {
        UsageRequestRecordLevel::Basic
    } else {
        UsageRequestRecordLevel::Full
    }
}

fn usage_body_capture_limit_from_value(value: Option<&Value>, default: usize) -> Option<usize> {
    match value.and_then(Value::as_u64) {
        Some(0) => None,
        Some(limit) => usize::try_from(limit).ok().filter(|limit| *limit > 0),
        None => Some(default),
    }
}

#[async_trait]
impl RequestAuditReader for GatewayDataState {
    async fn find_request_usage_audit_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        GatewayDataState::find_request_usage_by_request_id(self, request_id).await
    }

    async fn read_request_decision_trace(
        &self,
        request_id: &str,
        attempted_only: bool,
    ) -> Result<Option<DecisionTrace>, DataLayerError> {
        GatewayDataState::read_decision_trace(self, request_id, attempted_only).await
    }

    async fn read_resolved_auth_api_key_snapshot(
        &self,
        user_id: &str,
        api_key_id: &str,
        now_unix_secs: u64,
    ) -> Result<Option<aether_data::repository::auth::ResolvedAuthApiKeySnapshot>, DataLayerError>
    {
        GatewayDataState::read_auth_api_key_snapshot(self, user_id, api_key_id, now_unix_secs).await
    }
}

#[async_trait]
impl ResolvedAuthApiKeySnapshotReader for GatewayDataState {
    async fn find_stored_auth_api_key_snapshot(
        &self,
        key: AuthApiKeyLookupKey<'_>,
    ) -> Result<Option<StoredAuthApiKeySnapshot>, DataLayerError> {
        GatewayDataState::find_auth_api_key_snapshot(self, key).await
    }
}

#[async_trait]
impl StoredVideoTaskReadSide for GatewayDataState {
    async fn find_stored_video_task(
        &self,
        key: VideoTaskLookupKey<'_>,
    ) -> Result<Option<StoredVideoTask>, DataLayerError> {
        GatewayDataState::find_video_task(self, key).await
    }
}

#[async_trait]
impl ProviderTransportSnapshotSource for GatewayDataState {
    fn encryption_key(&self) -> Option<&str> {
        GatewayDataState::encryption_key(self)
    }

    async fn list_provider_catalog_providers_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogProvider>, DataLayerError> {
        GatewayDataState::list_provider_catalog_providers_by_ids(self, ids).await
    }

    async fn list_provider_catalog_endpoints_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, DataLayerError> {
        GatewayDataState::list_provider_catalog_endpoints_by_ids(self, ids).await
    }

    async fn list_provider_catalog_keys_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, DataLayerError> {
        GatewayDataState::list_provider_catalog_keys_by_ids(self, ids).await
    }
}

#[async_trait]
impl MinimalCandidateSelectionRowSource for GatewayDataState {
    async fn read_minimal_candidate_selection_rows_for_api_format_and_global_model(
        &self,
        api_format: &str,
        global_model_name: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        self.list_minimal_candidate_selection_rows(api_format, global_model_name)
            .await
    }

    async fn read_minimal_candidate_selection_rows_for_api_format(
        &self,
        api_format: &str,
    ) -> Result<Vec<StoredMinimalCandidateSelectionRow>, DataLayerError> {
        self.list_minimal_candidate_selection_rows_for_api_format(api_format)
            .await
    }
}

#[async_trait]
impl BillingModelContextLookup for GatewayDataState {
    async fn find_billing_model_context_by_model_id(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        model_id: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        GatewayDataState::find_billing_model_context_by_model_id(
            self,
            provider_id,
            provider_api_key_id,
            model_id,
        )
        .await
    }

    async fn find_billing_model_context(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        global_model_name: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        GatewayDataState::find_billing_model_context(
            self,
            provider_id,
            provider_api_key_id,
            global_model_name,
        )
        .await
    }
}

#[async_trait]
impl UsageSettlementWriter for GatewayDataState {
    fn has_usage_settlement_writer(&self) -> bool {
        GatewayDataState::has_settlement_writer(self)
    }

    async fn settle_usage(
        &self,
        input: UsageSettlementInput,
    ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
        GatewayDataState::settle_usage(self, input).await
    }
}

#[async_trait]
impl UsageBillingEventEnricher for GatewayDataState {
    async fn enrich_usage_event(&self, event: &mut UsageEvent) -> Result<(), DataLayerError> {
        enrich_usage_event_with_billing(self, event).await
    }
}

#[async_trait]
impl UsageRuntimeAccess for GatewayDataState {
    fn has_usage_writer(&self) -> bool {
        GatewayDataState::has_usage_writer(self)
    }

    fn has_usage_worker_runner(&self) -> bool {
        GatewayDataState::has_usage_worker_runner(self)
    }

    fn usage_worker_runner(&self) -> Option<RedisStreamRunner> {
        GatewayDataState::usage_worker_runner(self)
    }

    async fn body_capture_policy(&self) -> Result<UsageBodyCapturePolicy, DataLayerError> {
        let value = GatewayDataState::find_system_config_value(self, REQUEST_RECORD_LEVEL_KEY)
            .await?
            .or(
                GatewayDataState::find_system_config_value(self, LEGACY_REQUEST_LOG_LEVEL_KEY)
                    .await?,
            );
        let max_request_body_size =
            GatewayDataState::find_system_config_value(self, MAX_REQUEST_BODY_SIZE_KEY).await?;
        let max_response_body_size =
            GatewayDataState::find_system_config_value(self, MAX_RESPONSE_BODY_SIZE_KEY).await?;
        Ok(UsageBodyCapturePolicy {
            record_level: usage_request_record_level_from_value(value.as_ref()),
            max_request_body_bytes: usage_body_capture_limit_from_value(
                max_request_body_size.as_ref(),
                DEFAULT_USAGE_REQUEST_BODY_CAPTURE_LIMIT_BYTES,
            ),
            max_response_body_bytes: usage_body_capture_limit_from_value(
                max_response_body_size.as_ref(),
                DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES,
            ),
        })
    }
}

#[async_trait]
impl UsageRecordWriter for GatewayDataState {
    async fn upsert_usage_record(
        &self,
        record: UpsertUsageRecord,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
        GatewayDataState::upsert_usage(self, record).await
    }
}

#[cfg(test)]
mod tests {
    use aether_billing::enrich_usage_event_with_billing;
    use aether_usage_runtime::UsageRuntimeAccess;
    use serde_json::{json, Value};

    use super::GatewayDataState;
    use crate::usage::{UsageEvent, UsageEventData, UsageEventType, UsageRequestRecordLevel};

    #[tokio::test]
    async fn enriches_completed_usage_event_with_billing_snapshot() {
        let state = GatewayDataState::with_billing_reader_for_tests(
            std::sync::Arc::new(
                aether_data::repository::billing::InMemoryBillingReadRepository::seed(vec![
                    aether_data_contracts::repository::billing::StoredBillingModelContext::new(
                        "provider-1".to_string(),
                        Some("pay_as_you_go".to_string()),
                        Some("key-1".to_string()),
                        Some(serde_json::json!({"openai:chat": 0.5})),
                        Some(60),
                        "global-model-1".to_string(),
                        "gpt-5".to_string(),
                        None,
                        Some(0.02),
                        Some(serde_json::json!({"tiers":[{"up_to":null,"input_price_per_1m":3.0,"output_price_per_1m":15.0,"cache_creation_price_per_1m":3.75,"cache_read_price_per_1m":0.30}]})),
                        Some("model-1".to_string()),
                        Some("gpt-5-upstream".to_string()),
                        None,
                        None,
                        None,
                    )
                    .expect("billing context should build"),
                ]),
            ),
        );
        let mut event = UsageEvent::new(
            UsageEventType::Completed,
            "req-billing-1",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                provider_id: Some("provider-1".to_string()),
                provider_api_key_id: Some("key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                input_tokens: Some(1_000),
                output_tokens: Some(500),
                cache_read_input_tokens: Some(100),
                status_code: Some(200),
                ..UsageEventData::default()
            },
        );

        enrich_usage_event_with_billing(&state, &mut event)
            .await
            .expect("billing should succeed");

        assert!(event.data.total_cost_usd.unwrap_or_default() > 0.0);
        assert!(event.data.actual_total_cost_usd.unwrap_or_default() > 0.0);
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("billing_snapshot"))
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str),
            Some("complete")
        );
    }

    #[tokio::test]
    async fn usage_runtime_access_reads_base_request_record_level_as_basic() {
        let state = GatewayDataState::disabled().with_system_config_values_for_tests([(
            "request_record_level".to_string(),
            json!("base"),
        )]);

        let level = UsageRuntimeAccess::request_record_level(&state)
            .await
            .expect("request record level should read");

        assert_eq!(level, UsageRequestRecordLevel::Basic);
    }

    #[tokio::test]
    async fn usage_runtime_access_falls_back_to_legacy_request_log_level_alias() {
        let state = GatewayDataState::disabled().with_system_config_values_for_tests([(
            "request_log_level".to_string(),
            json!("headers"),
        )]);

        let level = UsageRuntimeAccess::request_record_level(&state)
            .await
            .expect("legacy request log level should read");

        assert_eq!(level, UsageRequestRecordLevel::Basic);
    }

    #[tokio::test]
    async fn usage_runtime_access_defaults_missing_request_record_level_to_full() {
        let state = GatewayDataState::disabled();

        let level = UsageRuntimeAccess::request_record_level(&state)
            .await
            .expect("missing request record level should fall back");

        assert_eq!(level, UsageRequestRecordLevel::Full);
    }

    #[tokio::test]
    async fn usage_runtime_access_reads_body_capture_limits_from_system_config() {
        let state = GatewayDataState::disabled().with_system_config_values_for_tests([
            ("max_request_body_size".to_string(), json!(1234)),
            ("max_response_body_size".to_string(), json!(5678)),
        ]);

        let policy = UsageRuntimeAccess::body_capture_policy(&state)
            .await
            .expect("body capture policy should read");

        assert_eq!(policy.record_level, UsageRequestRecordLevel::Full);
        assert_eq!(policy.max_request_body_bytes, Some(1234));
        assert_eq!(policy.max_response_body_bytes, Some(5678));
    }

    #[tokio::test]
    async fn usage_runtime_access_treats_zero_body_capture_limit_as_unbounded() {
        let state = GatewayDataState::disabled().with_system_config_values_for_tests([
            ("max_request_body_size".to_string(), json!(0)),
            ("max_response_body_size".to_string(), json!(0)),
        ]);

        let policy = UsageRuntimeAccess::body_capture_policy(&state)
            .await
            .expect("body capture policy should read");

        assert_eq!(policy.max_request_body_bytes, None);
        assert_eq!(policy.max_response_body_bytes, None);
    }
}
