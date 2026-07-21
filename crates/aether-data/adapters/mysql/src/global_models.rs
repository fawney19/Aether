use async_trait::async_trait;
use sqlx::{mysql::MySqlRow, Row};

use aether_data_contracts::repository::global_models::{
    metadata_supports_embedding, AdminGlobalModelListQuery, AdminProviderModelListQuery,
    CreateAdminGlobalModelRecord, GlobalModelReadRepository, GlobalModelSnapshot,
    GlobalModelWriteRepository, PublicCatalogModelListQuery, PublicCatalogModelSearchQuery,
    PublicGlobalModelQuery, StoredAdminGlobalModel, StoredAdminGlobalModelPage,
    StoredAdminProviderModel, StoredProviderActiveGlobalModel, StoredProviderModelStats,
    StoredPublicCatalogModel, StoredPublicGlobalModel, StoredPublicGlobalModelPage,
    UpdateAdminGlobalModelRecord, UpsertAdminProviderModelRecord,
};
use aether_data_contracts::DataLayerError;

use crate::error::SqlResultExt;
use crate::MysqlPool;

#[derive(Debug, Clone)]
pub struct MysqlGlobalModelReadRepository {
    pool: MysqlPool,
}

impl MysqlGlobalModelReadRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }

    async fn load_snapshot(&self) -> Result<GlobalModelSnapshot, DataLayerError> {
        Ok(
            GlobalModelSnapshot::seed(self.load_public_global_models().await?)
                .with_admin_global_models(self.load_admin_global_models().await?)
                .with_admin_provider_models(self.load_admin_provider_models().await?)
                .with_public_catalog_models(self.load_public_catalog_models().await?)
                .with_provider_model_stats(self.load_provider_model_stats().await?)
                .with_active_global_model_refs(self.load_active_global_model_refs().await?),
        )
    }

    async fn load_public_global_models(
        &self,
    ) -> Result<Vec<StoredPublicGlobalModel>, DataLayerError> {
        let rows = sqlx::query(
            r#"
SELECT id, name, display_name, is_active, default_price_per_request,
  default_tiered_pricing, supported_capabilities, config, usage_count
FROM global_models
"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_public_global_model_row).collect()
    }

    async fn load_admin_global_models(
        &self,
    ) -> Result<Vec<StoredAdminGlobalModel>, DataLayerError> {
        let rows = sqlx::query(
            r#"
SELECT
  id,
  name,
  COALESCE(NULLIF(display_name, ''), name) AS display_name,
  is_active,
  default_price_per_request,
  default_tiered_pricing,
  supported_capabilities,
  config,
  usage_count,
  created_at AS created_at_unix_ms,
  updated_at AS updated_at_unix_secs
FROM global_models
"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_admin_global_model_row).collect()
    }

    async fn load_admin_provider_models(
        &self,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        let rows = sqlx::query(
            r#"
SELECT
  m.id,
  m.provider_id,
  m.global_model_id,
  m.provider_model_name,
  m.provider_model_mappings,
  m.price_per_request,
  m.tiered_pricing,
  m.supports_vision,
  m.supports_function_calling,
  m.supports_streaming,
  m.supports_extended_thinking,
  m.supports_image_generation,
  m.is_active,
  m.is_available,
  m.config,
  m.created_at AS created_at_unix_ms,
  m.updated_at AS updated_at_unix_secs,
  gm.name AS global_model_name,
  gm.display_name AS global_model_display_name,
  gm.default_price_per_request AS global_model_default_price_per_request,
  gm.default_tiered_pricing AS global_model_default_tiered_pricing,
  gm.supported_capabilities AS global_model_supported_capabilities,
  gm.config AS global_model_config
FROM models m
LEFT JOIN global_models gm ON gm.id = m.global_model_id
WHERE m.global_model_id IS NOT NULL
"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_admin_provider_model_row).collect()
    }

    async fn load_public_catalog_models(
        &self,
    ) -> Result<Vec<StoredPublicCatalogModel>, DataLayerError> {
        let rows = sqlx::query(
            r#"
SELECT
  m.id,
  m.provider_id,
  p.name AS provider_name,
  p.is_active AS provider_is_active,
  m.provider_model_name,
  COALESCE(gm.name, m.provider_model_name) AS name,
  COALESCE(NULLIF(gm.display_name, ''), m.provider_model_name) AS display_name,
  gm.config AS global_model_config,
  gm.supported_capabilities AS global_model_supported_capabilities,
  m.config AS model_config,
  m.tiered_pricing,
  gm.default_tiered_pricing,
  m.supports_vision,
  m.supports_function_calling,
  m.supports_streaming,
  m.is_active,
  gm.is_active AS global_model_is_active
FROM models m
JOIN providers p ON p.id = m.provider_id
LEFT JOIN global_models gm ON gm.id = m.global_model_id
"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_public_catalog_model_row).collect()
    }

    async fn load_provider_model_stats(
        &self,
    ) -> Result<Vec<StoredProviderModelStats>, DataLayerError> {
        let rows = sqlx::query(
            r#"
SELECT
  provider_id,
  COUNT(id) AS total_models,
  SUM(CASE WHEN is_active = 1 THEN 1 ELSE 0 END) AS active_models
FROM models
GROUP BY provider_id
ORDER BY provider_id ASC
"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_provider_model_stats_row).collect()
    }

    async fn load_active_global_model_refs(
        &self,
    ) -> Result<Vec<StoredProviderActiveGlobalModel>, DataLayerError> {
        let rows = sqlx::query(
            r#"
SELECT DISTINCT provider_id, global_model_id
FROM models
WHERE is_active = 1
  AND global_model_id IS NOT NULL
ORDER BY provider_id ASC, global_model_id ASC
"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_active_global_model_row).collect()
    }

    pub async fn create_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        let now = current_unix_secs();
        sqlx::query(
            r#"
INSERT INTO models (
  id,
  provider_id,
  global_model_id,
  provider_model_name,
  provider_model_mappings,
  price_per_request,
  tiered_pricing,
  supports_vision,
  supports_function_calling,
  supports_streaming,
  supports_extended_thinking,
  supports_image_generation,
  is_active,
  is_available,
  config,
  created_at,
  updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
        )
        .bind(&record.id)
        .bind(&record.provider_id)
        .bind(&record.global_model_id)
        .bind(&record.provider_model_name)
        .bind(optional_json_to_string(
            &record.provider_model_mappings,
            "models.provider_model_mappings",
        )?)
        .bind(record.price_per_request)
        .bind(optional_json_to_string(
            &record.tiered_pricing,
            "models.tiered_pricing",
        )?)
        .bind(record.supports_vision)
        .bind(record.supports_function_calling)
        .bind(record.supports_streaming)
        .bind(record.supports_extended_thinking)
        .bind(record.supports_image_generation)
        .bind(record.is_active)
        .bind(record.is_available)
        .bind(optional_json_to_string(&record.config, "models.config")?)
        .bind(now as i64)
        .bind(now as i64)
        .execute(&self.pool)
        .await
        .map_sql_err()?;

        self.get_admin_provider_model(&record.provider_id, &record.id)
            .await
    }

    pub async fn update_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        let now = current_unix_secs();
        let updated = sqlx::query(
            r#"
UPDATE models
SET
  global_model_id = ?,
  provider_model_name = ?,
  provider_model_mappings = ?,
  price_per_request = ?,
  tiered_pricing = ?,
  supports_vision = ?,
  supports_function_calling = ?,
  supports_streaming = ?,
  supports_extended_thinking = ?,
  supports_image_generation = ?,
  is_active = ?,
  is_available = ?,
  config = ?,
  updated_at = ?
WHERE id = ?
  AND provider_id = ?
"#,
        )
        .bind(&record.global_model_id)
        .bind(&record.provider_model_name)
        .bind(optional_json_to_string(
            &record.provider_model_mappings,
            "models.provider_model_mappings",
        )?)
        .bind(record.price_per_request)
        .bind(optional_json_to_string(
            &record.tiered_pricing,
            "models.tiered_pricing",
        )?)
        .bind(record.supports_vision)
        .bind(record.supports_function_calling)
        .bind(record.supports_streaming)
        .bind(record.supports_extended_thinking)
        .bind(record.supports_image_generation)
        .bind(record.is_active)
        .bind(record.is_available)
        .bind(optional_json_to_string(&record.config, "models.config")?)
        .bind(now as i64)
        .bind(&record.id)
        .bind(&record.provider_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;

        if updated.rows_affected() == 0 {
            return Ok(None);
        }

        self.get_admin_provider_model(&record.provider_id, &record.id)
            .await
    }

    pub async fn delete_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<bool, DataLayerError> {
        let deleted = sqlx::query(
            r#"
DELETE FROM models
WHERE provider_id = ?
  AND id = ?
"#,
        )
        .bind(provider_id)
        .bind(model_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;

        Ok(deleted.rows_affected() > 0)
    }

    pub async fn create_admin_global_model(
        &self,
        record: &CreateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        let now = current_unix_secs();
        let usage_count =
            optional_admin_global_model_usage_count_i64(record.usage_count)?.unwrap_or_default();
        sqlx::query(
            r#"
INSERT INTO global_models (
  id,
  name,
  display_name,
  is_active,
  default_price_per_request,
  default_tiered_pricing,
  supported_capabilities,
  usage_count,
  config,
  created_at,
  updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
        )
        .bind(&record.id)
        .bind(&record.name)
        .bind(&record.display_name)
        .bind(record.is_active)
        .bind(record.default_price_per_request)
        .bind(optional_json_to_string(
            &record.default_tiered_pricing,
            "global_models.default_tiered_pricing",
        )?)
        .bind(optional_json_to_string(
            &record.supported_capabilities,
            "global_models.supported_capabilities",
        )?)
        .bind(usage_count)
        .bind(optional_json_to_string(
            &record.config,
            "global_models.config",
        )?)
        .bind(now as i64)
        .bind(now as i64)
        .execute(&self.pool)
        .await
        .map_sql_err()?;

        self.get_admin_global_model_by_id(&record.id).await
    }

    pub async fn update_admin_global_model(
        &self,
        record: &UpdateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        let now = current_unix_secs();
        let usage_count = optional_admin_global_model_usage_count_i64(record.usage_count)?;
        let updated = sqlx::query(
            r#"
UPDATE global_models
SET
  display_name = ?,
  is_active = ?,
  default_price_per_request = ?,
  default_tiered_pricing = ?,
  supported_capabilities = ?,
  config = ?,
  usage_count = COALESCE(?, usage_count),
  updated_at = ?
WHERE id = ?
"#,
        )
        .bind(&record.display_name)
        .bind(record.is_active)
        .bind(record.default_price_per_request)
        .bind(optional_json_to_string(
            &record.default_tiered_pricing,
            "global_models.default_tiered_pricing",
        )?)
        .bind(optional_json_to_string(
            &record.supported_capabilities,
            "global_models.supported_capabilities",
        )?)
        .bind(optional_json_to_string(
            &record.config,
            "global_models.config",
        )?)
        .bind(usage_count)
        .bind(now as i64)
        .bind(&record.id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;

        if updated.rows_affected() == 0 {
            return Ok(None);
        }

        self.get_admin_global_model_by_id(&record.id).await
    }

    pub async fn delete_admin_global_model(
        &self,
        global_model_id: &str,
    ) -> Result<bool, DataLayerError> {
        let mut tx = self.pool.begin().await.map_sql_err()?;

        sqlx::query(
            r#"
DELETE FROM models
WHERE global_model_id = ?
"#,
        )
        .bind(global_model_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        let deleted = sqlx::query(
            r#"
DELETE FROM global_models
WHERE id = ?
"#,
        )
        .bind(global_model_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        tx.commit().await.map_sql_err()?;

        Ok(deleted.rows_affected() > 0)
    }
}

#[async_trait]
impl GlobalModelReadRepository for MysqlGlobalModelReadRepository {
    async fn list_public_models(
        &self,
        query: &PublicGlobalModelQuery,
    ) -> Result<StoredPublicGlobalModelPage, DataLayerError> {
        Ok(self.load_snapshot().await?.list_public_models(query))
    }

    async fn get_public_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<StoredPublicGlobalModel>, DataLayerError> {
        Ok(self
            .load_snapshot()
            .await?
            .get_public_model_by_name(model_name))
    }

    async fn list_public_catalog_models(
        &self,
        query: &PublicCatalogModelListQuery,
    ) -> Result<Vec<StoredPublicCatalogModel>, DataLayerError> {
        Ok(self
            .load_snapshot()
            .await?
            .list_public_catalog_models(query))
    }

    async fn search_public_catalog_models(
        &self,
        query: &PublicCatalogModelSearchQuery,
    ) -> Result<Vec<StoredPublicCatalogModel>, DataLayerError> {
        Ok(self
            .load_snapshot()
            .await?
            .search_public_catalog_models(query))
    }

    async fn list_admin_global_models(
        &self,
        query: &AdminGlobalModelListQuery,
    ) -> Result<StoredAdminGlobalModelPage, DataLayerError> {
        Ok(self.load_snapshot().await?.list_admin_global_models(query))
    }

    async fn list_admin_provider_models(
        &self,
        query: &AdminProviderModelListQuery,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        Ok(self
            .load_snapshot()
            .await?
            .list_admin_provider_models(query))
    }

    async fn list_admin_provider_available_source_models(
        &self,
        provider_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        Ok(self
            .load_snapshot()
            .await?
            .list_admin_provider_available_source_models(provider_id))
    }

    async fn get_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        Ok(self
            .load_snapshot()
            .await?
            .get_admin_provider_model(provider_id, model_id))
    }

    async fn get_admin_global_model_by_id(
        &self,
        global_model_id: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        Ok(self
            .load_snapshot()
            .await?
            .get_admin_global_model_by_id(global_model_id))
    }

    async fn get_admin_global_model_by_name(
        &self,
        model_name: &str,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        Ok(self
            .load_snapshot()
            .await?
            .get_admin_global_model_by_name(model_name))
    }

    async fn list_admin_provider_models_by_global_model_id(
        &self,
        global_model_id: &str,
    ) -> Result<Vec<StoredAdminProviderModel>, DataLayerError> {
        Ok(self
            .load_snapshot()
            .await?
            .list_admin_provider_models_by_global_model_id(global_model_id))
    }

    async fn list_provider_model_stats(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderModelStats>, DataLayerError> {
        Ok(self
            .load_snapshot()
            .await?
            .list_provider_model_stats(provider_ids))
    }

    async fn list_active_global_model_ids_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderActiveGlobalModel>, DataLayerError> {
        Ok(self
            .load_snapshot()
            .await?
            .list_active_global_model_ids_by_provider_ids(provider_ids))
    }
}

#[async_trait]
impl GlobalModelWriteRepository for MysqlGlobalModelReadRepository {
    async fn create_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        Self::create_admin_provider_model(self, record).await
    }

    async fn update_admin_provider_model(
        &self,
        record: &UpsertAdminProviderModelRecord,
    ) -> Result<Option<StoredAdminProviderModel>, DataLayerError> {
        Self::update_admin_provider_model(self, record).await
    }

    async fn delete_admin_provider_model(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<bool, DataLayerError> {
        Self::delete_admin_provider_model(self, provider_id, model_id).await
    }

    async fn create_admin_global_model(
        &self,
        record: &CreateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        Self::create_admin_global_model(self, record).await
    }

    async fn update_admin_global_model(
        &self,
        record: &UpdateAdminGlobalModelRecord,
    ) -> Result<Option<StoredAdminGlobalModel>, DataLayerError> {
        Self::update_admin_global_model(self, record).await
    }

    async fn delete_admin_global_model(
        &self,
        global_model_id: &str,
    ) -> Result<bool, DataLayerError> {
        Self::delete_admin_global_model(self, global_model_id).await
    }
}

fn current_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn optional_json_to_string(
    value: &Option<serde_json::Value>,
    field_name: &str,
) -> Result<Option<String>, DataLayerError> {
    value
        .as_ref()
        .map(|value| {
            serde_json::to_string(value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "{field_name} contains unserializable JSON: {err}"
                ))
            })
        })
        .transpose()
}

fn optional_json_from_string(
    value: Option<String>,
    field_name: &str,
) -> Result<Option<serde_json::Value>, DataLayerError> {
    value
        .map(|value| {
            serde_json::from_str(&value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "{field_name} contains invalid JSON: {err}"
                ))
            })
        })
        .transpose()
}

fn optional_u64(value: Option<i64>, field_name: &str) -> Result<Option<u64>, DataLayerError> {
    value
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                DataLayerError::UnexpectedValue(format!("invalid {field_name}: {value}"))
            })
        })
        .transpose()
}

