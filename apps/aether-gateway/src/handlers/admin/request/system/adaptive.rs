use crate::handlers::admin::request::AdminAppState;
use crate::handlers::admin::shared::build_proxy_error_response;
use crate::GatewayError;
use aether_admin::system::{
    admin_adaptive_dispatcher_not_found_response, admin_adaptive_key_not_found_response,
    admin_adaptive_key_payload, build_admin_adaptive_reset_learning_payload,
    build_admin_adaptive_set_limit_payload, build_admin_adaptive_stats_payload,
    build_admin_adaptive_summary_payload, build_admin_adaptive_toggle_mode_payload,
};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn build_admin_adaptive_keys_response(
        &self,
        provider_id: Option<&str>,
    ) -> Result<Response<Body>, GatewayError> {
        let payload = self
            .load_admin_adaptive_candidate_keys(provider_id)
            .await?
            .into_iter()
            .filter(|key| key.rpm_limit.is_none())
            .map(|key| admin_adaptive_key_payload(&key))
            .collect::<Vec<_>>();
        Ok(Json(payload).into_response())
    }

    pub(crate) async fn build_admin_adaptive_summary_response(
        &self,
    ) -> Result<Response<Body>, GatewayError> {
        let keys = self.load_admin_adaptive_candidate_keys(None).await?;
        Ok(Json(build_admin_adaptive_summary_payload(&keys)).into_response())
    }

    pub(crate) async fn build_admin_adaptive_stats_response(
        &self,
        key_id: &str,
    ) -> Result<Response<Body>, GatewayError> {
        let Some(key) = self.find_admin_adaptive_key(key_id).await? else {
            return Ok(admin_adaptive_key_not_found_response(key_id));
        };
        Ok(Json(build_admin_adaptive_stats_payload(&key)).into_response())
    }

    pub(crate) async fn toggle_admin_adaptive_mode_response(
        &self,
        key_id: &str,
        request_body: &Bytes,
    ) -> Result<Response<Body>, GatewayError> {
        #[derive(Debug, Deserialize)]
        struct AdminAdaptiveToggleModeRequest {
            enabled: bool,
            #[serde(default)]
            fixed_limit: Option<u32>,
        }

        let body = match serde_json::from_slice::<AdminAdaptiveToggleModeRequest>(request_body) {
            Ok(payload) => payload,
            Err(_) => {
                return Ok(build_proxy_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "请求数据验证失败",
                    None,
                ));
            }
        };
        let Some(mut key) = self.find_admin_adaptive_key(key_id).await? else {
            return Ok(admin_adaptive_key_not_found_response(key_id));
        };
        let message = if body.enabled {
            key.rpm_limit = None;
            "已切换为自适应模式，系统将自动学习并调整 RPM 限制".to_string()
        } else {
            let Some(fixed_limit) = body.fixed_limit else {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({
                        "detail": "禁用自适应模式时必须提供 fixed_limit 参数",
                    })),
                )
                    .into_response());
            };
            if !(1..=100).contains(&fixed_limit) {
                return Ok((
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({
                        "detail": "fixed_limit 超出范围（1-100）",
                    })),
                )
                    .into_response());
            }
            key.rpm_limit = Some(fixed_limit);
            format!("已切换为固定限制模式，RPM 限制设为 {fixed_limit}")
        };
        let Some(updated) = self.update_provider_catalog_key(&key).await? else {
            return Ok(admin_adaptive_key_not_found_response(key_id));
        };
        Ok(Json(build_admin_adaptive_toggle_mode_payload(&updated, message)).into_response())
    }

    pub(crate) async fn set_admin_adaptive_limit_response(
        &self,
        key_id: &str,
        limit: u32,
    ) -> Result<Response<Body>, GatewayError> {
        let Some(mut key) = self.find_admin_adaptive_key(key_id).await? else {
            return Ok(admin_adaptive_key_not_found_response(key_id));
        };
        let was_adaptive = key.rpm_limit.is_none();
        key.rpm_limit = Some(limit);
        let Some(updated) = self.update_provider_catalog_key(&key).await? else {
            return Ok(admin_adaptive_key_not_found_response(key_id));
        };
        Ok(Json(build_admin_adaptive_set_limit_payload(
            &updated,
            was_adaptive,
            limit,
        ))
        .into_response())
    }

    pub(crate) async fn reset_admin_adaptive_learning_response(
        &self,
        key_id: &str,
    ) -> Result<Response<Body>, GatewayError> {
        let Some(mut key) = self.find_admin_adaptive_key(key_id).await? else {
            return Ok(admin_adaptive_key_not_found_response(key_id));
        };
        key.learned_rpm_limit = None;
        key.concurrent_429_count = None;
        key.rpm_429_count = None;
        key.last_429_at_unix_secs = None;
        key.last_429_type = None;
        key.adjustment_history = None;
        let Some(updated) = self.update_provider_catalog_key(&key).await? else {
            return Ok(admin_adaptive_key_not_found_response(key_id));
        };
        Ok(Json(build_admin_adaptive_reset_learning_payload(&updated.id)).into_response())
    }

    pub(crate) fn admin_adaptive_dispatcher_not_found_response(&self) -> Response<Body> {
        admin_adaptive_dispatcher_not_found_response()
    }

    async fn find_admin_adaptive_key(
        &self,
        key_id: &str,
    ) -> Result<
        Option<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey>,
        GatewayError,
    > {
        Ok(self
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id.to_string()))
            .await?
            .into_iter()
            .next())
    }

    async fn load_admin_adaptive_candidate_keys(
        &self,
        provider_id: Option<&str>,
    ) -> Result<
        Vec<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey>,
        GatewayError,
    > {
        if let Some(provider_id) = provider_id.filter(|value| !value.trim().is_empty()) {
            return self
                .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(
                    &provider_id.to_string(),
                ))
                .await;
        }

        let provider_ids = self
            .list_provider_catalog_providers(false)
            .await?
            .into_iter()
            .map(|provider| provider.id)
            .collect::<Vec<_>>();
        if provider_ids.is_empty() {
            return Ok(vec![]);
        }
        self.list_provider_catalog_keys_by_provider_ids(&provider_ids)
            .await
    }
}
