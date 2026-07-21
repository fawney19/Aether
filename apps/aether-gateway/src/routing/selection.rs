use aether_data_contracts::repository::routing_profiles::{
    RoutingGroupBindingQuery, RoutingGroupBindingSubject, RoutingGroupLookupKey,
    RoutingGroupReadRepository, StoredRoutingGroup,
};
use thiserror::Error;

pub(crate) const ROUTING_GROUP_HEADER: &str = "x-aether-scheduler-group";

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub(crate) enum GatewayRoutingSelectionError {
    #[error("routing group was explicitly requested but was not found: {0}")]
    NotFound(String),
    #[error("routing group was explicitly requested but is not enabled: {0}")]
    Disabled(String),
    #[error("routing group was explicitly requested but is not allowed for this principal: {0}")]
    Forbidden(String),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct GatewayRoutingSelectionInput<'a> {
    pub explicit_group: Option<&'a str>,
    pub user_id: Option<&'a str>,
    pub api_key_id: Option<&'a str>,
    pub user_group_ids: &'a [String],
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GatewayRoutingGroupSelection {
    pub group: Option<StoredRoutingGroup>,
    pub source: String,
}

pub(crate) async fn select_gateway_routing_group(
    repository: &(impl RoutingGroupReadRepository + ?Sized),
    input: GatewayRoutingSelectionInput<'_>,
) -> Result<GatewayRoutingGroupSelection, GatewayRoutingSelectionError> {
    if let Some(explicit) = input
        .explicit_group
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let group = repository
            .find_routing_group(RoutingGroupLookupKey::Id(explicit))
            .await
            .ok()
            .flatten()
            .or({
                let group: Option<StoredRoutingGroup> = repository
                    .find_routing_group(RoutingGroupLookupKey::Name(explicit))
                    .await
                    .unwrap_or_default();
                group
            });
        let Some(group) = group else {
            return Err(GatewayRoutingSelectionError::NotFound(explicit.to_string()));
        };
        if !group.enabled {
            return Err(GatewayRoutingSelectionError::Disabled(group.id));
        }
        if !explicit_group_allowed(repository, &group.id, &input).await {
            return Err(GatewayRoutingSelectionError::Forbidden(group.id));
        }
        return Ok(GatewayRoutingGroupSelection {
            group: Some(group),
            source: "explicit_header".to_string(),
        });
    }

    // When there are no bindings at all, no principal-specific lookup can
    // produce a group. The data-state repository answers this with a cached
    // existence query, so the common "routing configured but unused" case
    // does not materialize the binding table per API key/user.
    let has_bindings = repository.has_any_routing_group_binding().await;
    if matches!(has_bindings, Ok(false)) {
        let system_default = repository
            .find_routing_group(RoutingGroupLookupKey::SystemDefault)
            .await
            .ok()
            .flatten()
            .filter(|group| group.enabled);
        return Ok(GatewayRoutingGroupSelection {
            group: system_default,
            source: "system_default".to_string(),
        });
    }

    for (subject_type, subject_id, source) in default_binding_candidates(&input) {
        let bindings = repository
            .list_routing_group_bindings(&RoutingGroupBindingQuery {
                group_id: None,
                subject_type: Some(subject_type),
                subject_id: Some(subject_id.to_string()),
            })
            .await
            .unwrap_or_default();
        for binding in bindings.into_iter().filter(|binding| binding.is_default) {
            let group = repository
                .find_routing_group(RoutingGroupLookupKey::Id(&binding.group_id))
                .await
                .ok()
                .flatten();
            if let Some(group) = group.filter(|group| group.enabled) {
                return Ok(GatewayRoutingGroupSelection {
                    group: Some(group),
                    source: source.to_string(),
                });
            }
        }
    }

    let system_default = repository
        .find_routing_group(RoutingGroupLookupKey::SystemDefault)
        .await
        .ok()
        .flatten()
        .filter(|group| group.enabled);
    Ok(GatewayRoutingGroupSelection {
        group: system_default,
        source: "system_default".to_string(),
    })
}