fn first_tier_price(value: Option<&serde_json::Value>, key: &str) -> Option<f64> {
    value
        .and_then(|value| value.get("tiers"))
        .and_then(serde_json::Value::as_array)
        .and_then(|tiers| tiers.first())
        .and_then(|tier| tier.get(key))
        .and_then(serde_json::Value::as_f64)
}

fn map_public_global_model_row(row: &MySqlRow) -> Result<StoredPublicGlobalModel, DataLayerError> {
    StoredPublicGlobalModel::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("name").map_sql_err()?,
        row.try_get("display_name").map_sql_err()?,
        row.try_get("is_active").map_sql_err()?,
        row.try_get("default_price_per_request").map_sql_err()?,
        optional_json_from_string(
            row.try_get("default_tiered_pricing").map_sql_err()?,
            "global_models.default_tiered_pricing",
        )?,
        optional_json_from_string(
            row.try_get("supported_capabilities").map_sql_err()?,
            "global_models.supported_capabilities",
        )?,
        optional_json_from_string(row.try_get("config").map_sql_err()?, "global_models.config")?,
        row.try_get::<i64, _>("usage_count").map_sql_err()?.max(0) as u64,
    )
}

fn map_admin_global_model_row(row: &MySqlRow) -> Result<StoredAdminGlobalModel, DataLayerError> {
    StoredAdminGlobalModel::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("name").map_sql_err()?,
        row.try_get("display_name").map_sql_err()?,
        row.try_get("is_active").map_sql_err()?,
        row.try_get("default_price_per_request").map_sql_err()?,
        optional_json_from_string(
            row.try_get("default_tiered_pricing").map_sql_err()?,
            "global_models.default_tiered_pricing",
        )?,
        optional_json_from_string(
            row.try_get("supported_capabilities").map_sql_err()?,
            "global_models.supported_capabilities",
        )?,
        optional_json_from_string(row.try_get("config").map_sql_err()?, "global_models.config")?,
        0,
        0,
        row.try_get::<i64, _>("usage_count").map_sql_err()?.max(0) as u64,
        optional_u64(
            row.try_get("created_at_unix_ms").map_sql_err()?,
            "global_models.created_at",
        )?,
        optional_u64(
            row.try_get("updated_at_unix_secs").map_sql_err()?,
            "global_models.updated_at",
        )?,
    )
}

