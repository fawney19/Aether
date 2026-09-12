use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;

use super::{
    AdminBillingMutationOutcome, BillingPlanRecord, BillingPlanWriteInput, BillingReadRepository,
    PaymentGatewayConfigCasWriteInput, PaymentGatewayConfigRecord, PaymentGatewayConfigWriteInput,
    PaymentGatewaySecretCasUpdate, StoredBillingModelContext, UserDailyQuotaAvailabilityRecord,
    UserPlanEntitlementRecord,
};
use crate::DataLayerError;

type BillingContextKey = (String, String, Option<String>);
type BillingContextMap = BTreeMap<BillingContextKey, StoredBillingModelContext>;

#[derive(Debug, Default)]
pub struct InMemoryBillingReadRepository {
    by_key: RwLock<BillingContextMap>,
    gateway_configs_by_provider: RwLock<BTreeMap<String, PaymentGatewayConfigRecord>>,
    billing_plans_by_id: RwLock<BTreeMap<String, BillingPlanRecord>>,
    entitlements_by_id: RwLock<BTreeMap<String, UserPlanEntitlementRecord>>,
}

impl InMemoryBillingReadRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredBillingModelContext>,
    {
        let mut by_key = BTreeMap::new();
        for item in items {
            by_key.insert(
                (
                    item.provider_id.clone(),
                    item.global_model_name.clone(),
                    item.provider_api_key_id.clone(),
                ),
                item,
            );
        }
        Self {
            by_key: RwLock::new(by_key),
            gateway_configs_by_provider: RwLock::new(BTreeMap::new()),
            billing_plans_by_id: RwLock::new(BTreeMap::new()),
            entitlements_by_id: RwLock::new(BTreeMap::new()),
        }
    }
}

fn current_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn billing_plan_from_input(
    id: String,
    input: &BillingPlanWriteInput,
    created_at: u64,
) -> BillingPlanRecord {
    BillingPlanRecord {
        id,
        title: input.title.clone(),
        description: input.description.clone(),
        price_amount: input.price_amount,
        price_currency: input.price_currency.clone(),
        duration_unit: input.duration_unit.clone(),
        duration_value: input.duration_value,
        enabled: input.enabled,
        sort_order: input.sort_order,
        max_active_per_user: input.max_active_per_user,
        purchase_limit_scope: input.purchase_limit_scope.clone(),
        entitlements_json: input.entitlements_json.clone(),
        created_at_unix_secs: created_at,
        updated_at_unix_secs: current_unix_secs(),
    }
}

fn daily_quota_availability_from_entitlements(
    entitlements: impl IntoIterator<Item = UserPlanEntitlementRecord>,
    billing_plans: &BTreeMap<String, BillingPlanRecord>,
    now: u64,
) -> UserDailyQuotaAvailabilityRecord {
    let mut has_active_daily_quota = false;
    let mut total_quota_usd = 0.0;
    let used_usd = 0.0;
    let mut remaining_usd = 0.0;
    let mut allow_wallet_overage = true;
    for entitlement in entitlements {
        if entitlement.status != "active"
            || entitlement.starts_at_unix_secs > now
            || entitlement.expires_at_unix_secs <= now
        {
            continue;
        }
        let Some(items) = entitlement.entitlements_snapshot.as_array() else {
            continue;
        };
        let current_allow_wallet_overage = billing_plans
            .get(&entitlement.plan_id)
            .and_then(|plan| daily_quota_wallet_overage_policy(&plan.entitlements_json));
        for item in items {
            if item.get("type").and_then(serde_json::Value::as_str) != Some("daily_quota") {
                continue;
            }
            let daily_quota_usd = item
                .get("daily_quota_usd")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0);
            if !daily_quota_usd.is_finite() || daily_quota_usd <= 0.0 {
                continue;
            }
            has_active_daily_quota = true;
            total_quota_usd += daily_quota_usd;
            remaining_usd += daily_quota_usd;
            allow_wallet_overage &= current_allow_wallet_overage.unwrap_or_else(|| {
                item.get("allow_wallet_overage")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            });
        }
    }
    UserDailyQuotaAvailabilityRecord {
        has_active_daily_quota,
        total_quota_usd,
        used_usd,
        remaining_usd,
        allow_wallet_overage,
    }
}

