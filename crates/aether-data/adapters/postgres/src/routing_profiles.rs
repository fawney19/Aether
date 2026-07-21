use async_trait::async_trait;
use futures_util::TryStreamExt;
use sqlx::{postgres::PgRow, PgPool, Row};

use aether_data_contracts::repository::routing_profiles::*;
use aether_data_contracts::DataLayerError;

trait SqlxResultExt<T> {
    fn map_postgres_err(self) -> Result<T, DataLayerError>;
}

impl<T> SqlxResultExt<T> for Result<T, sqlx::Error> {
    fn map_postgres_err(self) -> Result<T, DataLayerError> {
        self.map_err(DataLayerError::postgres)
    }
}

const ROUTING_GROUP_SELECT: &str = r#"
SELECT
  id,
  name,
  description,
  enabled,
  is_system_default,
  config_json,
  version,
  created_at,
  updated_at,
  published_at
FROM routing_groups
"#;

const ROUTING_GROUP_BINDING_SELECT: &str = r#"
SELECT
  id,
  group_id,
  subject_type,
  subject_id,
  is_default,
  allow_explicit_select,
  created_at,
  updated_at
FROM routing_group_bindings
"#;

const ROUTING_GROUP_VERSION_SELECT: &str = r#"
SELECT
  id,
  group_id,
  version,
  config_json,
  created_at,
  created_by
FROM routing_group_versions
"#;

#[derive(Debug, Clone)]
pub struct PostgresRoutingGroupRepository {
    pool: PgPool,
}

impl PostgresRoutingGroupRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn reload_group(&self, id: &str) -> Result<Option<StoredRoutingGroup>, DataLayerError> {
        self.find_routing_group(RoutingGroupLookupKey::Id(id)).await
    }

    async fn find_binding_by_id(
        &self,
        id: &str,
    ) -> Result<Option<StoredRoutingGroupBinding>, DataLayerError> {
        let row = sqlx::query(&format!(
            "{ROUTING_GROUP_BINDING_SELECT} WHERE id = $1 LIMIT 1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_postgres_err()?;
        row.as_ref().map(map_binding_row).transpose()
    }
}

#[async_trait]
impl RoutingGroupReadRepository for PostgresRoutingGroupRepository {
    async fn list_routing_groups(&self) -> Result<Vec<StoredRoutingGroup>, DataLayerError> {
        let sql = format!("{ROUTING_GROUP_SELECT} ORDER BY name ASC, id ASC");
        let mut rows = sqlx::query(&sql).fetch(&self.pool);
        let mut groups = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            groups.push(map_group_row(&row)?);
        }
        Ok(groups)
    }

    async fn find_routing_group(
        &self,
        lookup: RoutingGroupLookupKey<'_>,
    ) -> Result<Option<StoredRoutingGroup>, DataLayerError> {
        let row = match lookup {
            RoutingGroupLookupKey::Id(id) => sqlx::query(&format!(
                "{ROUTING_GROUP_SELECT} WHERE id = $1 LIMIT 1"
            ))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?,
            RoutingGroupLookupKey::Name(name) => sqlx::query(&format!(
                "{ROUTING_GROUP_SELECT} WHERE name = $1 LIMIT 1"
            ))
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?,
            RoutingGroupLookupKey::SystemDefault => sqlx::query(&format!(
                "{ROUTING_GROUP_SELECT} WHERE is_system_default = TRUE AND enabled = TRUE ORDER BY updated_at DESC, id ASC LIMIT 1"
            ))
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?,
        };
        row.as_ref().map(map_group_row).transpose()
    }

    async fn list_routing_group_bindings(
        &self,
        query: &RoutingGroupBindingQuery,
    ) -> Result<Vec<StoredRoutingGroupBinding>, DataLayerError> {
        let sql = format!(
            r#"
{ROUTING_GROUP_BINDING_SELECT}
WHERE ($1::text IS NULL OR group_id = $1)
  AND ($2::text IS NULL OR subject_type = $2)
  AND ($3::text IS NULL OR subject_id = $3)
ORDER BY created_at ASC, id ASC
"#
        );
        let mut rows = sqlx::query(&sql)
            .bind(query.group_id.as_deref())
            .bind(query.subject_type.map(binding_subject_to_database))
            .bind(query.subject_id.as_deref())
            .fetch(&self.pool);
        let mut bindings = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            bindings.push(map_binding_row(&row)?);
        }
        Ok(bindings)
    }

    async fn has_any_routing_group_binding(&self) -> Result<bool, DataLayerError> {
        let row = sqlx::query("SELECT 1 FROM routing_group_bindings LIMIT 1")
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(row.is_some())
    }

    async fn list_routing_group_versions(
        &self,
        group_id: &str,
    ) -> Result<Vec<StoredRoutingGroupVersion>, DataLayerError> {
        let sql = format!(
            "{ROUTING_GROUP_VERSION_SELECT} WHERE group_id = $1 ORDER BY version DESC, created_at DESC, id ASC"
        );
        let mut rows = sqlx::query(&sql).bind(group_id).fetch(&self.pool);
        let mut versions = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            versions.push(map_version_row(&row)?);
        }
        Ok(versions)
    }
}