fn map_admin_provider_model_row(
    row: &MySqlRow,
) -> Result<StoredAdminProviderModel, DataLayerError> {
    StoredAdminProviderModel::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("provider_id").map_sql_err()?,
        row.try_get("global_model_id").map_sql_err()?,
        row.try_get("provider_model_name").map_sql_err()?,
        optional_json_from_string(
            row.try_get("provider_model_mappings").map_sql_err()?,
            "models.provider_model_mappings",
        )?,
        row.try_get("price_per_request").map_sql_err()?,
        optional_json_from_string(
            row.try_get("tiered_pricing").map_sql_err()?,
            "models.tiered_pricing",
        )?,
        row.try_get("supports_vision").map_sql_err()?,
        row.try_get("supports_function_calling").map_sql_err()?,
        row.try_get("supports_streaming").map_sql_err()?,
        row.try_get("supports_extended_thinking").map_sql_err()?,
        row.try_get("supports_image_generation").map_sql_err()?,
        row.try_get("is_active").map_sql_err()?,
        row.try_get("is_available").map_sql_err()?,
        optional_json_from_string(row.try_get("config").map_sql_err()?, "models.config")?,
        optional_u64(
            row.try_get("created_at_unix_ms").map_sql_err()?,
            "models.created_at",
        )?,
        optional_u64(
            row.try_get("updated_at_unix_secs").map_sql_err()?,
            "models.updated_at",
        )?,
        row.try_get("global_model_name").map_sql_err()?,
        row.try_get("global_model_display_name").map_sql_err()?,
        row.try_get("global_model_default_price_per_request")
            .map_sql_err()?,
        optional_json_from_string(
            row.try_get("global_model_default_tiered_pricing")
                .map_sql_err()?,
            "global_models.default_tiered_pricing",
        )?,
        optional_json_from_string(
            row.try_get("global_model_supported_capabilities")
                .map_sql_err()?,
            "global_models.supported_capabilities",
        )?,
        optional_json_from_string(
            row.try_get("global_model_config").map_sql_err()?,
            "global_models.config",
        )?,
    )
}