fn daily_quota_wallet_overage_policy(entitlements: &serde_json::Value) -> Option<bool> {
    entitlements.as_array()?.iter().find_map(|item| {
        (item.get("type").and_then(serde_json::Value::as_str) == Some("daily_quota"))
            .then(|| {
                item.get("allow_wallet_overage")
                    .and_then(serde_json::Value::as_bool)
            })
            .flatten()
    })
}

#[async_trait]
impl BillingReadRepository for InMemoryBillingReadRepository {
    async fn find_model_context(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        global_model_name: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        let by_key = self.by_key.read().expect("billing repository lock");
        if let Some(value) = find_context_by_provider_model_name(
            &by_key,
            provider_id,
            provider_api_key_id,
            global_model_name,
        ) {
            return Ok(Some(value));
        }

        let key = (
            provider_id.to_string(),
            global_model_name.to_string(),
            provider_api_key_id.map(ToOwned::to_owned),
        );
        if let Some(value) = by_key.get(&key) {
            return Ok(Some(value.clone()));
        }

        if let Some(value) = by_key
            .get(&(provider_id.to_string(), global_model_name.to_string(), None))
            .cloned()
        {
            return Ok(Some(value));
        }

        Ok(by_key
            .iter()
            .find(|((stored_provider_id, stored_model_name, _), _)| {
                stored_provider_id == provider_id && stored_model_name == global_model_name
            })
            .map(|(_, value)| value.clone()))
    }

    async fn find_model_context_by_model_id(
        &self,
        provider_id: &str,
        provider_api_key_id: Option<&str>,
        model_id: &str,
    ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
        let by_key = self.by_key.read().expect("billing repository lock");
        if let Some(value) =
            find_context_by_model_id_and_key(&by_key, provider_id, provider_api_key_id, model_id)
        {
            return Ok(Some(value));
        }
        if let Some(value) = find_context_by_model_id_and_key(&by_key, provider_id, None, model_id)
        {
            return Ok(Some(value));
        }
        Ok(by_key
            .iter()
            .find(|((stored_provider_id, _, _), value)| {
                stored_provider_id == provider_id && value.model_id.as_deref() == Some(model_id)
            })
            .map(|(_, value)| value.clone()))
    }

    async fn find_payment_gateway_config(
        &self,
        provider: &str,
    ) -> Result<Option<PaymentGatewayConfigRecord>, DataLayerError> {
        Ok(self
            .gateway_configs_by_provider
            .read()
            .expect("billing repository lock")
            .get(&provider.trim().to_ascii_lowercase())
            .cloned())
    }

    async fn compare_and_swap_payment_gateway_secret(
        &self,
        update: &PaymentGatewaySecretCasUpdate,
    ) -> Result<bool, DataLayerError> {
        let provider = update.provider.trim().to_ascii_lowercase();
        let mut configs = self
            .gateway_configs_by_provider
            .write()
            .expect("billing repository lock");
        let Some(record) = configs.get_mut(&provider) else {
            return Ok(false);
        };
        if record.merchant_key_encrypted.as_deref()
            != Some(update.expected_merchant_key_encrypted.as_str())
        {
            return Ok(false);
        }
        record.merchant_key_encrypted = Some(update.merchant_key_encrypted.clone());
        Ok(true)
    }

    async fn compare_and_swap_payment_gateway_config(
        &self,
        mutation: &PaymentGatewayConfigCasWriteInput,
    ) -> Result<AdminBillingMutationOutcome<PaymentGatewayConfigRecord>, DataLayerError> {
        let input = &mutation.input;
        let provider = input.provider.trim().to_ascii_lowercase();
        let now = current_unix_secs();
        let mut configs = self
            .gateway_configs_by_provider
            .write()
            .expect("billing repository lock");
        let existing = configs.get(&provider);
        if mutation.expected_existing {
            let Some(existing) = existing else {
                return Ok(AdminBillingMutationOutcome::NotFound);
            };
            if existing.merchant_key_encrypted != mutation.expected_merchant_key_encrypted {
                return Ok(AdminBillingMutationOutcome::NotFound);
            }
        } else if existing.is_some() {
            return Ok(AdminBillingMutationOutcome::NotFound);
        }

        let created_at = existing
            .map(|value| value.created_at_unix_secs)
            .unwrap_or(now);
        let merchant_key_encrypted = if input.preserve_existing_secret {
            existing.and_then(|value| value.merchant_key_encrypted.clone())
        } else {
            input.merchant_key_encrypted.clone()
        };
        let record = PaymentGatewayConfigRecord {
            provider: provider.clone(),
            enabled: input.enabled,
            endpoint_url: input.endpoint_url.clone(),
            callback_base_url: input.callback_base_url.clone(),
            merchant_id: input.merchant_id.clone(),
            merchant_key_encrypted,
            pay_currency: input.pay_currency.clone(),
            usd_exchange_rate: input.usd_exchange_rate,
            min_recharge_usd: input.min_recharge_usd,
            channels_json: input.channels_json.clone(),
            created_at_unix_secs: created_at,
            updated_at_unix_secs: now,
        };
        configs.insert(provider, record.clone());
        Ok(AdminBillingMutationOutcome::Applied(record))
    }

