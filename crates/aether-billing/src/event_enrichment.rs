use aether_data_contracts::repository::billing::StoredBillingModelContext;
use aether_data_contracts::repository::usage::{
    extract_provider_cache_ttl_minutes_from_metadata, resolve_provider_cache_ttl_minutes,
    resolve_provider_service_tier_from_request_capture,
};
use aether_data_contracts::DataLayerError;
use aether_usage_runtime::{UsageEvent, UsageEventType};
use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::{
    BillingComputation, BillingModelPricingSnapshot, BillingService, BillingSnapshotStatus,
    BillingUsageInput,
};

const SETTLEMENT_SNAPSHOT_SCHEMA_VERSION: &str = "3.0";

#[async_trait]
pub trait BillingModelContextLookup: Send + Sync {
    async fn find_billing_model_context_by_model_id(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        model_id: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        let _ = (provider_id, provider_api_key_id, model_id);
        Ok(None)
    }

    async fn find_billing_model_context(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        global_model_name: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError>;
}

pub async fn enrich_usage_event_with_billing(
    data: &dyn BillingModelContextLookup,
    event: &mut UsageEvent,
) -> Result<(), DataLayerError> {
    if !matches!(event.event_type, UsageEventType::Completed) {
        event.data.total_cost_usd = Some(0.0);
        event.data.actual_total_cost_usd = Some(0.0);
        return Ok(());
    }

    let Some(provider_id) = event
        .data
        .provider_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    if let Some(model_id) = event
        .data
        .model_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(context) = data
            .find_billing_model_context_by_model_id(
                provider_id,
                event.data.provider_api_key_id.as_deref(),
                model_id,
            )
            .await?
        {
            let pricing = map_pricing_context(context);
            let computation = calculate_billing_computation(&pricing, event)?;
            apply_billing_computation(event, &pricing, computation)?;
            return Ok(());
        }
    }

    let mut first_no_rule = None;
    for lookup_name in billing_model_lookup_names(&event.data) {
        let Some(context) = data
            .find_billing_model_context(
                provider_id,
                event.data.provider_api_key_id.as_deref(),
                lookup_name,
            )
            .await?
        else {
            continue;
        };

        let pricing = map_pricing_context(context);
        let computation = calculate_billing_computation(&pricing, event)?;
        if matches!(
            computation.cost_result.status,
            BillingSnapshotStatus::NoRule
        ) {
            first_no_rule.get_or_insert((pricing, computation));
            continue;
        }
        apply_billing_computation(event, &pricing, computation)?;
        return Ok(());
    }

    if let Some((pricing, computation)) = first_no_rule {
        apply_billing_computation(event, &pricing, computation)?;
    }
    Ok(())
}

fn billing_model_lookup_names(data: &aether_usage_runtime::UsageEventData) -> Vec<&str> {
    let mut names = Vec::new();
    for value in [data.target_model.as_deref(), Some(data.model.as_str())]
        .into_iter()
        .flatten()
    {
        let value = value.trim();
        if !value.is_empty() && !names.contains(&value) {
            names.push(value);
        }
    }
    names
}

