use async_trait::async_trait;
use serde_json::Value;
use sqlx::{sqlite::SqliteRow, Row};

use aether_data_contracts::repository::routing_profiles::*;
use aether_data_contracts::DataLayerError;

use crate::error::SqlResultExt;
use crate::pool::SqlitePool;

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
pub struct SqliteRoutingGroupRepository {
    pool: SqlitePool,
}

impl SqliteRoutingGroupRepository {
    pub fn new(pool: SqlitePool) -> Self {
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
            "{ROUTING_GROUP_BINDING_SELECT} WHERE id = ? LIMIT 1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?;
        row.as_ref().map(map_binding_row).transpose()
    }
}

#[async_trait]
impl RoutingGroupReadRepository for SqliteRoutingGroupRepository {
    async fn list_routing_groups(&self) -> Result<Vec<StoredRoutingGroup>, DataLayerError> {
        let rows = sqlx::query(&format!("{ROUTING_GROUP_SELECT} ORDER BY name ASC, id ASC"))
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?;
        rows.iter().map(map_group_row).collect()
    }

    async fn find_routing_group(
        &self,
        lookup: RoutingGroupLookupKey<'_>,
    ) -> Result<Option<StoredRoutingGroup>, DataLayerError> {
        let row = match lookup {
            RoutingGroupLookupKey::Id(id) => sqlx::query(&format!(
                "{ROUTING_GROUP_SELECT} WHERE id = ? LIMIT 1"
            ))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?,
            RoutingGroupLookupKey::Name(name) => sqlx::query(&format!(
                "{ROUTING_GROUP_SELECT} WHERE name = ? LIMIT 1"
            ))
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?,
            RoutingGroupLookupKey::SystemDefault => sqlx::query(&format!(
                "{ROUTING_GROUP_SELECT} WHERE is_system_default = 1 AND enabled = 1 ORDER BY updated_at DESC, id ASC LIMIT 1"
            ))
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?,
        };
        row.as_ref().map(map_group_row).transpose()
    }

    async fn list_routing_group_bindings(
        &self,
        query: &RoutingGroupBindingQuery,
    ) -> Result<Vec<StoredRoutingGroupBinding>, DataLayerError> {
        let rows = sqlx::query(&format!(
            r#"
{ROUTING_GROUP_BINDING_SELECT}
WHERE (? IS NULL OR group_id = ?)
  AND (? IS NULL OR subject_type = ?)
  AND (? IS NULL OR subject_id = ?)
ORDER BY created_at ASC, id ASC
"#
        ))
        .bind(query.group_id.as_deref())
        .bind(query.group_id.as_deref())
        .bind(query.subject_type.map(binding_subject_to_database))
        .bind(query.subject_type.map(binding_subject_to_database))
        .bind(query.subject_id.as_deref())
        .bind(query.subject_id.as_deref())
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_binding_row).collect()
    }

    async fn list_routing_group_versions(
        &self,
        group_id: &str,
    ) -> Result<Vec<StoredRoutingGroupVersion>, DataLayerError> {
        let rows = sqlx::query(&format!(
            "{ROUTING_GROUP_VERSION_SELECT} WHERE group_id = ? ORDER BY version DESC, created_at DESC, id ASC"
        ))
        .bind(group_id)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        rows.iter().map(map_version_row).collect()
    }
}

#[async_trait]
impl RoutingGroupWriteRepository for SqliteRoutingGroupRepository {
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
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
        )
        .bind(&group.id)
        .bind(&group.name)
        .bind(&group.description)
        .bind(group.enabled)
        .bind(group.is_system_default)
        .bind(json_to_string(
            &group.config_json,
            "routing_groups.config_json",
        )?)
        .bind(group.version)
        .bind(group.created_at)
        .bind(group.updated_at)
        .bind(group.published_at)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
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
SET name = ?,
    description = ?,
    enabled = ?,
    is_system_default = ?,
    config_json = ?,
    version = ?,
    updated_at = ?,
    published_at = ?