async fn explicit_group_allowed(
    repository: &(impl RoutingGroupReadRepository + ?Sized),
    group_id: &str,
    input: &GatewayRoutingSelectionInput<'_>,
) -> bool {
    if let Ok(Some(group)) = repository
        .find_routing_group(RoutingGroupLookupKey::Id(group_id))
        .await
    {
        if group.is_system_default {
            return true;
        }
    }
    for (subject_type, subject_id, _) in default_binding_candidates(input) {
        let bindings = repository
            .list_routing_group_bindings(&RoutingGroupBindingQuery {
                group_id: Some(group_id.to_string()),
                subject_type: Some(subject_type),
                subject_id: Some(subject_id.to_string()),
            })
            .await
            .unwrap_or_default();
        if bindings.iter().any(|binding| binding.allow_explicit_select) {
            return true;
        }
    }
    false
}

fn default_binding_candidates<'a>(
    input: &'a GatewayRoutingSelectionInput<'a>,
) -> Vec<(RoutingGroupBindingSubject, &'a str, &'static str)> {
    let mut candidates = Vec::new();
    if let Some(api_key_id) = input.api_key_id {
        candidates.push((
            RoutingGroupBindingSubject::ApiKey,
            api_key_id,
            "api_key_default",
        ));
    }
    if let Some(user_id) = input.user_id {
        candidates.push((RoutingGroupBindingSubject::User, user_id, "user_default"));
    }
    for group_id in input.user_group_ids {
        candidates.push((
            RoutingGroupBindingSubject::UserGroup,
            group_id.as_str(),
            "user_group_default",
        ));
    }
    candidates
}

#[cfg(test)]
mod tests {
    use aether_data::repository::routing_profiles::InMemoryRoutingGroupRepository;
    use aether_data_contracts::repository::routing_profiles::{
        CreateRoutingGroupBindingRecord, CreateRoutingGroupRecord, RoutingGroupWriteRepository,
    };
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn selects_api_key_default_binding() {
        let repository = InMemoryRoutingGroupRepository::default();
        repository
            .create_routing_group(CreateRoutingGroupRecord {
                id: "group-1".to_string(),
                name: "default".to_string(),
                description: None,
                enabled: true,
                is_system_default: false,
                config_json: json!({}),
                version: 1,
                created_at: 1,
                updated_at: 1,
                published_at: None,
            })
            .await
            .unwrap();
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

        let selection = select_gateway_routing_group(
            &repository,
            GatewayRoutingSelectionInput {
                explicit_group: None,
                user_id: None,
                api_key_id: Some("api-key-1"),
                user_group_ids: &[],
            },
        )
        .await
        .unwrap();

        assert_eq!(selection.source, "api_key_default");
        assert_eq!(selection.group.unwrap().id, "group-1");
    }

