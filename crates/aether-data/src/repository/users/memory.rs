use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;

use super::types::{
    normalize_user_group_name, LdapAuthUserProvisioningOutcome, StoredUserAuthRecord,
    StoredUserExportRow, StoredUserGroup, StoredUserGroupMember, StoredUserGroupMembership,
    StoredUserOAuthLinkSummary, StoredUserPreferenceRecord, StoredUserSessionRecord,
    StoredUserSummary, UpsertUserGroupRecord, UserExportListQuery, UserExportSummary,
    UserReadRepository,
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

fn looks_like_bcrypt_hash(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.len() == 60
        && matches!(value.get(0..4), Some("$2a$") | Some("$2b$") | Some("$2y$"))
        && bytes.get(4).is_some_and(u8::is_ascii_digit)
        && bytes.get(5).is_some_and(u8::is_ascii_digit)
        && bytes.get(6) == Some(&b'$')
}

fn normalize_optional_json_value(value: Option<serde_json::Value>) -> Option<serde_json::Value> {
    match value {
        Some(serde_json::Value::Null) | None => None,
        Some(value) => Some(value),
    }
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
        let mut rows = self
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
            let member_ids = self
                .group_members
                .read()
                .expect("user repository lock")
                .keys()
                .filter_map(|(candidate_group_id, user_id)| {
                    (candidate_group_id == group_id).then(|| user_id.clone())
                })
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
        Ok(rows
            .into_iter()
            .skip(query.skip)
            .take(query.limit)
            .collect())
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
        Ok(self
            .export_rows
            .read()
            .expect("user repository lock")
            .iter()
            .find(|row| row.id == user_id)
            .cloned())
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
        username: String,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        if self.read_only {
            return Ok(None);
        }

        let user = StoredUserAuthRecord::new(
            uuid::Uuid::new_v4().to_string(),
            email,
            true,
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

    async fn upsert_user_oauth_link(
        &self,
        user_id: &str,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<serde_json::Value>,
        linked_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), DataLayerError> {
        if self.read_only {
            return Ok(());
        }

        let provider_type = provider_type.trim().to_string();
        let provider_user_id = provider_user_id.trim().to_string();
        let mut links = self
            .oauth_links_by_id
            .write()
            .expect("user repository lock");
        if let Some(link) = links
            .values_mut()
            .find(|link| link.user_id == user_id && link.provider_type == provider_type)
        {
            link.provider_user_id = provider_user_id;
            link.provider_username = provider_username.map(ToOwned::to_owned);
            link.provider_email = provider_email.map(ToOwned::to_owned);
            link.extra_data = extra_data;
            link.last_login_at = Some(linked_at);
            return Ok(());
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
        Ok(())
    }

    async fn delete_user_oauth_link(
        &self,
        user_id: &str,
        provider_type: &str,
    ) -> Result<bool, DataLayerError> {
        if self.read_only {
            return Ok(false);
        }

        let provider_type = provider_type.trim();
        let mut links = self
            .oauth_links_by_id
            .write()
            .expect("user repository lock");
        let before = links.len();
        links.retain(|_, link| !(link.user_id == user_id && link.provider_type == provider_type));
        Ok(links.len() != before)
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
        email: Option<String>,
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
        if let Some(email) = email {
            user.email = Some(email);
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
        Ok(Some(user.clone()))
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
        let Some(user) = auth_by_id.get_mut(user_id) else {
            return Ok(None);
        };
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
        let updated = user.clone();
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

        let removed = self
            .auth_by_id
            .write()
            .expect("user repository lock")
            .remove(user_id);
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
        previous_refresh_token_hash: &str,
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
        let Some(session) = sessions
            .get_mut(session_id)
            .filter(|s| s.user_id == user_id)
        else {
            return Ok(false);
        };
        session.prev_refresh_token_hash = Some(previous_refresh_token_hash.to_string());
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
                        .is_some_and(looks_like_bcrypt_hash)
            })
            .count() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::users::{UserExportListQuery, UserReadRepository};

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
                Some("alice2@example.com".to_string()),
                Some("alice2".to_string()),
            )
            .await
            .expect("profile update should succeed")
            .expect("profile update should return user");
        assert_eq!(updated.email.as_deref(), Some("alice2@example.com"));
        assert_eq!(updated.username, "alice2");
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
            .update_local_auth_user_profile("missing-user", None, None)
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
                "oauth_user".to_string(),
                now,
            )
            .await
            .expect("oauth user should create")
            .expect("oauth user should exist");
        assert_eq!(user.auth_source, "oauth");
        assert_eq!(
            repository
                .find_active_user_auth_by_email_ci("oauth@example.com")
                .await
                .expect("ci lookup should work")
                .map(|user| user.id),
            Some(user.id.clone())
        );

        repository
            .upsert_user_oauth_link(
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
        assert!(repository
            .delete_user_oauth_link(&user.id, "linuxdo")
            .await
            .expect("link should delete"));
    }

    #[tokio::test]
    async fn counts_active_admin_auth_users() {
        let valid_hash = format!("$2b$12${}", "a".repeat(53));
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
        let repository = InMemoryUserReadRepository::default();

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
            })
            .await
            .expect("paged export should succeed");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "user-1");
    }
}
