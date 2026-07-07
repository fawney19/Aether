use super::{
    AdminBillingCollectorRecord, AdminBillingCollectorWriteInput, AdminBillingMutationOutcome,
    AdminBillingPresetApplyResult, AdminBillingRuleRecord, AdminBillingRuleWriteInput, AppState,
    BillingPlanRecord, BillingPlanWriteInput, GatewayError, LocalMutationOutcome,
    PaymentGatewayConfigRecord, PaymentGatewayConfigWriteInput, UserDailyQuotaAvailabilityRecord,
    UserPlanEntitlementRecord,
};

fn data_error(err: impl ToString) -> GatewayError {
    GatewayError::Internal(err.to_string())
}

fn local_mutation_outcome<T>(outcome: AdminBillingMutationOutcome<T>) -> LocalMutationOutcome<T> {
    match outcome {
        AdminBillingMutationOutcome::Applied(value) => LocalMutationOutcome::Applied(value),
        AdminBillingMutationOutcome::NotFound => LocalMutationOutcome::NotFound,
        AdminBillingMutationOutcome::Invalid(detail) => LocalMutationOutcome::Invalid(detail),
        AdminBillingMutationOutcome::Unavailable => LocalMutationOutcome::Unavailable,
    }
}

impl AppState {
    pub(crate) async fn admin_billing_enabled_default_value_exists(
        &self,
        api_format: &str,
        task_type: &str,
        dimension_name: &str,
        existing_id: Option<&str>,
    ) -> Result<bool, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_billing_collector_store.as_ref() {
            let exists = store
                .lock()
                .expect("admin billing collector store should lock")
                .values()
                .any(|collector| {
                    collector.api_format == api_format
                        && collector.task_type == task_type
                        && collector.dimension_name == dimension_name
                        && collector.is_enabled
                        && collector.default_value.is_some()
                        && existing_id.is_none_or(|value| collector.id != value)
                });
            return Ok(exists);
        }