    #[tokio::test]
    async fn selects_system_default_when_no_bindings_exist() {
        let repository = InMemoryRoutingGroupRepository::default();
        repository
            .create_routing_group(CreateRoutingGroupRecord {
                id: "system-default".to_string(),
                name: "system-default".to_string(),
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
            .unwrap();

        let selection = select_gateway_routing_group(
            &repository,
            GatewayRoutingSelectionInput {
                explicit_group: None,
                user_id: Some("user-1"),
                api_key_id: Some("api-key-1"),
                user_group_ids: &["user-group-1".to_string()],
            },
        )
        .await
        .unwrap();

        assert_eq!(selection.source, "system_default");
        assert_eq!(selection.group.unwrap().id, "system-default");
    }

    #[tokio::test]
    async fn selects_explicit_group_allowed_by_user_group_binding() {
        let repository = InMemoryRoutingGroupRepository::default();
        repository
            .create_routing_group(CreateRoutingGroupRecord {
                id: "private-group".to_string(),
                name: "private".to_string(),
                description: None,
                enabled: true,
                is_system_default: false,
                config_json: json!({}),
                version: 1,
                created_at: 1,
                updated_at: 1,
                published_at: None,
            })
            .await
            .unwrap();
        repository
            .create_routing_group_binding(CreateRoutingGroupBindingRecord {
                id: "binding-explicit".to_string(),
                group_id: "private-group".to_string(),
                subject_type: RoutingGroupBindingSubject::UserGroup,
                subject_id: "team-1".to_string(),
                is_default: false,
                allow_explicit_select: true,
                created_at: 1,
                updated_at: 1,
            })
            .await
            .unwrap();

        let selection = select_gateway_routing_group(
            &repository,
            GatewayRoutingSelectionInput {
                explicit_group: Some("private-group"),
                user_id: Some("user-1"),
                api_key_id: Some("api-key-1"),
                user_group_ids: &["team-1".to_string()],
            },
        )
        .await
        .unwrap();

        assert_eq!(selection.source, "explicit_header");
        assert_eq!(selection.group.unwrap().id, "private-group");
    }

    #[tokio::test]
    async fn rejects_explicit_group_that_does_not_exist() {
        let repository = InMemoryRoutingGroupRepository::default();

        let error = select_gateway_routing_group(
            &repository,
            GatewayRoutingSelectionInput {
                explicit_group: Some("missing"),
                user_id: Some("user-1"),
                api_key_id: Some("api-key-1"),
                user_group_ids: &[],
            },
        )
        .await
        .unwrap_err();

        assert_eq!(
            error,
            GatewayRoutingSelectionError::NotFound("missing".to_string())
        );
    }

    #[tokio::test]
    async fn rejects_explicit_disabled_group() {
        let repository = InMemoryRoutingGroupRepository::default();
        repository
            .create_routing_group(CreateRoutingGroupRecord {
                id: "disabled-group".to_string(),
                name: "disabled".to_string(),
                description: None,
                enabled: false,
                is_system_default: false,
                config_json: json!({}),
                version: 1,
                created_at: 1,
                updated_at: 1,
                published_at: None,
            })
            .await
            .unwrap();

        let error = select_gateway_routing_group(
            &repository,
            GatewayRoutingSelectionInput {
                explicit_group: Some("disabled-group"),
                user_id: Some("user-1"),
                api_key_id: Some("api-key-1"),
                user_group_ids: &[],
            },
        )
        .await
        .unwrap_err();

        assert_eq!(
            error,
            GatewayRoutingSelectionError::Disabled("disabled-group".to_string())
        );
    }

    #[tokio::test]
    async fn rejects_explicit_group_without_binding_permission() {
        let repository = InMemoryRoutingGroupRepository::default();
        repository
            .create_routing_group(CreateRoutingGroupRecord {
                id: "private-group".to_string(),
                name: "private".to_string(),
                description: None,
                enabled: true,
                is_system_default: false,
                config_json: json!({}),
                version: 1,
                created_at: 1,
                updated_at: 1,
                published_at: None,
            })
            .await
            .unwrap();
        repository
            .create_routing_group_binding(CreateRoutingGroupBindingRecord {
                id: "binding-1".to_string(),
                group_id: "private-group".to_string(),
                subject_type: RoutingGroupBindingSubject::ApiKey,
                subject_id: "api-key-1".to_string(),
                is_default: true,
                allow_explicit_select: false,
                created_at: 1,
                updated_at: 1,
            })
            .await
            .unwrap();

        let error = select_gateway_routing_group(
            &repository,
            GatewayRoutingSelectionInput {
                explicit_group: Some("private-group"),
                user_id: Some("user-1"),
                api_key_id: Some("api-key-1"),
                user_group_ids: &[],
            },
        )
        .await
        .unwrap_err();

        assert_eq!(
            error,
            GatewayRoutingSelectionError::Forbidden("private-group".to_string())
        );
    }
}