fn calculate_billing_computation(
    pricing: &BillingModelPricingSnapshot,
    event: &UsageEvent,
) -> Result<BillingComputation, DataLayerError> {
    let failed =
        event.data.status_code.unwrap_or_default() >= 400 || event.data.error_message.is_some();
    let is_image_usage = usage_event_is_image_usage(&event.data);
    let image_count = if failed {
        0
    } else {
        usage_event_image_count(&event.data).unwrap_or(0)
    };
    let request_count = if failed {
        0
    } else if is_image_usage && image_count > 0 {
        image_count
    } else {
        1
    };
    let processing_tiers = usage_event_processing_tiers(&event.data);
    let input = BillingUsageInput {
        task_type: if is_image_usage {
            "image".to_string()
        } else {
            event
                .data
                .request_type
                .clone()
                .unwrap_or_else(|| "chat".to_string())
        },
        api_format: event
            .data
            .endpoint_api_format
            .clone()
            .or_else(|| event.data.api_format.clone()),
        requested_processing_tier: processing_tiers.requested,
        // The response-reported tier remains usage audit data, but it is not authoritative for
        // pricing. Settlement follows the final request that was sent upstream.
        actual_processing_tier: None,
        request_count,
        input_tokens: event.data.input_tokens.unwrap_or_default() as i64,
        output_tokens: event.data.output_tokens.unwrap_or_default() as i64,
        cache_creation_tokens: event.data.cache_creation_input_tokens.unwrap_or_default() as i64,
        cache_creation_ephemeral_5m_tokens: event
            .data
            .cache_creation_ephemeral_5m_input_tokens
            .unwrap_or_default() as i64,
        cache_creation_ephemeral_1h_tokens: event
            .data
            .cache_creation_ephemeral_1h_input_tokens
            .unwrap_or_default() as i64,
        cache_read_tokens: event.data.cache_read_input_tokens.unwrap_or_default() as i64,
        image_count,
        image_size: usage_event_dimension_string(&event.data, "image_size"),
        image_quality: usage_event_dimension_string(&event.data, "image_quality"),
        image_output_format: usage_event_dimension_string(&event.data, "image_output_format"),
        cache_ttl_minutes: usage_event_provider_cache_ttl_minutes(&event.data)
            .or(pricing.provider_api_key_cache_ttl_minutes),
    };

    BillingService::new()
        .calculate(pricing, &input)
        .map_err(|err| {
            DataLayerError::UnexpectedValue(format!("billing calculation failed: {err}"))
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UsageEventProcessingTiers {
    requested: Option<String>,
}

fn usage_event_processing_tiers(
    data: &aether_usage_runtime::UsageEventData,
) -> UsageEventProcessingTiers {
    let requested = resolve_provider_service_tier_from_request_capture(
        data.provider_request_body.as_ref(),
        data.provider_request_body_state,
        data.request_metadata.as_ref(),
    );

    UsageEventProcessingTiers { requested }
}

fn usage_event_provider_cache_ttl_minutes(
    data: &aether_usage_runtime::UsageEventData,
) -> Option<i64> {
    resolve_provider_cache_ttl_minutes(
        data.endpoint_api_format
            .as_deref()
            .or(data.api_format.as_deref()),
        data.target_model.as_deref().or(Some(data.model.as_str())),
        Some(data.model.as_str()),
        data.provider_request_body.as_ref(),
    )
    .or_else(|| extract_provider_cache_ttl_minutes_from_metadata(data.request_metadata.as_ref()))
}

fn usage_event_is_image_usage(data: &aether_usage_runtime::UsageEventData) -> bool {
    data.request_type
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("image"))
        || api_format_endpoint_kind(data.endpoint_api_format.as_deref()) == Some("image")
        || api_format_endpoint_kind(data.api_format.as_deref()) == Some("image")
        || usage_event_image_count(data).is_some_and(|value| value > 0)
}

fn usage_event_image_count(data: &aether_usage_runtime::UsageEventData) -> Option<i64> {
    metadata_dimension_i64(data.request_metadata.as_ref(), "dimensions", "image_count")
        .or_else(|| {
            metadata_dimension_i64(
                data.request_metadata.as_ref(),
                "billing_dimensions",
                "image_count",
            )
        })
        .filter(|value| *value > 0)
}

fn usage_event_dimension_string(
    data: &aether_usage_runtime::UsageEventData,
    dimension_key: &str,
) -> Option<String> {
    metadata_dimension_string(data.request_metadata.as_ref(), "dimensions", dimension_key).or_else(
        || {
            metadata_dimension_string(
                data.request_metadata.as_ref(),
                "billing_dimensions",
                dimension_key,
            )
        },
    )
}

fn metadata_dimension_string(
    metadata: Option<&Value>,
    bag_key: &str,
    dimension_key: &str,
) -> Option<String> {
    metadata
        .and_then(Value::as_object)
        .and_then(|object| object.get(bag_key))
        .and_then(Value::as_object)
        .and_then(|object| object.get(dimension_key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn metadata_dimension_i64(
    metadata: Option<&Value>,
    bag_key: &str,
    dimension_key: &str,
) -> Option<i64> {
    metadata
        .and_then(Value::as_object)
        .and_then(|object| object.get(bag_key))
        .and_then(Value::as_object)
        .and_then(|object| object.get(dimension_key))
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        })
}

fn api_format_endpoint_kind(api_format: Option<&str>) -> Option<&str> {
    api_format
        .and_then(|value| value.split_once(':').map(|(_, kind)| kind))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn apply_billing_computation(
    event: &mut UsageEvent,
    pricing: &BillingModelPricingSnapshot,
    computation: BillingComputation,
) -> Result<(), DataLayerError> {
    event.data.total_cost_usd = Some(computation.cost_result.cost);
    event.data.actual_total_cost_usd = Some(computation.actual_total_cost);
    merge_billing_snapshot_metadata(&mut event.data.request_metadata, pricing, &computation)
}

fn map_pricing_context(context: StoredBillingModelContext) -> BillingModelPricingSnapshot {
    context.into()
}

fn merge_billing_snapshot_metadata(
    request_metadata: &mut Option<Value>,
    pricing: &BillingModelPricingSnapshot,
    computation: &BillingComputation,
) -> Result<(), DataLayerError> {
    let snapshot = &computation.cost_result.snapshot;
    let billing_snapshot = serde_json::to_value(snapshot).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("failed to serialize billing snapshot: {err}"))
    })?;
    let settlement_snapshot = build_settlement_snapshot(pricing, computation);

    let mut metadata = match request_metadata.take() {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    metadata.insert("billing_snapshot".to_string(), billing_snapshot);
    metadata.insert(
        "settlement_snapshot_schema_version".to_string(),
        Value::from(SETTLEMENT_SNAPSHOT_SCHEMA_VERSION),
    );
    metadata.insert("settlement_snapshot".to_string(), settlement_snapshot);
    metadata.insert(
        "billing_dimensions".to_string(),
        Value::Object(snapshot.resolved_dimensions.clone().into_iter().collect()),
    );
    metadata.insert(
        "rate_multiplier".to_string(),
        Value::from(computation.rate_multiplier),
    );
    metadata.insert(
        "is_free_tier".to_string(),
        Value::from(computation.is_free_tier),
    );
    *request_metadata = Some(Value::Object(metadata));
    Ok(())
}

fn build_settlement_snapshot(
    pricing: &BillingModelPricingSnapshot,
    computation: &BillingComputation,
) -> Value {
    let snapshot = &computation.cost_result.snapshot;
    let resolution = &computation.pricing_resolution;
    json!({
        "schema_version": SETTLEMENT_SNAPSHOT_SCHEMA_VERSION,
        "pricing_snapshot": {
            "provider_id": pricing.provider_id.clone(),
            "provider_billing_type": pricing.provider_billing_type.clone(),
            "provider_api_key_id": pricing.provider_api_key_id.clone(),
            "global_model_id": pricing.global_model_id.clone(),
            "global_model_name": pricing.global_model_name.clone(),
            "model_id": pricing.model_id.clone(),
            "provider_model_name": pricing.model_provider_model_name.clone(),
            "requested_processing_tier": resolution.requested_processing_tier,
            "actual_processing_tier": resolution.actual_processing_tier,
            "billing_processing_tier": resolution.billing_processing_tier,
            "pricing_source": resolution.pricing_source(),
            "tiered_pricing_source": resolution.tiered_pricing_source.map(|source| source.as_str()),
            "processing_tier_price_multiplier": resolution.processing_tier_price_multiplier,
            "price_per_request_source": resolution.price_per_request_source.map(|source| source.as_str()),
            "tiered_pricing": resolution.tiered_pricing,
            "price_per_request": resolution.price_per_request,
            "rate_multiplier": computation.rate_multiplier,
            "is_free_tier": computation.is_free_tier,
        },
        "billing_plan_snapshot": {
            "rule_id": snapshot.rule_id.clone(),
            "rule_name": snapshot.rule_name.clone(),
            "scope": snapshot.scope.clone(),
            "expression": snapshot.expression.clone(),
            "engine_version": snapshot.engine_version.clone(),
        },
        "resolved_dimensions": snapshot.resolved_dimensions.clone(),
        "resolved_variables": snapshot.resolved_variables.clone(),
        "cost_breakdown": snapshot.cost_breakdown.clone(),
        "total_cost": snapshot.total_cost,
        "actual_total_cost": computation.actual_total_cost,
        "status": snapshot.status,
        "calculated_at": snapshot.calculated_at.clone(),
    })
}

#[cfg(test)]
mod tests {
    use aether_data_contracts::repository::billing::StoredBillingModelContext;
    use aether_data_contracts::repository::usage::UsageBodyCaptureState;
    use aether_usage_runtime::{UsageEvent, UsageEventData, UsageEventType};
    use async_trait::async_trait;
    use serde_json::json;
    use serde_json::Value;

    use super::{
        enrich_usage_event_with_billing, usage_event_processing_tiers, BillingModelContextLookup,
    };

    struct TestLookup {
        name_context: Option<StoredBillingModelContext>,
        model_id_context: Option<StoredBillingModelContext>,
    }

    #[async_trait]
    impl BillingModelContextLookup for TestLookup {
        async fn find_billing_model_context_by_model_id(
            &self,
            _provider_id: &str,
            _provider_api_key_id: Option<&str>,
            _model_id: &str,
        ) -> Result<Option<StoredBillingModelContext>, aether_data_contracts::DataLayerError>
        {
            Ok(self.model_id_context.clone())
        }

        async fn find_billing_model_context(
            &self,
            _provider_id: &str,
            _provider_api_key_id: Option<&str>,
            _global_model_name: &str,
        ) -> Result<Option<StoredBillingModelContext>, aether_data_contracts::DataLayerError>
        {
            Ok(self.name_context.clone())
        }
    }

    #[test]
    fn processing_tier_fact_uses_final_provider_request_body() {
        let data = UsageEventData {
            provider_request_body: Some(json!({"service_tier": "Priority"})),
            response_body: Some(json!({"service_tier": "priority"})),
            request_metadata: Some(json!({
                "provider_service_tier": "batch",
                "provider_actual_service_tier": "Default"
            })),
            ..UsageEventData::default()
        };

        let tiers = usage_event_processing_tiers(&data);

        assert_eq!(tiers.requested.as_deref(), Some("priority"));
    }

    #[test]
    fn processing_tier_facts_recognize_anthropic_fast_speed() {
        let data = UsageEventData {
            provider_request_body: Some(json!({"speed": "fast"})),
            response_body: Some(json!({
                "usage": {"speed": "fast", "service_tier": "standard"}
            })),
            ..UsageEventData::default()
        };

        let tiers = usage_event_processing_tiers(&data);

        assert_eq!(tiers.requested.as_deref(), Some("fast"));
    }

    #[test]
    fn processing_tier_does_not_fall_back_to_stale_metadata_when_body_is_present() {
        let data = UsageEventData {
            provider_request_body: Some(json!({"model": "gpt-5"})),
            response_body: Some(json!({"service_tier": "priority"})),
            request_metadata: Some(json!({
                "provider_service_tier": "priority",
                "provider_actual_service_tier": "priority"
            })),
            ..UsageEventData::default()
        };

        let tiers = usage_event_processing_tiers(&data);

        assert_eq!(tiers.requested, None);
    }

    #[test]
    fn processing_tier_uses_request_derived_metadata_when_body_capture_was_disabled() {
        let data = UsageEventData {
            provider_request_body: None,
            provider_request_body_state: Some(UsageBodyCaptureState::Disabled),
            response_body: Some(json!({"service_tier": "flex"})),
            request_metadata: Some(json!({
                "provider_service_tier": "priority",
                "provider_actual_service_tier": "flex"
            })),
            ..UsageEventData::default()
        };

        let tiers = usage_event_processing_tiers(&data);

        assert_eq!(tiers.requested.as_deref(), Some("priority"));
    }

    #[test]
    fn processing_tier_does_not_use_metadata_or_response_when_final_request_body_is_missing() {
        let data = UsageEventData {
            provider_request_body: None,
            provider_request_body_state: Some(UsageBodyCaptureState::None),
            response_body: Some(json!({"service_tier": "priority"})),
            request_metadata: Some(json!({
                "provider_service_tier": "priority",
                "provider_actual_service_tier": "priority"
            })),
            ..UsageEventData::default()
        };

        let tiers = usage_event_processing_tiers(&data);

        assert_eq!(tiers.requested, None);
    }

    #[test]
    fn processing_tier_uses_request_derived_metadata_when_body_capture_was_truncated() {
        let data = UsageEventData {
            provider_request_body: Some(json!({
                "truncated": true,
                "reason": "body_capture_limit_exceeded",
                "max_bytes": 128,
                "source_bytes": 4096,
                "value_kind": "object"
            })),
            provider_request_body_state: Some(UsageBodyCaptureState::Truncated),
            response_body: Some(json!({"service_tier": "default"})),
            request_metadata: Some(json!({
                "provider_service_tier": "priority",
                "provider_actual_service_tier": "default"
            })),
            ..UsageEventData::default()
        };

        let tiers = usage_event_processing_tiers(&data);

        assert_eq!(tiers.requested.as_deref(), Some("priority"));
    }

    #[tokio::test]
    async fn settlement_uses_effective_gpt_5_6_cache_ttl_after_body_capture() {
        let lookup = TestLookup {
            name_context: Some(
                StoredBillingModelContext::new(
                    "provider-1".to_string(),
                    Some("pay_as_you_go".to_string()),
                    Some("key-1".to_string()),
                    None,
                    Some(60),
                    "global-model-1".to_string(),
                    "gpt-5.6-sol".to_string(),
                    None,
                    None,
                    Some(json!({
                        "tiers": [{
                            "up_to": null,
                            "input_price_per_1m": 5.0,
                            "output_price_per_1m": 30.0,
                            "cache_creation_price_per_1m": 6.25,
                            "cache_read_price_per_1m": 0.5,
                            "cache_ttl_pricing": [{
                                "ttl_minutes": 60,
                                "cache_creation_price_per_1m": 100.0,
                                "cache_read_price_per_1m": 100.0
                            }]
                        }]
                    })),
                    Some("model-1".to_string()),
                    Some("gpt-5.6-sol".to_string()),
                    None,
                    None,
                    None,
                )
                .expect("billing context should build"),
            ),
            model_id_context: None,
        };

        for (request_id, provider_request_body, request_metadata) in [
            (
                "req-cache-body",
                Some(json!({"model": "gpt-5.6-sol"})),
                None,
            ),
            (
                "req-cache-metadata",
                None,
                Some(json!({"provider_cache_ttl_minutes": 30})),
            ),
        ] {
            let mut event = UsageEvent::new(
                UsageEventType::Completed,
                request_id,
                UsageEventData {
                    provider_name: "OpenAI".to_string(),
                    model: "gpt-5.6-sol".to_string(),
                    target_model: Some("gpt-5.6-sol".to_string()),
                    provider_id: Some("provider-1".to_string()),
                    provider_api_key_id: Some("key-1".to_string()),
                    request_type: Some("chat".to_string()),
                    api_format: Some("openai:responses".to_string()),
                    endpoint_api_format: Some("openai:responses".to_string()),
                    provider_request_body,
                    request_metadata,
                    input_tokens: Some(1_000_000),
                    cache_creation_input_tokens: Some(1_000_000),
                    status_code: Some(200),
                    ..UsageEventData::default()
                },
            );

            enrich_usage_event_with_billing(&lookup, &mut event)
                .await
                .expect("billing should succeed");

            let snapshot = event
                .data
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("billing_snapshot"))
                .expect("billing snapshot should exist");
            assert_eq!(
                snapshot
                    .get("resolved_dimensions")
                    .and_then(|value| value.get("cache_ttl_minutes")),
                Some(&json!(30))
            );
            assert_eq!(
                snapshot
                    .get("resolved_variables")
                    .and_then(|value| value.get("cache_creation_price_per_1m")),
                Some(&json!(6.25))
            );
            assert_eq!(event.data.total_cost_usd, Some(6.25));
        }
    }

    #[tokio::test]
    async fn enriches_completed_usage_event_with_billing_snapshot() {
        let lookup = TestLookup {
            name_context: Some(
                StoredBillingModelContext::new(
                    "provider-1".to_string(),
                    Some("pay_as_you_go".to_string()),
                    Some("key-1".to_string()),
                    Some(json!({"openai:chat": 0.5})),
                    Some(60),
                    "global-model-1".to_string(),
                    "gpt-5".to_string(),
                    None,
                    Some(0.02),
                    Some(json!({"tiers":[{"up_to":null,"input_price_per_1m":3.0,"output_price_per_1m":15.0,"cache_creation_price_per_1m":3.75,"cache_read_price_per_1m":0.30}]})),
                    Some("model-1".to_string()),
                    Some("gpt-5-upstream".to_string()),
                    None,
                    None,
                    None,
                )
                .expect("billing context should build"),
            ),
            model_id_context: None,
        };
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

        enrich_usage_event_with_billing(&lookup, &mut event)
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
    async fn openai_fast_usage_without_overlay_inherits_global_model_pricing() {
        let lookup = TestLookup {
            name_context: Some(
                StoredBillingModelContext::new(
                    "provider-1".to_string(),
                    Some("pay_as_you_go".to_string()),
                    Some("key-1".to_string()),
                    None,
                    None,
                    "global-model-1".to_string(),
                    "gpt-5.6-sol".to_string(),
                    None,
                    None,
                    Some(json!({
                        "tiers": [{
                            "up_to": null,
                            "input_price_per_1m": 3.0,
                            "output_price_per_1m": 15.0
                        }]
                    })),
                    Some("model-1".to_string()),
                    Some("gpt-5.6-sol".to_string()),
                    None,
                    None,
                    None,
                )
                .expect("billing context should build"),
            ),
            model_id_context: None,
        };
        let mut event = UsageEvent::new(
            UsageEventType::Completed,
            "req-fast-global-fallback",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5.6-sol".to_string(),
                target_model: Some("gpt-5.6-sol".to_string()),
                provider_id: Some("provider-1".to_string()),
                provider_api_key_id: Some("key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:responses".to_string()),
                endpoint_api_format: Some("openai:responses".to_string()),
                // OpenAI calls the Fast request tier `priority` on the wire.
                provider_request_body: Some(json!({
                    "model": "gpt-5.6-sol",
                    "service_tier": "priority"
                })),
                input_tokens: Some(1_000_000),
                status_code: Some(200),
                ..UsageEventData::default()
            },
        );

        enrich_usage_event_with_billing(&lookup, &mut event)
            .await
            .expect("billing should succeed");

        assert_eq!(event.data.total_cost_usd, Some(3.0));
        let metadata = event.data.request_metadata.as_ref().expect("metadata");
        assert_eq!(
            metadata.pointer("/billing_snapshot/status"),
            Some(&json!("complete"))
        );
        let pricing = metadata
            .pointer("/settlement_snapshot/pricing_snapshot")
            .expect("settlement pricing snapshot");
        assert_eq!(pricing["billing_processing_tier"], "priority");
        assert_eq!(pricing["pricing_source"], "global_default");
        assert_eq!(pricing["tiered_pricing_source"], "global_default");
        assert_eq!(
            pricing["tiered_pricing"]["tiers"][0]["input_price_per_1m"],
            3.0
        );
    }

    #[tokio::test]
    async fn settlement_uses_requested_processing_tier_catalog_and_ignores_response_tier() {
        let lookup = TestLookup {
            name_context: Some(
                StoredBillingModelContext::new(
                    "provider-1".to_string(),
                    Some("pay_as_you_go".to_string()),
                    Some("key-1".to_string()),
                    None,
                    Some(60),
                    "global-model-1".to_string(),
                    "gpt-5.6".to_string(),
                    None,
                    None,
                    Some(json!({
                        "tiers": [{"up_to": null, "input_price_per_1m": 5.0, "output_price_per_1m": 30.0}],
                        "processing_tiers": {
                            "flex": {"price_multiplier": 0.5}
                        }
                    })),
                    Some("model-1".to_string()),
                    Some("gpt-5.6-upstream".to_string()),
                    None,
                    None,
                    Some(json!({
                        "processing_tiers": {
                            "priority": {"tiers": [{"up_to": 272000, "input_price_per_1m": 10.0, "output_price_per_1m": 60.0}]}
                        }
                    })),
                )
                .expect("billing context should build"),
            ),
            model_id_context: None,
        };
        let mut event = UsageEvent::new(
            UsageEventType::Completed,
            "req-billing-tier-1",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5.6".to_string(),
                provider_id: Some("provider-1".to_string()),
                provider_api_key_id: Some("key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:responses".to_string()),
                endpoint_api_format: Some("openai:responses".to_string()),
                provider_request_body: Some(json!({"service_tier": "priority"})),
                response_body: Some(json!({"service_tier": "priority"})),
                request_metadata: Some(json!({
                    "provider_service_tier": "priority",
                    "provider_actual_service_tier": "flex"
                })),
                input_tokens: Some(1_000),
                output_tokens: Some(100),
                status_code: Some(200),
                ..UsageEventData::default()
            },
        );

        enrich_usage_event_with_billing(&lookup, &mut event)
            .await
            .expect("billing should succeed");

        let pricing_snapshot = event
            .data
            .request_metadata
            .as_ref()
            .and_then(|value| value.pointer("/settlement_snapshot/pricing_snapshot"))
            .expect("settlement pricing snapshot should exist");
        assert_eq!(pricing_snapshot["requested_processing_tier"], "priority");
        assert!(pricing_snapshot["actual_processing_tier"].is_null());
        assert_eq!(pricing_snapshot["billing_processing_tier"], "priority");
        assert_eq!(
            pricing_snapshot["tiered_pricing_source"],
            "provider_override"
        );
        assert_eq!(
            pricing_snapshot["processing_tier_price_multiplier"],
            Value::Null
        );
        assert_eq!(
            pricing_snapshot["tiered_pricing"]["tiers"][0]["input_price_per_1m"],
            10.0
        );
        // The response fact remains available for audit, but does not influence settlement.
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("provider_actual_service_tier")),
            Some(&json!("flex"))
        );
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|value| { value.pointer("/billing_dimensions/actual_processing_tier") }),
            Some(&Value::Null)
        );
    }

    #[tokio::test]
    async fn requested_processing_catalog_controls_image_price_with_independent_fixed_price() {
        let lookup = TestLookup {
            name_context: Some(
                StoredBillingModelContext::new(
                    "provider-1".to_string(),
                    Some("pay_as_you_go".to_string()),
                    Some("key-1".to_string()),
                    None,
                    None,
                    "global-image-1".to_string(),
                    "gpt-image-2".to_string(),
                    None,
                    Some(0.01),
                    Some(json!({
                        "image_output_price_default": 0.1,
                        "processing_tiers": {
                            "flex": {"image_output_price_default": 0.2}
                        }
                    })),
                    Some("model-image-1".to_string()),
                    Some("gpt-image-2".to_string()),
                    None,
                    Some(0.02),
                    Some(json!({
                        "processing_tiers": {
                            "priority": {"image_output_price_default": 0.4}
                        }
                    })),
                )
                .expect("billing context should build"),
            ),
            model_id_context: None,
        };
        let mut event = UsageEvent::new(
            UsageEventType::Completed,
            "req-image-processing-tier-1",
            UsageEventData {
                provider_name: "OpenAI Image".to_string(),
                model: "gpt-image-2".to_string(),
                provider_id: Some("provider-1".to_string()),
                provider_api_key_id: Some("key-1".to_string()),
                request_type: Some("image".to_string()),
                api_format: Some("openai:image".to_string()),
                endpoint_api_format: Some("openai:image".to_string()),
                provider_request_body: Some(json!({"service_tier": "priority"})),
                request_metadata: Some(json!({
                    "provider_actual_service_tier": "flex",
                    "dimensions": {"image_count": 2}
                })),
                status_code: Some(200),
                ..UsageEventData::default()
            },
        );

        enrich_usage_event_with_billing(&lookup, &mut event)
            .await
            .expect("billing should succeed");

        assert_eq!(event.data.total_cost_usd, Some(0.84));
        assert_eq!(event.data.actual_total_cost_usd, Some(0.84));
        let metadata = event.data.request_metadata.as_ref().expect("metadata");
        let pricing = metadata
            .pointer("/settlement_snapshot/pricing_snapshot")
            .expect("pricing snapshot");
        assert_eq!(pricing["billing_processing_tier"], "priority");
        assert_eq!(pricing["tiered_pricing_source"], "provider_override");
        assert_eq!(pricing["price_per_request_source"], "provider_override");
        assert_eq!(pricing["pricing_source"], "provider_override");
        assert_eq!(
            metadata
                .pointer("/billing_snapshot/resolved_variables/image_output_price_per_image")
                .and_then(Value::as_f64),
            Some(0.4)
        );
        assert_eq!(
            metadata
                .pointer("/billing_snapshot/cost_breakdown/image_output_cost")
                .and_then(Value::as_f64),
            Some(0.8)
        );
        assert_eq!(
            metadata
                .pointer("/billing_snapshot/cost_breakdown/request_cost")
                .and_then(Value::as_f64),
            Some(0.04)
        );
    }

    #[tokio::test]
    async fn image_usage_uses_image_count_for_request_cost() {
        let lookup = TestLookup {
            name_context: Some(
                StoredBillingModelContext::new(
                    "provider-1".to_string(),
                    Some("pay_as_you_go".to_string()),
                    Some("key-1".to_string()),
                    None,
                    None,
                    "global-image-1".to_string(),
                    "gpt-image-2".to_string(),
                    None,
                    Some(0.02),
                    None,
                    Some("model-image-1".to_string()),
                    Some("gpt-image-2".to_string()),
                    None,
                    None,
                    None,
                )
                .expect("billing context should build"),
            ),
            model_id_context: None,
        };
        let mut event = UsageEvent::new(
            UsageEventType::Completed,
            "req-image-billing-1",
            UsageEventData {
                provider_name: "OpenAI Image".to_string(),
                model: "gpt-image-2".to_string(),
                provider_id: Some("provider-1".to_string()),
                provider_api_key_id: Some("key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                endpoint_api_format: Some("openai:image".to_string()),
                request_metadata: Some(json!({
                    "dimensions": {
                        "image_count": 3
                    }
                })),
                status_code: Some(200),
                ..UsageEventData::default()
            },
        );

        enrich_usage_event_with_billing(&lookup, &mut event)
            .await
            .expect("billing should succeed");

        assert_eq!(event.data.total_cost_usd, Some(0.06));
        assert_eq!(event.data.actual_total_cost_usd, Some(0.06));
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("billing_dimensions"))
                .and_then(|value| value.get("request_count"))
                .and_then(Value::as_i64),
            Some(3)
        );
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("billing_dimensions"))
                .and_then(|value| value.get("image_count"))
                .and_then(Value::as_i64),
            Some(3)
        );
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("billing_dimensions"))
                .and_then(|value| value.get("effective_task_type"))
                .and_then(Value::as_str),
            Some("image")
        );
    }

    #[tokio::test]
    async fn image_usage_uses_configured_output_price_matrix() {
        let lookup = TestLookup {
            name_context: Some(
                StoredBillingModelContext::new(
                    "provider-1".to_string(),
                    Some("pay_as_you_go".to_string()),
                    Some("key-1".to_string()),
                    None,
                    None,
                    "global-image-1".to_string(),
                    "gpt-image-2".to_string(),
                    None,
                    None,
                    Some(json!({
                        "tiers": [{
                            "up_to": null,
                            "input_price_per_1m": 5.0,
                            "output_price_per_1m": 30.0,
                            "cache_read_price_per_1m": 1.25
                        }],
                        "image_output_price_default": 0.01,
                        "image_output_prices": {
                            "1024x1024": {"low": 0.006, "medium": 0.053, "high": 0.211},
                            "1536x1024": {"low": 0.005, "medium": 0.041, "high": 0.165},
                            "1024x1536": {"low": 0.005, "medium": 0.041, "high": 0.165}
                        }
                    })),
                    Some("model-image-1".to_string()),
                    Some("gpt-image-2".to_string()),
                    None,
                    None,
                    None,
                )
                .expect("billing context should build"),
            ),
            model_id_context: None,
        };
        let mut event = UsageEvent::new(
            UsageEventType::Completed,
            "req-image-billing-matrix-1",
            UsageEventData {
                provider_name: "OpenAI Image".to_string(),
                model: "gpt-image-2".to_string(),
                provider_id: Some("provider-1".to_string()),
                provider_api_key_id: Some("key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:chat".to_string()),
                endpoint_api_format: Some("openai:image".to_string()),
                request_metadata: Some(json!({
                    "dimensions": {
                        "image_count": 2,
                        "image_size": "1536x1024",
                        "image_quality": "medium",
                        "image_output_format": "png"
                    }
                })),
                status_code: Some(200),
                ..UsageEventData::default()
            },
        );

        enrich_usage_event_with_billing(&lookup, &mut event)
            .await
            .expect("billing should succeed");

        assert_eq!(event.data.total_cost_usd, Some(0.082));
        assert_eq!(event.data.actual_total_cost_usd, Some(0.082));
        let metadata = event.data.request_metadata.as_ref().expect("metadata");
        assert_eq!(
            metadata
                .get("billing_dimensions")
                .and_then(|value| value.get("image_price_key"))
                .and_then(Value::as_str),
            Some("1536x1024:medium")
        );
        assert_eq!(
            metadata
                .get("billing_snapshot")
                .and_then(|value| value.get("resolved_variables"))
                .and_then(|value| value.get("image_output_price_per_image"))
                .and_then(Value::as_f64),
            Some(0.041)
        );
        assert_eq!(
            metadata
                .get("billing_snapshot")
                .and_then(|value| value.get("cost_breakdown"))
                .and_then(|value| value.get("image_output_cost"))
                .and_then(Value::as_f64),
            Some(0.082)
        );
    }

    #[tokio::test]
    async fn cancelled_usage_event_remains_unbilled() {
        let lookup = TestLookup {
            name_context: Some(
                StoredBillingModelContext::new(
                    "provider-1".to_string(),
                    Some("pay_as_you_go".to_string()),
                    Some("key-1".to_string()),
                    Some(json!({"openai:responses": 0.5})),
                    Some(60),
                    "global-model-1".to_string(),
                    "gpt-5".to_string(),
                    None,
                    Some(0.02),
                    Some(json!({"tiers":[{"up_to":null,"input_price_per_1m":3.0,"output_price_per_1m":15.0,"cache_creation_price_per_1m":3.75,"cache_read_price_per_1m":0.30}]})),
                    Some("model-1".to_string()),
                    Some("gpt-5-upstream".to_string()),
                    None,
                    None,
                    None,
                )
                .expect("billing context should build"),
            ),
            model_id_context: None,
        };
        let mut event = UsageEvent::new(
            UsageEventType::Cancelled,
            "req-billing-cancelled-1",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                provider_id: Some("provider-1".to_string()),
                provider_api_key_id: Some("key-1".to_string()),
                request_type: Some("chat".to_string()),
                api_format: Some("openai:responses".to_string()),
                endpoint_api_format: Some("openai:responses".to_string()),
                input_tokens: Some(1_000),
                output_tokens: Some(500),
                cache_read_input_tokens: Some(100),
                status_code: Some(499),
                ..UsageEventData::default()
            },
        );

        enrich_usage_event_with_billing(&lookup, &mut event)
            .await
            .expect("billing should succeed");

        assert_eq!(event.data.total_cost_usd, Some(0.0));
        assert_eq!(event.data.actual_total_cost_usd, Some(0.0));
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("billing_snapshot"))
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str),
            None
        );
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("billing_dimensions"))
                .and_then(|value| value.get("input_tokens"))
                .and_then(Value::as_i64),
            None
        );
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|value| value.get("billing_dimensions"))
                .and_then(|value| value.get("cache_read_tokens"))
                .and_then(Value::as_i64),
            None
        );
    }

    #[tokio::test]
    async fn failed_usage_event_remains_unbilled() {
        let lookup = TestLookup {
            name_context: None,
            model_id_context: None,
        };
        let mut event = UsageEvent::new(
            UsageEventType::Failed,
            "req-billing-failed-1",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                provider_id: Some("provider-1".to_string()),
                provider_api_key_id: Some("key-1".to_string()),
                request_type: Some("chat".to_string()),
                input_tokens: Some(1_000),
                output_tokens: Some(500),
                status_code: Some(500),
                ..UsageEventData::default()
            },
        );

        enrich_usage_event_with_billing(&lookup, &mut event)
            .await
            .expect("billing should succeed");

        assert_eq!(event.data.total_cost_usd, Some(0.0));
        assert_eq!(event.data.actual_total_cost_usd, Some(0.0));
        assert!(event.data.request_metadata.is_none());
    }

    #[tokio::test]
    async fn enriches_by_provider_model_id_before_name_fallback() {
        let blank_name_context = StoredBillingModelContext::new(
            "provider-1".to_string(),
            Some("pay_as_you_go".to_string()),
            Some("key-1".to_string()),
            None,
            Some(60),
            "global-model-blank".to_string(),
            "claude-sonnet-4-6".to_string(),
            None,
            None,
            None,
            Some("model-blank".to_string()),
            Some("claude-sonnet-4-6".to_string()),
            None,
            None,
            None,
        )
        .expect("blank billing context should build");
        let priced_model_context = StoredBillingModelContext::new(
            "provider-1".to_string(),
            Some("pay_as_you_go".to_string()),
            Some("key-1".to_string()),
            None,
            Some(60),
            "global-model-priced".to_string(),
            "claude-sonnet-4-6".to_string(),
            None,
            None,
            None,
            Some("model-priced".to_string()),
            Some("claude-sonnet-4-6".to_string()),
            None,
            None,
            Some(
                json!({"tiers":[{"up_to":null,"input_price_per_1m":3.0,"output_price_per_1m":15.0}]}),
            ),
        )
        .expect("priced billing context should build");
        let lookup = TestLookup {
            name_context: Some(blank_name_context),
            model_id_context: Some(priced_model_context),
        };
        let mut event = UsageEvent::new(
            UsageEventType::Completed,
            "req-billing-model-id-1",
            UsageEventData {
                provider_name: "NekoCode".to_string(),
                model: "claude-sonnet-4-6".to_string(),
                model_id: Some("model-priced".to_string()),
                provider_id: Some("provider-1".to_string()),
                provider_api_key_id: Some("key-1".to_string()),
                request_type: Some("chat".to_string()),
                input_tokens: Some(1_000),
                output_tokens: Some(500),
                status_code: Some(200),
                ..UsageEventData::default()
            },
        );

        enrich_usage_event_with_billing(&lookup, &mut event)
            .await
            .expect("billing should succeed");

        assert!(event.data.total_cost_usd.unwrap_or_default() > 0.0);
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
}