fn map_public_catalog_model_row(
    row: &MySqlRow,
) -> Result<StoredPublicCatalogModel, DataLayerError> {
    let global_model_config = optional_json_from_string(
        row.try_get("global_model_config").map_sql_err()?,
        "global_models.config",
    )?;
    let global_model_supported_capabilities = optional_json_from_string(
        row.try_get("global_model_supported_capabilities")
            .map_sql_err()?,
        "global_models.supported_capabilities",
    )?;
    let model_config =
        optional_json_from_string(row.try_get("model_config").map_sql_err()?, "models.config")?;
    let tiered_pricing = optional_json_from_string(
        row.try_get("tiered_pricing").map_sql_err()?,
        "models.tiered_pricing",
    )?;
    let default_tiered_pricing = optional_json_from_string(
        row.try_get("default_tiered_pricing").map_sql_err()?,
        "global_models.default_tiered_pricing",
    )?;
    let pricing = tiered_pricing.as_ref().or(default_tiered_pricing.as_ref());
    let global_model_is_active = row
        .try_get::<Option<bool>, _>("global_model_is_active")
        .map_sql_err()?
        .unwrap_or(true);
    let model_is_active: bool = row.try_get("is_active").map_sql_err()?;
    let provider_is_active: bool = row.try_get("provider_is_active").map_sql_err()?;

    StoredPublicCatalogModel::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("provider_id").map_sql_err()?,
        row.try_get("provider_name").map_sql_err()?,
        row.try_get("provider_model_name").map_sql_err()?,
        row.try_get("name").map_sql_err()?,
        row.try_get("display_name").map_sql_err()?,
        global_model_config
            .as_ref()
            .and_then(|value| value.get("description"))
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        global_model_config
            .as_ref()
            .and_then(|value| value.get("icon_url"))
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        first_tier_price(pricing, "input_price_per_1m"),
        first_tier_price(pricing, "output_price_per_1m"),
        first_tier_price(pricing, "cache_creation_price_per_1m"),
        first_tier_price(pricing, "cache_read_price_per_1m"),
        row.try_get("supports_vision").map_sql_err()?,
        row.try_get("supports_function_calling").map_sql_err()?,
        row.try_get("supports_streaming").map_sql_err()?,
        metadata_supports_embedding(
            global_model_supported_capabilities.as_ref(),
            global_model_config.as_ref(),
            model_config.as_ref(),
        ),
        model_is_active && provider_is_active && global_model_is_active,
    )
}

