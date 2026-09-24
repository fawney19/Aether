use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;

use super::{
    is_valid_bcrypt_hash, last_oauth_unbind_denial, normalize_user_group_name,
    BindUserOAuthLinkOutcome, BindUserOAuthLinkSessionExpectation, DeleteUserOAuthLinkOutcome,
    LdapAuthUserProvisioningOutcome, ResolveOAuthLinkedUserOutcome, StoredUserAuthRecord,
    StoredUserExportRow, StoredUserGroup, StoredUserGroupMember, StoredUserGroupMembership,
    StoredUserOAuthLinkSummary, StoredUserPreferenceRecord, StoredUserSessionRecord,
    StoredUserSummary, UpsertUserGroupRecord, UserExportListQuery, UserExportSortBy,
    UserExportSummary, UserReadRepository, LAST_ACTIVE_ADMIN_DELETE_DENIED,
    LAST_ACTIVE_ADMIN_UPDATE_DENIED,
};
use crate::DataLayerError;

#[derive(Debug, Clone)]
struct StoredMemoryOAuthLink {
    id: String,
    user_id: String,
    provider_type: String,
    provider_user_id: String,
    provider_username: Option<String>,
    provider_email: Option<String>,
    extra_data: Option<serde_json::Value>,
    linked_at: chrono::DateTime<chrono::Utc>,
    last_login_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Default)]
pub struct InMemoryUserReadRepository {
    by_id: RwLock<BTreeMap<String, StoredUserSummary>>,
    auth_by_id: RwLock<BTreeMap<String, StoredUserAuthRecord>>,
    auth_by_identifier: RwLock<BTreeMap<String, String>>,
    oauth_links_by_id: RwLock<BTreeMap<String, StoredMemoryOAuthLink>>,
    ldap_dn_by_user_id: RwLock<BTreeMap<String, String>>,
    ldap_username_by_user_id: RwLock<BTreeMap<String, String>>,
    preferences_by_user_id: RwLock<BTreeMap<String, StoredUserPreferenceRecord>>,
    sessions_by_id: RwLock<BTreeMap<String, StoredUserSessionRecord>>,
    model_settings_by_user_id: RwLock<BTreeMap<String, serde_json::Value>>,
    feature_settings_by_user_id: RwLock<BTreeMap<String, serde_json::Value>>,
    groups_by_id: RwLock<BTreeMap<String, StoredUserGroup>>,
    group_members: RwLock<BTreeMap<(String, String), chrono::DateTime<chrono::Utc>>>,
    export_rows: RwLock<Vec<StoredUserExportRow>>,
    read_only: bool,
}

