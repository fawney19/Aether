use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;

use super::{
    CreateRoutingGroupBindingRecord, CreateRoutingGroupRecord, CreateRoutingGroupVersionRecord,
    RoutingGroupBindingQuery, RoutingGroupLookupKey, RoutingGroupReadRepository,
    RoutingGroupWriteRepository, StoredRoutingGroup, StoredRoutingGroupBinding,
    StoredRoutingGroupVersion, UpdateRoutingGroupBindingRecord, UpdateRoutingGroupRecord,
};
use crate::DataLayerError;

#[derive(Debug, Default)]
pub struct InMemoryRoutingGroupRepository {
    groups: RwLock<BTreeMap<String, StoredRoutingGroup>>,
    bindings: RwLock<BTreeMap<String, StoredRoutingGroupBinding>>,
    versions: RwLock<BTreeMap<String, StoredRoutingGroupVersion>>,
}

impl InMemoryRoutingGroupRepository {
    pub fn seed<I, B, V>(groups: I, bindings: B, versions: V) -> Self
    where
        I: IntoIterator<Item = StoredRoutingGroup>,
        B: IntoIterator<Item = StoredRoutingGroupBinding>,
        V: IntoIterator<Item = StoredRoutingGroupVersion>,
    {
        Self {
            groups: RwLock::new(
                groups
                    .into_iter()
                    .map(|item| (item.id.clone(), item))
                    .collect(),
            ),
            bindings: RwLock::new(
                bindings
                    .into_iter()
                    .map(|item| (item.id.clone(), item))
                    .collect(),
            ),
            versions: RwLock::new(
                versions
                    .into_iter()
                    .map(|item| (item.id.clone(), item))
                    .collect(),
            ),
        }
    }
}

#[async_trait]
impl RoutingGroupReadRepository for InMemoryRoutingGroupRepository {
    async fn list_routing_groups(&self) -> Result<Vec<StoredRoutingGroup>, DataLayerError> {
        let mut groups = self
            .groups
            .read()
            .expect("routing group repository lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        groups.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        Ok(groups)
    }

    async fn find_routing_group(
        &self,
        lookup: RoutingGroupLookupKey<'_>,
    ) -> Result<Option<StoredRoutingGroup>, DataLayerError> {
        let groups = self.groups.read().expect("routing group repository lock");
        Ok(match lookup {
            RoutingGroupLookupKey::Id(id) => groups.get(id).cloned(),
            RoutingGroupLookupKey::Name(name) => {
                groups.values().find(|group| group.name == name).cloned()
            }
            RoutingGroupLookupKey::SystemDefault => groups
                .values()
                .find(|group| group.is_system_default && group.enabled)
                .cloned(),
        })
    }

    async fn list_routing_group_bindings(
        &self,
        query: &RoutingGroupBindingQuery,
    ) -> Result<Vec<StoredRoutingGroupBinding>, DataLayerError> {
        let mut rows = self
            .bindings
            .read()
            .expect("routing group binding repository lock")
            .values()
            .filter(|row| {
                query
                    .group_id
                    .as_ref()
                    .is_none_or(|group_id| &row.group_id == group_id)
                    && query
                        .subject_type
                        .as_ref()
                        .is_none_or(|subject_type| &row.subject_type == subject_type)
                    && query
                        .subject_id
                        .as_ref()
                        .is_none_or(|subject_id| &row.subject_id == subject_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then(left.id.cmp(&right.id))
        });
        Ok(rows)
    }

    async fn has_any_routing_group_binding(&self) -> Result<bool, DataLayerError> {
        Ok(!self
            .bindings
            .read()
            .expect("routing group binding repository lock")
            .is_empty())
    }

    async fn list_routing_group_versions(
        &self,
        group_id: &str,
    ) -> Result<Vec<StoredRoutingGroupVersion>, DataLayerError> {
        let mut rows = self
            .versions
            .read()
            .expect("routing group version repository lock")
            .values()
            .filter(|row| row.group_id == group_id)
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            right
                .version
                .cmp(&left.version)
                .then(right.created_at.cmp(&left.created_at))
        });
        Ok(rows)
    }
}