        Ok(self
            .data
            .admin_billing_enabled_default_value_exists(
                api_format,
                task_type,
                dimension_name,
                existing_id,
            )
            .await
            .map_err(data_error)?
            .unwrap_or(false))
    }

    pub(crate) async fn create_admin_billing_rule(
        &self,
        input: &AdminBillingRuleWriteInput,
    ) -> Result<LocalMutationOutcome<AdminBillingRuleRecord>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_billing_rule_store.as_ref() {
            let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
            let record = AdminBillingRuleRecord {
                id: uuid::Uuid::new_v4().to_string(),
                name: input.name.clone(),
                task_type: input.task_type.clone(),
                global_model_id: input.global_model_id.clone(),
                model_id: input.model_id.clone(),
                expression: input.expression.clone(),
                variables: input.variables.clone(),
                dimension_mappings: input.dimension_mappings.clone(),
                is_enabled: input.is_enabled,
                created_at_unix_ms: now_unix_secs,
                updated_at_unix_secs: now_unix_secs,
            };
            store
                .lock()
                .expect("admin billing rule store should lock")
                .insert(record.id.clone(), record.clone());
            return Ok(LocalMutationOutcome::Applied(record));
        }

        self.data
            .create_admin_billing_rule(input)
            .await
            .map(local_mutation_outcome)
            .map_err(data_error)
    }

    pub(crate) async fn list_admin_billing_rules(
        &self,
        task_type: Option<&str>,
        is_enabled: Option<bool>,
        page: u32,
        page_size: u32,
    ) -> Result<Option<(Vec<AdminBillingRuleRecord>, u64)>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_billing_rule_store.as_ref() {
            let mut items = store
                .lock()
                .expect("admin billing rule store should lock")
                .values()
                .filter(|record| {
                    task_type.is_none_or(|expected| record.task_type == expected)
                        && is_enabled.is_none_or(|expected| record.is_enabled == expected)
                })
                .cloned()
                .collect::<Vec<_>>();
            items.sort_by(|left, right| {
                right
                    .updated_at_unix_secs
                    .cmp(&left.updated_at_unix_secs)
                    .then_with(|| right.id.cmp(&left.id))
            });
            let total = items.len() as u64;
            let offset = (page.saturating_sub(1) as usize) * (page_size as usize);
            let items = items
                .into_iter()
                .skip(offset)
                .take(page_size as usize)
                .collect::<Vec<_>>();
            return Ok(Some((items, total)));
        }

        self.data
            .list_admin_billing_rules(task_type, is_enabled, page, page_size)
            .await
            .map_err(data_error)
    }

    pub(crate) async fn read_admin_billing_rule(
        &self,
        rule_id: &str,
    ) -> Result<Option<AdminBillingRuleRecord>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_billing_rule_store.as_ref() {
            return Ok(store
                .lock()
                .expect("admin billing rule store should lock")
                .get(rule_id)
                .cloned());
        }

        self.data
            .find_admin_billing_rule(rule_id)
            .await
            .map_err(data_error)
    }

    pub(crate) async fn update_admin_billing_rule(
        &self,
        rule_id: &str,
        input: &AdminBillingRuleWriteInput,
    ) -> Result<LocalMutationOutcome<AdminBillingRuleRecord>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_billing_rule_store.as_ref() {
            let mut guard = store.lock().expect("admin billing rule store should lock");
            let Some(record) = guard.get_mut(rule_id) else {
                return Ok(LocalMutationOutcome::NotFound);
            };
            record.name = input.name.clone();
            record.task_type = input.task_type.clone();
            record.global_model_id = input.global_model_id.clone();
            record.model_id = input.model_id.clone();
            record.expression = input.expression.clone();
            record.variables = input.variables.clone();
            record.dimension_mappings = input.dimension_mappings.clone();
            record.is_enabled = input.is_enabled;
            record.updated_at_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
            return Ok(LocalMutationOutcome::Applied(record.clone()));
        }

        self.data
            .update_admin_billing_rule(rule_id, input)
            .await
            .map(local_mutation_outcome)
            .map_err(data_error)
    }

    pub(crate) async fn create_admin_billing_collector(
        &self,
        input: &AdminBillingCollectorWriteInput,
    ) -> Result<LocalMutationOutcome<AdminBillingCollectorRecord>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_billing_collector_store.as_ref() {
            let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
            let record = AdminBillingCollectorRecord {
                id: uuid::Uuid::new_v4().to_string(),
                api_format: input.api_format.clone(),
                task_type: input.task_type.clone(),
                dimension_name: input.dimension_name.clone(),
                source_type: input.source_type.clone(),
                source_path: input.source_path.clone(),
                value_type: input.value_type.clone(),
                transform_expression: input.transform_expression.clone(),
                default_value: input.default_value.clone(),
                priority: input.priority,
                is_enabled: input.is_enabled,
                created_at_unix_ms: now_unix_secs,
                updated_at_unix_secs: now_unix_secs,
            };
            store
                .lock()
                .expect("admin billing collector store should lock")
                .insert(record.id.clone(), record.clone());
            return Ok(LocalMutationOutcome::Applied(record));
        }

        self.data
            .create_admin_billing_collector(input)
            .await
            .map(local_mutation_outcome)
            .map_err(data_error)
    }

    pub(crate) async fn list_admin_billing_collectors(
        &self,
        api_format: Option<&str>,
        task_type: Option<&str>,
        dimension_name: Option<&str>,
        is_enabled: Option<bool>,
        page: u32,
        page_size: u32,
    ) -> Result<Option<(Vec<AdminBillingCollectorRecord>, u64)>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_billing_collector_store.as_ref() {
            let mut items = store
                .lock()
                .expect("admin billing collector store should lock")
                .values()
                .filter(|record| {
                    api_format.is_none_or(|expected| record.api_format == expected)
                        && task_type.is_none_or(|expected| record.task_type == expected)
                        && dimension_name.is_none_or(|expected| record.dimension_name == expected)
                        && is_enabled.is_none_or(|expected| record.is_enabled == expected)
                })
                .cloned()
                .collect::<Vec<_>>();
            items.sort_by(|left, right| {
                right
                    .updated_at_unix_secs
                    .cmp(&left.updated_at_unix_secs)
                    .then_with(|| right.priority.cmp(&left.priority))
                    .then_with(|| right.id.cmp(&left.id))
            });
            let total = items.len() as u64;
            let offset = (page.saturating_sub(1) as usize) * (page_size as usize);
            let items = items
                .into_iter()
                .skip(offset)
                .take(page_size as usize)
                .collect::<Vec<_>>();
            return Ok(Some((items, total)));
        }

        self.data
            .list_admin_billing_collectors(
                api_format,
                task_type,
                dimension_name,
                is_enabled,
                page,
                page_size,
            )
            .await
            .map_err(data_error)
    }

    pub(crate) async fn read_admin_billing_collector(
        &self,
        collector_id: &str,
    ) -> Result<Option<AdminBillingCollectorRecord>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_billing_collector_store.as_ref() {
            return Ok(store
                .lock()
                .expect("admin billing collector store should lock")
                .get(collector_id)
                .cloned());
        }

        self.data
            .find_admin_billing_collector(collector_id)
            .await
            .map_err(data_error)
    }

    pub(crate) async fn update_admin_billing_collector(
        &self,
        collector_id: &str,
        input: &AdminBillingCollectorWriteInput,
    ) -> Result<LocalMutationOutcome<AdminBillingCollectorRecord>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_billing_collector_store.as_ref() {
            let mut guard = store
                .lock()
                .expect("admin billing collector store should lock");
            let Some(record) = guard.get_mut(collector_id) else {
                return Ok(LocalMutationOutcome::NotFound);
            };
            record.api_format = input.api_format.clone();
            record.task_type = input.task_type.clone();
            record.dimension_name = input.dimension_name.clone();
            record.source_type = input.source_type.clone();
            record.source_path = input.source_path.clone();
            record.value_type = input.value_type.clone();
            record.transform_expression = input.transform_expression.clone();
            record.default_value = input.default_value.clone();
            record.priority = input.priority;
            record.is_enabled = input.is_enabled;
            record.updated_at_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
            return Ok(LocalMutationOutcome::Applied(record.clone()));
        }

        self.data
            .update_admin_billing_collector(collector_id, input)
            .await
            .map(local_mutation_outcome)
            .map_err(data_error)
    }

    pub(crate) async fn apply_admin_billing_preset(
        &self,
        preset: &str,
        mode: &str,
        collectors: &[AdminBillingCollectorWriteInput],
    ) -> Result<LocalMutationOutcome<AdminBillingPresetApplyResult>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_billing_collector_store.as_ref() {
            let mut created = 0_u64;
            let mut updated = 0_u64;
            let mut skipped = 0_u64;
            let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
            let mut guard = store
                .lock()
                .expect("admin billing collector store should lock");
            for collector in collectors {
                let existing_id = guard
                    .values()
                    .find(|record| {
                        record.api_format == collector.api_format
                            && record.task_type == collector.task_type
                            && record.dimension_name == collector.dimension_name
                            && record.priority == collector.priority
                            && record.is_enabled
                    })
                    .map(|record| record.id.clone());

                match existing_id {
                    Some(existing_id) if mode == "overwrite" => {
                        if let Some(record) = guard.get_mut(&existing_id) {
                            record.source_type = collector.source_type.clone();
                            record.source_path = collector.source_path.clone();
                            record.value_type = collector.value_type.clone();
                            record.transform_expression = collector.transform_expression.clone();
                            record.default_value = collector.default_value.clone();
                            record.is_enabled = collector.is_enabled;
                            record.updated_at_unix_secs = now_unix_secs;
                            updated += 1;
                        } else {
                            skipped += 1;
                        }
                    }
                    Some(_) => {
                        skipped += 1;
                    }
                    None => {
                        let record = AdminBillingCollectorRecord {
                            id: uuid::Uuid::new_v4().to_string(),
                            api_format: collector.api_format.clone(),
                            task_type: collector.task_type.clone(),
                            dimension_name: collector.dimension_name.clone(),
                            source_type: collector.source_type.clone(),
                            source_path: collector.source_path.clone(),
                            value_type: collector.value_type.clone(),
                            transform_expression: collector.transform_expression.clone(),
                            default_value: collector.default_value.clone(),
                            priority: collector.priority,
                            is_enabled: collector.is_enabled,
                            created_at_unix_ms: now_unix_secs,
                            updated_at_unix_secs: now_unix_secs,
                        };
                        guard.insert(record.id.clone(), record);
                        created += 1;
                    }
                }
            }
            return Ok(LocalMutationOutcome::Applied(
                AdminBillingPresetApplyResult {
                    preset: preset.to_string(),
                    mode: mode.to_string(),
                    created,
                    updated,
                    skipped,
                    errors: Vec::new(),
                },
            ));
        }

        self.data
            .apply_admin_billing_preset(preset, mode, collectors)
            .await
            .map(local_mutation_outcome)
            .map_err(data_error)
    }

    pub(crate) async fn find_payment_gateway_config(
        &self,
        provider: &str,
    ) -> Result<Option<PaymentGatewayConfigRecord>, GatewayError> {
        self.data
            .find_payment_gateway_config(provider)
            .await
            .map_err(data_error)
    }

    pub(crate) async fn upsert_payment_gateway_config(
        &self,
        input: &PaymentGatewayConfigWriteInput,
    ) -> Result<LocalMutationOutcome<PaymentGatewayConfigRecord>, GatewayError> {
        self.data
            .upsert_payment_gateway_config(input)
            .await
            .map(local_mutation_outcome)
            .map_err(data_error)
    }

    pub(crate) async fn list_billing_plans(
        &self,
        include_disabled: bool,
    ) -> Result<Option<Vec<BillingPlanRecord>>, GatewayError> {
        self.data
            .list_billing_plans(include_disabled)
            .await
            .map_err(data_error)
    }

    pub(crate) async fn find_billing_plan(
        &self,
        plan_id: &str,
    ) -> Result<Option<BillingPlanRecord>, GatewayError> {
        self.data
            .find_billing_plan(plan_id)
            .await
            .map_err(data_error)
    }

    pub(crate) async fn create_billing_plan(
        &self,
        input: &BillingPlanWriteInput,
    ) -> Result<LocalMutationOutcome<BillingPlanRecord>, GatewayError> {
        self.data
            .create_billing_plan(input)
            .await
            .map(local_mutation_outcome)
            .map_err(data_error)
    }

    pub(crate) async fn update_billing_plan(
        &self,
        plan_id: &str,
        input: &BillingPlanWriteInput,
    ) -> Result<LocalMutationOutcome<BillingPlanRecord>, GatewayError> {
        self.data
            .update_billing_plan(plan_id, input)
            .await
            .map(local_mutation_outcome)
            .map_err(data_error)
    }

    pub(crate) async fn set_billing_plan_enabled(
        &self,
        plan_id: &str,
        enabled: bool,
    ) -> Result<LocalMutationOutcome<BillingPlanRecord>, GatewayError> {
        self.data
            .set_billing_plan_enabled(plan_id, enabled)
            .await
            .map(local_mutation_outcome)
            .map_err(data_error)
    }

    pub(crate) async fn delete_billing_plan(
        &self,
        plan_id: &str,
    ) -> Result<LocalMutationOutcome<()>, GatewayError> {
        self.data
            .delete_billing_plan(plan_id)
            .await
            .map(local_mutation_outcome)
            .map_err(data_error)
    }

    pub(crate) async fn list_user_plan_entitlements(
        &self,
        user_id: &str,
    ) -> Result<Option<Vec<UserPlanEntitlementRecord>>, GatewayError> {
        self.data
            .list_user_plan_entitlements(user_id)
            .await
            .map_err(data_error)
    }

    pub(crate) async fn find_user_daily_quota_availability(
        &self,
        user_id: &str,
    ) -> Result<Option<UserDailyQuotaAvailabilityRecord>, GatewayError> {
        self.data
            .find_user_daily_quota_availability(user_id)
            .await
            .map_err(data_error)
    }

    pub(crate) async fn find_user_daily_quota_availability_for_auth(
        &self,
        user_id: &str,
    ) -> Result<Option<UserDailyQuotaAvailabilityRecord>, GatewayError> {
        let user_id = user_id.trim();
        if user_id.is_empty() {
            return Ok(None);
        }
        let ttl = self.frontdoor_runtime_guards.auth_capacity_cache_ttl;
        if ttl.is_zero() {
            return self.find_user_daily_quota_availability(user_id).await;
        }
        self.auth_daily_quota_availability_cache
            .get_or_load(user_id.to_string(), ttl, || async move {
                self.find_user_daily_quota_availability(user_id).await
            })
            .await
    }
}