    async fn upsert_payment_gateway_config(
        &self,
        input: &PaymentGatewayConfigWriteInput,
    ) -> Result<AdminBillingMutationOutcome<PaymentGatewayConfigRecord>, DataLayerError> {
        let provider = input.provider.trim().to_ascii_lowercase();
        let now = current_unix_secs();
        let mut configs = self
            .gateway_configs_by_provider
            .write()
            .expect("billing repository lock");
        let created_at = configs
            .get(&provider)
            .map(|value| value.created_at_unix_secs)
            .unwrap_or(now);
        let merchant_key_encrypted = if input.preserve_existing_secret {
            configs
                .get(&provider)
                .and_then(|value| value.merchant_key_encrypted.clone())
        } else {
            input.merchant_key_encrypted.clone()
        };
        let record = PaymentGatewayConfigRecord {
            provider: provider.clone(),
            enabled: input.enabled,
            endpoint_url: input.endpoint_url.clone(),
            callback_base_url: input.callback_base_url.clone(),
            merchant_id: input.merchant_id.clone(),
            merchant_key_encrypted,
            pay_currency: input.pay_currency.clone(),
            usd_exchange_rate: input.usd_exchange_rate,
            min_recharge_usd: input.min_recharge_usd,
            channels_json: input.channels_json.clone(),
            created_at_unix_secs: created_at,
            updated_at_unix_secs: now,
        };
        configs.insert(provider, record.clone());
        Ok(AdminBillingMutationOutcome::Applied(record))
    }

