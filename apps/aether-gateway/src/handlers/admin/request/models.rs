use super::AdminAppState;
use crate::handlers::admin::provider::shared::payloads::{
    AdminImportProviderModelsRequest, AdminProviderModelCreateRequest,
    AdminProviderModelUpdatePatch,
};
use crate::handlers::admin::shared::{normalize_json_array, normalize_json_object};
use crate::GatewayError;
use aether_admin::provider::{
    models as admin_provider_models_pure, models_write as admin_provider_models_write_pure,
};
use aether_data_contracts::repository::global_models::{
    AdminProviderModelListQuery, StoredAdminProviderModel, UpsertAdminProviderModelRecord,
};
use serde_json::json;
use std::collections::BTreeMap;
use uuid::Uuid;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn admin_provider_model_name_exists(
        &self,
        provider_id: &str,
        provider_model_name: &str,
        exclude_model_id: Option<&str>,
    ) -> Result<bool, GatewayError> {
        let target = provider_model_name.trim();
        if target.is_empty() {
            return Ok(false);
        }
        let models = self
            .list_admin_provider_models(
                &aether_data_contracts::repository::global_models::AdminProviderModelListQuery {
                    provider_id: provider_id.to_string(),
                    is_active: None,
                    offset: 0,
                    limit: 10_000,
                },
            )
            .await?;
        Ok(models.into_iter().any(|model| {
            model.provider_model_name == target
                && exclude_model_id.is_none_or(|exclude| model.id != exclude)
        }))
    }

    pub(crate) async fn resolve_admin_global_model_by_id_or_err(
        &self,
        global_model_id: &str,
    ) -> Result<aether_data_contracts::repository::global_models::StoredAdminGlobalModel, String>
    {
        self.get_admin_global_model_by_id(global_model_id)
            .await
            .map_err(|err| format!("{err:?}"))?
            .ok_or_else(|| format!("GlobalModel {global_model_id} 不存在"))
    }

    pub(crate) async fn build_admin_provider_available_source_models_payload(
        &self,
        provider_id: &str,
    ) -> Option<serde_json::Value> {
        if !self.has_global_model_data_reader() || !self.has_provider_catalog_data_reader() {
            return None;
        }
        let provider = self
            .read_provider_catalog_providers_by_ids(&[provider_id.to_string()])
            .await
            .ok()?
            .into_iter()
            .next()?;
        let models = self
            .list_admin_provider_available_source_models(&provider.id)
            .await
            .ok()?;
        Some(
            admin_provider_models_pure::build_admin_provider_available_source_models_payload(
                models,
            ),
        )
    }

    pub(crate) async fn build_admin_provider_model_create_record(
        &self,
        provider_id: &str,
        payload: AdminProviderModelCreateRequest,
    ) -> Result<UpsertAdminProviderModelRecord, String> {
        let provider_model_name =
            admin_provider_models_write_pure::normalize_required_trimmed_string(
                &payload.provider_model_name,
                "provider_model_name",
            )?;
        if self
            .admin_provider_model_name_exists(provider_id, &provider_model_name, None)
            .await
            .map_err(|err| format!("{err:?}"))?
        {
            return Err(format!("模型 '{provider_model_name}' 已存在"));
        }
        let global_model_id = admin_provider_models_write_pure::normalize_required_trimmed_string(
            &payload.global_model_id,
            "global_model_id",
        )?;
        self.resolve_admin_global_model_by_id_or_err(&global_model_id)
            .await?;
        let price_per_request = admin_provider_models_write_pure::normalize_optional_price(
            payload.price_per_request,
            "price_per_request",
        )?;
        let tiered_pricing = normalize_json_object(payload.tiered_pricing, "tiered_pricing")?;
        let provider_model_mappings =
            normalize_json_array(payload.provider_model_mappings, "provider_model_mappings")?;
        let config = normalize_json_object(payload.config, "config")?;
        admin_provider_models_write_pure::build_admin_provider_model_create_record(
            Uuid::new_v4().to_string(),
            provider_id.to_string(),
            global_model_id,
            provider_model_name,
            provider_model_mappings,
            price_per_request,
            tiered_pricing,
            payload.supports_vision,
            payload.supports_function_calling,
            payload.supports_streaming,
            payload.supports_extended_thinking,
            payload.supports_image_generation,
            payload.is_active,
            config,
        )
    }

    pub(crate) async fn build_admin_provider_model_update_record(
        &self,
        existing: &StoredAdminProviderModel,
        patch: AdminProviderModelUpdatePatch,
    ) -> Result<UpsertAdminProviderModelRecord, String> {
        let (fields, payload) = patch.into_parts();
        let provider_model_name = if fields.contains("provider_model_name") {
            let Some(name) = payload.provider_model_name.as_deref() else {
                return Err(if fields.is_null("provider_model_name") {
                    "provider_model_name 不能为空".to_string()
                } else {
                    "provider_model_name 必须是字符串".to_string()
                });
            };
            let name = admin_provider_models_write_pure::normalize_required_trimmed_string(
                name,
                "provider_model_name",
            )?;
            if self
                .admin_provider_model_name_exists(&existing.provider_id, &name, Some(&existing.id))
                .await
                .map_err(|err| format!("{err:?}"))?
            {
                return Err(format!("模型 '{name}' 已存在"));
            }
            name
        } else {
            existing.provider_model_name.clone()
        };

        let global_model_id = if fields.contains("global_model_id") {
            let Some(global_model_id) = payload.global_model_id.as_deref() else {
                return Err(if fields.is_null("global_model_id") {
                    "global_model_id 不能为空".to_string()
                } else {
                    "global_model_id 必须是字符串".to_string()
                });
            };
            let global_model_id =
                admin_provider_models_write_pure::normalize_required_trimmed_string(
                    global_model_id,
                    "global_model_id",
                )?;
            self.resolve_admin_global_model_by_id_or_err(&global_model_id)
                .await?;
            global_model_id
        } else {
            existing.global_model_id.clone()
        };

        let price_per_request = if fields.contains("price_per_request") {
            admin_provider_models_write_pure::normalize_optional_price(
                payload.price_per_request,
                "price_per_request",
            )?
        } else {
            existing.price_per_request
        };
        let tiered_pricing = if fields.contains("tiered_pricing") {
            normalize_json_object(payload.tiered_pricing, "tiered_pricing")?
        } else {
            existing.tiered_pricing.clone()
        };
        let provider_model_mappings = if fields.contains("provider_model_mappings") {
            normalize_json_array(payload.provider_model_mappings, "provider_model_mappings")?
        } else {
            existing.provider_model_mappings.clone()
        };
        let config = if fields.contains("config") {
            normalize_json_object(payload.config, "config")?
        } else {
            existing.config.clone()
        };

        admin_provider_models_write_pure::build_admin_provider_model_update_record(
            existing,
            global_model_id,
            provider_model_name,
            provider_model_mappings,
            price_per_request,
            tiered_pricing,
            if fields.contains("supports_vision") {
                payload.supports_vision
            } else {
                existing.supports_vision
            },
            if fields.contains("supports_function_calling") {
                payload.supports_function_calling
            } else {
                existing.supports_function_calling
            },
            if fields.contains("supports_streaming") {
                payload.supports_streaming
            } else {
                existing.supports_streaming
            },
            if fields.contains("supports_extended_thinking") {
                payload.supports_extended_thinking
            } else {
                existing.supports_extended_thinking
            },
            if fields.contains("supports_image_generation") {
                payload.supports_image_generation
            } else {
                existing.supports_image_generation
            },
            payload.is_active.unwrap_or(existing.is_active),
            payload.is_available.unwrap_or(existing.is_available),
            config,
        )
    }

    pub(crate) async fn build_admin_import_provider_models_payload(
        &self,
        provider_id: &str,
        payload: AdminImportProviderModelsRequest,
    ) -> Result<serde_json::Value, String> {
        let tiered_pricing = normalize_json_object(payload.tiered_pricing, "tiered_pricing")?;

        let existing_models = self
            .list_admin_provider_models(&AdminProviderModelListQuery {
                provider_id: provider_id.to_string(),
                is_active: None,
                offset: 0,
                limit: 10_000,
            })
            .await
            .map_err(|err| format!("{err:?}"))?;
        let mut existing_by_name = existing_models
            .iter()
            .map(|model| (model.provider_model_name.clone(), model.clone()))
            .collect::<BTreeMap<_, _>>();

        let mut success = Vec::new();
        let mut errors = Vec::new();

        for model_id in payload.model_ids {
            let trimmed = match admin_provider_models_write_pure::normalize_admin_import_model_id(
                &model_id,
            ) {
                Ok(value) => value,
                Err(detail) => {
                    let raw = model_id.trim();
                    errors.push(json!({
                        "model_id": if raw.is_empty() { "<empty>" } else { raw },
                        "error": detail,
                    }));
                    continue;
                }
            };

            if let Some(existing) = existing_by_name.get(trimmed.as_str()) {
                success.push(json!({
                    "model_id": trimmed,
                    "global_model_id": existing.global_model_id,
                    "global_model_name": existing.global_model_name,
                    "provider_model_id": existing.id,
                    "created_global_model": false,
                }));
                continue;
            }

            let mut created_global_model = false;
            let global_model = if let Some(existing) = self
                .get_admin_global_model_by_name(&trimmed)
                .await
                .map_err(|err| format!("{err:?}"))?
            {
                existing
            } else {
                let created = self
                    .create_admin_global_model(
                        &admin_provider_models_write_pure::build_admin_import_global_model_record(
                            Uuid::new_v4().to_string(),
                            trimmed.to_string(),
                            payload.price_per_request,
                            tiered_pricing.clone(),
                        )
                        .map_err(|err| err.to_string())?,
                    )
                    .await
                    .map_err(|err| format!("{err:?}"))?;
                let Some(created) = created else {
                    errors.push(json!({"model_id": trimmed, "error": "Create GlobalModel failed"}));
                    continue;
                };
                created_global_model = true;
                created
            };

            let record =
                admin_provider_models_write_pure::build_admin_import_provider_model_record(
                    Uuid::new_v4().to_string(),
                    provider_id.to_string(),
                    global_model.id.clone(),
                    trimmed.to_string(),
                    payload.price_per_request,
                    tiered_pricing.clone(),
                )?;

            match self.create_admin_provider_model(&record).await {
                Ok(Some(created)) => {
                    existing_by_name.insert(trimmed.to_string(), created.clone());
                    success.push(json!({
                        "model_id": trimmed,
                        "global_model_id": global_model.id,
                        "global_model_name": global_model.name,
                        "provider_model_id": created.id,
                        "created_global_model": created_global_model,
                    }));
                }
                Ok(None) => errors.push(json!({
                    "model_id": trimmed,
                    "error": "Create provider model failed",
                })),
                Err(err) => errors.push(json!({
                    "model_id": trimmed,
                    "error": format!("{err:?}"),
                })),
            }
        }

        Ok(json!({
            "success": success,
            "errors": errors,
        }))
    }

    pub(crate) async fn build_admin_batch_assign_global_models_payload(
        &self,
        provider_id: &str,
        global_model_ids: Vec<String>,
    ) -> Result<serde_json::Value, String> {
        let existing_models = self
            .list_admin_provider_models(&AdminProviderModelListQuery {
                provider_id: provider_id.to_string(),
                is_active: None,
                offset: 0,
                limit: 10_000,
            })
            .await
            .map_err(|err| format!("{err:?}"))?;
        let existing_global_model_ids = existing_models
            .into_iter()
            .map(|model| model.global_model_id)
            .collect::<std::collections::BTreeSet<_>>();

        let mut success = Vec::new();
        let mut errors = Vec::new();
        for global_model_id in global_model_ids {
            let global_model_id = global_model_id.trim().to_string();
            if global_model_id.is_empty() {
                continue;
            }
            let global_model = match self
                .resolve_admin_global_model_by_id_or_err(&global_model_id)
                .await
            {
                Ok(model) => model,
                Err(detail) => {
                    errors.push(json!({
                        "global_model_id": global_model_id,
                        "error": detail,
                    }));
                    continue;
                }
            };
            if existing_global_model_ids.contains(&global_model.id) {
                errors.push(json!({
                    "global_model_id": global_model.id,
                    "error": "Model already exists",
                }));
                continue;
            }
            let record =
                admin_provider_models_write_pure::build_admin_batch_assign_provider_model_record(
                    Uuid::new_v4().to_string(),
                    provider_id.to_string(),
                    global_model.id.clone(),
                    global_model.name.clone(),
                )?;
            match self.create_admin_provider_model(&record).await {
                Ok(Some(created)) => success.push(json!({
                    "global_model_id": global_model.id,
                    "global_model_name": global_model.name,
                    "provider_model_id": created.id,
                })),
                Ok(None) => errors.push(json!({
                    "global_model_id": global_model.id,
                    "error": "Create provider model failed",
                })),
                Err(err) => errors.push(json!({
                    "global_model_id": global_model.id,
                    "error": format!("{err:?}"),
                })),
            }
        }
        Ok(json!({
            "success": success,
            "errors": errors,
        }))
    }

    pub(crate) async fn read_admin_external_models_cache(
        &self,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        crate::handlers::admin::model::read_admin_external_models_cache(self).await
    }

    pub(crate) async fn clear_admin_external_models_cache(
        &self,
    ) -> Result<serde_json::Value, GatewayError> {
        crate::handlers::admin::model::clear_admin_external_models_cache(self).await
    }

    pub(crate) async fn list_admin_provider_models(
        &self,
        query: &aether_data_contracts::repository::global_models::AdminProviderModelListQuery,
    ) -> Result<
        Vec<aether_data_contracts::repository::global_models::StoredAdminProviderModel>,
        GatewayError,
    > {
        self.app.list_admin_provider_models(query).await
    }

    pub(crate) async fn list_admin_provider_available_source_models(
        &self,
        provider_id: &str,
    ) -> Result<
        Vec<aether_data_contracts::repository::global_models::StoredAdminProviderModel>,
        GatewayError,
    > {
        self.app
            .list_admin_provider_available_source_models(provider_id)
            .await
    }

    pub(crate) async fn get_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<
        Option<aether_data_contracts::repository::global_models::StoredAdminProviderModel>,
        GatewayError,
    > {
        self.app
            .get_admin_provider_model(provider_id, model_id)
            .await
    }

    pub(crate) async fn get_admin_global_model_by_id(
        &self,
        global_model_id: &str,
    ) -> Result<
        Option<aether_data_contracts::repository::global_models::StoredAdminGlobalModel>,
        GatewayError,
    > {
        self.app.get_admin_global_model_by_id(global_model_id).await
    }

    pub(crate) async fn get_admin_global_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<
        Option<aether_data_contracts::repository::global_models::StoredAdminGlobalModel>,
        GatewayError,
    > {
        self.app.get_admin_global_model_by_name(model_name).await
    }

    pub(crate) async fn create_admin_provider_model(
        &self,
        record: &aether_data_contracts::repository::global_models::UpsertAdminProviderModelRecord,
    ) -> Result<
        Option<aether_data_contracts::repository::global_models::StoredAdminProviderModel>,
        GatewayError,
    > {
        self.app.create_admin_provider_model(record).await
    }

    pub(crate) async fn update_admin_provider_model(
        &self,
        record: &aether_data_contracts::repository::global_models::UpsertAdminProviderModelRecord,
    ) -> Result<
        Option<aether_data_contracts::repository::global_models::StoredAdminProviderModel>,
        GatewayError,
    > {
        self.app.update_admin_provider_model(record).await
    }

    pub(crate) async fn delete_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<bool, GatewayError> {
        self.app
            .delete_admin_provider_model(provider_id, model_id)
            .await
    }

    pub(crate) async fn create_admin_global_model(
        &self,
        record: &aether_data_contracts::repository::global_models::CreateAdminGlobalModelRecord,
    ) -> Result<
        Option<aether_data_contracts::repository::global_models::StoredAdminGlobalModel>,
        GatewayError,
    > {
        self.app.create_admin_global_model(record).await
    }

    pub(crate) async fn list_admin_global_models(
        &self,
        query: &aether_data_contracts::repository::global_models::AdminGlobalModelListQuery,
    ) -> Result<
        aether_data_contracts::repository::global_models::StoredAdminGlobalModelPage,
        GatewayError,
    > {
        self.app.list_admin_global_models(query).await
    }

    pub(crate) async fn list_admin_provider_models_by_global_model_id(
        &self,
        global_model_id: &str,
    ) -> Result<
        Vec<aether_data_contracts::repository::global_models::StoredAdminProviderModel>,
        GatewayError,
    > {
        self.app
            .list_admin_provider_models_by_global_model_id(global_model_id)
            .await
    }

    pub(crate) async fn update_admin_global_model(
        &self,
        record: &aether_data_contracts::repository::global_models::UpdateAdminGlobalModelRecord,
    ) -> Result<
        Option<aether_data_contracts::repository::global_models::StoredAdminGlobalModel>,
        GatewayError,
    > {
        self.app.update_admin_global_model(record).await
    }

    pub(crate) async fn delete_admin_global_model(
        &self,
        global_model_id: &str,
    ) -> Result<bool, GatewayError> {
        self.app.delete_admin_global_model(global_model_id).await
    }
}
