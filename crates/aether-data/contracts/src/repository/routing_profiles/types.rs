use async_trait::async_trait;
use serde_json::Value;

use aether_routing_core::RoutingGroupBindingSubject;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRoutingGroup {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub is_system_default: bool,
    pub config_json: Value,
    pub version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub published_at: Option<i64>,
}

impl StoredRoutingGroup {
    pub fn new(record: CreateRoutingGroupRecord) -> Result<Self, crate::DataLayerError> {
        validate_non_empty(&record.id, "routing_groups.id")?;
        validate_non_empty(&record.name, "routing_groups.name")?;
        validate_config_object(&record.config_json, "routing_groups.config_json")?;
        Ok(Self {
            id: record.id,
            name: record.name,
            description: record.description,
            enabled: record.enabled,
            is_system_default: record.is_system_default,
            config_json: record.config_json,
            version: record.version.max(1),
            created_at: record.created_at,
            updated_at: record.updated_at,
            published_at: record.published_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRoutingGroupRecord {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub is_system_default: bool,
    pub config_json: Value,
    pub version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub published_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateRoutingGroupRecord {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub enabled: Option<bool>,
    pub is_system_default: Option<bool>,
    pub config_json: Option<Value>,
    pub version: Option<i64>,
    pub updated_at: i64,
    pub published_at: Option<Option<i64>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateRoutingGroupBindingRecord {
    pub group_id: Option<String>,
    pub subject_type: Option<RoutingGroupBindingSubject>,
    pub subject_id: Option<String>,
    pub is_default: Option<bool>,
    pub allow_explicit_select: Option<bool>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRoutingGroupBinding {
    pub id: String,
    pub group_id: String,
    pub subject_type: RoutingGroupBindingSubject,
    pub subject_id: String,
    pub is_default: bool,
    pub allow_explicit_select: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl StoredRoutingGroupBinding {
    pub fn new(record: CreateRoutingGroupBindingRecord) -> Result<Self, crate::DataLayerError> {
        validate_non_empty(&record.id, "routing_group_bindings.id")?;
        validate_non_empty(&record.group_id, "routing_group_bindings.group_id")?;
        validate_non_empty(&record.subject_id, "routing_group_bindings.subject_id")?;
        Ok(Self {
            id: record.id,
            group_id: record.group_id,
            subject_type: record.subject_type,
            subject_id: record.subject_id,
            is_default: record.is_default,
            allow_explicit_select: record.allow_explicit_select,
            created_at: record.created_at,
            updated_at: record.updated_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRoutingGroupBindingRecord {
    pub id: String,
    pub group_id: String,
    pub subject_type: RoutingGroupBindingSubject,
    pub subject_id: String,
    pub is_default: bool,
    pub allow_explicit_select: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRoutingGroupVersion {
    pub id: String,
    pub group_id: String,
    pub version: i64,
    pub config_json: Value,
    pub created_at: i64,
    pub created_by: Option<String>,
}

impl StoredRoutingGroupVersion {
    pub fn new(record: CreateRoutingGroupVersionRecord) -> Result<Self, crate::DataLayerError> {
        validate_non_empty(&record.id, "routing_group_versions.id")?;
        validate_non_empty(&record.group_id, "routing_group_versions.group_id")?;
        validate_config_object(&record.config_json, "routing_group_versions.config_json")?;
        Ok(Self {
            id: record.id,
            group_id: record.group_id,
            version: record.version.max(1),
            config_json: record.config_json,
            created_at: record.created_at,
            created_by: record.created_by,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRoutingGroupVersionRecord {
    pub id: String,
    pub group_id: String,
    pub version: i64,
    pub config_json: Value,
    pub created_at: i64,
    pub created_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingGroupLookupKey<'a> {
    Id(&'a str),
    Name(&'a str),
    SystemDefault,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoutingGroupBindingQuery {
    pub group_id: Option<String>,
    pub subject_type: Option<RoutingGroupBindingSubject>,
    pub subject_id: Option<String>,
}

#[async_trait]
pub trait RoutingGroupReadRepository: Send + Sync {
    fn clear_local_cache(&self) {}

    async fn list_routing_groups(&self) -> Result<Vec<StoredRoutingGroup>, crate::DataLayerError>;

    async fn find_routing_group(
        &self,
        lookup: RoutingGroupLookupKey<'_>,
    ) -> Result<Option<StoredRoutingGroup>, crate::DataLayerError>;

    async fn list_routing_group_bindings(
        &self,
        query: &RoutingGroupBindingQuery,
    ) -> Result<Vec<StoredRoutingGroupBinding>, crate::DataLayerError>;

    async fn list_routing_group_versions(
        &self,
        group_id: &str,
    ) -> Result<Vec<StoredRoutingGroupVersion>, crate::DataLayerError>;
}

#[async_trait]
pub trait RoutingGroupWriteRepository: Send + Sync {
    async fn create_routing_group(
        &self,
        record: CreateRoutingGroupRecord,
    ) -> Result<StoredRoutingGroup, crate::DataLayerError>;

    async fn update_routing_group(
        &self,
        id: &str,
        patch: UpdateRoutingGroupRecord,
    ) -> Result<Option<StoredRoutingGroup>, crate::DataLayerError>;

    async fn delete_routing_group(&self, id: &str) -> Result<bool, crate::DataLayerError>;

    async fn create_routing_group_binding(
        &self,
        record: CreateRoutingGroupBindingRecord,
    ) -> Result<StoredRoutingGroupBinding, crate::DataLayerError>;

    async fn delete_routing_group_binding(&self, id: &str) -> Result<bool, crate::DataLayerError>;

    async fn update_routing_group_binding(
        &self,
        id: &str,
        patch: UpdateRoutingGroupBindingRecord,
    ) -> Result<Option<StoredRoutingGroupBinding>, crate::DataLayerError>;

    async fn create_routing_group_version(
        &self,
        record: CreateRoutingGroupVersionRecord,
    ) -> Result<StoredRoutingGroupVersion, crate::DataLayerError>;
}

pub fn apply_group_patch(
    group: &mut StoredRoutingGroup,
    patch: UpdateRoutingGroupRecord,
) -> Result<(), crate::DataLayerError> {
    if let Some(name) = patch.name {
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "routing_groups.name is empty".to_string(),
            ));
        }
        group.name = name;
    }
    if let Some(description) = patch.description {
        group.description = description;
    }
    if let Some(enabled) = patch.enabled {
        group.enabled = enabled;
    }
    if let Some(is_system_default) = patch.is_system_default {
        group.is_system_default = is_system_default;
    }
    if let Some(config_json) = patch.config_json {
        if !config_json.is_object() {
            return Err(crate::DataLayerError::InvalidInput(
                "routing_groups.config_json must be a JSON object".to_string(),
            ));
        }
        group.config_json = config_json;
    }
    if let Some(version) = patch.version {
        group.version = version.max(1);
    }
    if let Some(published_at) = patch.published_at {
        group.published_at = published_at;
    }
    group.updated_at = patch.updated_at;
    Ok(())
}

pub fn apply_binding_patch(
    binding: &mut StoredRoutingGroupBinding,
    patch: UpdateRoutingGroupBindingRecord,
) -> Result<(), crate::DataLayerError> {
    if let Some(group_id) = patch.group_id {
        if group_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "routing_group_bindings.group_id is empty".to_string(),
            ));
        }
        binding.group_id = group_id;
    }
    if let Some(subject_type) = patch.subject_type {
        binding.subject_type = subject_type;
    }
    if let Some(subject_id) = patch.subject_id {
        if subject_id.trim().is_empty() {
            return Err(crate::DataLayerError::InvalidInput(
                "routing_group_bindings.subject_id is empty".to_string(),
            ));
        }
        binding.subject_id = subject_id;
    }
    if let Some(is_default) = patch.is_default {
        binding.is_default = is_default;
    }
    if let Some(allow_explicit_select) = patch.allow_explicit_select {
        binding.allow_explicit_select = allow_explicit_select;
    }
    binding.updated_at = patch.updated_at;
    Ok(())
}

pub fn binding_subject_to_database(subject: RoutingGroupBindingSubject) -> &'static str {
    match subject {
        RoutingGroupBindingSubject::User => "user",
        RoutingGroupBindingSubject::ApiKey => "api_key",
        RoutingGroupBindingSubject::UserGroup => "user_group",
    }
}

pub fn binding_subject_from_database(
    value: String,
) -> Result<RoutingGroupBindingSubject, crate::DataLayerError> {
    match value.as_str() {
        "user" => Ok(RoutingGroupBindingSubject::User),
        "api_key" => Ok(RoutingGroupBindingSubject::ApiKey),
        "user_group" => Ok(RoutingGroupBindingSubject::UserGroup),
        _ => Err(crate::DataLayerError::UnexpectedValue(format!(
            "invalid routing_group_bindings.subject_type: {value}"
        ))),
    }
}

fn validate_non_empty(value: &str, field: &str) -> Result<(), crate::DataLayerError> {
    if value.trim().is_empty() {
        return Err(crate::DataLayerError::InvalidInput(format!(
            "{field} is empty"
        )));
    }
    Ok(())
}

fn validate_config_object(value: &Value, field: &str) -> Result<(), crate::DataLayerError> {
    if !value.is_object() {
        return Err(crate::DataLayerError::InvalidInput(format!(
            "{field} must be a JSON object"
        )));
    }
    Ok(())
}