    async fn list_billing_plans(
        &self,
        include_disabled: bool,
    ) -> Result<Option<Vec<BillingPlanRecord>>, DataLayerError> {
        let mut items = self
            .billing_plans_by_id
            .read()
            .expect("billing repository lock")
            .values()
            .filter(|item| include_disabled || item.enabled)
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by(|left, right| {
            left.sort_order
                .cmp(&right.sort_order)
                .then_with(|| left.price_amount.total_cmp(&right.price_amount))
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(Some(items))
    }

    async fn find_billing_plan(
        &self,
        plan_id: &str,
    ) -> Result<Option<BillingPlanRecord>, DataLayerError> {
        Ok(self
            .billing_plans_by_id
            .read()
            .expect("billing repository lock")
            .get(plan_id)
            .cloned())
    }

    async fn create_billing_plan(
        &self,
        input: &BillingPlanWriteInput,
    ) -> Result<AdminBillingMutationOutcome<BillingPlanRecord>, DataLayerError> {
        let id = uuid::Uuid::new_v4().to_string();
        let record = billing_plan_from_input(id.clone(), input, current_unix_secs());
        self.billing_plans_by_id
            .write()
            .expect("billing repository lock")
            .insert(id, record.clone());
        Ok(AdminBillingMutationOutcome::Applied(record))
    }

    async fn update_billing_plan(
        &self,
        plan_id: &str,
        input: &BillingPlanWriteInput,
    ) -> Result<AdminBillingMutationOutcome<BillingPlanRecord>, DataLayerError> {
        let mut plans = self
            .billing_plans_by_id
            .write()
            .expect("billing repository lock");
        let Some(existing) = plans.get(plan_id).cloned() else {
            return Ok(AdminBillingMutationOutcome::NotFound);
        };
        let record =
            billing_plan_from_input(plan_id.to_string(), input, existing.created_at_unix_secs);
        plans.insert(plan_id.to_string(), record.clone());
        Ok(AdminBillingMutationOutcome::Applied(record))
    }

    async fn set_billing_plan_enabled(
        &self,
        plan_id: &str,
        enabled: bool,
    ) -> Result<AdminBillingMutationOutcome<BillingPlanRecord>, DataLayerError> {
        let mut plans = self
            .billing_plans_by_id
            .write()
            .expect("billing repository lock");
        let Some(record) = plans.get_mut(plan_id) else {
            return Ok(AdminBillingMutationOutcome::NotFound);
        };
        record.enabled = enabled;
        record.updated_at_unix_secs = current_unix_secs();
        Ok(AdminBillingMutationOutcome::Applied(record.clone()))
    }

    async fn delete_billing_plan(
        &self,
        plan_id: &str,
    ) -> Result<AdminBillingMutationOutcome<()>, DataLayerError> {
        let mut plans = self
            .billing_plans_by_id
            .write()
            .expect("billing repository lock");
        if !plans.contains_key(plan_id) {
            return Ok(AdminBillingMutationOutcome::NotFound);
        }
        let has_entitlements = self
            .entitlements_by_id
            .read()
            .expect("billing repository lock")
            .values()
            .any(|item| item.plan_id == plan_id);
        if has_entitlements {
            return Ok(AdminBillingMutationOutcome::Invalid(
                "套餐已有订单或权益，不能删除，请停用该套餐".to_string(),
            ));
        }
        plans.remove(plan_id);
        Ok(AdminBillingMutationOutcome::Applied(()))
    }

    async fn list_user_plan_entitlements(
        &self,
        user_id: &str,
    ) -> Result<Option<Vec<UserPlanEntitlementRecord>>, DataLayerError> {
        let now = current_unix_secs();
        let mut items = self
            .entitlements_by_id
            .read()
            .expect("billing repository lock")
            .values()
            .filter(|item| {
                item.user_id == user_id
                    && item.status == "active"
                    && item.expires_at_unix_secs > now
            })
            .cloned()
            .collect::<Vec<_>>();
        items.sort_by_key(|item| item.expires_at_unix_secs);
        Ok(Some(items))
    }

    async fn revoke_user_plan_entitlement(
        &self,
        user_id: &str,
        entitlement_id: &str,
    ) -> Result<AdminBillingMutationOutcome<()>, DataLayerError> {
        let now = current_unix_secs();
        let mut entitlements = self
            .entitlements_by_id
            .write()
            .expect("billing repository lock");
        let Some(entitlement) = entitlements.get_mut(entitlement_id) else {
            return Ok(AdminBillingMutationOutcome::NotFound);
        };
        if entitlement.user_id != user_id
            || entitlement.status != "active"
            || entitlement.expires_at_unix_secs <= now
        {
            return Ok(AdminBillingMutationOutcome::NotFound);
        }
        entitlement.status = "revoked".to_string();
        entitlement.expires_at_unix_secs = entitlement.expires_at_unix_secs.min(now);
        entitlement.updated_at_unix_secs = now;
        Ok(AdminBillingMutationOutcome::Applied(()))
    }

    async fn find_user_daily_quota_availability(
        &self,
        user_id: &str,
    ) -> Result<Option<UserDailyQuotaAvailabilityRecord>, DataLayerError> {
        let now = current_unix_secs();
        let entitlements = self
            .entitlements_by_id
            .read()
            .expect("billing repository lock")
            .values()
            .filter(|item| item.user_id == user_id)
            .cloned()
            .collect::<Vec<_>>();
        let billing_plans = self
            .billing_plans_by_id
            .read()
            .expect("billing repository lock");
        Ok(Some(daily_quota_availability_from_entitlements(
            entitlements,
            &billing_plans,
            now,
        )))
    }
}

fn find_context_by_provider_model_name(
    by_key: &BillingContextMap,
    provider_id: &str,
    provider_api_key_id: Option<&str>,
    provider_model_name: &str,
) -> Option<StoredBillingModelContext> {
    find_context_by_provider_model_name_and_key(
        by_key,
        provider_id,
        provider_api_key_id,
        provider_model_name,
    )
    .or_else(|| {
        find_context_by_provider_model_name_and_key(by_key, provider_id, None, provider_model_name)
    })
    .or_else(|| {
        by_key
            .iter()
            .find(|((stored_provider_id, _, _), value)| {
                stored_provider_id == provider_id
                    && value.model_provider_model_name.as_deref() == Some(provider_model_name)
            })
            .map(|(_, value)| value.clone())
    })
}

fn find_context_by_provider_model_name_and_key(
    by_key: &BillingContextMap,
    provider_id: &str,
    provider_api_key_id: Option<&str>,
    provider_model_name: &str,
) -> Option<StoredBillingModelContext> {
    by_key
        .iter()
        .find(|((stored_provider_id, _, stored_key_id), value)| {
            stored_provider_id == provider_id
                && stored_key_id.as_deref() == provider_api_key_id
                && value.model_provider_model_name.as_deref() == Some(provider_model_name)
        })
        .map(|(_, value)| value.clone())
}

fn find_context_by_model_id_and_key(
    by_key: &BillingContextMap,
    provider_id: &str,
    provider_api_key_id: Option<&str>,
    model_id: &str,
) -> Option<StoredBillingModelContext> {
    by_key
        .iter()
        .find(|((stored_provider_id, _, stored_key_id), value)| {
            stored_provider_id == provider_id
                && stored_key_id.as_deref() == provider_api_key_id
                && value.model_id.as_deref() == Some(model_id)
        })
        .map(|(_, value)| value.clone())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::InMemoryBillingReadRepository;
    use crate::repository::billing::{
        AdminBillingMutationOutcome, BillingReadRepository, PaymentGatewayConfigCasWriteInput,
        PaymentGatewayConfigWriteInput, PaymentGatewaySecretCasUpdate, StoredBillingModelContext,
    };

    fn sample_context() -> StoredBillingModelContext {
        StoredBillingModelContext::new(
            "provider-1".to_string(),
            Some("pay_as_you_go".to_string()),
            Some("key-1".to_string()),
            Some(json!({"openai:chat": 0.8})),
            Some(60),
            "global-model-1".to_string(),
            "gpt-5".to_string(),
            Some(json!({"streaming": true})),
            Some(0.02),
            Some(json!({"tiers":[{"up_to":null,"input_price_per_1m":3.0,"output_price_per_1m":15.0}]})),
            Some("model-1".to_string()),
            Some("gpt-5-upstream".to_string()),
            None,
            Some(0.01),
            None,
        )
        .expect("billing context should build")
    }

    #[tokio::test]
    async fn falls_back_to_provider_without_key_scope() {
        let repository = InMemoryBillingReadRepository::seed(vec![sample_context()]);
        let stored = repository
            .find_model_context("provider-1", Some("key-2"), "gpt-5")
            .await
            .expect("lookup should succeed")
            .expect("context should exist");

        assert_eq!(stored.provider_id, "provider-1");
        assert_eq!(stored.global_model_name, "gpt-5");
    }

    #[tokio::test]
    async fn resolves_by_provider_model_name_before_global_name_collision() {
        let global_named_context = StoredBillingModelContext::new(
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
            Some("blank-upstream".to_string()),
            None,
            None,
            None,
        )
        .expect("blank billing context should build");
        let provider_priced_context = StoredBillingModelContext::new(
            "provider-1".to_string(),
            Some("pay_as_you_go".to_string()),
            Some("key-1".to_string()),
            None,
            Some(60),
            "global-model-priced".to_string(),
            "claude-opus-4-6".to_string(),
            None,
            None,
            Some(json!({"tiers":[{"up_to":null,"input_price_per_1m":3.0,"output_price_per_1m":15.0}]})),
            Some("model-priced".to_string()),
            Some("claude-sonnet-4-6".to_string()),
            None,
            None,
            None,
        )
        .expect("priced billing context should build");
        let repository = InMemoryBillingReadRepository::seed(vec![
            global_named_context,
            provider_priced_context,
        ]);

        let stored = repository
            .find_model_context("provider-1", Some("key-1"), "claude-sonnet-4-6")
            .await
            .expect("lookup should succeed")
            .expect("context should exist");

        assert_eq!(stored.global_model_name, "claude-opus-4-6");
        assert_eq!(
            stored.model_provider_model_name.as_deref(),
            Some("claude-sonnet-4-6")
        );
        assert!(stored.default_tiered_pricing.is_some());
    }

    #[tokio::test]
    async fn resolves_by_model_id() {
        let repository = InMemoryBillingReadRepository::seed(vec![sample_context()]);
        let stored = repository
            .find_model_context_by_model_id("provider-1", Some("key-1"), "model-1")
            .await
            .expect("lookup should succeed")
            .expect("context should exist");

        assert_eq!(stored.global_model_name, "gpt-5");
        assert_eq!(
            stored.model_provider_model_name.as_deref(),
            Some("gpt-5-upstream")
        );
    }

    fn gateway_input(secret: Option<&str>) -> PaymentGatewayConfigWriteInput {
        PaymentGatewayConfigWriteInput {
            provider: "stripe".to_string(),
            enabled: true,
            endpoint_url: "https://api.stripe.com".to_string(),
            callback_base_url: Some("https://example.com".to_string()),
            merchant_id: "merchant".to_string(),
            merchant_key_encrypted: secret.map(ToOwned::to_owned),
            preserve_existing_secret: false,
            pay_currency: "USD".to_string(),
            usd_exchange_rate: 1.0,
            min_recharge_usd: 1.0,
            channels_json: json!({"channels": []}),
        }
    }

    #[tokio::test]
    async fn payment_gateway_cas_prevents_create_overwrite_and_uses_exact_secret_fence() {
        let repository = InMemoryBillingReadRepository::default();
        let create = PaymentGatewayConfigCasWriteInput {
            input: gateway_input(Some("ciphertext-a")),
            expected_existing: false,
            expected_merchant_key_encrypted: None,
        };
        assert!(matches!(
            repository
                .compare_and_swap_payment_gateway_config(&create)
                .await
                .expect("create should succeed"),
            AdminBillingMutationOutcome::Applied(_)
        ));

        let mut competing_create = create.clone();
        competing_create.input.merchant_id = "overwritten".to_string();
        assert_eq!(
            repository
                .compare_and_swap_payment_gateway_config(&competing_create)
                .await
                .expect("conflicting create should be handled"),
            AdminBillingMutationOutcome::NotFound
        );

        let mut stale_update = create.clone();
        stale_update.expected_existing = true;
        stale_update.expected_merchant_key_encrypted = Some("ciphertext-stale".to_string());
        stale_update.input.merchant_id = "stale-update".to_string();
        assert_eq!(
            repository
                .compare_and_swap_payment_gateway_config(&stale_update)
                .await
                .expect("stale update should be handled"),
            AdminBillingMutationOutcome::NotFound
        );
        let stored = repository
            .find_payment_gateway_config("stripe")
            .await
            .expect("lookup should succeed")
            .expect("config should exist");
        assert_eq!(stored.merchant_id, "merchant");
    }

    #[tokio::test]
    async fn payment_gateway_secret_cas_changes_no_other_fields() {
        let repository = InMemoryBillingReadRepository::default();
        repository
            .upsert_payment_gateway_config(&gateway_input(Some("legacy-ciphertext")))
            .await
            .expect("seed should succeed");
        let before = repository
            .find_payment_gateway_config("stripe")
            .await
            .expect("lookup should succeed")
            .expect("config should exist");

        assert!(!repository
            .compare_and_swap_payment_gateway_secret(&PaymentGatewaySecretCasUpdate {
                provider: "stripe".to_string(),
                expected_merchant_key_encrypted: "wrong-ciphertext".to_string(),
                merchant_key_encrypted: "v2-ciphertext".to_string(),
            })
            .await
            .expect("stale secret CAS should be handled"));
        assert!(repository
            .compare_and_swap_payment_gateway_secret(&PaymentGatewaySecretCasUpdate {
                provider: "stripe".to_string(),
                expected_merchant_key_encrypted: "legacy-ciphertext".to_string(),
                merchant_key_encrypted: "v2-ciphertext".to_string(),
            })
            .await
            .expect("secret CAS should succeed"));

        let mut expected = before.clone();
        expected.merchant_key_encrypted = Some("v2-ciphertext".to_string());
        let after = repository
            .find_payment_gateway_config("stripe")
            .await
            .expect("lookup should succeed")
            .expect("config should exist");
        assert_eq!(after, expected);
    }
}
