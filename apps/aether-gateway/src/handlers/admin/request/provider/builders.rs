use super::*;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn build_admin_keys_grouped_by_format_payload(
        &self,
    ) -> Option<serde_json::Value> {
        crate::handlers::public::build_admin_keys_grouped_by_format_payload(self.app).await
    }

    pub(crate) async fn build_admin_create_provider_key_record(
        &self,
        provider: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
        payload: crate::handlers::admin::provider::shared::payloads::AdminProviderKeyCreateRequest,
    ) -> Result<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey, String>
    {
        crate::handlers::admin::provider::write::keys::build_admin_create_provider_key_record(
            self, provider, payload,
        )
        .await
    }

    pub(crate) async fn build_admin_update_provider_key_record(
        &self,
        provider: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
        existing: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey,
        patch: crate::handlers::admin::provider::shared::payloads::AdminProviderKeyUpdatePatch,
    ) -> Result<aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey, String>
    {
        crate::handlers::admin::provider::write::keys::build_admin_update_provider_key_record(
            self, provider, existing, patch,
        )
        .await
    }

    pub(crate) fn build_admin_provider_key_response(
        &self,
        key: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey,
        provider_type: &str,
        api_formats: &[String],
        now_unix_secs: u64,
    ) -> serde_json::Value {
        crate::handlers::admin::shared::build_admin_provider_key_response(
            self.app,
            key,
            provider_type,
            api_formats,
            now_unix_secs,
        )
    }

    pub(crate) fn masked_catalog_api_key(
        &self,
        key: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey,
    ) -> String {
        crate::handlers::admin::shared::masked_catalog_api_key(self.app, key)
    }

    pub(crate) async fn build_admin_provider_keys_payload(
        &self,
        provider_id: &str,
        skip: usize,
        limit: usize,
    ) -> Option<serde_json::Value> {
        crate::handlers::admin::provider::write::keys::build_admin_provider_keys_payload(
            self,
            provider_id,
            skip,
            limit,
        )
        .await
    }

    pub(crate) fn build_admin_reveal_key_payload(
        &self,
        key: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey,
    ) -> Result<serde_json::Value, String> {
        crate::handlers::admin::provider::write::reveal::build_admin_reveal_key_payload(self, key)
    }

    pub(crate) async fn build_admin_export_key_payload(
        &self,
        key: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey,
    ) -> Result<serde_json::Value, String> {
        crate::handlers::admin::provider::write::reveal::build_admin_export_key_payload(self, key)
            .await
    }

    pub(crate) async fn build_admin_providers_payload(
        &self,
        skip: usize,
        limit: usize,
        is_active: Option<bool>,
    ) -> Option<serde_json::Value> {
        crate::handlers::admin::provider::summary::build_admin_providers_payload(
            self, skip, limit, is_active,
        )
        .await
    }

    pub(crate) async fn build_admin_provider_summary_payload(
        &self,
        provider_id: &str,
    ) -> Option<serde_json::Value> {
        crate::handlers::admin::provider::summary::build_admin_provider_summary_payload(
            self,
            provider_id,
        )
        .await
    }

    pub(crate) async fn build_admin_create_provider_record(
        &self,
        payload: crate::handlers::admin::provider::shared::payloads::AdminProviderCreateRequest,
    ) -> Result<
        (
            aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
            Option<i32>,
        ),
        String,
    > {
        crate::handlers::admin::provider::write::provider::build_admin_create_provider_record(
            self, payload,
        )
        .await
    }

    pub(crate) async fn build_admin_update_provider_record(
        &self,
        existing: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
        patch: crate::handlers::admin::provider::shared::payloads::AdminProviderUpdatePatch,
    ) -> Result<
        aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
        String,
    > {
        crate::handlers::admin::provider::write::provider::build_admin_update_provider_record(
            self, existing, patch,
        )
        .await
    }

    pub(crate) async fn build_admin_providers_summary_payload(
        &self,
        page: usize,
        page_size: usize,
        search: &str,
        status: &str,
        api_format: &str,
        model_id: &str,
    ) -> Option<serde_json::Value> {
        crate::handlers::admin::provider::summary::build_admin_providers_summary_payload(
            self, page, page_size, search, status, api_format, model_id,
        )
        .await
    }

    pub(crate) async fn build_admin_provider_health_monitor_payload(
        &self,
        provider_id: &str,
        lookback_hours: u64,
        per_endpoint_limit: usize,
    ) -> Option<serde_json::Value> {
        crate::handlers::admin::provider::summary::build_admin_provider_health_monitor_payload(
            self,
            provider_id,
            lookback_hours,
            per_endpoint_limit,
        )
        .await
    }

    pub(crate) async fn build_admin_provider_mapping_preview_payload(
        &self,
        provider_id: &str,
    ) -> Option<serde_json::Value> {
        crate::handlers::admin::provider::delete_task::build_admin_provider_mapping_preview_payload(
            self,
            provider_id,
        )
        .await
    }

    pub(crate) async fn build_admin_provider_pool_status_payload(
        &self,
        provider_id: &str,
    ) -> Option<serde_json::Value> {
        crate::handlers::admin::provider::pool::runtime::build_admin_provider_pool_status_payload(
            self,
            provider_id,
        )
        .await
    }

    pub(crate) async fn build_admin_create_provider_endpoint_record(
        &self,
        provider: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
        payload: crate::handlers::admin::provider::endpoints_admin::payloads::AdminProviderEndpointCreateRequest,
    ) -> Result<
        aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint,
        String,
    > {
        use crate::api::ai::{
            admin_default_body_rules_for_signature, admin_endpoint_signature_parts,
        };
        use crate::handlers::public::normalize_admin_base_url;
        use aether_admin::provider::endpoints as admin_provider_endpoints_pure;

        if payload.provider_id.trim() != provider.id {
            return Err("provider_id 不匹配".to_string());
        }
        if self.provider_type_is_fixed(&provider.provider_type) {
            return Err("固定类型 Provider 不允许手动新增 Endpoint".to_string());
        }
        if !(0..=999).contains(&payload.max_retries) {
            return Err("max_retries 必须在 0 到 999 之间".to_string());
        }

        let (normalized_api_format, api_family, endpoint_kind) =
            admin_endpoint_signature_parts(&payload.api_format)
                .ok_or_else(|| format!("无效的 api_format: {}", payload.api_format))?;
        let base_url = normalize_admin_base_url(&payload.base_url)?;

        let existing_endpoints = self
            .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
            .await
            .map_err(|err| format!("{err:?}"))?;
        if existing_endpoints
            .iter()
            .any(|endpoint| endpoint.api_format == normalized_api_format)
        {
            return Err(format!(
                "Provider {} 已存在 {} 格式的 Endpoint",
                provider.name, normalized_api_format
            ));
        }

        let body_rules = match payload.body_rules {
            Some(value) => Some(value),
            None => admin_default_body_rules_for_signature(
                normalized_api_format,
                Some(provider.provider_type.as_str()),
            )
            .and_then(|(_, rules)| (!rules.is_empty()).then_some(serde_json::Value::Array(rules))),
        };

        let now_unix_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(0);

        admin_provider_endpoints_pure::build_admin_provider_endpoint_record(
            uuid::Uuid::new_v4().to_string(),
            provider.id.clone(),
            normalized_api_format.to_string(),
            api_family.to_string(),
            endpoint_kind.to_string(),
            base_url,
            payload.custom_path,
            payload.header_rules,
            body_rules,
            payload.max_retries,
            payload.config,
            payload.proxy,
            payload.format_acceptance_config,
            now_unix_secs,
        )
    }

    pub(crate) async fn build_admin_update_provider_endpoint_record(
        &self,
        provider: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider,
        existing_endpoint: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint,
        patch: crate::handlers::admin::provider::endpoints_admin::payloads::AdminProviderEndpointUpdatePatch,
    ) -> Result<
        aether_data_contracts::repository::provider_catalog::StoredProviderCatalogEndpoint,
        String,
    > {
        use crate::api::ai::admin_endpoint_signature_parts;
        use crate::handlers::admin::provider::write::provider::apply_admin_fixed_provider_endpoint_template_overrides;
        use crate::handlers::public::{admin_requested_force_stream, normalize_admin_base_url};
        use aether_admin::provider::endpoints as admin_provider_endpoints_pure;
        let (fields, payload) = patch.into_parts();

        if self.provider_type_is_fixed(&provider.provider_type)
            && (fields.contains("base_url") || fields.contains("custom_path"))
        {
            return Err(
                "固定类型 Provider 的 Endpoint 不允许修改 base_url/custom_path".to_string(),
            );
        }

        let mut update_fields = admin_provider_endpoints_pure::AdminProviderEndpointUpdateFields {
            base_url: payload.base_url,
            custom_path: payload.custom_path,
            header_rules: payload.header_rules,
            body_rules: payload.body_rules,
            max_retries: payload.max_retries,
            is_active: payload.is_active,
            config: payload.config,
            proxy: payload.proxy,
            format_acceptance_config: payload.format_acceptance_config,
        };
        if let Some(base_url) = update_fields.base_url.as_deref() {
            update_fields.base_url = Some(normalize_admin_base_url(base_url)?);
        }
        let mut updated =
            admin_provider_endpoints_pure::apply_admin_provider_endpoint_update_fields(
                existing_endpoint,
                |field| fields.contains(field),
                |field| fields.is_null(field),
                &update_fields,
            )?;

        let provider_type = provider.provider_type.trim().to_ascii_lowercase();
        if provider_type == "codex"
            && crate::ai_serving::is_openai_responses_format(&existing_endpoint.api_format)
        {
            let has_config_in_payload = fields.contains("config");
            let config_payload = if has_config_in_payload {
                updated
                    .config
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({}))
            } else {
                existing_endpoint
                    .config
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({}))
            };
            let mut config = config_payload.as_object().cloned().unwrap_or_default();
            let requested = config
                .get("upstream_stream_policy")
                .or_else(|| config.get("upstreamStreamPolicy"))
                .or_else(|| config.get("upstream_stream"));
            if has_config_in_payload
                && requested.is_some()
                && !admin_requested_force_stream(requested.expect("checked above"))
            {
                return Err("Codex OpenAI Responses 端点固定为强制流式，不允许修改".to_string());
            }
            config.remove("upstreamStreamPolicy");
            config.remove("upstream_stream");
            config.insert(
                "upstream_stream_policy".to_string(),
                serde_json::json!("force_stream"),
            );
            updated.config = Some(serde_json::Value::Object(config));
        }

        let (_, api_family, endpoint_kind) = admin_endpoint_signature_parts(&updated.api_format)
            .ok_or_else(|| format!("无效的 api_format: {}", updated.api_format))?;
        updated.api_family = Some(api_family.to_string());
        updated.endpoint_kind = Some(endpoint_kind.to_string());
        apply_admin_fixed_provider_endpoint_template_overrides(
            provider,
            existing_endpoint,
            &mut updated,
        )?;
        updated.updated_at_unix_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs());

        Ok(updated)
    }
}