fn map_provider_model_stats_row(
    row: &MySqlRow,
) -> Result<StoredProviderModelStats, DataLayerError> {
    StoredProviderModelStats::new(
        row.try_get("provider_id").map_sql_err()?,
        row.try_get("total_models").map_sql_err()?,
        row.try_get::<Option<i64>, _>("active_models")
            .map_sql_err()?
            .unwrap_or(0),
    )
}

fn map_active_global_model_row(
    row: &MySqlRow,
) -> Result<StoredProviderActiveGlobalModel, DataLayerError> {
    StoredProviderActiveGlobalModel::new(
        row.try_get("provider_id").map_sql_err()?,
        row.try_get("global_model_id").map_sql_err()?,
    )
}

fn optional_admin_global_model_usage_count_i64(
    value: Option<u64>,
) -> Result<Option<i64>, DataLayerError> {
    value
        .map(|value| {
            i64::try_from(value).map_err(|_| {
                DataLayerError::InvalidInput(
                    "global_models.usage_count exceeds i64 range".to_string(),
                )
            })
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::MysqlGlobalModelReadRepository;

    #[tokio::test]
    async fn repository_builds_from_lazy_pool() {
        let pool = sqlx::mysql::MySqlPoolOptions::new().connect_lazy_with(
            "mysql://user:pass@localhost:3306/aether"
                .parse()
                .expect("mysql options should parse"),
        );

        let _repository = MysqlGlobalModelReadRepository::new(pool);
    }
}