#[async_trait]
impl RoutingGroupWriteRepository for InMemoryRoutingGroupRepository {
    async fn create_routing_group(
        &self,
        record: CreateRoutingGroupRecord,
    ) -> Result<StoredRoutingGroup, DataLayerError> {
        let group = StoredRoutingGroup::new(record)?;
        self.groups
            .write()
            .expect("routing group repository lock")
            .insert(group.id.clone(), group.clone());
        Ok(group)
    }

    async fn update_routing_group(
        &self,
        id: &str,
        patch: UpdateRoutingGroupRecord,
    ) -> Result<Option<StoredRoutingGroup>, DataLayerError> {
        let mut groups = self.groups.write().expect("routing group repository lock");
        let Some(group) = groups.get_mut(id) else {
            return Ok(None);
        };
        if let Some(name) = patch.name {
            if name.trim().is_empty() {
                return Err(DataLayerError::InvalidInput(
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
                return Err(DataLayerError::InvalidInput(
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
        Ok(Some(group.clone()))
    }

    async fn delete_routing_group(&self, id: &str) -> Result<bool, DataLayerError> {
        Ok(self
            .groups
            .write()
            .expect("routing group repository lock")
            .remove(id)
            .is_some())
    }

    async fn create_routing_group_binding(
        &self,
        record: CreateRoutingGroupBindingRecord,
    ) -> Result<StoredRoutingGroupBinding, DataLayerError> {
        let binding = StoredRoutingGroupBinding::new(record)?;
        self.bindings
            .write()
            .expect("routing group binding repository lock")
            .insert(binding.id.clone(), binding.clone());
        Ok(binding)
    }

    async fn delete_routing_group_binding(&self, id: &str) -> Result<bool, DataLayerError> {
        Ok(self
            .bindings
            .write()
            .expect("routing group binding repository lock")
            .remove(id)
            .is_some())
    }

    async fn update_routing_group_binding(
        &self,
        id: &str,
        patch: UpdateRoutingGroupBindingRecord,
    ) -> Result<Option<StoredRoutingGroupBinding>, DataLayerError> {
        let mut bindings = self
            .bindings
            .write()
            .expect("routing group binding repository lock");
        let Some(binding) = bindings.get_mut(id) else {
            return Ok(None);
        };
        if let Some(group_id) = patch.group_id {
            if group_id.trim().is_empty() {
                return Err(DataLayerError::InvalidInput(
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
                return Err(DataLayerError::InvalidInput(
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
        Ok(Some(binding.clone()))
    }

    async fn create_routing_group_version(
        &self,
        record: CreateRoutingGroupVersionRecord,
    ) -> Result<StoredRoutingGroupVersion, DataLayerError> {
        let version = StoredRoutingGroupVersion::new(record)?;
        self.versions
            .write()
            .expect("routing group version repository lock")
            .insert(version.id.clone(), version.clone());
        Ok(version)
    }
}

#[cfg(test)]
mod tests {
    use aether_data_contracts::repository::routing_profiles::RoutingGroupBindingSubject;
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn stores_groups_bindings_and_versions() {
        let repository = InMemoryRoutingGroupRepository::default();
        let group = repository
            .create_routing_group(CreateRoutingGroupRecord {
                id: "group-1".to_string(),
                name: "default".to_string(),
                description: None,
                enabled: true,
                is_system_default: true,
                config_json: json!({}),
                version: 1,
                created_at: 1,
                updated_at: 1,
                published_at: None,
            })
            .await
            .expect("group should store");

        assert_eq!(
            repository
                .find_routing_group(RoutingGroupLookupKey::SystemDefault)
                .await
                .unwrap()
                .as_ref()
                .map(|group| group.id.as_str()),
            Some(group.id.as_str())
        );

        repository
            .create_routing_group_binding(CreateRoutingGroupBindingRecord {
                id: "binding-1".to_string(),
                group_id: "group-1".to_string(),
                subject_type: RoutingGroupBindingSubject::ApiKey,
                subject_id: "api-key-1".to_string(),
                is_default: true,
                allow_explicit_select: true,
                created_at: 1,
                updated_at: 1,
            })
            .await
            .unwrap();

        assert_eq!(
            repository
                .list_routing_group_bindings(&RoutingGroupBindingQuery {
                    subject_type: Some(RoutingGroupBindingSubject::ApiKey),
                    subject_id: Some("api-key-1".to_string()),
                    group_id: None,
                })
                .await
                .unwrap()
                .len(),
            1
        );
    }
}
