use super::*;
use crate::ai_serving::provider_key_pool_score_scope;
use aether_data_contracts::repository::pool_scores::{
    ListPoolMemberScoresQuery, PoolMemberHardState, POOL_KIND_PROVIDER_KEY_POOL,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

impl<'a> AdminAppState<'a> {
    pub(crate) async fn clear_admin_provider_pool_cooldown(&self, provider_id: &str, key_id: &str) {
        crate::handlers::admin::provider::pool::runtime::clear_admin_provider_pool_cooldown(
            self,
            provider_id,
            key_id,
        )
        .await
    }

    pub(crate) async fn reset_admin_provider_pool_cost(&self, provider_id: &str, key_id: &str) {
        crate::handlers::admin::provider::pool::runtime::reset_admin_provider_pool_cost(
            self,
            provider_id,
            key_id,
        )
        .await
    }

    pub(crate) fn put_provider_delete_task(&self, task: crate::LocalProviderDeleteTaskState) {
        self.app.put_provider_delete_task(task)
    }

    pub(crate) async fn run_admin_provider_delete_task(
        &self,
        provider_id: &str,
        task_id: &str,
    ) -> Result<crate::LocalProviderDeleteTaskState, GatewayError> {
        crate::handlers::admin::provider::delete_task::run_admin_provider_delete_task(
            self,
            provider_id,
            task_id,
        )
        .await
    }

    pub(crate) fn get_provider_delete_task(
        &self,
        task_id: &str,
    ) -> Option<crate::LocalProviderDeleteTaskState> {
        self.app.get_provider_delete_task(task_id)
    }

    pub(crate) fn get_admin_pool_batch_delete_task_for_provider(
        &self,
        provider_id: &str,
        task_id: &str,
    ) -> Result<crate::LocalProviderDeleteTaskState, Response<Body>> {
        let Some(task) = self.get_provider_delete_task(task_id) else {
            return Err((
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": "批量删除任务不存在" })),
            )
                .into_response());
        };
        if task.provider_id != provider_id {
            return Err((
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": "批量删除任务不存在" })),
            )
                .into_response());
        }
        Ok(task)
    }

    pub(crate) async fn build_admin_pool_batch_import_response(
        &self,
        provider_id: &str,
        payload: aether_admin::provider::pool::AdminPoolBatchImportRequest,
    ) -> Result<Response<Body>, GatewayError> {
        use aether_admin::provider::pool as admin_provider_pool_pure;

        let Some(provider) = self
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id.to_string()))
            .await?
            .into_iter()
            .next()
        else {
            return Ok((
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
            )
                .into_response());
        };

        let endpoints = self
            .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
            .await?;
        let existing_keys = self
            .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
            .await?;
        let available_api_formats =
            admin_provider_pool_pure::admin_pool_resolved_api_formats(&endpoints, &existing_keys);
        if available_api_formats.is_empty() {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "Provider 没有可用 endpoint 或现有 key，无法推断 api_formats" })),
            )
                .into_response());
        }

        let requested_api_formats = payload
            .api_formats
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let available_api_format_set = available_api_formats
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let api_formats = if requested_api_formats.is_empty() {
            available_api_formats.clone()
        } else {
            if let Some(unsupported) = requested_api_formats
                .iter()
                .find(|value| !available_api_format_set.contains(*value))
            {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": format!("Provider 不支持 api_format: {unsupported}") })),
                )
                    .into_response());
            }
            requested_api_formats
        };

        let mut settings_map = match payload.settings {
            Some(Value::Object(map)) => map,
            Some(_) => {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "settings payload must be an object" })),
                )
                    .into_response());
            }
            None => Map::new(),
        };
        if let Some(proxy_node_id) = payload.proxy_node_id {
            settings_map
                .entry("proxy_node_id".to_string())
                .or_insert(Value::String(proxy_node_id));
        }
        let shared_settings = (!settings_map.is_empty()).then_some(Value::Object(settings_map));
        if let Some(settings) = shared_settings.as_ref() {
            if let Err(detail) =
                admin_provider_pool_pure::validate_admin_pool_key_settings_payload(settings)
            {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": detail })),
                )
                    .into_response());
            }
        }

        let mut known_names = existing_keys
            .iter()
            .map(|key| key.name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect::<BTreeSet<_>>();
        let mut known_api_keys = existing_keys
            .iter()
            .filter_map(|key| key.encrypted_api_key.as_deref())
            .filter_map(|ciphertext| self.decrypt_catalog_secret_with_fallbacks(ciphertext))
            .filter(|value| value != "__placeholder__")
            .collect::<BTreeSet<_>>();
        let mut imported = 0usize;
        let skipped = 0usize;
        let mut errors = Vec::new();
        let now_unix_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0);

        for (index, item) in payload.keys.iter().enumerate() {
            let api_key = item.api_key.trim();
            if api_key.is_empty() {
                errors.push(json!({
                    "index": index,
                    "reason": "api_key is empty",
                }));
                continue;
            }

            let name = item.name.trim();
            if name.is_empty() {
                errors.push(json!({
                    "index": index,
                    "reason": "name is empty",
                }));
                continue;
            }
            if known_names.contains(name) {
                errors.push(json!({
                    "index": index,
                    "reason": "该名称已存在于当前 Provider 或本次导入中",
                }));
                continue;
            }

            let auth_type = item.auth_type.trim().to_ascii_lowercase();
            let auth_type = if auth_type.is_empty() {
                "api_key".to_string()
            } else {
                auth_type
            };
            if !matches!(auth_type.as_str(), "api_key" | "bearer") {
                errors.push(json!({
                    "index": index,
                    "reason": "auth_type must be api_key or bearer",
                }));
                continue;
            }
            let requested_item_api_formats = item
                .api_formats
                .iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let item_api_formats = if requested_item_api_formats.is_empty() {
                api_formats.clone()
            } else {
                if let Some(unsupported) = requested_item_api_formats
                    .iter()
                    .find(|value| !available_api_format_set.contains(*value))
                {
                    errors.push(json!({
                        "index": index,
                        "reason": format!("Provider 不支持 api_format: {unsupported}"),
                    }));
                    continue;
                }
                requested_item_api_formats
            };
            let item_settings = match admin_provider_pool_pure::resolve_admin_pool_key_settings(
                shared_settings.as_ref(),
                item.settings.as_ref(),
            ) {
                Ok(value) => value,
                Err(detail) => {
                    errors.push(json!({
                        "index": index,
                        "reason": detail,
                    }));
                    continue;
                }
            };
            if known_api_keys.contains(api_key) {
                errors.push(json!({
                    "index": index,
                    "reason": "该 API Key 已存在于当前 Provider 或本次导入中",
                }));
                continue;
            }

            let Some(encrypted_api_key) = self.encrypt_catalog_secret_with_fallbacks(api_key)
            else {
                errors.push(json!({
                    "index": index,
                    "reason": "gateway 未配置 provider key 加密密钥",
                }));
                continue;
            };
            let record = match admin_provider_pool_pure::build_admin_pool_batch_import_key_record(
                uuid::Uuid::new_v4().to_string(),
                provider.id.clone(),
                name.to_string(),
                auth_type,
                item_api_formats,
                encrypted_api_key,
                None,
                item_settings.as_ref(),
                now_unix_secs,
            ) {
                Ok(value) => value,
                Err(err) => {
                    errors.push(json!({
                        "index": index,
                        "reason": err.to_string(),
                    }));
                    continue;
                }
            };
            let Some(_) = self.create_provider_catalog_key(&record).await? else {
                return Ok((
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(
                        json!({ "detail": "Admin pool cleanup requires provider catalog writer" }),
                    ),
                )
                    .into_response());
            };
            known_names.insert(name.to_string());
            known_api_keys.insert(api_key.to_string());
            imported += 1;
        }

        Ok(Json(
            admin_provider_pool_pure::build_admin_pool_batch_import_result_payload(
                imported, skipped, errors,
            ),
        )
        .into_response())
    }

    pub(crate) async fn build_admin_pool_cleanup_banned_keys_response(
        &self,
        provider_id: &str,
    ) -> Result<Response<Body>, GatewayError> {
        let Some(provider) = self
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id.to_string()))
            .await?
            .into_iter()
            .next()
        else {
            return Ok((
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
            )
                .into_response());
        };

        let affected = self
            .cleanup_known_banned_provider_catalog_keys(&provider)
            .await?;
        if affected == 0 {
            return Ok(Json(
                aether_admin::provider::pool::build_admin_pool_cleanup_empty_payload(
                    "未发现可清理的异常账号",
                ),
            )
            .into_response());
        }

        Ok(
            Json(aether_admin::provider::pool::build_admin_pool_cleanup_result_payload(affected))
                .into_response(),
        )
    }

    pub(crate) async fn cleanup_known_banned_provider_catalog_keys(
        &self,
        provider: &StoredProviderCatalogProvider,
    ) -> Result<usize, GatewayError> {
        use aether_admin::provider::pool as admin_provider_pool_pure;

        let banned_keys = self
            .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
            .await?
            .into_iter()
            .filter(admin_provider_pool_pure::admin_pool_key_is_known_banned)
            .collect::<Vec<_>>();
        if banned_keys.is_empty() {
            return Ok(0);
        }

        let deleted_key_ids = banned_keys
            .iter()
            .map(|key| key.id.clone())
            .collect::<Vec<_>>();
        for key in &banned_keys {
            self.clear_admin_provider_pool_cooldown(&provider.id, &key.id)
                .await;
            self.reset_admin_provider_pool_cost(&provider.id, &key.id)
                .await;
        }

        let mut affected = 0usize;
        for key_id in &deleted_key_ids {
            if self.delete_provider_catalog_key(key_id).await? {
                affected += 1;
            }
        }
        self.cleanup_deleted_provider_catalog_refs(&provider.id, false, &[], &deleted_key_ids)
            .await?;

        Ok(affected)
    }

    pub(crate) async fn cleanup_quota_exhausted_provider_catalog_keys(
        &self,
        provider: &StoredProviderCatalogProvider,
        provider_type: &str,
    ) -> Result<usize, GatewayError> {
        use aether_admin::provider::pool as admin_provider_pool_pure;

        let keys = self
            .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
            .await?;
        if keys.is_empty() {
            return Ok(0);
        }

        let known_key_ids = keys
            .iter()
            .map(|key| key.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let mut exhausted_key_ids = keys
            .iter()
            .filter(|key| {
                admin_provider_pool_pure::admin_pool_key_account_quota_exhausted(key, provider_type)
            })
            .map(|key| key.id.clone())
            .collect::<std::collections::BTreeSet<_>>();

        if self.app().data.has_pool_score_reader() {
            let scope = provider_key_pool_score_scope();
            let page_size = 10_000usize;
            let mut offset = 0usize;
            loop {
                let scores = self
                    .app()
                    .data
                    .list_pool_member_scores(&ListPoolMemberScoresQuery {
                        pool_kind: POOL_KIND_PROVIDER_KEY_POOL.to_string(),
                        pool_id: provider.id.clone(),
                        capability: Some(scope.capability.clone()),
                        scope_kind: Some(scope.scope_kind.clone()),
                        scope_id: scope.scope_id.clone(),
                        hard_states: vec![PoolMemberHardState::QuotaExhausted],
                        probe_statuses: None,
                        offset,
                        limit: page_size,
                    })
                    .await
                    .map_err(|err| GatewayError::Internal(err.to_string()))?;
                if scores.is_empty() {
                    break;
                }
                let page_len = scores.len();
                for score in scores {
                    if known_key_ids.contains(score.member_id.as_str()) {
                        exhausted_key_ids.insert(score.member_id);
                    }
                }
                if page_len < page_size {
                    break;
                }
                offset = offset.saturating_add(page_size);
            }
        }

        let exhausted_keys = keys
            .iter()
            .filter(|key| exhausted_key_ids.contains(&key.id))
            .collect::<Vec<_>>();
        if exhausted_keys.is_empty() {
            return Ok(0);
        }

        let deleted_key_ids = exhausted_keys
            .iter()
            .map(|key| key.id.clone())
            .collect::<Vec<_>>();
        for key in exhausted_keys {
            self.clear_admin_provider_pool_cooldown(&provider.id, &key.id)
                .await;
            self.reset_admin_provider_pool_cost(&provider.id, &key.id)
                .await;
        }

        let mut affected = 0usize;
        for key_id in &deleted_key_ids {
            if self.delete_provider_catalog_key(key_id).await? {
                affected += 1;
            }
        }
        self.cleanup_deleted_provider_catalog_refs(&provider.id, false, &[], &deleted_key_ids)
            .await?;

        Ok(affected)
    }

    pub(crate) async fn cleanup_provider_catalog_key_if_current<F>(
        &self,
        provider: &StoredProviderCatalogProvider,
        key_id: &str,
        should_delete: F,
    ) -> Result<bool, GatewayError>
    where
        F: FnOnce(&StoredProviderCatalogKey) -> bool,
    {
        let key_ids = [key_id.to_string()];
        let Some(key) = self
            .read_provider_catalog_keys_by_ids(&key_ids)
            .await?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };
        if key.provider_id != provider.id || !should_delete(&key) {
            return Ok(false);
        }

        self.clear_admin_provider_pool_cooldown(&provider.id, &key.id)
            .await;
        self.reset_admin_provider_pool_cost(&provider.id, &key.id)
            .await;
        let deleted = self.delete_provider_catalog_key(&key.id).await?;
        if deleted {
            let deleted_key_ids = [key.id.clone()];
            self.cleanup_deleted_provider_catalog_refs(&provider.id, false, &[], &deleted_key_ids)
                .await?;
        }
        Ok(deleted)
    }

    pub(crate) async fn build_admin_pool_batch_action_response(
        &self,
        provider_id: &str,
        payload: aether_admin::provider::pool::AdminPoolBatchActionRequest,
    ) -> Result<Response<Body>, GatewayError> {
        use aether_admin::provider::pool::{
            self as admin_provider_pool_pure, AdminPoolBatchActionKind,
        };

        let Some(provider) = self
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id.to_string()))
            .await?
            .into_iter()
            .next()
        else {
            return Ok((
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
            )
                .into_response());
        };

        let plan = match admin_provider_pool_pure::build_admin_pool_batch_action_plan(payload) {
            Ok(plan) => plan,
            Err(detail) => {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": detail })),
                )
                    .into_response());
            }
        };

        let keys = self
            .read_provider_catalog_keys_by_ids(&plan.key_ids)
            .await?
            .into_iter()
            .filter(|key| key.provider_id == provider.id)
            .collect::<Vec<_>>();

        if plan.action == AdminPoolBatchActionKind::Delete {
            let deleted_key_ids = keys.iter().map(|key| key.id.clone()).collect::<Vec<_>>();
            for key in &keys {
                self.clear_admin_provider_pool_cooldown(&provider.id, &key.id)
                    .await;
                self.reset_admin_provider_pool_cost(&provider.id, &key.id)
                    .await;
            }

            let mut affected = 0usize;
            for key_id in &deleted_key_ids {
                if self.delete_provider_catalog_key(key_id).await? {
                    affected = affected.saturating_add(1);
                }
            }
            self.cleanup_deleted_provider_catalog_refs(&provider.id, false, &[], &deleted_key_ids)
                .await?;

            return Ok(Json(
                admin_provider_pool_pure::build_admin_pool_batch_action_result_payload(
                    affected,
                    plan.action_label,
                ),
            )
            .into_response());
        }

        let mut updated_keys = Vec::with_capacity(keys.len());
        for mut key in keys {
            match plan.action {
                AdminPoolBatchActionKind::Enable => key.is_active = true,
                AdminPoolBatchActionKind::Disable => key.is_active = false,
                AdminPoolBatchActionKind::ClearProxy => key.proxy = None,
                AdminPoolBatchActionKind::SetProxy => key.proxy = plan.proxy_payload.clone(),
                AdminPoolBatchActionKind::UpdateSettings => {
                    if let Some(settings) = plan.settings_payload.as_ref() {
                        admin_provider_pool_pure::apply_admin_pool_key_settings(&mut key, settings)
                            .map_err(GatewayError::Internal)?;
                    }
                }
                AdminPoolBatchActionKind::RegenerateFingerprint => {
                    key.fingerprint =
                        Some(aether_provider_transport::claude_code::generate_random_fingerprint())
                }
                AdminPoolBatchActionKind::Delete => unreachable!(),
            }
            updated_keys.push(key);
        }
        let affected = self
            .update_provider_catalog_keys(&updated_keys)
            .await?
            .map(|keys| keys.len())
            .unwrap_or(0);

        Ok(Json(
            admin_provider_pool_pure::build_admin_pool_batch_action_result_payload(
                affected,
                plan.action_label,
            ),
        )
        .into_response())
    }

    pub(crate) async fn build_admin_pool_batch_update_response(
        &self,
        provider_id: &str,
        payload: crate::handlers::admin::provider::shared::payloads::AdminProviderKeyBatchUpdateRequest,
    ) -> Result<Response<Body>, GatewayError> {
        use crate::handlers::admin::provider::pool_admin::admin_provider_pool_config;
        use crate::handlers::admin::provider::shared::payloads::AdminProviderKeyUpdatePatch;
        use crate::handlers::admin::provider::write::keys::{
            admin_provider_key_update_requires_immediate_model_fetch,
            build_admin_update_provider_key_record_with_existing_keys,
            parse_admin_provider_key_batch_update_patch,
        };
        use crate::maintenance::ensure_provider_key_pool_scores_for_keys;
        use crate::model_fetch::perform_model_fetch_for_keys;

        let Some(provider) = self
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id.to_string()))
            .await?
            .into_iter()
            .next()
        else {
            return Ok((
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
            )
                .into_response());
        };

        let requested_key_ids = payload
            .key_ids
            .into_iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect::<BTreeSet<_>>();
        if requested_key_ids.is_empty() {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "key_ids 不能为空" })),
            )
                .into_response());
        }

        let patch = match parse_admin_provider_key_batch_update_patch(payload.patch) {
            Ok(patch) => patch,
            Err(detail) => {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": detail })),
                )
                    .into_response());
            }
        };

        let provider_ids = vec![provider.id.clone()];
        let existing_keys = self
            .list_provider_catalog_keys_by_provider_ids(&provider_ids)
            .await?;
        let keys_by_id = existing_keys
            .iter()
            .map(|key| (key.id.clone(), key))
            .collect::<BTreeMap<_, _>>();
        let missing_key_ids = requested_key_ids
            .iter()
            .filter(|key_id| !keys_by_id.contains_key(*key_id))
            .cloned()
            .collect::<Vec<_>>();
        if !missing_key_ids.is_empty() {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({
                    "detail": format!(
                        "以下密钥不存在或不属于当前 Provider: {}",
                        missing_key_ids.join(", ")
                    )
                })),
            )
                .into_response());
        }

        let mut staged_updates = Vec::with_capacity(requested_key_ids.len());
        for key_id in &requested_key_ids {
            let existing = keys_by_id
                .get(key_id)
                .expect("validated provider key should exist");
            let typed_patch = AdminProviderKeyUpdatePatch::from_object(patch.clone())
                .expect("validated batch patch should remain parseable");
            let updated = match build_admin_update_provider_key_record_with_existing_keys(
                self,
                &provider,
                existing,
                &existing_keys,
                typed_patch,
            ) {
                Ok(updated) => updated,
                Err(detail) => {
                    return Ok((
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({
                            "detail": format!("密钥 {} 配置无效: {detail}", existing.name)
                        })),
                    )
                        .into_response());
                }
            };
            staged_updates.push(((*existing).clone(), updated));
        }

        let model_fetch_key_ids = staged_updates
            .iter()
            .filter(|(existing, updated)| {
                admin_provider_key_update_requires_immediate_model_fetch(existing, updated)
            })
            .map(|(_, updated)| updated.id.clone())
            .collect::<BTreeSet<_>>();

        let staged_records = staged_updates
            .iter()
            .map(|(_, updated)| updated.clone())
            .collect::<Vec<_>>();
        let Some(mut updated_keys) = self.update_provider_catalog_keys(&staged_records).await?
        else {
            return Ok((
                http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "detail": "Provider 密钥写入能力不可用" })),
            )
                .into_response());
        };

        for (existing, requested) in &staged_updates {
            if requested.learned_rpm_limit == existing.learned_rpm_limit {
                continue;
            }
            let Some(reloaded) = self
                .set_provider_catalog_key_learned_rpm_limit(
                    &requested.id,
                    requested.learned_rpm_limit,
                    requested.updated_at_unix_secs,
                )
                .await?
            else {
                return Ok((
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "detail": format!("Provider 密钥 {} 已不存在", requested.id) })),
                )
                    .into_response());
            };
            if let Some(updated) = updated_keys.iter_mut().find(|key| key.id == requested.id) {
                *updated = reloaded;
            }
        }

        let endpoints = self
            .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
            .await?;
        if let Some(pool_config) = admin_provider_pool_config(&provider) {
            let now_unix_secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            let score_ensure_budget =
                (pool_config.score_fallback_scan_limit as usize).clamp(1, 50_000);
            if let Err(err) = ensure_provider_key_pool_scores_for_keys(
                self.as_ref(),
                &provider,
                &pool_config,
                &endpoints,
                &updated_keys,
                now_unix_secs,
                score_ensure_budget,
            )
            .await
            {
                tracing::debug!(
                    provider_id = %provider.id,
                    updated_keys = updated_keys.len(),
                    error = ?err,
                    "gateway admin provider key batch update: failed to seed pool score rows"
                );
            }
        }

        let model_sync = if model_fetch_key_ids.is_empty() {
            serde_json::Value::Null
        } else {
            let requested = model_fetch_key_ids.len();
            match perform_model_fetch_for_keys(self.as_ref(), &provider.id, &model_fetch_key_ids)
                .await
            {
                Ok(summary) => json!({
                    "requested": requested,
                    "attempted": summary.attempted,
                    "succeeded": summary.succeeded,
                    "failed": summary.failed,
                    "skipped": summary.skipped,
                }),
                Err(err) => json!({
                    "requested": requested,
                    "attempted": 0,
                    "succeeded": 0,
                    "failed": requested,
                    "skipped": 0,
                    "error": err.into_message(),
                }),
            }
        };

        let affected = updated_keys.len();
        Ok(Json(json!({
            "affected": affected,
            "message": format!("已更新 {affected} 个密钥"),
            "model_sync": model_sync,
        }))
        .into_response())
    }
}
