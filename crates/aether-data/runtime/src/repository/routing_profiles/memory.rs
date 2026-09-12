use std::collections::BTreeMap;
use std::sync::RwLock;

use aether_data_contracts::repository::routing_profiles::{apply_binding_patch, apply_group_patch};
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
        groups.sort_by(|left, right| {
            right
                .enabled
                .cmp(&left.enabled)
                .then(left.sort_order.cmp(&right.sort_order))
                .then(left.name.cmp(&right.name))
                .then(left.id.cmp(&right.id))
        });
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
        let mut groups = self.groups.write().expect("routing group repository lock");
        if group.is_system_default {
            for existing in groups.values_mut() {
                existing.is_system_default = false;
            }
        }
        groups.insert(group.id.clone(), group.clone());
        Ok(group)
    }

    async fn update_routing_group(
        &self,
        id: &str,
        patch: UpdateRoutingGroupRecord,
    ) -> Result<Option<StoredRoutingGroup>, DataLayerError> {
        let mut groups = self.groups.write().expect("routing group repository lock");
        let Some(mut group) = groups.get(id).cloned() else {
            return Ok(None);
        };
        apply_group_patch(&mut group, patch)?;
        if group.is_system_default {
            for existing in groups.values_mut() {
                existing.is_system_default = false;
            }
        }
        groups.insert(id.to_string(), group.clone());
        Ok(Some(group))
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
        let mut bindings = self
            .bindings
            .write()
            .expect("routing group binding repository lock");
        if binding.is_default {
            for existing in bindings.values_mut().filter(|existing| {
                existing.subject_type == binding.subject_type
                    && existing.subject_id == binding.subject_id
            }) {
                existing.is_default = false;
            }
        }
        bindings.insert(binding.id.clone(), binding.clone());
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
        let Some(mut binding) = bindings.get(id).cloned() else {
            return Ok(None);
        };
        apply_binding_patch(&mut binding, patch)?;
        if binding.is_default {
            for existing in bindings.values_mut().filter(|existing| {
                existing.subject_type == binding.subject_type
                    && existing.subject_id == binding.subject_id
            }) {
                existing.is_default = false;
            }
        }
        bindings.insert(id.to_string(), binding.clone());
        Ok(Some(binding))
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
                sort_order: 0,
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

    #[tokio::test]
    async fn lists_enabled_groups_first_and_respects_sort_order() {
        let repository = InMemoryRoutingGroupRepository::default();
        for (id, enabled, sort_order) in [
            ("disabled-first", false, 0),
            ("enabled-second", true, 20),
            ("enabled-first", true, 10),
        ] {
            repository
                .create_routing_group(CreateRoutingGroupRecord {
                    id: id.to_string(),
                    name: id.to_string(),
                    description: None,
                    enabled,
                    is_system_default: false,
                    sort_order,
                    config_json: json!({}),
                    version: 1,
                    created_at: 1,
                    updated_at: 1,
                    published_at: None,
                })
                .await
                .expect("group should store");
        }

        let ids = repository
            .list_routing_groups()
            .await
            .expect("groups should list")
            .into_iter()
            .map(|group| group.id)
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![
                "enabled-first".to_string(),
                "enabled-second".to_string(),
                "disabled-first".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn keeps_system_and_subject_defaults_unique() {
        let repository = InMemoryRoutingGroupRepository::default();
        for (id, is_system_default) in [("group-1", true), ("group-2", true), ("group-3", false)] {
            repository
                .create_routing_group(group_record(id, is_system_default))
                .await
                .expect("group should store");
        }

        assert_eq!(system_default_ids(&repository).await, vec!["group-2"]);

        repository
            .update_routing_group(
                "group-1",
                UpdateRoutingGroupRecord {
                    is_system_default: Some(true),
                    updated_at: 2,
                    ..UpdateRoutingGroupRecord::default()
                },
            )
            .await
            .expect("group should update");
        assert_eq!(system_default_ids(&repository).await, vec!["group-1"]);

        repository
            .create_routing_group_binding(binding_record("binding-1", "group-1", "subject-1", true))
            .await
            .expect("binding should store");
        repository
            .create_routing_group_binding(binding_record("binding-2", "group-2", "subject-1", true))
            .await
            .expect("binding should store");
        repository
            .create_routing_group_binding(binding_record("binding-3", "group-3", "subject-2", true))
            .await
            .expect("binding should store");

        assert_eq!(
            default_binding_ids(&repository, "subject-1").await,
            vec!["binding-2"]
        );
        assert_eq!(
            default_binding_ids(&repository, "subject-2").await,
            vec!["binding-3"]
        );

        repository
            .update_routing_group_binding(
                "binding-1",
                UpdateRoutingGroupBindingRecord {
                    is_default: Some(true),
                    updated_at: 2,
                    ..UpdateRoutingGroupBindingRecord::default()
                },
            )
            .await
            .expect("binding should update");
        assert_eq!(
            default_binding_ids(&repository, "subject-1").await,
            vec!["binding-1"]
        );
        assert_eq!(
            default_binding_ids(&repository, "subject-2").await,
            vec!["binding-3"]
        );

        repository
            .update_routing_group_binding(
                "binding-3",
                UpdateRoutingGroupBindingRecord {
                    subject_id: Some("subject-1".to_string()),
                    updated_at: 3,
                    ..UpdateRoutingGroupBindingRecord::default()
                },
            )
            .await
            .expect("binding should move");
        assert_eq!(
            default_binding_ids(&repository, "subject-1").await,
            vec!["binding-3"]
        );
        assert!(default_binding_ids(&repository, "subject-2")
            .await
            .is_empty());
    }

    fn group_record(id: &str, is_system_default: bool) -> CreateRoutingGroupRecord {
        CreateRoutingGroupRecord {
            id: id.to_string(),
            name: id.to_string(),
            description: None,
            enabled: true,
            is_system_default,
            sort_order: 0,
            config_json: json!({}),
            version: 1,
            created_at: 1,
            updated_at: 1,
            published_at: None,
        }
    }

    fn binding_record(
        id: &str,
        group_id: &str,
        subject_id: &str,
        is_default: bool,
    ) -> CreateRoutingGroupBindingRecord {
        CreateRoutingGroupBindingRecord {
            id: id.to_string(),
            group_id: group_id.to_string(),
            subject_type: RoutingGroupBindingSubject::ApiKey,
            subject_id: subject_id.to_string(),
            is_default,
            allow_explicit_select: true,
            created_at: 1,
            updated_at: 1,
        }
    }

    async fn system_default_ids(repository: &InMemoryRoutingGroupRepository) -> Vec<String> {
        repository
            .list_routing_groups()
            .await
            .expect("groups should list")
            .into_iter()
            .filter(|group| group.is_system_default)
            .map(|group| group.id)
            .collect()
    }

    async fn default_binding_ids(
        repository: &InMemoryRoutingGroupRepository,
        subject_id: &str,
    ) -> Vec<String> {
        repository
            .list_routing_group_bindings(&RoutingGroupBindingQuery {
                group_id: None,
                subject_type: Some(RoutingGroupBindingSubject::ApiKey),
                subject_id: Some(subject_id.to_string()),
            })
            .await
            .expect("bindings should list")
            .into_iter()
            .filter(|binding| binding.is_default)
            .map(|binding| binding.id)
            .collect()
    }
}