WHERE id = ?
"#,
        )
        .bind(&group.name)
        .bind(&group.description)
        .bind(group.enabled)
        .bind(group.is_system_default)
        .bind(json_to_string(
            &group.config_json,
            "routing_groups.config_json",
        )?)
        .bind(group.version)
        .bind(group.updated_at)
        .bind(group.published_at)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(Some(group))
    }

    async fn delete_routing_group(&self, id: &str) -> Result<bool, DataLayerError> {
        let mut tx = self.pool.begin().await.map_sql_err()?;
        sqlx::query("DELETE FROM routing_group_bindings WHERE group_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        sqlx::query("DELETE FROM routing_group_versions WHERE group_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        let rows_affected = sqlx::query("DELETE FROM routing_groups WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?
            .rows_affected();
        tx.commit().await.map_sql_err()?;
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
VALUES (?, ?, ?, ?, ?, ?, ?, ?)
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
        .map_sql_err()?;
        Ok(binding)
    }

    async fn delete_routing_group_binding(&self, id: &str) -> Result<bool, DataLayerError> {
        Ok(
            sqlx::query("DELETE FROM routing_group_bindings WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
                .await
                .map_sql_err()?
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
SET group_id = ?,
    subject_type = ?,
    subject_id = ?,
    is_default = ?,
    allow_explicit_select = ?,
    updated_at = ?
WHERE id = ?
"#,
        )
        .bind(&binding.group_id)
        .bind(binding_subject_to_database(binding.subject_type))
        .bind(&binding.subject_id)
        .bind(binding.is_default)
        .bind(binding.allow_explicit_select)
        .bind(binding.updated_at)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
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
VALUES (?, ?, ?, ?, ?, ?)
"#,
        )
        .bind(&version.id)
        .bind(&version.group_id)
        .bind(version.version)
        .bind(json_to_string(
            &version.config_json,
            "routing_group_versions.config_json",
        )?)
        .bind(version.created_at)
        .bind(&version.created_by)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(version)
    }
}

fn map_group_row(row: &SqliteRow) -> Result<StoredRoutingGroup, DataLayerError> {
    Ok(StoredRoutingGroup {
        id: row.try_get("id").map_sql_err()?,
        name: row.try_get("name").map_sql_err()?,
        description: row.try_get("description").map_sql_err()?,
        enabled: row.try_get("enabled").map_sql_err()?,
        is_system_default: row.try_get("is_system_default").map_sql_err()?,
        config_json: json_from_string(
            row.try_get("config_json").map_sql_err()?,
            "routing_groups.config_json",
        )?,
        version: row.try_get("version").map_sql_err()?,
        created_at: row.try_get("created_at").map_sql_err()?,
        updated_at: row.try_get("updated_at").map_sql_err()?,
        published_at: row.try_get("published_at").map_sql_err()?,
    })
}

fn map_binding_row(row: &SqliteRow) -> Result<StoredRoutingGroupBinding, DataLayerError> {
    Ok(StoredRoutingGroupBinding {
        id: row.try_get("id").map_sql_err()?,
        group_id: row.try_get("group_id").map_sql_err()?,
        subject_type: binding_subject_from_database(row.try_get("subject_type").map_sql_err()?)?,
        subject_id: row.try_get("subject_id").map_sql_err()?,
        is_default: row.try_get("is_default").map_sql_err()?,
        allow_explicit_select: row.try_get("allow_explicit_select").map_sql_err()?,
        created_at: row.try_get("created_at").map_sql_err()?,
        updated_at: row.try_get("updated_at").map_sql_err()?,
    })
}

fn map_version_row(row: &SqliteRow) -> Result<StoredRoutingGroupVersion, DataLayerError> {
    Ok(StoredRoutingGroupVersion {
        id: row.try_get("id").map_sql_err()?,
        group_id: row.try_get("group_id").map_sql_err()?,
        version: row.try_get("version").map_sql_err()?,
        config_json: json_from_string(
            row.try_get("config_json").map_sql_err()?,
            "routing_group_versions.config_json",
        )?,
        created_at: row.try_get("created_at").map_sql_err()?,
        created_by: row.try_get("created_by").map_sql_err()?,
    })
}

fn json_to_string(value: &Value, field_name: &str) -> Result<String, DataLayerError> {
    serde_json::to_string(value).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("{field_name} contains unserializable JSON: {err}"))
    })
}

fn json_from_string(value: String, field_name: &str) -> Result<Value, DataLayerError> {
    serde_json::from_str(&value).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("{field_name} contains invalid JSON: {err}"))
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::run_migrations as run_sqlite_migrations;

    #[tokio::test]
    async fn sqlite_routing_group_repository_round_trips() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");

        let repository = SqliteRoutingGroupRepository::new(pool);
        repository
            .create_routing_group(CreateRoutingGroupRecord {
                id: "routing-group-1".to_string(),
                name: "default".to_string(),
                description: Some("initial".to_string()),
                enabled: true,
                is_system_default: true,
                config_json: json!({"allowed_models": ["gpt-*"]}),
                version: 1,
                created_at: 10,
                updated_at: 10,
                published_at: None,
            })
            .await
            .expect("group should create");

        let system_default = repository
            .find_routing_group(RoutingGroupLookupKey::SystemDefault)
            .await
            .expect("group lookup should succeed")
            .expect("system default should exist");
        assert_eq!(system_default.id, "routing-group-1");

        repository
            .update_routing_group(
                "routing-group-1",
                UpdateRoutingGroupRecord {
                    description: Some(None),
                    version: Some(2),
                    updated_at: 20,
                    published_at: Some(Some(20)),
                    ..UpdateRoutingGroupRecord::default()
                },
            )
            .await
            .expect("group should update");

        let binding = repository
            .create_routing_group_binding(CreateRoutingGroupBindingRecord {
                id: "binding-1".to_string(),
                group_id: "routing-group-1".to_string(),
                subject_type: RoutingGroupBindingSubject::ApiKey,
                subject_id: "api-key-1".to_string(),
                is_default: true,
                allow_explicit_select: true,
                created_at: 10,
                updated_at: 10,
            })
            .await
            .expect("binding should create");

        assert_eq!(binding.subject_type, RoutingGroupBindingSubject::ApiKey);
        assert_eq!(
            repository
                .list_routing_group_bindings(&RoutingGroupBindingQuery {
                    group_id: Some("routing-group-1".to_string()),
                    subject_type: Some(RoutingGroupBindingSubject::ApiKey),
                    subject_id: Some("api-key-1".to_string()),
                })
                .await
                .expect("bindings should list")
                .len(),
            1
        );

        repository
            .create_routing_group_version(CreateRoutingGroupVersionRecord {
                id: "version-1".to_string(),
                group_id: "routing-group-1".to_string(),
                version: 2,
                config_json: json!({"allowed_models": ["gpt-*"]}),
                created_at: 20,
                created_by: Some("admin".to_string()),
            })
            .await
            .expect("version should create");

        assert_eq!(
            repository
                .list_routing_group_versions("routing-group-1")
                .await
                .expect("versions should list")
                .len(),
            1
        );
    }
}