#[async_trait]
impl RoutingGroupWriteRepository for PostgresRoutingGroupRepository {
    async fn create_routing_group(
        &self,
        record: CreateRoutingGroupRecord,
    ) -> Result<StoredRoutingGroup, DataLayerError> {
        let group = StoredRoutingGroup::new(record)?;
        sqlx::query(
            r#"
INSERT INTO routing_groups (
  id, name, description, enabled, is_system_default, config_json,
  version, created_at, updated_at, published_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
"#,
        )
        .bind(&group.id)
        .bind(&group.name)
        .bind(&group.description)
        .bind(group.enabled)
        .bind(group.is_system_default)
        .bind(&group.config_json)
        .bind(group.version)
        .bind(group.created_at)
        .bind(group.updated_at)
        .bind(group.published_at)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(group)
    }

    async fn update_routing_group(
        &self,
        id: &str,
        patch: UpdateRoutingGroupRecord,
    ) -> Result<Option<StoredRoutingGroup>, DataLayerError> {
        let Some(mut group) = self.reload_group(id).await? else {
            return Ok(None);
        };
        apply_group_patch(&mut group, patch)?;
        sqlx::query(
            r#"
UPDATE routing_groups
SET name = $2,
    description = $3,
    enabled = $4,
    is_system_default = $5,
    config_json = $6,
    version = $7,
    updated_at = $8,
    published_at = $9
WHERE id = $1
"#,
        )
        .bind(id)
        .bind(&group.name)
        .bind(&group.description)
        .bind(group.enabled)
        .bind(group.is_system_default)
        .bind(&group.config_json)
        .bind(group.version)
        .bind(group.updated_at)
        .bind(group.published_at)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(Some(group))
    }

    async fn delete_routing_group(&self, id: &str) -> Result<bool, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        sqlx::query("DELETE FROM routing_group_bindings WHERE group_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        sqlx::query("DELETE FROM routing_group_versions WHERE group_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        let rows_affected = sqlx::query("DELETE FROM routing_groups WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?
            .rows_affected();
        tx.commit().await.map_postgres_err()?;
        Ok(rows_affected > 0)
    }

    async fn create_routing_group_binding(
        &self,
        record: CreateRoutingGroupBindingRecord,
    ) -> Result<StoredRoutingGroupBinding, DataLayerError> {
        let binding = StoredRoutingGroupBinding::new(record)?;
        sqlx::query(
            r#"
INSERT INTO routing_group_bindings (
  id, group_id, subject_type, subject_id, is_default,
  allow_explicit_select, created_at, updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
"#,
        )
        .bind(&binding.id)
        .bind(&binding.group_id)
        .bind(binding_subject_to_database(binding.subject_type))
        .bind(&binding.subject_id)
        .bind(binding.is_default)
        .bind(binding.allow_explicit_select)
        .bind(binding.created_at)
        .bind(binding.updated_at)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(binding)
    }

    async fn delete_routing_group_binding(&self, id: &str) -> Result<bool, DataLayerError> {
        Ok(
            sqlx::query("DELETE FROM routing_group_bindings WHERE id = $1")
                .bind(id)
                .execute(&self.pool)
                .await
                .map_postgres_err()?
                .rows_affected()
                > 0,
        )
    }

    async fn update_routing_group_binding(
        &self,
        id: &str,
        patch: UpdateRoutingGroupBindingRecord,
    ) -> Result<Option<StoredRoutingGroupBinding>, DataLayerError> {
        let Some(mut binding) = self.find_binding_by_id(id).await? else {
            return Ok(None);
        };
        apply_binding_patch(&mut binding, patch)?;
        sqlx::query(
            r#"
UPDATE routing_group_bindings
SET group_id = $2,
    subject_type = $3,
    subject_id = $4,
    is_default = $5,
    allow_explicit_select = $6,
    updated_at = $7
WHERE id = $1
"#,
        )
        .bind(id)
        .bind(&binding.group_id)
        .bind(binding_subject_to_database(binding.subject_type))
        .bind(&binding.subject_id)
        .bind(binding.is_default)
        .bind(binding.allow_explicit_select)
        .bind(binding.updated_at)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(Some(binding))
    }

    async fn create_routing_group_version(
        &self,
        record: CreateRoutingGroupVersionRecord,
    ) -> Result<StoredRoutingGroupVersion, DataLayerError> {
        let version = StoredRoutingGroupVersion::new(record)?;
        sqlx::query(
            r#"
INSERT INTO routing_group_versions (
  id, group_id, version, config_json, created_at, created_by
)
VALUES ($1, $2, $3, $4, $5, $6)
"#,
        )
        .bind(&version.id)
        .bind(&version.group_id)
        .bind(version.version)
        .bind(&version.config_json)
        .bind(version.created_at)
        .bind(&version.created_by)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(version)
    }
}

fn map_group_row(row: &PgRow) -> Result<StoredRoutingGroup, DataLayerError> {
    Ok(StoredRoutingGroup {
        id: row.try_get("id").map_postgres_err()?,
        name: row.try_get("name").map_postgres_err()?,
        description: row.try_get("description").map_postgres_err()?,
        enabled: row.try_get("enabled").map_postgres_err()?,
        is_system_default: row.try_get("is_system_default").map_postgres_err()?,
        config_json: row.try_get("config_json").map_postgres_err()?,
        version: row.try_get("version").map_postgres_err()?,
        created_at: row.try_get("created_at").map_postgres_err()?,
        updated_at: row.try_get("updated_at").map_postgres_err()?,
        published_at: row.try_get("published_at").map_postgres_err()?,
    })
}

fn map_binding_row(row: &PgRow) -> Result<StoredRoutingGroupBinding, DataLayerError> {
    Ok(StoredRoutingGroupBinding {
        id: row.try_get("id").map_postgres_err()?,
        group_id: row.try_get("group_id").map_postgres_err()?,
        subject_type: binding_subject_from_database(
            row.try_get("subject_type").map_postgres_err()?,
        )?,
        subject_id: row.try_get("subject_id").map_postgres_err()?,
        is_default: row.try_get("is_default").map_postgres_err()?,
        allow_explicit_select: row.try_get("allow_explicit_select").map_postgres_err()?,
        created_at: row.try_get("created_at").map_postgres_err()?,
        updated_at: row.try_get("updated_at").map_postgres_err()?,
    })
}

fn map_version_row(row: &PgRow) -> Result<StoredRoutingGroupVersion, DataLayerError> {
    Ok(StoredRoutingGroupVersion {
        id: row.try_get("id").map_postgres_err()?,
        group_id: row.try_get("group_id").map_postgres_err()?,
        version: row.try_get("version").map_postgres_err()?,
        config_json: row.try_get("config_json").map_postgres_err()?,
        created_at: row.try_get("created_at").map_postgres_err()?,
        created_by: row.try_get("created_by").map_postgres_err()?,
    })
}