impl InMemoryUserReadRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredUserSummary>,
    {
        let mut by_id = BTreeMap::new();
        for item in items {
            by_id.insert(item.id.clone(), item);
        }
        Self {
            by_id: RwLock::new(by_id),
            auth_by_id: RwLock::new(BTreeMap::new()),
            auth_by_identifier: RwLock::new(BTreeMap::new()),
            oauth_links_by_id: RwLock::new(BTreeMap::new()),
            ldap_dn_by_user_id: RwLock::new(BTreeMap::new()),
            ldap_username_by_user_id: RwLock::new(BTreeMap::new()),
            preferences_by_user_id: RwLock::new(BTreeMap::new()),
            sessions_by_id: RwLock::new(BTreeMap::new()),
            model_settings_by_user_id: RwLock::new(BTreeMap::new()),
            feature_settings_by_user_id: RwLock::new(BTreeMap::new()),
            groups_by_id: RwLock::new(BTreeMap::new()),
            group_members: RwLock::new(BTreeMap::new()),
            export_rows: RwLock::new(Vec::new()),
            read_only: false,
        }
    }

    pub fn seed_auth_users<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredUserAuthRecord>,
    {
        let mut by_id = BTreeMap::new();
        let mut auth_by_id = BTreeMap::new();
        let mut auth_by_identifier = BTreeMap::new();
        for item in items {
            let summary = item
                .to_summary()
                .expect("in-memory auth user should convert to summary");
            by_id.insert(summary.id.clone(), summary);
            auth_by_identifier.insert(item.username.clone(), item.id.clone());
            if let Some(email) = item.email.as_ref() {
                auth_by_identifier.insert(email.clone(), item.id.clone());
            }
            auth_by_id.insert(item.id.clone(), item);
        }
        Self {
            by_id: RwLock::new(by_id),
            auth_by_id: RwLock::new(auth_by_id),
            auth_by_identifier: RwLock::new(auth_by_identifier),
            oauth_links_by_id: RwLock::new(BTreeMap::new()),
            ldap_dn_by_user_id: RwLock::new(BTreeMap::new()),
            ldap_username_by_user_id: RwLock::new(BTreeMap::new()),
            preferences_by_user_id: RwLock::new(BTreeMap::new()),
            sessions_by_id: RwLock::new(BTreeMap::new()),
            model_settings_by_user_id: RwLock::new(BTreeMap::new()),
            feature_settings_by_user_id: RwLock::new(BTreeMap::new()),
            groups_by_id: RwLock::new(BTreeMap::new()),
            group_members: RwLock::new(BTreeMap::new()),
            export_rows: RwLock::new(Vec::new()),
            read_only: false,
        }
    }

    pub fn seed_export_users<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredUserExportRow>,
    {
        Self {
            by_id: RwLock::new(BTreeMap::new()),
            auth_by_id: RwLock::new(BTreeMap::new()),
            auth_by_identifier: RwLock::new(BTreeMap::new()),
            oauth_links_by_id: RwLock::new(BTreeMap::new()),
            ldap_dn_by_user_id: RwLock::new(BTreeMap::new()),
            ldap_username_by_user_id: RwLock::new(BTreeMap::new()),
            preferences_by_user_id: RwLock::new(BTreeMap::new()),
            sessions_by_id: RwLock::new(BTreeMap::new()),
            model_settings_by_user_id: RwLock::new(BTreeMap::new()),
            feature_settings_by_user_id: RwLock::new(BTreeMap::new()),
            groups_by_id: RwLock::new(BTreeMap::new()),
            group_members: RwLock::new(BTreeMap::new()),
            export_rows: RwLock::new(items.into_iter().collect()),
            read_only: false,
        }
    }

    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    pub fn with_export_users<I>(self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredUserExportRow>,
    {
        let rows = items.into_iter().collect();
        *self.export_rows.write().expect("user repository lock") = rows;
        self
    }

    pub fn with_user_preferences<I>(self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredUserPreferenceRecord>,
    {
        let preferences = items
            .into_iter()
            .map(|item| (item.user_id.clone(), item))
            .collect();
        *self
            .preferences_by_user_id
            .write()
            .expect("user repository lock") = preferences;
        self
    }

    pub fn with_user_sessions<I>(self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredUserSessionRecord>,
    {
        let sessions = items
            .into_iter()
            .map(|item| (item.id.clone(), item))
            .collect();
        *self.sessions_by_id.write().expect("user repository lock") = sessions;
        self
    }

    fn insert_auth_user(
        &self,
        user: StoredUserAuthRecord,
    ) -> Result<StoredUserAuthRecord, DataLayerError> {
        let summary = user.to_summary()?;
        self.by_id
            .write()
            .expect("user repository lock")
            .insert(summary.id.clone(), summary);
        let mut identifiers = self
            .auth_by_identifier
            .write()
            .expect("user repository lock");
        identifiers.insert(user.username.clone(), user.id.clone());
        if let Some(email) = user.email.as_ref() {
            identifiers.insert(email.clone(), user.id.clone());
        }
        self.auth_by_id
            .write()
            .expect("user repository lock")
            .insert(user.id.clone(), user.clone());
        Ok(user)
    }
}

fn normalize_optional_json_value(value: Option<serde_json::Value>) -> Option<serde_json::Value> {
    match value {
        Some(serde_json::Value::Null) | None => None,
        Some(value) => Some(value),
    }
}

fn normalized_ids(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn find_memory_ldap_user_id(
    repository: &InMemoryUserReadRepository,
    ldap_dn: Option<&str>,
    ldap_username: Option<&str>,
    email: &str,
) -> Option<String> {
    if let Some(ldap_dn) = ldap_dn.filter(|value| !value.trim().is_empty()) {
        let ldap_dn_by_user_id = repository
            .ldap_dn_by_user_id
            .read()
            .expect("user repository lock");
        if let Some((user_id, _)) = ldap_dn_by_user_id
            .iter()
            .find(|(_, value)| value.as_str() == ldap_dn)
        {
            return Some(user_id.clone());
        }
    }
    if let Some(ldap_username) = ldap_username.filter(|value| !value.trim().is_empty()) {
        let ldap_username_by_user_id = repository
            .ldap_username_by_user_id
            .read()
            .expect("user repository lock");
        if let Some((user_id, _)) = ldap_username_by_user_id
            .iter()
            .find(|(_, value)| value.as_str() == ldap_username)
        {
            return Some(user_id.clone());
        }
    }
    repository
        .auth_by_id
        .read()
        .expect("user repository lock")
        .values()
        .find(|user| user.email.as_deref() == Some(email))
        .map(|user| user.id.clone())
}

fn upsert_memory_ldap_identifiers(
    repository: &InMemoryUserReadRepository,
    user_id: &str,
    ldap_dn: Option<String>,
    ldap_username: Option<String>,
) {
    if let Some(ldap_dn) = ldap_dn.filter(|value| !value.trim().is_empty()) {
        repository
            .ldap_dn_by_user_id
            .write()
            .expect("user repository lock")
            .insert(user_id.to_string(), ldap_dn);
    }
    if let Some(ldap_username) = ldap_username.filter(|value| !value.trim().is_empty()) {
        repository
            .ldap_username_by_user_id
            .write()
            .expect("user repository lock")
            .insert(user_id.to_string(), ldap_username);
    }
}

fn memory_group_from_record(
    record: UpsertUserGroupRecord,
) -> Result<StoredUserGroup, DataLayerError> {
    let now = chrono::Utc::now();
    let name = normalize_user_group_name(&record.name);
    StoredUserGroup::new(
        uuid::Uuid::new_v4().to_string(),
        name.clone(),
        name.to_ascii_lowercase(),
        record.description,
        record.priority,
        record.allowed_providers.map(serde_json::Value::from),
        record.allowed_providers_mode,
        Some(
            serde_json::to_value(record.provider_key_policies)
                .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?,
        ),
        record.allowed_api_formats.map(serde_json::Value::from),
        record.allowed_api_formats_mode,
        record.allowed_models.map(serde_json::Value::from),
        record.allowed_models_mode,
        record.rate_limit,
        record.rate_limit_mode,
        Some(now),
        Some(now),
    )
}

fn memory_update_group_from_record(
    mut group: StoredUserGroup,
    record: UpsertUserGroupRecord,
) -> Result<StoredUserGroup, DataLayerError> {
    let name = normalize_user_group_name(&record.name);
    group.name = name.clone();
    group.normalized_name = name.to_ascii_lowercase();
    group.description = record.description;
    group.priority = record.priority;
    group.allowed_providers = record.allowed_providers;
    group.allowed_providers_mode = record.allowed_providers_mode;
    group.provider_key_policies = record.provider_key_policies;
    group.allowed_api_formats = record.allowed_api_formats;
    group.allowed_api_formats_mode = record.allowed_api_formats_mode;
    group.allowed_models = record.allowed_models;
    group.allowed_models_mode = record.allowed_models_mode;
    group.rate_limit = record.rate_limit;
    group.rate_limit_mode = record.rate_limit_mode;
    group.updated_at = Some(chrono::Utc::now());
    StoredUserGroup::new(
        group.id,
        group.name,
        group.normalized_name,
        group.description,
        group.priority,
        group.allowed_providers.map(serde_json::Value::from),
        group.allowed_providers_mode,
        Some(
            serde_json::to_value(group.provider_key_policies)
                .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?,
        ),
        group.allowed_api_formats.map(serde_json::Value::from),
        group.allowed_api_formats_mode,
        group.allowed_models.map(serde_json::Value::from),
        group.allowed_models_mode,
        group.rate_limit,
        group.rate_limit_mode,
        group.created_at,
        group.updated_at,
    )
}

fn memory_group_members(
    repository: &InMemoryUserReadRepository,
    group_id: &str,
) -> Vec<StoredUserGroupMember> {
    let members = repository
        .group_members
        .read()
        .expect("user repository lock")
        .clone();
    let users = repository.auth_by_id.read().expect("user repository lock");
    members
        .into_iter()
        .filter(|((candidate_group_id, _), _)| candidate_group_id == group_id)
        .filter_map(|((candidate_group_id, user_id), created_at)| {
            users.get(&user_id).map(|user| StoredUserGroupMember {
                group_id: candidate_group_id,
                user_id: user.id.clone(),
                username: user.username.clone(),
                email: user.email.clone(),
                role: user.role.clone(),
                is_active: user.is_active,
                is_deleted: user.is_deleted,
                created_at: Some(created_at),
            })
        })
        .collect()
}

fn filter_memory_export_rows(
    repository: &InMemoryUserReadRepository,
    query: &UserExportListQuery,
) -> Vec<StoredUserExportRow> {
    let mut rows = repository
        .export_rows
        .read()
        .expect("user repository lock")
        .clone();
    if let Some(role) = query.role.as_deref() {
        rows.retain(|row| row.role.eq_ignore_ascii_case(role));
    }
    if let Some(is_active) = query.is_active {
        rows.retain(|row| row.is_active == is_active);
    }
    if let Some(group_id) = query
        .group_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let member_ids = repository
            .group_members
            .read()
            .expect("user repository lock")
            .keys()
            .filter(|(candidate_group_id, _)| candidate_group_id == group_id)
            .map(|(_, user_id)| user_id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        rows.retain(|row| member_ids.contains(&row.id));
    }
    if let Some(search) = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let search = search.to_ascii_lowercase();
        rows.retain(|row| {
            row.id.to_ascii_lowercase().contains(&search)
                || row.username.to_ascii_lowercase().contains(&search)
                || row
                    .email
                    .as_deref()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .contains(&search)
        });
    }
    match query.sort_by {
        UserExportSortBy::CreatedAt => {
            let created_at_by_id = repository
                .auth_by_id
                .read()
                .expect("user repository lock")
                .iter()
                .filter_map(|(user_id, user)| {
                    user.created_at
                        .map(|created_at| (user_id.clone(), created_at.timestamp_millis()))
                })
                .collect::<BTreeMap<_, _>>();
            rows.sort_by(|left, right| {
                let primary = created_at_by_id
                    .get(&left.id)
                    .cmp(&created_at_by_id.get(&right.id));
                let ordered = if query.sort_order.is_desc() {
                    primary.reverse()
                } else {
                    primary
                };
                ordered.then_with(|| left.id.cmp(&right.id))
            });
        }
        UserExportSortBy::Id => {
            rows.sort_by(|left, right| left.id.cmp(&right.id));
        }
    }
    rows
}

fn memory_export_row_from_auth_user(
    repository: &InMemoryUserReadRepository,
    user: &StoredUserAuthRecord,
) -> Result<StoredUserExportRow, DataLayerError> {
    let model_capability_settings = repository
        .model_settings_by_user_id
        .read()
        .expect("user repository lock")
        .get(&user.id)
        .cloned();
    let feature_settings = repository
        .feature_settings_by_user_id
        .read()
        .expect("user repository lock")
        .get(&user.id)
        .cloned();
    StoredUserExportRow::new(
        user.id.clone(),
        user.email.clone(),
        user.email_verified,
        user.username.clone(),
        user.password_hash.clone(),
        user.role.clone(),
        user.auth_source.clone(),
        user.allowed_providers.clone().map(serde_json::Value::from),
        user.allowed_api_formats
            .clone()
            .map(serde_json::Value::from),
        user.allowed_models.clone().map(serde_json::Value::from),
        None,
        model_capability_settings,
        user.is_active,
    )?
    .with_feature_settings(feature_settings)
    .with_policy_modes(
        user.allowed_providers_mode.clone(),
        user.allowed_api_formats_mode.clone(),
        user.allowed_models_mode.clone(),
        "system".to_string(),
    )
}

#[async_trait]
impl UserReadRepository for InMemoryUserReadRepository {
    async fn list_users_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserSummary>, DataLayerError> {
        let index = self.by_id.read().expect("user repository lock");
        Ok(user_ids
            .iter()
            .filter_map(|user_id| index.get(user_id).cloned())
            .collect())
    }

    async fn list_users_by_username_search(
        &self,
        username_search: &str,
    ) -> Result<Vec<StoredUserSummary>, DataLayerError> {
        let username_search = username_search.trim().to_ascii_lowercase();
        if username_search.is_empty() {
            return Ok(Vec::new());
        }

        Ok(self
            .by_id
            .read()
            .expect("user repository lock")
            .values()
            .filter(|user| {
                user.username
                    .to_ascii_lowercase()
                    .contains(&username_search)
            })
            .cloned()
            .collect())
    }

    async fn list_non_admin_export_users(
        &self,
    ) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        let rows = self.export_rows.read().expect("user repository lock");
        if !rows.is_empty() {
            return Ok(rows
                .iter()
                .filter(|row| !row.role.eq_ignore_ascii_case("admin"))
                .cloned()
                .collect());
        }
        Ok(self
            .auth_by_id
            .read()
            .expect("user repository lock")
            .iter()
            .filter(|(_, user)| !user.role.eq_ignore_ascii_case("admin"))
            .map(|(_, user)| memory_export_row_from_auth_user(self, user))
            .collect::<Result<Vec<_>, _>>()?)
    }

    async fn list_export_users(&self) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        let rows = self.export_rows.read().expect("user repository lock");
        if !rows.is_empty() {
            return Ok(rows.clone());
        }
        Ok(self
            .auth_by_id
            .read()
            .expect("user repository lock")
            .values()
            .map(|user| memory_export_row_from_auth_user(self, user))
            .collect::<Result<Vec<_>, _>>()?)
    }

    async fn list_export_users_page(
        &self,
        query: &UserExportListQuery,
    ) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        Ok(filter_memory_export_rows(self, query)
            .into_iter()
            .skip(query.skip)
            .take(query.limit)
            .collect())
    }

    async fn count_export_users(&self, query: &UserExportListQuery) -> Result<u64, DataLayerError> {
        Ok(filter_memory_export_rows(self, query).len() as u64)
    }

    async fn summarize_export_users(&self) -> Result<UserExportSummary, DataLayerError> {
        let rows = self.export_rows.read().expect("user repository lock");
        Ok(UserExportSummary {
            total: rows.len() as u64,
            active: rows.iter().filter(|row| row.is_active).count() as u64,
        })
    }

    async fn find_export_user_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserExportRow>, DataLayerError> {
        if let Some(row) = self
            .export_rows
            .read()
            .expect("user repository lock")
            .iter()
            .find(|row| row.id == user_id)
            .cloned()
        {
            return Ok(Some(row));
        }

        self.auth_by_id
            .read()
            .expect("user repository lock")
            .get(user_id)
            .map(|user| memory_export_row_from_auth_user(self, user))
            .transpose()
    }

    async fn list_user_groups(&self) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        let mut groups = self
            .groups_by_id
            .read()
            .expect("user repository lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        groups.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(groups)
    }

    async fn find_user_group_by_id(
        &self,
        group_id: &str,
    ) -> Result<Option<StoredUserGroup>, DataLayerError> {
        Ok(self
            .groups_by_id
            .read()
            .expect("user repository lock")
            .get(group_id)
            .cloned())
    }

    async fn list_user_groups_by_ids(
        &self,
        group_ids: &[String],
    ) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        let groups = self.groups_by_id.read().expect("user repository lock");
        Ok(group_ids
            .iter()
            .filter_map(|group_id| groups.get(group_id).cloned())
            .collect())
    }

    async fn create_user_group(
        &self,
        record: UpsertUserGroupRecord,
    ) -> Result<Option<StoredUserGroup>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }
        let group = memory_group_from_record(record)?;
        let mut groups = self.groups_by_id.write().expect("user repository lock");
        if groups
            .values()
            .any(|existing| existing.normalized_name == group.normalized_name)
        {
            return Err(DataLayerError::InvalidInput(format!(
                "duplicate user group name: {}",
                group.name
            )));
        }
        groups.insert(group.id.clone(), group.clone());
        Ok(Some(group))
    }

    async fn update_user_group(
        &self,
        group_id: &str,
        record: UpsertUserGroupRecord,
    ) -> Result<Option<StoredUserGroup>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }
        let mut groups = self.groups_by_id.write().expect("user repository lock");
        let Some(existing) = groups.get(group_id).cloned() else {
            return Ok(None);
        };
        let group = memory_update_group_from_record(existing, record)?;
        if groups.values().any(|existing| {
            existing.id != group.id && existing.normalized_name == group.normalized_name
        }) {
            return Err(DataLayerError::InvalidInput(format!(
                "duplicate user group name: {}",
                group.name
            )));
        }
        groups.insert(group.id.clone(), group.clone());
        Ok(Some(group))
    }

    async fn restore_user_group_if_matches(
        &self,
        expected: &StoredUserGroup,
        restored: &StoredUserGroup,
    ) -> Result<bool, DataLayerError> {
        if self.read_only || expected.id != restored.id || expected.id.trim().is_empty() {
            return Ok(false);
        }
        let mut groups = self.groups_by_id.write().expect("user repository lock");
        let Some(current) = groups.get(&expected.id) else {
            return Ok(false);
        };
        if current != expected {
            return Ok(false);
        }
        groups.insert(restored.id.clone(), restored.clone());
        Ok(true)
    }

    async fn delete_user_group(&self, group_id: &str) -> Result<bool, DataLayerError> {
        if self.read_only {
            return Ok(false);
        }
        let removed = self
            .groups_by_id
            .write()
            .expect("user repository lock")
            .remove(group_id)
            .is_some();
        if removed {
            self.group_members
                .write()
                .expect("user repository lock")
                .retain(|key, _| key.0 != group_id);
        }
        Ok(removed)
    }

    async fn list_user_group_members(
        &self,
        group_id: &str,
    ) -> Result<Vec<StoredUserGroupMember>, DataLayerError> {
        Ok(memory_group_members(self, group_id))
    }

    async fn replace_user_group_members(
        &self,
        group_id: &str,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserGroupMember>, DataLayerError> {
        if self.read_only {
            return Ok(Vec::new());
        }
        if !self
            .groups_by_id
            .read()
            .expect("user repository lock")
            .contains_key(group_id)
        {
            return Ok(Vec::new());
        }
        let valid_user_ids = {
            let users = self.auth_by_id.read().expect("user repository lock");
            user_ids
                .iter()
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .filter(|user_id| users.contains_key(*user_id))
                .map(ToOwned::to_owned)
                .collect::<std::collections::BTreeSet<_>>()
        };
        let now = chrono::Utc::now();
        let mut members = self.group_members.write().expect("user repository lock");
        members.retain(|key, _| key.0 != group_id);
        for user_id in valid_user_ids {
            members.insert((group_id.to_string(), user_id), now);
        }
        drop(members);
        Ok(memory_group_members(self, group_id))
    }

    async fn list_user_groups_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        let group_ids = self
            .group_members
            .read()
            .expect("user repository lock")
            .keys()
            .filter_map(|(group_id, candidate_user_id)| {
                (candidate_user_id == user_id).then(|| group_id.clone())
            })
            .collect::<Vec<_>>();
        self.list_user_groups_by_ids(&group_ids).await
    }

    async fn list_user_group_memberships_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserGroupMembership>, DataLayerError> {
        let requested = user_ids
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect::<std::collections::BTreeSet<_>>();
        if requested.is_empty() {
            return Ok(Vec::new());
        }
        let groups = self.groups_by_id.read().expect("user repository lock");
        let members = self.group_members.read().expect("user repository lock");
        let mut memberships = members
            .iter()
            .filter(|((_, user_id), _)| requested.contains(user_id))
            .filter_map(|((group_id, user_id), created_at)| {
                groups.get(group_id).map(|group| StoredUserGroupMembership {
                    user_id: user_id.clone(),
                    group_id: group.id.clone(),
                    group_name: group.name.clone(),
                    group_priority: group.priority,
                    created_at: Some(*created_at),
                })
            })
            .collect::<Vec<_>>();
        memberships.sort_by(|left, right| {
            left.user_id
                .cmp(&right.user_id)
                .then_with(|| left.group_name.cmp(&right.group_name))
                .then_with(|| left.group_id.cmp(&right.group_id))
        });
        Ok(memberships)
    }

    async fn replace_user_groups_for_user(
        &self,
        user_id: &str,
        group_ids: &[String],
    ) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        if self.read_only {
            return Ok(Vec::new());
        }
        let existing_group_ids = {
            let groups = self.groups_by_id.read().expect("user repository lock");
            group_ids
                .iter()
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .filter(|group_id| groups.contains_key(*group_id))
                .map(ToOwned::to_owned)
                .collect::<std::collections::BTreeSet<_>>()
        };
        {
            let now = chrono::Utc::now();
            let mut members = self.group_members.write().expect("user repository lock");
            members.retain(|key, _| key.1 != user_id);
            for group_id in &existing_group_ids {
                members.insert((group_id.clone(), user_id.to_string()), now);
            }
        }
        self.list_user_groups_by_ids(&existing_group_ids.into_iter().collect::<Vec<_>>())
            .await
    }

    async fn restore_user_groups_if_matches(
        &self,
        user_id: &str,
        expected_group_ids: &[String],
        restored_group_ids: &[String],
    ) -> Result<bool, DataLayerError> {
        if self.read_only {
            return Ok(false);
        }
        let expected = normalized_ids(expected_group_ids);
        let restored = normalized_ids(restored_group_ids);
        let groups = self.groups_by_id.read().expect("user repository lock");
        if restored
            .iter()
            .any(|group_id| !groups.contains_key(group_id))
        {
            return Ok(false);
        }
        let mut members = self.group_members.write().expect("user repository lock");
        let mut current = members
            .keys()
            .filter(|(_, candidate_user_id)| candidate_user_id == user_id)
            .map(|(group_id, _)| group_id.clone())
            .collect::<Vec<_>>();
        current.sort();
        current.dedup();
        if current != expected {
            return Ok(false);
        }
        members.retain(|(_, candidate_user_id), _| candidate_user_id != user_id);
        let now = chrono::Utc::now();
        for group_id in restored {
            members.insert((group_id, user_id.to_string()), now);
        }
        Ok(true)
    }

    async fn add_user_to_group(
        &self,
        group_id: &str,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        if self.read_only {
            return Ok(false);
        }
        if !self
            .groups_by_id
            .read()
            .expect("user repository lock")
            .contains_key(group_id)
        {
            return Ok(false);
        }
        if !self
            .auth_by_id
            .read()
            .expect("user repository lock")
            .contains_key(user_id)
        {
            return Ok(false);
        }
        self.group_members
            .write()
            .expect("user repository lock")
            .insert(
                (group_id.to_string(), user_id.to_string()),
                chrono::Utc::now(),
            );
        Ok(true)
    }

    async fn find_user_auth_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        Ok(self
            .auth_by_id
            .read()
            .expect("user repository lock")
            .get(user_id)
            .cloned())
    }

    async fn list_user_auth_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserAuthRecord>, DataLayerError> {
        let auth_by_id = self.auth_by_id.read().expect("user repository lock");
        Ok(user_ids
            .iter()
            .filter_map(|user_id| auth_by_id.get(user_id).cloned())
            .collect())
    }

    async fn find_user_auth_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let auth_by_identifier = self
            .auth_by_identifier
            .read()
            .expect("user repository lock");
        let Some(user_id) = auth_by_identifier.get(identifier) else {
            return Ok(None);
        };
        Ok(self
            .auth_by_id
            .read()
            .expect("user repository lock")
            .get(user_id)
            .cloned())
    }

    async fn find_user_auth_by_email(
        &self,
        email: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        Ok(self
            .auth_by_id
            .read()
            .expect("user repository lock")
            .values()
            .find(|user| user.email.as_deref() == Some(email))
            .cloned())
    }

    async fn find_active_user_auth_by_email_ci(
        &self,
        email: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let email = email.trim().to_ascii_lowercase();
        if email.is_empty() {
            return Ok(None);
        }
        Ok(self
            .auth_by_id
            .read()
            .expect("user repository lock")
            .values()
            .find(|user| {
                !user.is_deleted
                    && user
                        .email
                        .as_deref()
                        .is_some_and(|value| value.eq_ignore_ascii_case(&email))
            })
            .cloned())
    }

    async fn find_user_auth_by_username(
        &self,
        username: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        Ok(self
            .auth_by_id
            .read()
            .expect("user repository lock")
            .values()
            .find(|user| user.username == username)
            .cloned())
    }

    async fn list_user_oauth_links(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserOAuthLinkSummary>, DataLayerError> {
        let mut links = self
            .oauth_links_by_id
            .read()
            .expect("user repository lock")
            .values()
            .filter(|link| link.user_id == user_id)
            .map(|link| {
                StoredUserOAuthLinkSummary::new(
                    link.provider_type.clone(),
                    link.provider_type.clone(),
                    link.provider_username.clone(),
                    link.provider_email.clone(),
                    Some(link.linked_at),
                    link.last_login_at,
                    true,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        links.sort_by_key(|link| (link.linked_at, link.provider_type.clone()));
        Ok(links)
    }

    async fn find_oauth_linked_user(
        &self,
        provider_type: &str,
        provider_user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let provider_type = provider_type.trim();
        let provider_user_id = provider_user_id.trim();
        let user_id = self
            .oauth_links_by_id
            .read()
            .expect("user repository lock")
            .values()
            .find(|link| {
                link.provider_type == provider_type && link.provider_user_id == provider_user_id
            })
            .map(|link| link.user_id.clone());
        let Some(user_id) = user_id else {
            return Ok(None);
        };
        Ok(self
            .auth_by_id
            .read()
            .expect("user repository lock")
            .get(&user_id)
            .cloned())
    }

    async fn resolve_enabled_oauth_linked_user(
        &self,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<serde_json::Value>,
        verified_email: Option<&str>,
        touched_at: chrono::DateTime<chrono::Utc>,
        provider_enabled_snapshot: bool,
    ) -> Result<ResolveOAuthLinkedUserOutcome, DataLayerError> {
        if !provider_enabled_snapshot {
            return Ok(ResolveOAuthLinkedUserOutcome::ProviderUnavailable);
        }
        let Some(mut user) = self
            .find_oauth_linked_user(provider_type, provider_user_id)
            .await?
        else {
            return Ok(ResolveOAuthLinkedUserOutcome::NotLinked);
        };
        self.touch_oauth_link(
            provider_type,
            provider_user_id,
            provider_username,
            provider_email,
            extra_data,
            touched_at,
        )
        .await?;
        if let Some(verified_email) = verified_email {
            if self
                .upgrade_oauth_email_verification_if_matches(&user.id, verified_email, touched_at)
                .await?
            {
                user.email_verified = true;
            }
        }
        Ok(ResolveOAuthLinkedUserOutcome::Linked(user))
    }

    async fn touch_oauth_link(
        &self,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<serde_json::Value>,
        touched_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        if self.read_only {
            return Ok(false);
        }

        let provider_type = provider_type.trim();
        let provider_user_id = provider_user_id.trim();
        let mut links = self
            .oauth_links_by_id
            .write()
            .expect("user repository lock");
        let Some(link) = links.values_mut().find(|link| {
            link.provider_type == provider_type && link.provider_user_id == provider_user_id
        }) else {
            return Ok(false);
        };
        if let Some(provider_username) = provider_username {
            link.provider_username = Some(provider_username.to_string());
        }
        if let Some(provider_email) = provider_email {
            link.provider_email = Some(provider_email.to_string());
        }
        if let Some(extra_data) = extra_data {
            link.extra_data = Some(extra_data);
        }
        link.last_login_at = Some(touched_at);
        Ok(true)
    }

    async fn create_oauth_auth_user(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }

        let user = StoredUserAuthRecord::new(
            uuid::Uuid::new_v4().to_string(),
            email,
            email_verified,
            username,
            None,
            "user".to_string(),
            "oauth".to_string(),
            None,
            None,
            None,
            true,
            false,
            Some(created_at),
            Some(created_at),
        )?
        .with_policy_modes(
            "inherit".to_string(),
            "inherit".to_string(),
            "inherit".to_string(),
        )?;
        self.insert_auth_user(user).map(Some)
    }

    async fn find_oauth_link_owner(
        &self,
        provider_type: &str,
        provider_user_id: &str,
    ) -> Result<Option<String>, DataLayerError> {
        let provider_type = provider_type.trim();
        let provider_user_id = provider_user_id.trim();
        Ok(self
            .oauth_links_by_id
            .read()
            .expect("user repository lock")
            .values()
            .find(|link| {
                link.provider_type == provider_type && link.provider_user_id == provider_user_id
            })
            .map(|link| link.user_id.clone()))
    }

    async fn has_user_oauth_provider_link(
        &self,
        user_id: &str,
        provider_type: &str,
    ) -> Result<bool, DataLayerError> {
        let provider_type = provider_type.trim();
        Ok(self
            .oauth_links_by_id
            .read()
            .expect("user repository lock")
            .values()
            .any(|link| link.user_id == user_id && link.provider_type == provider_type))
    }

    async fn count_user_oauth_links(&self, user_id: &str) -> Result<u64, DataLayerError> {
        Ok(self
            .oauth_links_by_id
            .read()
            .expect("user repository lock")
            .values()
            .filter(|link| link.user_id == user_id)
            .count() as u64)
    }

    async fn has_oauth_links_for_provider(
        &self,
        provider_type: &str,
    ) -> Result<bool, DataLayerError> {
        let provider_type = provider_type.trim();
        Ok(self
            .oauth_links_by_id
            .read()
            .expect("user repository lock")
            .values()
            .any(|link| link.provider_type == provider_type))
    }

    async fn count_locked_users_if_oauth_provider_disabled(
        &self,
        provider_type: &str,
        enabled_provider_types_snapshot: &[String],
        ldap_exclusive: bool,
    ) -> Result<usize, DataLayerError> {
        let provider_type = provider_type.trim();
        let enabled = enabled_provider_types_snapshot
            .iter()
            .map(|value| value.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let links = self.oauth_links_by_id.read().expect("user repository lock");
        let users = self.auth_by_id.read().expect("user repository lock");
        Ok(users
            .values()
            .filter(|user| user.is_active && !user.is_deleted)
            .filter(|user| {
                links
                    .values()
                    .any(|link| link.user_id == user.id && link.provider_type == provider_type)
            })
            .filter(|user| {
                !links.values().any(|link| {
                    link.user_id == user.id
                        && link.provider_type != provider_type
                        && enabled.contains(link.provider_type.as_str())
                })
            })
            .filter(|user| {
                user.auth_source.eq_ignore_ascii_case("oauth")
                    || (ldap_exclusive
                        && user.auth_source.eq_ignore_ascii_case("local")
                        && !user.role.eq_ignore_ascii_case("admin"))
            })
            .count())
    }

    async fn bind_user_oauth_link_if_provider_enabled(
        &self,
        user_id: &str,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<serde_json::Value>,
        linked_at: chrono::DateTime<chrono::Utc>,
        provider_enabled_snapshot: bool,
        session_expectation: Option<&BindUserOAuthLinkSessionExpectation>,
    ) -> Result<BindUserOAuthLinkOutcome, DataLayerError> {
        if self.read_only {
            return Ok(BindUserOAuthLinkOutcome::UserNotFound);
        }

        let provider_type = provider_type.trim().to_string();
        let provider_user_id = provider_user_id.trim().to_string();
        if provider_type.is_empty() || provider_user_id.is_empty() {
            return Err(DataLayerError::InvalidInput(
                "OAuth provider type and subject must not be empty".to_string(),
            ));
        }
        if !provider_enabled_snapshot {
            return Ok(BindUserOAuthLinkOutcome::ProviderDisabled);
        }
        let users = self.auth_by_id.read().expect("user repository lock");
        let Some(user) = users.get(user_id) else {
            return Ok(BindUserOAuthLinkOutcome::UserNotFound);
        };
        let sessions =
            session_expectation.map(|_| self.sessions_by_id.read().expect("user repository lock"));
        if let Some(expectation) = session_expectation {
            let checked_at = std::cmp::max(expectation.checked_at, chrono::Utc::now());
            let session_is_current = sessions
                .as_ref()
                .and_then(|sessions| sessions.get(&expectation.session_id))
                .is_some_and(|session| {
                    user.is_active
                        && !user.is_deleted
                        && user.security_version == expectation.security_version
                        && session.user_id == user_id
                        && session.client_device_id == expectation.client_device_id
                        && session.security_version == expectation.security_version
                        && !session.is_revoked()
                        && !session.is_expired(checked_at)
                });
            if !session_is_current {
                return Ok(BindUserOAuthLinkOutcome::SessionUnavailable);
            }
        }
        let mut links = self
            .oauth_links_by_id
            .write()
            .expect("user repository lock");
        if let Some(link) = links.values().find(|link| {
            link.provider_type == provider_type && link.provider_user_id == provider_user_id
        }) {
            return Ok(if link.user_id == user_id {
                BindUserOAuthLinkOutcome::IdentityAlreadyBoundToUser
            } else {
                BindUserOAuthLinkOutcome::IdentityBoundToAnotherUser
            });
        }
        if links
            .values()
            .any(|link| link.user_id == user_id && link.provider_type == provider_type)
        {
            return Ok(BindUserOAuthLinkOutcome::UserAlreadyLinkedProvider);
        }
        let link = StoredMemoryOAuthLink {
            id: uuid::Uuid::new_v4().to_string(),
            user_id: user_id.to_string(),
            provider_type,
            provider_user_id,
            provider_username: provider_username.map(ToOwned::to_owned),
            provider_email: provider_email.map(ToOwned::to_owned),
            extra_data,
            linked_at,
            last_login_at: Some(linked_at),
        };
        links.insert(link.id.clone(), link);
        Ok(BindUserOAuthLinkOutcome::Bound)
    }

    async fn upgrade_oauth_email_verification_if_matches(
        &self,
        user_id: &str,
        verified_email: &str,
        _verified_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        if self.read_only {
            return Ok(false);
        }
        let verified_email = verified_email.trim();
        let mut users = self.auth_by_id.write().expect("user repository lock");
        let Some(user) = users.get_mut(user_id) else {
            return Ok(false);
        };
        if user.email_verified
            || !user
                .email
                .as_deref()
                .is_some_and(|email| email.trim().eq_ignore_ascii_case(verified_email))
        {
            return Ok(false);
        }
        user.email_verified = true;
        Ok(true)
    }

    async fn delete_user_oauth_link(
        &self,
        user_id: &str,
        provider_type: &str,
        local_password_login_allowed: bool,
        enabled_provider_types_snapshot: &[String],
    ) -> Result<DeleteUserOAuthLinkOutcome, DataLayerError> {
        if self.read_only {
            return Ok(DeleteUserOAuthLinkOutcome::NotFound);
        }

        let provider_type = provider_type.trim();
        let users = self.auth_by_id.read().expect("user repository lock");
        let Some(user) = users.get(user_id) else {
            return Ok(DeleteUserOAuthLinkOutcome::NotFound);
        };
        let mut links = self
            .oauth_links_by_id
            .write()
            .expect("user repository lock");
        let target_exists = links
            .values()
            .any(|link| link.user_id == user_id && link.provider_type == provider_type);
        if !target_exists {
            return Ok(DeleteUserOAuthLinkOutcome::NotFound);
        }
        // The in-memory provider repository has a separate lock, so callers pass a
        // point-in-time enabled-provider snapshot. SQL implementations instead read
        // and lock provider rows in the same database transaction.
        let has_remaining_enabled_oauth_link = links.values().any(|link| {
            link.user_id == user_id
                && link.provider_type != provider_type
                && enabled_provider_types_snapshot
                    .iter()
                    .any(|enabled| enabled == &link.provider_type)
        });
        if !has_remaining_enabled_oauth_link {
            if let Some(outcome) = last_oauth_unbind_denial(
                &user.auth_source,
                user.password_hash.as_deref(),
                local_password_login_allowed,
            ) {
                return Ok(outcome);
            }
        }
        links.retain(|_, link| !(link.user_id == user_id && link.provider_type == provider_type));
        Ok(DeleteUserOAuthLinkOutcome::Deleted)
    }

    async fn get_or_create_ldap_auth_user(
        &self,
        email: String,
        username: String,
        ldap_dn: Option<String>,
        ldap_username: Option<String>,
        logged_in_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<LdapAuthUserProvisioningOutcome>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }

        let existing_id =
            find_memory_ldap_user_id(self, ldap_dn.as_deref(), ldap_username.as_deref(), &email);
        if let Some(existing_id) = existing_id {
            let email_conflict = self
                .auth_by_id
                .read()
                .expect("user repository lock")
                .values()
                .any(|user| {
                    user.email.as_deref() == Some(email.as_str()) && user.id != existing_id
                });
            let mut auth_by_id = self.auth_by_id.write().expect("user repository lock");
            let Some(existing) = auth_by_id.get_mut(&existing_id) else {
                return Ok(None);
            };
            if existing.is_deleted
                || !existing.is_active
                || !existing.auth_source.eq_ignore_ascii_case("ldap")
            {
                return Ok(None);
            }
            if existing.email.as_deref() != Some(email.as_str()) && email_conflict {
                return Ok(None);
            }
            let old_email = existing.email.clone();
            existing.email = Some(email.clone());
            existing.email_verified = true;
            existing.last_login_at = Some(logged_in_at);
            let updated = existing.clone();
            drop(auth_by_id);

            let mut identifiers = self
                .auth_by_identifier
                .write()
                .expect("user repository lock");
            if let Some(old_email) = old_email {
                identifiers.remove(&old_email);
            }
            identifiers.insert(email, updated.id.clone());
            drop(identifiers);
            upsert_memory_ldap_identifiers(self, &updated.id, ldap_dn, ldap_username);
            return Ok(Some(LdapAuthUserProvisioningOutcome {
                user: updated,
                created: false,
            }));
        }

        let base_username = ldap_username
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(username.as_str())
            .trim()
            .to_string();
        let mut candidate_username = base_username.clone();
        for _attempt in 0..3 {
            if self
                .auth_by_id
                .read()
                .expect("user repository lock")
                .values()
                .any(|user| user.username == candidate_username)
            {
                let suffix = uuid::Uuid::new_v4().simple().to_string();
                candidate_username = format!(
                    "{}_ldap_{}{}",
                    base_username,
                    logged_in_at.timestamp(),
                    &suffix[..4]
                );
                continue;
            }

            let user = StoredUserAuthRecord::new(
                uuid::Uuid::new_v4().to_string(),
                Some(email),
                true,
                candidate_username,
                None,
                "user".to_string(),
                "ldap".to_string(),
                None,
                None,
                None,
                true,
                false,
                Some(logged_in_at),
                Some(logged_in_at),
            )?;
            let user = self.insert_auth_user(user)?;
            upsert_memory_ldap_identifiers(self, &user.id, ldap_dn, ldap_username);
            return Ok(Some(LdapAuthUserProvisioningOutcome {
                user,
                created: true,
            }));
        }
        Ok(None)
    }

    async fn touch_auth_user_last_login(
        &self,
        user_id: &str,
        logged_in_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        if self.read_only {
            return Ok(false);
        }

        let mut users = self.auth_by_id.write().expect("user repository lock");
        let Some(user) = users.get_mut(user_id) else {
            return Ok(false);
        };
        user.last_login_at = Some(logged_in_at);
        Ok(true)
    }

    async fn update_local_auth_user_profile(
        &self,
        user_id: &str,
        email_present: bool,
        email: Option<String>,
        email_verified: Option<bool>,
        username: Option<String>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }

        let mut auth_by_id = self.auth_by_id.write().expect("user repository lock");
        let Some(user) = auth_by_id.get_mut(user_id) else {
            return Ok(None);
        };

        let old_email = user.email.clone();
        let old_username = user.username.clone();
        if email_present {
            user.email = email;
        }
        if let Some(email_verified) = email_verified {
            user.email_verified = email_verified;
        }
        if let Some(username) = username {
            user.username = username;
        }
        let updated = user.clone();
        drop(auth_by_id);

        let mut identifiers = self
            .auth_by_identifier
            .write()
            .expect("user repository lock");
        identifiers.remove(&old_username);
        if let Some(old_email) = old_email {
            identifiers.remove(&old_email);
        }
        identifiers.insert(updated.username.clone(), updated.id.clone());
        if let Some(email) = updated.email.as_ref() {
            identifiers.insert(email.clone(), updated.id.clone());
        }
        drop(identifiers);

        if let Some(summary) = self
            .by_id
            .write()
            .expect("user repository lock")
            .get_mut(user_id)
        {
            summary.email = updated.email.clone();
            summary.username = updated.username.clone();
        }

        Ok(Some(updated))
    }

    async fn restore_local_auth_user_state_if_matches(
        &self,
        expected_auth: &StoredUserAuthRecord,
        restored_auth: &StoredUserAuthRecord,
        expected_export: &StoredUserExportRow,
        restored_export: &StoredUserExportRow,
        expected_model_capability_settings: Option<&serde_json::Value>,
        restored_model_capability_settings: Option<serde_json::Value>,
        expected_feature_settings: Option<&serde_json::Value>,
        restored_feature_settings: Option<serde_json::Value>,
    ) -> Result<bool, DataLayerError> {
        if self.read_only
            || expected_auth.id != restored_auth.id
            || expected_export.id != expected_auth.id
            || restored_export.id != restored_auth.id
        {
            return Ok(false);
        }

        let mut users = self.auth_by_id.write().expect("user repository lock");
        let Some(current) = users.get(expected_auth.id.as_str()) else {
            return Ok(false);
        };
        if !current.matches_restore_state(expected_auth) {
            return Ok(false);
        }
        let current_model = self
            .model_settings_by_user_id
            .read()
            .expect("user repository lock")
            .get(&expected_auth.id)
            .cloned();
        let current_feature = self
            .feature_settings_by_user_id
            .read()
            .expect("user repository lock")
            .get(&expected_auth.id)
            .cloned();
        if current_model.as_ref() != expected_model_capability_settings
            || current_feature.as_ref() != expected_feature_settings
        {
            return Ok(false);
        }
        let current_export = self
            .export_rows
            .read()
            .expect("user repository lock")
            .iter()
            .find(|row| row.id == expected_auth.id)
            .cloned();
        if current_export.as_ref().is_some_and(|row| {
            row.rate_limit != expected_export.rate_limit
                || row.rate_limit_mode != expected_export.rate_limit_mode
        }) {
            return Ok(false);
        }

        let removes_active_admin = current.role.eq_ignore_ascii_case("admin")
            && current.is_active
            && !current.is_deleted
            && (!restored_auth.role.eq_ignore_ascii_case("admin") || !restored_auth.is_active);
        if removes_active_admin
            && users
                .values()
                .filter(|user| {
                    user.role.eq_ignore_ascii_case("admin") && user.is_active && !user.is_deleted
                })
                .count()
                <= 1
        {
            return Err(DataLayerError::InvalidInput(
                LAST_ACTIVE_ADMIN_UPDATE_DENIED.to_string(),
            ));
        }

        let old_email = current.email.clone();
        let old_username = current.username.clone();
        let security_state_changed =
            current.role != restored_auth.role || current.is_active != restored_auth.is_active;
        let user = users
            .get_mut(expected_auth.id.as_str())
            .expect("user existence checked while holding write lock");
        user.email = restored_auth.email.clone();
        user.email_verified = restored_auth.email_verified;
        user.username = restored_auth.username.clone();
        user.role = restored_auth.role.clone();
        user.allowed_providers = restored_auth.allowed_providers.clone();
        user.allowed_providers_mode = restored_auth.allowed_providers_mode.clone();
        user.allowed_api_formats = restored_auth.allowed_api_formats.clone();
        user.allowed_api_formats_mode = restored_auth.allowed_api_formats_mode.clone();
        user.allowed_models = restored_auth.allowed_models.clone();
        user.allowed_models_mode = restored_auth.allowed_models_mode.clone();
        user.is_active = restored_auth.is_active;
        if security_state_changed {
            user.security_version = user.security_version.checked_add(1).ok_or_else(|| {
                DataLayerError::UnexpectedValue("users.security_version overflow".to_string())
            })?;
        }
        let updated = user.clone();
        drop(users);

        let mut identifiers = self
            .auth_by_identifier
            .write()
            .expect("user repository lock");
        identifiers.remove(&old_username);
        if let Some(old_email) = old_email {
            identifiers.remove(&old_email);
        }
        identifiers.insert(updated.username.clone(), updated.id.clone());
        if let Some(email) = updated.email.as_ref() {
            identifiers.insert(email.clone(), updated.id.clone());
        }
        drop(identifiers);
        if let Some(summary) = self
            .by_id
            .write()
            .expect("user repository lock")
            .get_mut(&updated.id)
        {
            summary.email = updated.email.clone();
            summary.username = updated.username.clone();
            summary.role = updated.role.clone();
            summary.is_active = updated.is_active;
        }
        let restored_model_capability_settings =
            normalize_optional_json_value(restored_model_capability_settings);
        let restored_feature_settings = normalize_optional_json_value(restored_feature_settings);
        {
            let mut settings = self
                .model_settings_by_user_id
                .write()
                .expect("user repository lock");
            match restored_model_capability_settings.clone() {
                Some(value) => {
                    settings.insert(updated.id.clone(), value);
                }
                None => {
                    settings.remove(&updated.id);
                }
            }
        }
        {
            let mut settings = self
                .feature_settings_by_user_id
                .write()
                .expect("user repository lock");
            match restored_feature_settings.clone() {
                Some(value) => {
                    settings.insert(updated.id.clone(), value);
                }
                None => {
                    settings.remove(&updated.id);
                }
            }
        }
        if let Some(row) = self
            .export_rows
            .write()
            .expect("user repository lock")
            .iter_mut()
            .find(|row| row.id == updated.id)
        {
            row.email = updated.email.clone();
            row.email_verified = updated.email_verified;
            row.username = updated.username.clone();
            row.role = updated.role.clone();
            row.auth_source = updated.auth_source.clone();
            row.allowed_providers = updated.allowed_providers.clone();
            row.allowed_providers_mode = updated.allowed_providers_mode.clone();
            row.allowed_api_formats = updated.allowed_api_formats.clone();
            row.allowed_api_formats_mode = updated.allowed_api_formats_mode.clone();
            row.allowed_models = updated.allowed_models.clone();
            row.allowed_models_mode = updated.allowed_models_mode.clone();
            row.rate_limit = restored_export.rate_limit;
            row.rate_limit_mode = restored_export.rate_limit_mode.clone();
            row.model_capability_settings = restored_model_capability_settings.clone();
            row.feature_settings = restored_feature_settings.clone();
            row.is_active = updated.is_active;
        }
        if security_state_changed {
            let now = chrono::Utc::now();
            let mut sessions = self.sessions_by_id.write().expect("user repository lock");
            for session in sessions
                .values_mut()
                .filter(|session| session.user_id == updated.id && session.revoked_at.is_none())
            {
                session.revoked_at = Some(now);
                session.revoke_reason = Some("user_security_state_changed".to_string());
                session.updated_at = Some(now);
            }
        }
        Ok(true)
    }

    async fn update_local_auth_user_password_hash(
        &self,
        user_id: &str,
        password_hash: String,
        _updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }

        let mut auth_by_id = self.auth_by_id.write().expect("user repository lock");
        let Some(user) = auth_by_id.get_mut(user_id) else {
            return Ok(None);
        };
        user.password_hash = Some(password_hash);
        user.security_version = user.security_version.checked_add(1).ok_or_else(|| {
            DataLayerError::UnexpectedValue("users.security_version overflow".to_string())
        })?;
        Ok(Some(user.clone()))
    }

    async fn restore_local_auth_user_password_hash_if_matches(
        &self,
        user_id: &str,
        expected_password_hash: Option<&str>,
        password_hash: Option<String>,
        _updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        if self.read_only {
            return Ok(false);
        }

        let mut auth_by_id = self.auth_by_id.write().expect("user repository lock");
        let Some(user) = auth_by_id.get_mut(user_id) else {
            return Ok(false);
        };
        if user.password_hash.as_deref() != expected_password_hash {
            return Ok(false);
        }
        user.password_hash = password_hash;
        user.security_version = user.security_version.checked_add(1).ok_or_else(|| {
            DataLayerError::UnexpectedValue("users.security_version overflow".to_string())
        })?;
        Ok(true)
    }

    async fn reset_local_auth_user_password_and_revoke_sessions(
        &self,
        user_id: &str,
        password_hash: String,
        changed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        if self.read_only {
            return Ok(false);
        }
        let mut users = self.auth_by_id.write().expect("user repository lock");
        let mut sessions = self.sessions_by_id.write().expect("user repository lock");
        let Some(user) = users.get_mut(user_id).filter(|user| !user.is_deleted) else {
            return Ok(false);
        };
        user.password_hash = Some(password_hash);
        user.security_version = user.security_version.checked_add(1).ok_or_else(|| {
            DataLayerError::UnexpectedValue("users.security_version overflow".to_string())
        })?;
        for session in sessions
            .values_mut()
            .filter(|session| session.user_id == user_id && !session.is_revoked())
        {
            session.revoked_at = Some(changed_at);
            session.revoke_reason = Some("admin_password_reset".to_string());
            session.updated_at = Some(changed_at);
        }
        Ok(true)
    }

    async fn change_local_auth_password_and_revoke_sessions(
        &self,
        user_id: &str,
        current_session_id: &str,
        expected_password_hash: Option<&str>,
        next_password_hash: String,
        changed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        if self.read_only {
            return Ok(false);
        }
        let mut users = self.auth_by_id.write().expect("user repository lock");
        let mut sessions = self.sessions_by_id.write().expect("user repository lock");
        let Some(user) = users.get_mut(user_id) else {
            return Ok(false);
        };
        if user.password_hash.as_deref() != expected_password_hash
            || !user.is_active
            || user.is_deleted
        {
            return Ok(false);
        }
        if !sessions.get(current_session_id).is_some_and(|session| {
            session.user_id == user_id && !session.is_revoked() && !session.is_expired(changed_at)
        }) {
            return Ok(false);
        }
        user.password_hash = Some(next_password_hash);
        user.security_version = user.security_version.checked_add(1).ok_or_else(|| {
            DataLayerError::UnexpectedValue("users.security_version overflow".to_string())
        })?;
        for session in sessions
            .values_mut()
            .filter(|session| session.user_id == user_id && !session.is_revoked())
        {
            session.revoked_at = Some(changed_at);
            session.revoke_reason = Some("password_changed".to_string());
            session.updated_at = Some(changed_at);
        }
        Ok(true)
    }

    async fn update_local_auth_user_admin_fields(
        &self,
        user_id: &str,
        role: Option<String>,
        allowed_providers_present: bool,
        allowed_providers: Option<Vec<String>>,
        allowed_api_formats_present: bool,
        allowed_api_formats: Option<Vec<String>>,
        allowed_models_present: bool,
        allowed_models: Option<Vec<String>>,
        rate_limit_present: bool,
        rate_limit: Option<i32>,
        is_active: Option<bool>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }

        let mut auth_by_id = self.auth_by_id.write().expect("user repository lock");
        let Some(current_user) = auth_by_id.get(user_id) else {
            return Ok(None);
        };
        let current_role = current_user.role.clone();
        let current_active = current_user.is_active;
        let current_deleted = current_user.is_deleted;
        let next_role = role.as_deref().unwrap_or(current_role.as_str());
        let next_active = is_active.unwrap_or(current_active);
        if current_role.eq_ignore_ascii_case("admin")
            && current_active
            && !current_deleted
            && (!next_role.eq_ignore_ascii_case("admin") || !next_active)
            && auth_by_id
                .values()
                .filter(|user| {
                    user.role.eq_ignore_ascii_case("admin") && user.is_active && !user.is_deleted
                })
                .count()
                <= 1
        {
            return Err(DataLayerError::InvalidInput(
                LAST_ACTIVE_ADMIN_UPDATE_DENIED.to_string(),
            ));
        }
        let security_state_changed =
            !current_role.eq_ignore_ascii_case(next_role) || current_active != next_active;
        let mut sessions = self.sessions_by_id.write().expect("user repository lock");
        let user = auth_by_id
            .get_mut(user_id)
            .expect("user existence checked while holding write lock");
        if let Some(role) = role {
            user.role = role;
        }
        if allowed_providers_present {
            user.allowed_providers = allowed_providers;
            user.allowed_providers_mode = if user
                .allowed_providers
                .as_ref()
                .is_some_and(|values| !values.is_empty())
            {
                "specific".to_string()
            } else {
                "unrestricted".to_string()
            };
        }
        if allowed_api_formats_present {
            user.allowed_api_formats = allowed_api_formats;
            user.allowed_api_formats_mode = if user
                .allowed_api_formats
                .as_ref()
                .is_some_and(|values| !values.is_empty())
            {
                "specific".to_string()
            } else {
                "unrestricted".to_string()
            };
        }
        if allowed_models_present {
            user.allowed_models = allowed_models;
            user.allowed_models_mode = if user
                .allowed_models
                .as_ref()
                .is_some_and(|values| !values.is_empty())
            {
                "specific".to_string()
            } else {
                "unrestricted".to_string()
            };
        }
        if let Some(is_active) = is_active {
            user.is_active = is_active;
        }
        if security_state_changed {
            user.security_version = user.security_version.checked_add(1).ok_or_else(|| {
                DataLayerError::UnexpectedValue("users.security_version overflow".to_string())
            })?;
            let revoked_at = chrono::Utc::now();
            for session in sessions
                .values_mut()
                .filter(|session| session.user_id == user_id && session.revoked_at.is_none())
            {
                session.revoked_at = Some(revoked_at);
                session.revoke_reason = Some("user_security_state_changed".to_string());
                session.updated_at = Some(revoked_at);
            }
        }
        let updated = user.clone();
        drop(sessions);
        drop(auth_by_id);

        if let Some(summary) = self
            .by_id
            .write()
            .expect("user repository lock")
            .get_mut(user_id)
        {
            summary.role = updated.role.clone();
            summary.is_active = updated.is_active;
        }
        if let Some(row) = self
            .export_rows
            .write()
            .expect("user repository lock")
            .iter_mut()
            .find(|row| row.id == user_id)
        {
            row.role = updated.role.clone();
            row.allowed_providers = updated.allowed_providers.clone();
            row.allowed_providers_mode = updated.allowed_providers_mode.clone();
            row.allowed_api_formats = updated.allowed_api_formats.clone();
            row.allowed_api_formats_mode = updated.allowed_api_formats_mode.clone();
            row.allowed_models = updated.allowed_models.clone();
            row.allowed_models_mode = updated.allowed_models_mode.clone();
            if rate_limit_present {
                row.rate_limit = rate_limit;
                row.rate_limit_mode = if row.rate_limit.is_some() {
                    "custom".to_string()
                } else {
                    "system".to_string()
                };
            }
            row.is_active = updated.is_active;
        }
        Ok(Some(updated))
    }

    async fn update_local_auth_user_policy_modes(
        &self,
        user_id: &str,
        allowed_providers_mode: Option<String>,
        allowed_api_formats_mode: Option<String>,
        allowed_models_mode: Option<String>,
        rate_limit_mode: Option<String>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }

        let mut auth_by_id = self.auth_by_id.write().expect("user repository lock");
        let Some(user) = auth_by_id.get_mut(user_id) else {
            return Ok(None);
        };
        if let Some(mode) = allowed_providers_mode.clone() {
            user.allowed_providers_mode = mode;
        }
        if let Some(mode) = allowed_api_formats_mode.clone() {
            user.allowed_api_formats_mode = mode;
        }
        if let Some(mode) = allowed_models_mode.clone() {
            user.allowed_models_mode = mode;
        }
        let updated = user.clone();
        drop(auth_by_id);

        if let Some(row) = self
            .export_rows
            .write()
            .expect("user repository lock")
            .iter_mut()
            .find(|row| row.id == user_id)
        {
            if let Some(mode) = allowed_providers_mode {
                row.allowed_providers_mode = mode;
            }
            if let Some(mode) = allowed_api_formats_mode {
                row.allowed_api_formats_mode = mode;
            }
            if let Some(mode) = allowed_models_mode {
                row.allowed_models_mode = mode;
            }
            if let Some(mode) = rate_limit_mode {
                row.rate_limit_mode = mode;
            }
        }
        Ok(Some(updated))
    }

    async fn update_user_model_capability_settings(
        &self,
        user_id: &str,
        settings: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }

        let user_exists = self
            .auth_by_id
            .read()
            .expect("user repository lock")
            .contains_key(user_id)
            || self
                .export_rows
                .read()
                .expect("user repository lock")
                .iter()
                .any(|row| row.id == user_id);
        if !user_exists {
            return Ok(None);
        }

        let normalized = normalize_optional_json_value(settings);
        let mut settings_by_user = self
            .model_settings_by_user_id
            .write()
            .expect("user repository lock");
        match normalized.clone() {
            Some(value) => {
                settings_by_user.insert(user_id.to_string(), value);
            }
            None => {
                settings_by_user.remove(user_id);
            }
        }
        drop(settings_by_user);

        if let Some(row) = self
            .export_rows
            .write()
            .expect("user repository lock")
            .iter_mut()
            .find(|row| row.id == user_id)
        {
            row.model_capability_settings = normalized.clone();
        }

        Ok(normalized)
    }

    async fn update_user_feature_settings(
        &self,
        user_id: &str,
        settings: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }

        let user_exists = self
            .auth_by_id
            .read()
            .expect("user repository lock")
            .contains_key(user_id)
            || self
                .export_rows
                .read()
                .expect("user repository lock")
                .iter()
                .any(|row| row.id == user_id);
        if !user_exists {
            return Ok(None);
        }

        let normalized = normalize_optional_json_value(settings);
        let mut feature_settings_by_user = self
            .feature_settings_by_user_id
            .write()
            .expect("user repository lock");
        match normalized.clone() {
            Some(value) => {
                feature_settings_by_user.insert(user_id.to_string(), value);
            }
            None => {
                feature_settings_by_user.remove(user_id);
            }
        }
        drop(feature_settings_by_user);

        if let Some(row) = self
            .export_rows
            .write()
            .expect("user repository lock")
            .iter_mut()
            .find(|row| row.id == user_id)
        {
            row.feature_settings = normalized.clone();
        }

        Ok(normalized)
    }

    async fn create_local_auth_user(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }

        let now = chrono::Utc::now();
        let user = StoredUserAuthRecord::new(
            uuid::Uuid::new_v4().to_string(),
            email,
            email_verified,
            username,
            Some(password_hash),
            "user".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            Some(now),
            None,
        )?
        .with_policy_modes(
            "inherit".to_string(),
            "inherit".to_string(),
            "inherit".to_string(),
        )?;
        self.insert_auth_user(user).map(Some)
    }

    async fn create_local_auth_user_with_settings(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
        role: String,
        allowed_providers: Option<Vec<String>>,
        allowed_api_formats: Option<Vec<String>>,
        allowed_models: Option<Vec<String>>,
        _rate_limit: Option<i32>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }

        let now = chrono::Utc::now();
        let user = StoredUserAuthRecord::new(
            uuid::Uuid::new_v4().to_string(),
            email,
            email_verified,
            username,
            Some(password_hash),
            role,
            "local".to_string(),
            allowed_providers.map(serde_json::Value::from),
            allowed_api_formats.map(serde_json::Value::from),
            allowed_models.map(serde_json::Value::from),
            true,
            false,
            Some(now),
            None,
        )?;
        self.insert_auth_user(user).map(Some)
    }

    async fn delete_local_auth_user(&self, user_id: &str) -> Result<bool, DataLayerError> {
        if self.read_only {
            return Ok(false);
        }

        let mut auth_by_id = self.auth_by_id.write().expect("user repository lock");
        if auth_by_id.get(user_id).is_some_and(|user| {
            user.role.eq_ignore_ascii_case("admin") && user.is_active && !user.is_deleted
        }) && auth_by_id
            .values()
            .filter(|user| {
                user.role.eq_ignore_ascii_case("admin") && user.is_active && !user.is_deleted
            })
            .count()
            <= 1
        {
            return Err(DataLayerError::InvalidInput(
                LAST_ACTIVE_ADMIN_DELETE_DENIED.to_string(),
            ));
        }
        let mut sessions = self.sessions_by_id.write().expect("user repository lock");
        let removed = auth_by_id.remove(user_id);
        if removed.is_some() {
            sessions.retain(|_, session| session.user_id != user_id);
        }
        drop(sessions);
        drop(auth_by_id);
        let Some(removed) = removed else {
            return Ok(false);
        };
        self.by_id
            .write()
            .expect("user repository lock")
            .remove(user_id);
        self.oauth_links_by_id
            .write()
            .expect("user repository lock")
            .retain(|_, link| link.user_id != user_id);
        self.group_members
            .write()
            .expect("user repository lock")
            .retain(|key, _| key.1 != user_id);
        self.preferences_by_user_id
            .write()
            .expect("user repository lock")
            .remove(user_id);
        self.model_settings_by_user_id
            .write()
            .expect("user repository lock")
            .remove(user_id);
        self.feature_settings_by_user_id
            .write()
            .expect("user repository lock")
            .remove(user_id);
        self.ldap_dn_by_user_id
            .write()
            .expect("user repository lock")
            .remove(user_id);
        self.ldap_username_by_user_id
            .write()
            .expect("user repository lock")
            .remove(user_id);
        self.export_rows
            .write()
            .expect("user repository lock")
            .retain(|row| row.id != user_id);

        let mut identifiers = self
            .auth_by_identifier
            .write()
            .expect("user repository lock");
        identifiers.remove(&removed.username);
        if let Some(email) = removed.email {
            identifiers.remove(&email);
        }
        Ok(true)
    }

    async fn read_user_preferences(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserPreferenceRecord>, DataLayerError> {
        Ok(self
            .preferences_by_user_id
            .read()
            .expect("user repository lock")
            .get(user_id)
            .cloned())
    }

    async fn write_user_preferences(
        &self,
        preferences: &StoredUserPreferenceRecord,
    ) -> Result<Option<StoredUserPreferenceRecord>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }

        self.preferences_by_user_id
            .write()
            .expect("user repository lock")
            .insert(preferences.user_id.clone(), preferences.clone());
        Ok(Some(preferences.clone()))
    }

    async fn find_user_session(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> Result<Option<StoredUserSessionRecord>, DataLayerError> {
        Ok(self
            .sessions_by_id
            .read()
            .expect("user repository lock")
            .get(session_id)
            .filter(|session| session.user_id == user_id)
            .cloned())
    }

    async fn list_user_sessions(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserSessionRecord>, DataLayerError> {
        let now = chrono::Utc::now();
        let mut sessions = self
            .sessions_by_id
            .read()
            .expect("user repository lock")
            .values()
            .filter(|session| {
                session.user_id == user_id && !session.is_revoked() && !session.is_expired(now)
            })
            .cloned()
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| {
            std::cmp::Reverse((session.last_seen_at, session.created_at, session.id.clone()))
        });
        Ok(sessions)
    }

    async fn create_user_session(
        &self,
        session: &StoredUserSessionRecord,
    ) -> Result<Option<StoredUserSessionRecord>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }

        let now = session
            .created_at
            .or(session.updated_at)
            .or(session.last_seen_at)
            .unwrap_or_else(chrono::Utc::now);
        let users = self.auth_by_id.read().expect("user repository lock");
        let Some(user) = users.get(&session.user_id) else {
            return Ok(None);
        };
        if !user.is_active || user.is_deleted || user.security_version != session.security_version {
            return Ok(None);
        }
        let mut sessions = self.sessions_by_id.write().expect("user repository lock");
        for existing in sessions.values_mut() {
            if existing.user_id == session.user_id
                && existing.client_device_id == session.client_device_id
                && existing.revoked_at.is_none()
                && !existing.is_expired(now)
            {
                existing.revoked_at = Some(now);
                existing.revoke_reason = Some("replaced_by_new_login".to_string());
                existing.updated_at = Some(now);
            }
        }
        sessions.insert(session.id.clone(), session.clone());
        Ok(Some(session.clone()))
    }

    async fn create_user_session_if_password_matches(
        &self,
        session: &StoredUserSessionRecord,
        expected_password_hash: &str,
    ) -> Result<Option<StoredUserSessionRecord>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }
        let mut users = self.auth_by_id.write().expect("user repository lock");
        let Some(user) = users.get_mut(&session.user_id) else {
            return Ok(None);
        };
        if user.password_hash.as_deref() != Some(expected_password_hash)
            || !user.auth_source.eq_ignore_ascii_case("local")
            || !user.is_active
            || user.is_deleted
            || user.security_version != session.security_version
        {
            return Ok(None);
        }
        let now = session
            .created_at
            .or(session.updated_at)
            .or(session.last_seen_at)
            .unwrap_or_else(chrono::Utc::now);
        user.last_login_at = Some(now);
        let mut sessions = self.sessions_by_id.write().expect("user repository lock");
        for existing in sessions.values_mut() {
            if existing.user_id == session.user_id
                && existing.client_device_id == session.client_device_id
                && existing.revoked_at.is_none()
                && !existing.is_expired(now)
            {
                existing.revoked_at = Some(now);
                existing.revoke_reason = Some("replaced_by_new_login".to_string());
                existing.updated_at = Some(now);
            }
        }
        sessions.insert(session.id.clone(), session.clone());
        Ok(Some(session.clone()))
    }

    async fn touch_user_session(
        &self,
        user_id: &str,
        session_id: &str,
        touched_at: chrono::DateTime<chrono::Utc>,
        ip_address: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<bool, DataLayerError> {
        if self.read_only {
            return Ok(false);
        }

        let mut sessions = self.sessions_by_id.write().expect("user repository lock");
        let Some(session) = sessions
            .get_mut(session_id)
            .filter(|s| s.user_id == user_id)
        else {
            return Ok(false);
        };
        session.last_seen_at = Some(touched_at);
        if let Some(ip_address) = ip_address {
            session.ip_address = Some(ip_address.to_string());
        }
        if let Some(user_agent) = user_agent {
            session.user_agent = Some(user_agent.chars().take(1000).collect());
        }
        session.updated_at = Some(touched_at);
        Ok(true)
    }

    async fn update_user_session_device_label(
        &self,
        user_id: &str,
        session_id: &str,
        device_label: &str,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        if self.read_only {
            return Ok(false);
        }

        let mut sessions = self.sessions_by_id.write().expect("user repository lock");
        let Some(session) = sessions
            .get_mut(session_id)
            .filter(|s| s.user_id == user_id)
        else {
            return Ok(false);
        };
        session.device_label = Some(device_label.chars().take(120).collect());
        session.updated_at = Some(updated_at);
        Ok(true)
    }

    async fn rotate_user_session_refresh_token(
        &self,
        user_id: &str,
        session_id: &str,
        expected_refresh_token_hash: &str,
        next_refresh_token_hash: &str,
        rotated_at: chrono::DateTime<chrono::Utc>,
        expires_at: chrono::DateTime<chrono::Utc>,
        ip_address: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<bool, DataLayerError> {
        if self.read_only {
            return Ok(false);
        }

        let mut sessions = self.sessions_by_id.write().expect("user repository lock");
        let Some(session) = sessions.get_mut(session_id).filter(|session| {
            session.user_id == user_id
                && session.refresh_token_hash == expected_refresh_token_hash
                && !session.is_revoked()
                && !session.is_expired(rotated_at)
        }) else {
            return Ok(false);
        };
        session.prev_refresh_token_hash = Some(expected_refresh_token_hash.to_string());
        session.refresh_token_hash = next_refresh_token_hash.to_string();
        session.rotated_at = Some(rotated_at);
        session.expires_at = Some(expires_at);
        session.last_seen_at = Some(rotated_at);
        if let Some(ip_address) = ip_address {
            session.ip_address = Some(ip_address.to_string());
        }
        if let Some(user_agent) = user_agent {
            session.user_agent = Some(user_agent.chars().take(1000).collect());
        }
        session.updated_at = Some(rotated_at);
        Ok(true)
    }

    async fn revoke_user_session(
        &self,
        user_id: &str,
        session_id: &str,
        revoked_at: chrono::DateTime<chrono::Utc>,
        reason: &str,
    ) -> Result<bool, DataLayerError> {
        if self.read_only {
            return Ok(false);
        }

        let mut sessions = self.sessions_by_id.write().expect("user repository lock");
        let Some(session) = sessions
            .get_mut(session_id)
            .filter(|s| s.user_id == user_id)
        else {
            return Ok(false);
        };
        session.revoked_at = Some(revoked_at);
        session.revoke_reason = Some(reason.chars().take(100).collect());
        session.updated_at = Some(revoked_at);
        Ok(true)
    }

    async fn revoke_all_user_sessions(
        &self,
        user_id: &str,
        revoked_at: chrono::DateTime<chrono::Utc>,
        reason: &str,
    ) -> Result<u64, DataLayerError> {
        if self.read_only {
            return Ok(0);
        }

        let mut count = 0u64;
        for session in self
            .sessions_by_id
            .write()
            .expect("user repository lock")
            .values_mut()
        {
            if session.user_id == user_id && session.revoked_at.is_none() {
                session.revoked_at = Some(revoked_at);
                session.revoke_reason = Some(reason.chars().take(100).collect());
                session.updated_at = Some(revoked_at);
                count += 1;
            }
        }
        Ok(count)
    }

    async fn count_active_admin_users(&self) -> Result<u64, DataLayerError> {
        Ok(self
            .auth_by_id
            .read()
            .expect("user repository lock")
            .values()
            .filter(|user| {
                user.role.eq_ignore_ascii_case("admin") && user.is_active && !user.is_deleted
            })
            .count() as u64)
    }

    async fn count_active_local_admin_users_with_valid_password(
        &self,
    ) -> Result<u64, DataLayerError> {
        Ok(self
            .auth_by_id
            .read()
            .expect("user repository lock")
            .values()
            .filter(|user| {
                user.role.eq_ignore_ascii_case("admin")
                    && user.auth_source.eq_ignore_ascii_case("local")
                    && user.is_active
                    && !user.is_deleted
                    && user
                        .password_hash
                        .as_deref()
                        .is_some_and(is_valid_bcrypt_hash)
            })
            .count() as u64)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::repository::users::{UserExportListQuery, UserReadRepository};

    fn user_group_record(name: &str, priority: i32) -> UpsertUserGroupRecord {
        UpsertUserGroupRecord {
            name: name.to_string(),
            description: Some(format!("{name} description")),
            priority,
            allowed_providers: Some(vec!["provider-a".to_string()]),
            allowed_providers_mode: "specific".to_string(),
            allowed_api_formats: Some(vec!["chat".to_string()]),
            allowed_api_formats_mode: "specific".to_string(),
            allowed_models: Some(vec!["model-a".to_string()]),
            allowed_models_mode: "specific".to_string(),
            rate_limit: Some(10),
            rate_limit_mode: "custom".to_string(),
            provider_key_policies: Default::default(),
        }
    }

    #[tokio::test]
    async fn lists_seeded_users() {
        let user = StoredUserSummary::new(
            "user-1".to_string(),
            "alice".to_string(),
            Some("alice@example.com".to_string()),
            "user".to_string(),
            true,
            false,
        )
        .expect("user should build");
        let repository = InMemoryUserReadRepository::seed(vec![user.clone()]);
        let rows = repository
            .list_users_by_ids(&["user-1".to_string()])
            .await
            .expect("lookup should succeed");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], user);
    }

    #[tokio::test]
    async fn restores_user_group_only_when_complete_snapshot_matches() {
        let repository = InMemoryUserReadRepository::default();
        let before = repository
            .create_user_group(user_group_record("cas-group", 1))
            .await
            .expect("group creation should succeed")
            .expect("group should be created");
        let after = repository
            .update_user_group(&before.id, user_group_record("cas-group-imported", 2))
            .await
            .expect("group update should succeed")
            .expect("group should exist");

        assert!(repository
            .restore_user_group_if_matches(&after, &before)
            .await
            .expect("matching group restore should succeed"));
        assert_eq!(
            repository
                .find_user_group_by_id(&before.id)
                .await
                .expect("group lookup should succeed"),
            Some(before.clone())
        );

        let current_after_second_update = repository
            .update_user_group(&before.id, user_group_record("cas-group-concurrent", 3))
            .await
            .expect("second group update should succeed")
            .expect("group should exist");
        assert!(!repository
            .restore_user_group_if_matches(&after, &before)
            .await
            .expect("stale group restore should return a conflict"));
        assert_eq!(
            repository
                .find_user_group_by_id(&before.id)
                .await
                .expect("group lookup should succeed"),
            Some(current_after_second_update)
        );
    }

    #[tokio::test]
    async fn user_group_restore_rejects_identity_mismatch_and_missing_rows() {
        let repository = InMemoryUserReadRepository::default();
        let expected = repository
            .create_user_group(user_group_record("cas-identity", 1))
            .await
            .expect("group creation should succeed")
            .expect("group should be created");
        let mut different_id = expected.clone();
        different_id.id = "different-group-id".to_string();
        assert!(!repository
            .restore_user_group_if_matches(&expected, &different_id)
            .await
            .expect("identity mismatch should return false"));

        let mut missing = expected.clone();
        missing.id = "missing-group-id".to_string();
        assert!(!repository
            .restore_user_group_if_matches(&missing, &missing)
            .await
            .expect("missing row should return false"));
    }

    #[tokio::test]
    async fn lists_seeded_non_admin_export_users() {
        let user = StoredUserExportRow::new(
            "user-1".to_string(),
            Some("alice@example.com".to_string()),
            true,
            "alice".to_string(),
            Some("hash".to_string()),
            "user".to_string(),
            "local".to_string(),
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-4.1"])),
            Some(60),
            Some(serde_json::json!({"gpt-4.1": {"cache_1h": true}})),
            true,
        )
        .expect("user export row should build");
        let repository = InMemoryUserReadRepository::seed_export_users(vec![user.clone()]);

        let rows = repository
            .list_non_admin_export_users()
            .await
            .expect("export should succeed");

        assert_eq!(rows, vec![user]);
    }

    #[tokio::test]
    async fn finds_seeded_auth_user_by_id_and_identifier() {
        let user = StoredUserAuthRecord::new(
            "user-1".to_string(),
            Some("alice@example.com".to_string()),
            true,
            "alice".to_string(),
            Some("hash".to_string()),
            "user".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            None,
            None,
        )
        .expect("auth user should build");
        let repository = InMemoryUserReadRepository::seed_auth_users(vec![user.clone()]);

        let by_id = repository
            .find_user_auth_by_id("user-1")
            .await
            .expect("lookup by id should succeed");
        let by_email = repository
            .find_user_auth_by_identifier("alice@example.com")
            .await
            .expect("lookup by email should succeed");
        let by_username = repository
            .find_user_auth_by_identifier("alice")
            .await
            .expect("lookup by username should succeed");
        let by_exact_email = repository
            .find_user_auth_by_email("alice@example.com")
            .await
            .expect("exact email lookup should succeed");
        let by_exact_username = repository
            .find_user_auth_by_username("alice")
            .await
            .expect("exact username lookup should succeed");
        let email_should_not_match_username = repository
            .find_user_auth_by_email("alice")
            .await
            .expect("exact email lookup should succeed");

        assert_eq!(by_id, Some(user.clone()));
        assert_eq!(by_email, Some(user.clone()));
        assert_eq!(by_username, Some(user.clone()));
        assert_eq!(by_exact_email, Some(user.clone()));
        assert_eq!(by_exact_username, Some(user));
        assert_eq!(email_should_not_match_username, None);
    }

    #[tokio::test]
    async fn touches_auth_user_last_login_in_memory() {
        let user = StoredUserAuthRecord::new(
            "user-1".to_string(),
            Some("alice@example.com".to_string()),
            true,
            "alice".to_string(),
            Some("hash".to_string()),
            "user".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            None,
            None,
        )
        .expect("auth user should build");
        let repository = InMemoryUserReadRepository::seed_auth_users(vec![user]);
        let logged_in_at = chrono::Utc::now();

        assert!(repository
            .touch_auth_user_last_login("user-1", logged_in_at)
            .await
            .expect("touch should succeed"));
        assert!(!repository
            .touch_auth_user_last_login("missing-user", logged_in_at)
            .await
            .expect("missing touch should succeed"));
        assert_eq!(
            repository
                .find_user_auth_by_id("user-1")
                .await
                .expect("auth lookup should succeed")
                .expect("auth user should exist")
                .last_login_at,
            Some(logged_in_at)
        );
    }

    #[tokio::test]
    async fn updates_local_auth_user_profile_and_password_in_memory() {
        let user = StoredUserAuthRecord::new(
            "user-1".to_string(),
            Some("alice@example.com".to_string()),
            true,
            "alice".to_string(),
            Some("old-hash".to_string()),
            "user".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            None,
            None,
        )
        .expect("auth user should build");
        let export_user = StoredUserExportRow::new(
            "user-1".to_string(),
            Some("alice@example.com".to_string()),
            true,
            "alice".to_string(),
            Some("old-hash".to_string()),
            "user".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            Some(10),
            None,
            true,
        )
        .expect("export user should build");
        let repository = InMemoryUserReadRepository::seed_auth_users(vec![user])
            .with_export_users([export_user]);

        let updated = repository
            .update_local_auth_user_profile(
                "user-1",
                true,
                Some("alice2@example.com".to_string()),
                Some(true),
                Some("alice2".to_string()),
            )
            .await
            .expect("profile update should succeed")
            .expect("profile update should return user");
        assert_eq!(updated.email.as_deref(), Some("alice2@example.com"));
        assert!(updated.email_verified);
        assert_eq!(updated.username, "alice2");

        let cleared = repository
            .update_local_auth_user_profile("user-1", true, None, Some(false), None)
            .await
            .expect("nullable email update should succeed")
            .expect("user should exist");
        assert!(cleared.email.is_none());
        assert!(!cleared.email_verified);
        assert!(repository
            .find_user_auth_by_identifier("alice@example.com")
            .await
            .expect("old email lookup should succeed")
            .is_none());
        assert_eq!(
            repository
                .find_user_auth_by_identifier("alice2")
                .await
                .expect("new username lookup should succeed")
                .expect("new username should resolve")
                .id,
            "user-1"
        );

        let password_updated = repository
            .update_local_auth_user_password_hash(
                "user-1",
                "new-hash".to_string(),
                chrono::Utc::now(),
            )
            .await
            .expect("password update should succeed")
            .expect("password update should return user");
        assert_eq!(password_updated.password_hash.as_deref(), Some("new-hash"));
        let admin_updated = repository
            .update_local_auth_user_admin_fields(
                "user-1",
                Some("admin".to_string()),
                true,
                Some(vec!["openai".to_string()]),
                true,
                None,
                true,
                Some(vec!["gpt-4.1".to_string()]),
                true,
                Some(50),
                Some(false),
            )
            .await
            .expect("admin fields update should succeed")
            .expect("admin fields update should return user");
        assert_eq!(admin_updated.role, "admin");
        assert_eq!(
            admin_updated.allowed_providers,
            Some(vec!["openai".to_string()])
        );
        assert_eq!(admin_updated.allowed_api_formats, None);
        assert_eq!(
            admin_updated.allowed_models,
            Some(vec!["gpt-4.1".to_string()])
        );
        assert!(!admin_updated.is_active);
        assert_eq!(
            repository
                .find_export_user_by_id("user-1")
                .await
                .expect("export lookup should succeed")
                .expect("export row should exist")
                .rate_limit,
            Some(50)
        );
        repository
            .update_local_auth_user_admin_fields(
                "user-1", None, false, None, false, None, false, None, true, None, None,
            )
            .await
            .expect("rate limit clear should succeed")
            .expect("rate limit clear should return user");
        assert_eq!(
            repository
                .find_export_user_by_id("user-1")
                .await
                .expect("export lookup should succeed")
                .expect("export row should exist")
                .rate_limit,
            None
        );
        assert_eq!(
            repository
                .update_user_model_capability_settings(
                    "user-1",
                    Some(serde_json::json!({"gpt-4.1": {"enabled": true}})),
                )
                .await
                .expect("model settings update should succeed"),
            Some(serde_json::json!({"gpt-4.1": {"enabled": true}}))
        );
        assert_eq!(
            repository
                .update_user_model_capability_settings("user-1", Some(serde_json::Value::Null))
                .await
                .expect("model settings clear should succeed"),
            None
        );
        assert!(repository
            .update_local_auth_user_profile("missing-user", false, None, None, None)
            .await
            .expect("missing profile update should succeed")
            .is_none());
        assert!(repository
            .delete_local_auth_user("user-1")
            .await
            .expect("delete should succeed"));
        assert!(!repository
            .delete_local_auth_user("user-1")
            .await
            .expect("second delete should succeed"));
        assert!(repository
            .find_user_auth_by_identifier("alice2")
            .await
            .expect("deleted username lookup should succeed")
            .is_none());
    }

    #[tokio::test]
    async fn restores_nullable_password_only_when_expected_hash_matches() {
        let user = StoredUserAuthRecord::new(
            "user-password-cas".to_string(),
            Some("password-cas@example.com".to_string()),
            true,
            "password-cas".to_string(),
            Some("imported-hash".to_string()),
            "user".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            None,
            None,
        )
        .expect("auth user should build");
        let repository = InMemoryUserReadRepository::seed_auth_users([user]);

        assert!(repository
            .restore_local_auth_user_password_hash_if_matches(
                "user-password-cas",
                Some("imported-hash"),
                None,
                chrono::Utc::now(),
            )
            .await
            .expect("nullable restore should succeed"));
        assert!(repository
            .find_user_auth_by_id("user-password-cas")
            .await
            .expect("user lookup should succeed")
            .expect("user should exist")
            .password_hash
            .is_none());

        assert!(!repository
            .restore_local_auth_user_password_hash_if_matches(
                "user-password-cas",
                Some("stale-hash"),
                Some("old-hash".to_string()),
                chrono::Utc::now(),
            )
            .await
            .expect("conflicting restore should return false"));
        assert!(repository
            .find_user_auth_by_id("user-password-cas")
            .await
            .expect("user lookup should succeed")
            .expect("user should exist")
            .password_hash
            .is_none());
    }

    #[tokio::test]
    async fn hard_delete_preserves_last_admin_then_cleans_owned_memory_state() {
        let now = chrono::Utc::now();
        let admin = StoredUserAuthRecord::new(
            "admin-delete-target".to_string(),
            Some("admin-delete@example.com".to_string()),
            true,
            "admin-delete".to_string(),
            Some("password-hash".to_string()),
            "admin".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            Some(now),
            None,
        )
        .expect("admin should build");
        let export_row = StoredUserExportRow::new(
            admin.id.clone(),
            admin.email.clone(),
            admin.email_verified,
            admin.username.clone(),
            admin.password_hash.clone(),
            admin.role.clone(),
            admin.auth_source.clone(),
            None,
            None,
            None,
            None,
            Some(serde_json::json!({"gpt-4.1": {"enabled": true}})),
            true,
        )
        .expect("export row should build")
        .with_feature_settings(Some(serde_json::json!({"feature": true})));
        let session = StoredUserSessionRecord::new(
            "admin-delete-session".to_string(),
            admin.id.clone(),
            "admin-delete-device".to_string(),
            None,
            StoredUserSessionRecord::hash_refresh_token("admin-delete-refresh"),
            None,
            None,
            Some(now),
            Some(now + chrono::Duration::hours(1)),
            None,
            None,
            None,
            None,
            Some(now),
            Some(now),
        )
        .expect("session should build");
        let preferences = StoredUserPreferenceRecord {
            user_id: admin.id.clone(),
            avatar_url: None,
            bio: None,
            default_provider_id: None,
            default_provider_name: None,
            theme: "system".to_string(),
            language: "zh-CN".to_string(),
            timezone: "Asia/Shanghai".to_string(),
            email_notifications: true,
            usage_alerts: true,
            announcement_notifications: true,
        };
        let repository = InMemoryUserReadRepository::seed_auth_users([admin.clone()])
            .with_export_users([export_row])
            .with_user_preferences([preferences])
            .with_user_sessions([session]);
        repository
            .oauth_links_by_id
            .write()
            .expect("user repository lock")
            .insert(
                "admin-delete-oauth".to_string(),
                StoredMemoryOAuthLink {
                    id: "admin-delete-oauth".to_string(),
                    user_id: admin.id.clone(),
                    provider_type: "test".to_string(),
                    provider_user_id: "admin-delete-subject".to_string(),
                    provider_username: None,
                    provider_email: admin.email.clone(),
                    extra_data: None,
                    linked_at: now,
                    last_login_at: None,
                },
            );
        repository
            .group_members
            .write()
            .expect("user repository lock")
            .insert(("group-1".to_string(), admin.id.clone()), now);
        repository
            .model_settings_by_user_id
            .write()
            .expect("user repository lock")
            .insert(admin.id.clone(), serde_json::json!({"model": true}));
        repository
            .feature_settings_by_user_id
            .write()
            .expect("user repository lock")
            .insert(admin.id.clone(), serde_json::json!({"feature": true}));
        repository
            .ldap_dn_by_user_id
            .write()
            .expect("user repository lock")
            .insert(admin.id.clone(), "uid=admin-delete,dc=example".to_string());
        repository
            .ldap_username_by_user_id
            .write()
            .expect("user repository lock")
            .insert(admin.id.clone(), "admin-delete-ldap".to_string());

        let error = repository
            .delete_local_auth_user(&admin.id)
            .await
            .expect_err("last active admin delete must be rejected");
        assert!(crate::repository::users::is_last_active_admin_delete_denied(&error));
        assert!(repository
            .auth_by_id
            .read()
            .expect("user repository lock")
            .contains_key(&admin.id));
        assert!(repository
            .sessions_by_id
            .read()
            .expect("user repository lock")
            .values()
            .any(|session| session.user_id == admin.id));
        assert!(repository
            .oauth_links_by_id
            .read()
            .expect("user repository lock")
            .values()
            .any(|link| link.user_id == admin.id));

        repository
            .create_local_auth_user_with_settings(
                Some("admin-keeper@example.com".to_string()),
                true,
                "admin-keeper".to_string(),
                "password-hash".to_string(),
                "admin".to_string(),
                None,
                None,
                None,
                None,
            )
            .await
            .expect("second admin creation should succeed")
            .expect("second admin should exist");
        assert!(repository
            .delete_local_auth_user(&admin.id)
            .await
            .expect("delete with another active admin should succeed"));

        assert!(!repository
            .by_id
            .read()
            .expect("user repository lock")
            .contains_key(&admin.id));
        assert!(!repository
            .auth_by_id
            .read()
            .expect("user repository lock")
            .contains_key(&admin.id));
        assert!(!repository
            .auth_by_identifier
            .read()
            .expect("user repository lock")
            .values()
            .any(|user_id| user_id == &admin.id));
        assert!(!repository
            .sessions_by_id
            .read()
            .expect("user repository lock")
            .values()
            .any(|session| session.user_id == admin.id));
        assert!(!repository
            .oauth_links_by_id
            .read()
            .expect("user repository lock")
            .values()
            .any(|link| link.user_id == admin.id));
        assert!(!repository
            .group_members
            .read()
            .expect("user repository lock")
            .keys()
            .any(|(_, user_id)| user_id == &admin.id));
        assert!(!repository
            .preferences_by_user_id
            .read()
            .expect("user repository lock")
            .contains_key(&admin.id));
        assert!(!repository
            .model_settings_by_user_id
            .read()
            .expect("user repository lock")
            .contains_key(&admin.id));
        assert!(!repository
            .feature_settings_by_user_id
            .read()
            .expect("user repository lock")
            .contains_key(&admin.id));
        assert!(!repository
            .ldap_dn_by_user_id
            .read()
            .expect("user repository lock")
            .contains_key(&admin.id));
        assert!(!repository
            .ldap_username_by_user_id
            .read()
            .expect("user repository lock")
            .contains_key(&admin.id));
        assert!(!repository
            .export_rows
            .read()
            .expect("user repository lock")
            .iter()
            .any(|row| row.id == admin.id));
    }

    #[tokio::test]
    async fn creates_local_auth_users_in_memory() {
        let repository = InMemoryUserReadRepository::default();

        let user = repository
            .create_local_auth_user(
                Some("alice@example.com".to_string()),
                true,
                "alice".to_string(),
                "hash".to_string(),
            )
            .await
            .expect("user create should succeed")
            .expect("user create should return user");
        assert_eq!(user.email.as_deref(), Some("alice@example.com"));
        assert_eq!(user.username, "alice");
        assert_eq!(user.role, "user");
        assert_eq!(user.auth_source, "local");

        let admin = repository
            .create_local_auth_user_with_settings(
                Some("admin@example.com".to_string()),
                true,
                "admin".to_string(),
                "admin-hash".to_string(),
                "admin".to_string(),
                Some(vec!["openai".to_string()]),
                Some(vec!["chat".to_string()]),
                Some(vec!["gpt-4.1".to_string()]),
                Some(10),
            )
            .await
            .expect("admin create should succeed")
            .expect("admin create should return user");
        assert_eq!(admin.role, "admin");
        assert_eq!(admin.allowed_providers, Some(vec!["openai".to_string()]));
        assert_eq!(admin.allowed_api_formats, Some(vec!["chat".to_string()]));
        assert_eq!(admin.allowed_models, Some(vec!["gpt-4.1".to_string()]));
        assert_eq!(
            repository
                .find_user_auth_by_username("admin")
                .await
                .expect("created admin lookup should succeed")
                .expect("created admin should exist")
                .id,
            admin.id
        );
    }

    #[tokio::test]
    async fn provisions_ldap_auth_users_in_memory() {
        let repository = InMemoryUserReadRepository::default();
        let logged_in_at = chrono::Utc::now();

        let created = repository
            .get_or_create_ldap_auth_user(
                "ldap@example.com".to_string(),
                "ldap_user".to_string(),
                Some("cn=ldap-user,dc=example".to_string()),
                Some("ldap_user".to_string()),
                logged_in_at,
            )
            .await
            .expect("ldap create should succeed")
            .expect("ldap create should return user");
        assert!(created.created);
        assert_eq!(created.user.auth_source, "ldap");
        assert_eq!(created.user.email.as_deref(), Some("ldap@example.com"));
        assert_eq!(created.user.username, "ldap_user");

        let existing = repository
            .get_or_create_ldap_auth_user(
                "ldap2@example.com".to_string(),
                "ignored".to_string(),
                Some("cn=ldap-user,dc=example".to_string()),
                Some("ldap_user".to_string()),
                logged_in_at,
            )
            .await
            .expect("ldap update should succeed")
            .expect("ldap update should return user");
        assert!(!existing.created);
        assert_eq!(existing.user.id, created.user.id);
        assert_eq!(existing.user.email.as_deref(), Some("ldap2@example.com"));
    }

    #[tokio::test]
    async fn manages_oauth_users_and_links_in_memory() {
        let repository = InMemoryUserReadRepository::default();
        let now = chrono::Utc::now();
        let user = repository
            .create_oauth_auth_user(
                Some("OAuth@Example.com".to_string()),
                false,
                "oauth_user".to_string(),
                now,
            )
            .await
            .expect("oauth user should create")
            .expect("oauth user should exist");
        assert_eq!(user.auth_source, "oauth");
        assert!(!user.email_verified);
        assert_eq!(
            repository
                .find_active_user_auth_by_email_ci("oauth@example.com")
                .await
                .expect("ci lookup should work")
                .map(|user| user.id),
            Some(user.id.clone())
        );

        repository
            .bind_user_oauth_link(
                &user.id,
                "linuxdo",
                "subject-1",
                Some("alice"),
                Some("alice@example.com"),
                Some(serde_json::json!({"sub": "subject-1"})),
                now,
            )
            .await
            .expect("link should upsert");
        assert_eq!(
            repository
                .find_oauth_link_owner("linuxdo", "subject-1")
                .await
                .expect("owner lookup should work"),
            Some(user.id.clone())
        );
        assert!(repository
            .has_user_oauth_provider_link(&user.id, "linuxdo")
            .await
            .expect("provider link lookup should work"));
        assert_eq!(
            repository
                .list_user_oauth_links(&user.id)
                .await
                .expect("links should list")
                .len(),
            1
        );
        assert!(repository
            .touch_oauth_link(
                "linuxdo",
                "subject-1",
                Some("alice2"),
                None,
                Some(serde_json::json!({"fresh": true})),
                now + chrono::Duration::seconds(10),
            )
            .await
            .expect("link should touch"));
        assert_eq!(
            repository
                .delete_user_oauth_link(&user.id, "linuxdo", false, &["linuxdo".to_string()],)
                .await
                .expect("last link deletion should resolve"),
            DeleteUserOAuthLinkOutcome::LastOAuthBinding
        );
        repository
            .bind_user_oauth_link(
                &user.id,
                "github",
                "subject-2",
                Some("alice"),
                Some("alice@example.com"),
                None,
                now,
            )
            .await
            .expect("second link should upsert");
        assert_eq!(
            repository
                .delete_user_oauth_link(
                    &user.id,
                    "linuxdo",
                    false,
                    &["linuxdo".to_string(), "github".to_string()],
                )
                .await
                .expect("link should delete"),
            DeleteUserOAuthLinkOutcome::Deleted
        );
    }

    #[tokio::test]
    async fn oauth_bind_rejects_unavailable_session_without_creating_link_in_memory() {
        let now = chrono::Utc::now();
        let user = StoredUserAuthRecord::new(
            "oauth-bind-user".to_string(),
            Some("oauth-bind@example.com".to_string()),
            true,
            "oauth-bind-user".to_string(),
            None,
            "user".to_string(),
            "oauth".to_string(),
            None,
            None,
            None,
            true,
            false,
            Some(now),
            None,
        )
        .expect("oauth bind user should build")
        .with_security_version(7)
        .expect("user security version should be valid");
        let valid_session = StoredUserSessionRecord::new(
            "oauth-bind-session".to_string(),
            user.id.clone(),
            "oauth-bind-device".to_string(),
            None,
            StoredUserSessionRecord::hash_refresh_token("oauth-bind-refresh"),
            None,
            None,
            Some(now),
            Some(now + chrono::Duration::hours(1)),
            None,
            None,
            None,
            None,
            Some(now),
            Some(now),
        )
        .expect("oauth bind session should build")
        .with_security_version(7)
        .expect("session security version should be valid");

        let mut revoked_session = valid_session.clone();
        revoked_session.revoked_at = Some(now);
        let mut expired_session = valid_session.clone();
        expired_session.expires_at = Some(now - chrono::Duration::seconds(1));
        let cases = [
            (
                "revoked",
                revoked_session,
                BindUserOAuthLinkSessionExpectation::new(
                    "oauth-bind-session",
                    "oauth-bind-device",
                    7,
                    now,
                )
                .expect("revoked expectation should build"),
            ),
            (
                "expired",
                expired_session,
                BindUserOAuthLinkSessionExpectation::new(
                    "oauth-bind-session",
                    "oauth-bind-device",
                    7,
                    now,
                )
                .expect("expired expectation should build"),
            ),
            (
                "device-mismatch",
                valid_session.clone(),
                BindUserOAuthLinkSessionExpectation::new(
                    "oauth-bind-session",
                    "other-device",
                    7,
                    now,
                )
                .expect("device expectation should build"),
            ),
            (
                "security-version-mismatch",
                valid_session,
                BindUserOAuthLinkSessionExpectation::new(
                    "oauth-bind-session",
                    "oauth-bind-device",
                    6,
                    now,
                )
                .expect("security expectation should build"),
            ),
        ];

        for (case, session, expectation) in cases {
            let repository = InMemoryUserReadRepository::seed_auth_users([user.clone()])
                .with_user_sessions([session]);
            let subject = format!("subject-{case}");
            assert_eq!(
                repository
                    .bind_user_oauth_link_if_provider_enabled(
                        &user.id,
                        "linuxdo",
                        &subject,
                        None,
                        None,
                        None,
                        now,
                        true,
                        Some(&expectation),
                    )
                    .await
                    .expect("session-bound OAuth bind should resolve"),
                BindUserOAuthLinkOutcome::SessionUnavailable,
                "case {case} should reject",
            );
            assert_eq!(
                repository
                    .count_user_oauth_links(&user.id)
                    .await
                    .expect("OAuth links should count"),
                0,
                "case {case} must not create a link",
            );
        }
    }

    #[tokio::test]
    async fn concurrent_oauth_binds_preserve_single_identity_owner_in_memory() {
        let repository = Arc::new(InMemoryUserReadRepository::default());
        let now = chrono::Utc::now();
        let first_user = repository
            .create_oauth_auth_user(None, false, "bind-first".to_string(), now)
            .await
            .expect("first user should create")
            .expect("first user should exist");
        let second_user = repository
            .create_oauth_auth_user(None, false, "bind-second".to_string(), now)
            .await
            .expect("second user should create")
            .expect("second user should exist");
        let barrier = Arc::new(tokio::sync::Barrier::new(3));

        let first_repository = Arc::clone(&repository);
        let first_barrier = Arc::clone(&barrier);
        let first_id = first_user.id.clone();
        let first = tokio::spawn(async move {
            first_barrier.wait().await;
            first_repository
                .bind_user_oauth_link(
                    &first_id,
                    "linuxdo",
                    "shared-subject",
                    None,
                    None,
                    None,
                    now,
                )
                .await
                .expect("first bind should resolve")
        });
        let second_repository = Arc::clone(&repository);
        let second_barrier = Arc::clone(&barrier);
        let second_id = second_user.id.clone();
        let second = tokio::spawn(async move {
            second_barrier.wait().await;
            second_repository
                .bind_user_oauth_link(
                    &second_id,
                    "linuxdo",
                    "shared-subject",
                    None,
                    None,
                    None,
                    now,
                )
                .await
                .expect("second bind should resolve")
        });
        barrier.wait().await;
        let outcomes = [
            first.await.expect("first bind task should join"),
            second.await.expect("second bind task should join"),
        ];

        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == BindUserOAuthLinkOutcome::Bound)
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| {
                    **outcome == BindUserOAuthLinkOutcome::IdentityBoundToAnotherUser
                })
                .count(),
            1
        );
        assert!(repository
            .find_oauth_link_owner("linuxdo", "shared-subject")
            .await
            .expect("identity owner should load")
            .is_some());
    }

    #[tokio::test]
    async fn concurrent_oauth_unbinds_preserve_one_login_method_in_memory() {
        let now = chrono::Utc::now();
        let repository = Arc::new(InMemoryUserReadRepository::default());
        let user = repository
            .create_oauth_auth_user(
                Some("concurrent-oauth@example.com".to_string()),
                true,
                "concurrent-oauth".to_string(),
                now,
            )
            .await
            .expect("oauth user should create")
            .expect("oauth user should exist");
        for (provider_type, subject) in [("linuxdo", "subject-1"), ("github", "subject-2")] {
            repository
                .bind_user_oauth_link(&user.id, provider_type, subject, None, None, None, now)
                .await
                .expect("oauth link should upsert");
        }

        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let first_repository = Arc::clone(&repository);
        let first_barrier = Arc::clone(&barrier);
        let first_user_id = user.id.clone();
        let first = tokio::spawn(async move {
            first_barrier.wait().await;
            first_repository
                .delete_user_oauth_link(
                    &first_user_id,
                    "linuxdo",
                    false,
                    &["linuxdo".to_string(), "github".to_string()],
                )
                .await
                .expect("first unlink should resolve")
        });
        let second_repository = Arc::clone(&repository);
        let second_barrier = Arc::clone(&barrier);
        let second_user_id = user.id.clone();
        let second = tokio::spawn(async move {
            second_barrier.wait().await;
            second_repository
                .delete_user_oauth_link(
                    &second_user_id,
                    "github",
                    false,
                    &["linuxdo".to_string(), "github".to_string()],
                )
                .await
                .expect("second unlink should resolve")
        });
        barrier.wait().await;
        let outcomes = [
            first.await.expect("first unlink task should join"),
            second.await.expect("second unlink task should join"),
        ];

        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == DeleteUserOAuthLinkOutcome::Deleted)
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| **outcome == DeleteUserOAuthLinkOutcome::LastOAuthBinding)
                .count(),
            1
        );
        assert_eq!(
            repository
                .count_user_oauth_links(&user.id)
                .await
                .expect("remaining links should count"),
            1
        );
    }

    #[tokio::test]
    async fn oauth_unbind_in_memory_only_counts_enabled_provider_links() {
        let repository = InMemoryUserReadRepository::default();
        let now = chrono::Utc::now();
        let user = repository
            .create_oauth_auth_user(
                Some("enabled-link@example.com".to_string()),
                true,
                "enabled-link".to_string(),
                now,
            )
            .await
            .expect("oauth user should create")
            .expect("oauth user should exist");
        for (provider_type, subject) in [("linuxdo", "subject-1"), ("github", "subject-2")] {
            repository
                .bind_user_oauth_link(&user.id, provider_type, subject, None, None, None, now)
                .await
                .expect("oauth link should upsert");
        }
        let enabled_provider_types_snapshot = ["linuxdo".to_string()];

        assert_eq!(
            repository
                .delete_user_oauth_link(
                    &user.id,
                    "linuxdo",
                    false,
                    &enabled_provider_types_snapshot,
                )
                .await
                .expect("enabled link deletion should resolve"),
            DeleteUserOAuthLinkOutcome::LastOAuthBinding
        );
        assert_eq!(
            repository
                .delete_user_oauth_link(
                    &user.id,
                    "github",
                    false,
                    &enabled_provider_types_snapshot,
                )
                .await
                .expect("disabled link deletion should resolve"),
            DeleteUserOAuthLinkOutcome::Deleted
        );
        assert!(repository
            .has_user_oauth_provider_link(&user.id, "linuxdo")
            .await
            .expect("enabled provider link lookup should work"));
    }

    #[tokio::test]
    async fn oauth_unbind_in_memory_respects_ldap_exclusive_local_login_policy() {
        let valid_hash = "$2b$12$4qL4tdcsFwVaDTw5Ck3xzu8GpNdre56DiNR6Dnw7t6gCXaEnqAe7G".to_string();
        let user = StoredUserAuthRecord::new(
            "ldap-exclusive-local".to_string(),
            Some("ldap-exclusive-local@example.com".to_string()),
            true,
            "ldap-exclusive-local".to_string(),
            Some(valid_hash),
            "user".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            None,
            None,
        )
        .expect("local user should build");
        let repository = InMemoryUserReadRepository::seed_auth_users([user.clone()]);
        repository
            .bind_user_oauth_link(
                &user.id,
                "linuxdo",
                "subject-1",
                None,
                None,
                None,
                chrono::Utc::now(),
            )
            .await
            .expect("oauth link should upsert");

        assert_eq!(
            repository
                .delete_user_oauth_link(&user.id, "linuxdo", false, &["linuxdo".to_string()],)
                .await
                .expect("unlink should resolve"),
            DeleteUserOAuthLinkOutcome::LastLoginMethod
        );
        assert!(repository
            .has_user_oauth_provider_link(&user.id, "linuxdo")
            .await
            .expect("oauth link lookup should work"));
    }

    #[tokio::test]
    async fn counts_active_admin_auth_users() {
        let valid_hash = "$2b$12$4qL4tdcsFwVaDTw5Ck3xzu8GpNdre56DiNR6Dnw7t6gCXaEnqAe7G".to_string();
        let admin = StoredUserAuthRecord::new(
            "admin-1".to_string(),
            Some("admin@example.com".to_string()),
            true,
            "admin".to_string(),
            Some(valid_hash),
            "admin".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            None,
            None,
        )
        .expect("admin should build");
        let invalid_admin = StoredUserAuthRecord::new(
            "admin-2".to_string(),
            Some("admin2@example.com".to_string()),
            true,
            "admin2".to_string(),
            Some("not-bcrypt".to_string()),
            "admin".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            None,
            None,
        )
        .expect("admin should build");
        let repository = InMemoryUserReadRepository::seed_auth_users(vec![admin, invalid_admin]);

        assert_eq!(
            repository
                .count_active_admin_users()
                .await
                .expect("active admin count should succeed"),
            2
        );
        assert_eq!(
            repository
                .count_active_local_admin_users_with_valid_password()
                .await
                .expect("valid local admin count should succeed"),
            1
        );
    }

    #[tokio::test]
    async fn reads_and_writes_user_preferences_in_memory() {
        let repository = InMemoryUserReadRepository::default();
        let preferences = StoredUserPreferenceRecord {
            user_id: "user-1".to_string(),
            avatar_url: Some("https://example.test/avatar.png".to_string()),
            bio: Some("hello".to_string()),
            default_provider_id: Some("provider-1".to_string()),
            default_provider_name: Some("Provider One".to_string()),
            theme: "dark".to_string(),
            language: "en-US".to_string(),
            timezone: "UTC".to_string(),
            email_notifications: false,
            usage_alerts: true,
            announcement_notifications: false,
        };

        assert!(repository
            .read_user_preferences("user-1")
            .await
            .expect("preferences read should succeed")
            .is_none());
        assert_eq!(
            repository
                .write_user_preferences(&preferences)
                .await
                .expect("preferences write should succeed"),
            Some(preferences.clone())
        );
        assert_eq!(
            repository
                .read_user_preferences("user-1")
                .await
                .expect("preferences read should succeed"),
            Some(preferences)
        );
    }

    #[tokio::test]
    async fn manages_user_sessions_in_memory() {
        let now = chrono::Utc::now();
        let user = StoredUserAuthRecord::new(
            "user-1".to_string(),
            Some("session-user@example.com".to_string()),
            true,
            "session-user".to_string(),
            None,
            "user".to_string(),
            "oauth".to_string(),
            None,
            None,
            None,
            true,
            false,
            Some(now),
            None,
        )
        .expect("session user should build");
        let session = StoredUserSessionRecord::new(
            "session-1".to_string(),
            "user-1".to_string(),
            "device-1".to_string(),
            Some("Laptop".to_string()),
            StoredUserSessionRecord::hash_refresh_token("refresh-1"),
            None,
            None,
            Some(now),
            Some(now + chrono::Duration::hours(1)),
            None,
            None,
            Some("127.0.0.1".to_string()),
            Some("agent".to_string()),
            Some(now),
            Some(now),
        )
        .expect("session should build");
        let repository = InMemoryUserReadRepository::seed_auth_users([user]);

        assert_eq!(
            repository
                .create_user_session(&session)
                .await
                .expect("session should create"),
            Some(session.clone())
        );
        assert_eq!(
            repository
                .list_user_sessions("user-1")
                .await
                .expect("sessions should list")
                .len(),
            1
        );
        assert!(repository
            .touch_user_session(
                "user-1",
                "session-1",
                now + chrono::Duration::minutes(1),
                Some("127.0.0.2"),
                Some("updated-agent"),
            )
            .await
            .expect("session should touch"));
        assert!(repository
            .rotate_user_session_refresh_token(
                "user-1",
                "session-1",
                &StoredUserSessionRecord::hash_refresh_token("refresh-1"),
                &StoredUserSessionRecord::hash_refresh_token("refresh-2"),
                now + chrono::Duration::minutes(2),
                now + chrono::Duration::hours(2),
                None,
                None,
            )
            .await
            .expect("session should rotate"));
        assert!(!repository
            .rotate_user_session_refresh_token(
                "user-1",
                "session-1",
                &StoredUserSessionRecord::hash_refresh_token("refresh-1"),
                &StoredUserSessionRecord::hash_refresh_token("refresh-race-loser"),
                now + chrono::Duration::minutes(2),
                now + chrono::Duration::hours(2),
                None,
                None,
            )
            .await
            .expect("stale session rotation should be rejected"));
        let rotated = repository
            .find_user_session("user-1", "session-1")
            .await
            .expect("session lookup should succeed")
            .expect("session should exist");
        assert_eq!(
            rotated.refresh_token_hash,
            StoredUserSessionRecord::hash_refresh_token("refresh-2")
        );
        assert!(repository
            .revoke_user_session("user-1", "session-1", now, "logout")
            .await
            .expect("session should revoke"));
        assert!(repository
            .list_user_sessions("user-1")
            .await
            .expect("sessions should list")
            .is_empty());
    }

    #[tokio::test]
    async fn security_state_change_revokes_sessions_without_reactivation() {
        let now = chrono::Utc::now();
        let user = StoredUserAuthRecord::new(
            "security-state-user".to_string(),
            Some("security-state@example.com".to_string()),
            true,
            "security-state-user".to_string(),
            None,
            "user".to_string(),
            "oauth".to_string(),
            None,
            None,
            None,
            true,
            false,
            Some(now),
            None,
        )
        .expect("security-state user should build");
        let session = StoredUserSessionRecord::new(
            "security-state-session".to_string(),
            user.id.clone(),
            "security-state-device".to_string(),
            None,
            StoredUserSessionRecord::hash_refresh_token("security-state-refresh"),
            None,
            None,
            Some(now),
            Some(now + chrono::Duration::hours(1)),
            None,
            None,
            None,
            None,
            Some(now),
            Some(now),
        )
        .expect("security-state session should build");
        let repository = InMemoryUserReadRepository::seed_auth_users([user])
            .with_user_sessions([session.clone()]);

        repository
            .update_local_auth_user_admin_fields(
                "security-state-user",
                None,
                false,
                None,
                false,
                None,
                false,
                None,
                false,
                None,
                Some(false),
            )
            .await
            .expect("disable should succeed")
            .expect("user should exist");
        let revoked = repository
            .find_user_session("security-state-user", "security-state-session")
            .await
            .expect("revoked session should load")
            .expect("revoked session should remain stored");
        assert!(revoked.is_revoked());
        assert_eq!(
            revoked.revoke_reason.as_deref(),
            Some("user_security_state_changed")
        );

        repository
            .update_local_auth_user_admin_fields(
                "security-state-user",
                None,
                false,
                None,
                false,
                None,
                false,
                None,
                false,
                None,
                Some(true),
            )
            .await
            .expect("reactivation should succeed")
            .expect("user should exist");
        assert!(repository
            .list_user_sessions("security-state-user")
            .await
            .expect("sessions should list")
            .is_empty());
        assert!(repository
            .create_user_session(&session)
            .await
            .expect("stale login should resolve")
            .is_none());
        let current_version = repository
            .find_user_auth_by_id("security-state-user")
            .await
            .expect("user lookup should resolve")
            .expect("user should exist")
            .security_version;
        let fresh_session = session
            .with_security_version(current_version)
            .expect("security version should be valid");
        assert!(repository
            .create_user_session(&fresh_session)
            .await
            .expect("fresh login should resolve")
            .is_some());

        repository
            .update_local_auth_user_admin_fields(
                "security-state-user",
                Some("audit_admin".to_string()),
                false,
                None,
                false,
                None,
                false,
                None,
                false,
                None,
                None,
            )
            .await
            .expect("role update should succeed")
            .expect("user should exist");
        assert!(repository
            .list_user_sessions("security-state-user")
            .await
            .expect("sessions should list")
            .is_empty());
    }

    #[tokio::test]
    async fn unchanged_security_state_preserves_sessions() {
        let now = chrono::Utc::now();
        let user = StoredUserAuthRecord::new(
            "unchanged-security-user".to_string(),
            None,
            false,
            "unchanged-security-user".to_string(),
            None,
            "user".to_string(),
            "oauth".to_string(),
            None,
            None,
            None,
            true,
            false,
            Some(now),
            None,
        )
        .expect("unchanged-security user should build");
        let session = StoredUserSessionRecord::new(
            "unchanged-security-session".to_string(),
            user.id.clone(),
            "unchanged-security-device".to_string(),
            None,
            StoredUserSessionRecord::hash_refresh_token("unchanged-security-refresh"),
            None,
            None,
            Some(now),
            Some(now + chrono::Duration::hours(1)),
            None,
            None,
            None,
            None,
            Some(now),
            Some(now),
        )
        .expect("unchanged-security session should build");
        let repository =
            InMemoryUserReadRepository::seed_auth_users([user]).with_user_sessions([session]);

        repository
            .update_local_auth_user_admin_fields(
                "unchanged-security-user",
                Some("USER".to_string()),
                false,
                None,
                false,
                None,
                false,
                None,
                false,
                None,
                Some(true),
            )
            .await
            .expect("idempotent security update should succeed")
            .expect("user should exist");
        assert_eq!(
            repository
                .list_user_sessions("unchanged-security-user")
                .await
                .expect("sessions should list")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn password_change_revokes_sessions_and_fences_stale_login() {
        let now = chrono::Utc::now();
        let user = StoredUserAuthRecord::new(
            "user-password-fence".to_string(),
            Some("fence@example.com".to_string()),
            true,
            "fence-user".to_string(),
            Some("old-password-hash".to_string()),
            "user".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            Some(now),
            None,
        )
        .expect("user should build");
        let current = StoredUserSessionRecord::new(
            "session-current".to_string(),
            user.id.clone(),
            "device-current".to_string(),
            None,
            StoredUserSessionRecord::hash_refresh_token("refresh-current"),
            None,
            None,
            Some(now),
            Some(now + chrono::Duration::hours(1)),
            None,
            None,
            None,
            None,
            Some(now),
            Some(now),
        )
        .expect("current session should build");
        let stale_login = StoredUserSessionRecord::new(
            "session-stale-login".to_string(),
            user.id.clone(),
            "device-stale-login".to_string(),
            None,
            StoredUserSessionRecord::hash_refresh_token("refresh-stale"),
            None,
            None,
            Some(now),
            Some(now + chrono::Duration::hours(1)),
            None,
            None,
            None,
            None,
            Some(now),
            Some(now),
        )
        .expect("stale login session should build");
        let repository =
            InMemoryUserReadRepository::seed_auth_users([user]).with_user_sessions([current]);

        assert!(repository
            .change_local_auth_password_and_revoke_sessions(
                "user-password-fence",
                "session-current",
                Some("old-password-hash"),
                "new-password-hash".to_string(),
                now,
            )
            .await
            .expect("password change should succeed"));
        assert!(repository
            .list_user_sessions("user-password-fence")
            .await
            .expect("sessions should list")
            .is_empty());
        assert!(repository
            .create_user_session_if_password_matches(&stale_login, "old-password-hash")
            .await
            .expect("stale login should resolve")
            .is_none());
    }

    #[tokio::test]
    async fn admin_password_reset_revokes_sessions_and_fences_stale_login() {
        let now = chrono::Utc::now();
        let user = StoredUserAuthRecord::new(
            "user-admin-reset-fence".to_string(),
            Some("admin-reset@example.com".to_string()),
            true,
            "admin-reset-user".to_string(),
            Some("old-password-hash".to_string()),
            "user".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            Some(now),
            None,
        )
        .expect("user should build");
        let active = StoredUserSessionRecord::new(
            "session-before-admin-reset".to_string(),
            user.id.clone(),
            "device-before-admin-reset".to_string(),
            None,
            StoredUserSessionRecord::hash_refresh_token("refresh-before-admin-reset"),
            None,
            None,
            Some(now),
            Some(now + chrono::Duration::hours(1)),
            None,
            None,
            None,
            None,
            Some(now),
            Some(now),
        )
        .expect("active session should build");
        let stale_login = StoredUserSessionRecord::new(
            "session-stale-admin-reset-login".to_string(),
            user.id.clone(),
            "device-stale-admin-reset-login".to_string(),
            None,
            StoredUserSessionRecord::hash_refresh_token("refresh-stale-admin-reset"),
            None,
            None,
            Some(now),
            Some(now + chrono::Duration::hours(1)),
            None,
            None,
            None,
            None,
            Some(now),
            Some(now),
        )
        .expect("stale login session should build");
        let repository =
            InMemoryUserReadRepository::seed_auth_users([user]).with_user_sessions([active]);

        assert!(repository
            .reset_local_auth_user_password_and_revoke_sessions(
                "user-admin-reset-fence",
                "new-password-hash".to_string(),
                now,
            )
            .await
            .expect("admin password reset should succeed"));
        assert!(repository
            .list_user_sessions("user-admin-reset-fence")
            .await
            .expect("sessions should list")
            .is_empty());
        assert!(repository
            .create_user_session_if_password_matches(&stale_login, "old-password-hash")
            .await
            .expect("stale login should resolve")
            .is_none());
        assert_eq!(
            repository
                .find_user_auth_by_id("user-admin-reset-fence")
                .await
                .expect("user lookup should succeed")
                .expect("user should exist")
                .password_hash
                .as_deref(),
            Some("new-password-hash")
        );
    }

    #[tokio::test]
    async fn paginates_export_users_in_memory() {
        let repository = InMemoryUserReadRepository::seed_export_users(vec![
            StoredUserExportRow::new(
                "user-1".to_string(),
                Some("alice@example.com".to_string()),
                true,
                "alice".to_string(),
                Some("hash".to_string()),
                "user".to_string(),
                "local".to_string(),
                None,
                None,
                None,
                Some(60),
                None,
                true,
            )
            .expect("user export row should build"),
            StoredUserExportRow::new(
                "user-2".to_string(),
                Some("bob@example.com".to_string()),
                true,
                "bob".to_string(),
                Some("hash".to_string()),
                "admin".to_string(),
                "local".to_string(),
                None,
                None,
                None,
                Some(30),
                None,
                true,
            )
            .expect("user export row should build"),
            StoredUserExportRow::new(
                "user-3".to_string(),
                Some("carol@example.com".to_string()),
                true,
                "carol".to_string(),
                Some("hash".to_string()),
                "user".to_string(),
                "local".to_string(),
                None,
                None,
                None,
                Some(10),
                None,
                false,
            )
            .expect("user export row should build"),
        ]);

        let rows = repository
            .list_export_users_page(&UserExportListQuery {
                skip: 0,
                limit: 10,
                role: Some("user".to_string()),
                is_active: Some(true),
                search: None,
                group_id: None,
                ..Default::default()
            })
            .await
            .expect("paged export should succeed");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "user-1");
    }
}
