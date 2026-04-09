use super::*;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

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
        let api_formats =
            admin_provider_pool_pure::admin_pool_resolved_api_formats(&endpoints, &existing_keys);
        if api_formats.is_empty() {
            return Ok((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "Provider 没有可用 endpoint 或现有 key，无法推断 api_formats" })),
            )
                .into_response());
        }

        let proxy =
            admin_provider_pool_pure::admin_pool_key_proxy_value(payload.proxy_node_id.as_deref());
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

            let Some(encrypted_api_key) = self.encrypt_catalog_secret_with_fallbacks(api_key)
            else {
                errors.push(json!({
                    "index": index,
                    "reason": "gateway 未配置 provider key 加密密钥",
                }));
                continue;
            };

            let auth_type = item.auth_type.trim().to_ascii_lowercase();
            let auth_type = if auth_type.is_empty() {
                "api_key".to_string()
            } else {
                auth_type
            };
            let name = item.name.trim();
            let record = match admin_provider_pool_pure::build_admin_pool_batch_import_key_record(
                uuid::Uuid::new_v4().to_string(),
                provider.id.clone(),
                if name.is_empty() {
                    format!("imported-{index}")
                } else {
                    name.to_string()
                },
                auth_type,
                api_formats.clone(),
                encrypted_api_key,
                proxy.clone(),
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

        let banned_keys = self
            .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
            .await?
            .into_iter()
            .filter(admin_provider_pool_pure::admin_pool_key_is_known_banned)
            .collect::<Vec<_>>();
        if banned_keys.is_empty() {
            return Ok(Json(
                admin_provider_pool_pure::build_admin_pool_cleanup_empty_payload(
                    "未发现可清理的异常账号",
                ),
            )
            .into_response());
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
        self.cleanup_deleted_provider_catalog_refs(&provider.id, &[], &deleted_key_ids)
            .await?;

        Ok(
            Json(admin_provider_pool_pure::build_admin_pool_cleanup_result_payload(affected))
                .into_response(),
        )
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
            self.cleanup_deleted_provider_catalog_refs(&provider.id, &[], &deleted_key_ids)
                .await?;

            return Ok(Json(
                admin_provider_pool_pure::build_admin_pool_batch_action_result_payload(
                    affected,
                    plan.action_label,
                ),
            )
            .into_response());
        }

        let mut affected = 0usize;
        for mut key in keys {
            match plan.action {
                AdminPoolBatchActionKind::Enable => key.is_active = true,
                AdminPoolBatchActionKind::Disable => key.is_active = false,
                AdminPoolBatchActionKind::ClearProxy => key.proxy = None,
                AdminPoolBatchActionKind::SetProxy => key.proxy = plan.proxy_payload.clone(),
                AdminPoolBatchActionKind::RegenerateFingerprint => {
                    key.fingerprint =
                        Some(aether_provider_transport::claude_code::generate_random_fingerprint())
                }
                AdminPoolBatchActionKind::Delete => unreachable!(),
            }
            if self.update_provider_catalog_key(&key).await?.is_some() {
                affected = affected.saturating_add(1);
            }
        }

        Ok(Json(
            admin_provider_pool_pure::build_admin_pool_batch_action_result_payload(
                affected,
                plan.action_label,
            ),
        )
        .into_response())
    }
}
