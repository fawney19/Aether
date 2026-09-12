use std::collections::BTreeMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::DataLayerError;
use aether_data_contracts::repository::management_tokens::{
    ActivateManagementTokenIfMatches, CreateManagementTokenRecord, ManagementTokenListQuery,
    ManagementTokenReadRepository, ManagementTokenWriteRepository, RegenerateManagementTokenSecret,
    StoredManagementToken, StoredManagementTokenListPage, StoredManagementTokenWithUser,
    UpdateManagementTokenRecord,
};

#[derive(Debug, Default)]
pub struct InMemoryManagementTokenRepository {
    items: RwLock<Vec<StoredManagementTokenWithUser>>,
    hashes: RwLock<BTreeMap<String, String>>,
}

impl InMemoryManagementTokenRepository {
    pub fn seed<I>(items: I) -> Self
    where
        I: IntoIterator<Item = StoredManagementTokenWithUser>,
    {
        Self {
            items: RwLock::new(items.into_iter().collect()),
            hashes: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn seed_with_hashes<I, J>(items: I, hashes: J) -> Self
    where
        I: IntoIterator<Item = StoredManagementTokenWithUser>,
        J: IntoIterator<Item = (String, String)>,
    {
        Self {
            items: RwLock::new(items.into_iter().collect()),
            hashes: RwLock::new(hashes.into_iter().collect()),
        }
    }

    fn now_unix_secs() -> Option<u64> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
    }

    fn remove_hash_for_token(hashes: &mut BTreeMap<String, String>, token_id: &str) {
        hashes.retain(|_, existing_token_id| existing_token_id != token_id);
    }

    fn update_management_token_scoped(
        &self,
        record: &UpdateManagementTokenRecord,
        expected_user_id: Option<&str>,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        record.validate()?;

        let mut items = self
            .items
            .write()
            .expect("management token repository lock");
        let Some(index) = items.iter().position(|item| {
            item.token.id == record.token_id
                && expected_user_id
                    .map(|user_id| item.token.user_id == user_id)
                    .unwrap_or(true)
        }) else {
            return Ok(None);
        };

        if let Some(name) = &record.name {
            if items.iter().enumerate().any(|(position, item)| {
                position != index
                    && item.token.user_id == items[index].token.user_id
                    && item.token.name == *name
            }) {
                return Err(DataLayerError::InvalidInput(format!(
                    "已存在名为 '{}' 的 Token",
                    name
                )));
            }
            items[index].token.name = name.clone();
        }

        if record.clear_description {
            items[index].token.description = None;
        } else if let Some(description) = &record.description {
            items[index].token.description = Some(description.clone());
        }

        if record.clear_allowed_ips {
            items[index].token.allowed_ips = None;
        } else if let Some(allowed_ips) = &record.allowed_ips {
            items[index].token.allowed_ips = Some(allowed_ips.clone());
        }

        if let Some(permissions) = &record.permissions {
            items[index].token.permissions = Some(permissions.clone());
        }

        if record.clear_expires_at {
            items[index].token.expires_at_unix_secs = None;
        } else if let Some(expires_at_unix_secs) = record.expires_at_unix_secs {
            items[index].token.expires_at_unix_secs = Some(expires_at_unix_secs);
        }

        if let Some(is_active) = record.is_active {
            items[index].token.is_active = is_active;
        }

        items[index].token.updated_at_unix_secs = Self::now_unix_secs();
        Ok(Some(items[index].token.clone()))
    }

    fn delete_management_token_scoped(
        &self,
        token_id: &str,
        expected_user_id: Option<&str>,
    ) -> bool {
        let mut items = self
            .items
            .write()
            .expect("management token repository lock");
        let mut hashes = self
            .hashes
            .write()
            .expect("management token repository lock");
        let original_len = items.len();
        items.retain(|item| {
            item.token.id != token_id
                || expected_user_id
                    .map(|user_id| item.token.user_id != user_id)
                    .unwrap_or(false)
        });
        if items.len() != original_len {
            Self::remove_hash_for_token(&mut hashes, token_id);
            return true;
        }
        false
    }

    fn set_management_token_active_scoped(
        &self,
        token_id: &str,
        expected_user_id: Option<&str>,
        is_active: bool,
    ) -> Option<StoredManagementToken> {
        let mut items = self
            .items
            .write()
            .expect("management token repository lock");
        let item = items.iter_mut().find(|item| {
            item.token.id == token_id
                && expected_user_id
                    .map(|user_id| item.token.user_id == user_id)
                    .unwrap_or(true)
        })?;
        item.token.is_active = is_active;
        item.token.updated_at_unix_secs = Self::now_unix_secs();
        Some(item.token.clone())
    }

    fn activate_management_token_if_matches_inner(
        &self,
        mutation: &ActivateManagementTokenIfMatches,
    ) -> Result<bool, DataLayerError> {
        mutation.validate()?;
        // The in-memory token store does not own the independently stored user row and cannot
        // atomically verify role/status/security_version with this mutation. Pretending that the
        // user summary cached beside the token is authoritative would recreate the TOCTOU, so
        // one-time install activation is intentionally unavailable on this backend.
        Ok(false)
    }

    fn delete_inactive_management_token_if_matches_inner(
        &self,
        mutation: &ActivateManagementTokenIfMatches,
    ) -> Result<bool, DataLayerError> {
        mutation.validate()?;

        let mut items = self
            .items
            .write()
            .expect("management token repository lock");
        let mut hashes = self
            .hashes
            .write()
            .expect("management token repository lock");
        if hashes.get(&mutation.token_hash).map(String::as_str)
            != Some(mutation.expected_token.id.as_str())
        {
            return Ok(false);
        }
        let Some(index) = items.iter().position(|item| {
            mutation.matches_locked_token_snapshot(&item.token, &mutation.token_hash)
        }) else {
            return Ok(false);
        };
        items.remove(index);
        Self::remove_hash_for_token(&mut hashes, &mutation.expected_token.id);
        Ok(true)
    }

    fn regenerate_management_token_secret_scoped(
        &self,
        mutation: &RegenerateManagementTokenSecret,
        expected_user_id: Option<&str>,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        mutation.validate()?;

        let mut items = self
            .items
            .write()
            .expect("management token repository lock");
        let mut hashes = self
            .hashes
            .write()
            .expect("management token repository lock");
        let Some(item) = items.iter_mut().find(|item| {
            item.token.id == mutation.token_id
                && expected_user_id
                    .map(|user_id| item.token.user_id == user_id)
                    .unwrap_or(true)
        }) else {
            return Ok(None);
        };
        Self::remove_hash_for_token(&mut hashes, &mutation.token_id);
        hashes.insert(mutation.token_hash.clone(), mutation.token_id.clone());
        item.token.token_prefix = mutation.token_prefix.clone();
        item.token.updated_at_unix_secs = Self::now_unix_secs();
        Ok(Some(item.token.clone()))
    }
}

#[async_trait]
impl ManagementTokenReadRepository for InMemoryManagementTokenRepository {
    async fn list_management_tokens(
        &self,
        query: &ManagementTokenListQuery,
    ) -> Result<StoredManagementTokenListPage, DataLayerError> {
        let items = self.items.read().expect("management token repository lock");
        let mut filtered = items
            .iter()
            .filter(|item| match query.user_id.as_deref() {
                Some(user_id) => item.token.user_id == user_id,
                None => true,
            })
            .filter(|item| match query.is_active {
                Some(is_active) => item.token.is_active == is_active,
                None => true,
            })
            .cloned()
            .collect::<Vec<_>>();

        filtered.sort_by(|left, right| {
            right
                .token
                .created_at_unix_ms
                .cmp(&left.token.created_at_unix_ms)
                .then_with(|| right.token.id.cmp(&left.token.id))
        });

        let total = filtered.len();
        let items = filtered
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect();
        Ok(StoredManagementTokenListPage { items, total })
    }

    async fn get_management_token_with_user(
        &self,
        token_id: &str,
    ) -> Result<Option<StoredManagementTokenWithUser>, DataLayerError> {
        let items = self.items.read().expect("management token repository lock");
        Ok(items.iter().find(|item| item.token.id == token_id).cloned())
    }

    async fn get_management_token_with_user_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<StoredManagementTokenWithUser>, DataLayerError> {
        let token_id = {
            let hashes = self
                .hashes
                .read()
                .expect("management token repository lock");
            hashes.get(token_hash).cloned()
        };
        let Some(token_id) = token_id else {
            return Ok(None);
        };
        let items = self.items.read().expect("management token repository lock");
        Ok(items.iter().find(|item| item.token.id == token_id).cloned())
    }
}

#[async_trait]
impl ManagementTokenWriteRepository for InMemoryManagementTokenRepository {
    async fn create_management_token(
        &self,
        record: &CreateManagementTokenRecord,
    ) -> Result<StoredManagementToken, DataLayerError> {
        record.validate()?;

        let mut items = self
            .items
            .write()
            .expect("management token repository lock");
        let mut hashes = self
            .hashes
            .write()
            .expect("management token repository lock");
        if items
            .iter()
            .any(|item| item.token.user_id == record.user_id && item.token.name == record.name)
        {
            return Err(DataLayerError::InvalidInput(format!(
                "已存在名为 '{}' 的 Token",
                record.name
            )));
        }

        let now = Self::now_unix_secs();
        let token = StoredManagementToken::new(
            record.id.clone(),
            record.user_id.clone(),
            record.name.clone(),
        )?
        .with_display_fields(
            record.description.clone(),
            record.token_prefix.clone(),
            record.allowed_ips.clone(),
        )
        .with_permissions(record.permissions.clone())
        .with_runtime_fields(record.expires_at_unix_secs, None, None, 0, record.is_active)
        .with_timestamps(now, now);
        items.push(StoredManagementTokenWithUser::new(
            token.clone(),
            record.user.clone(),
        ));
        hashes.insert(record.token_hash.clone(), record.id.clone());
        Ok(token)
    }

    async fn update_management_token(
        &self,
        record: &UpdateManagementTokenRecord,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        self.update_management_token_scoped(record, None)
    }

    async fn update_management_token_for_user(
        &self,
        record: &UpdateManagementTokenRecord,
        user_id: &str,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        self.update_management_token_scoped(record, Some(user_id))
    }

    async fn delete_management_token(&self, token_id: &str) -> Result<bool, DataLayerError> {
        Ok(self.delete_management_token_scoped(token_id, None))
    }

    async fn delete_management_token_for_user(
        &self,
        token_id: &str,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        Ok(self.delete_management_token_scoped(token_id, Some(user_id)))
    }

    async fn set_management_token_active(
        &self,
        token_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        Ok(self.set_management_token_active_scoped(token_id, None, is_active))
    }

    async fn set_management_token_active_for_user(
        &self,
        token_id: &str,
        user_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        Ok(self.set_management_token_active_scoped(token_id, Some(user_id), is_active))
    }

    async fn activate_management_token_if_matches(
        &self,
        mutation: &ActivateManagementTokenIfMatches,
    ) -> Result<bool, DataLayerError> {
        self.activate_management_token_if_matches_inner(mutation)
    }

    async fn delete_inactive_management_token_if_matches(
        &self,
        mutation: &ActivateManagementTokenIfMatches,
    ) -> Result<bool, DataLayerError> {
        self.delete_inactive_management_token_if_matches_inner(mutation)
    }

    async fn regenerate_management_token_secret(
        &self,
        mutation: &RegenerateManagementTokenSecret,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        self.regenerate_management_token_secret_scoped(mutation, None)
    }

    async fn regenerate_management_token_secret_for_user(
        &self,
        mutation: &RegenerateManagementTokenSecret,
        user_id: &str,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        self.regenerate_management_token_secret_scoped(mutation, Some(user_id))
    }

    async fn record_management_token_usage(
        &self,
        token_id: &str,
        last_used_ip: Option<&str>,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        let mut items = self
            .items
            .write()
            .expect("management token repository lock");
        let Some(item) = items.iter_mut().find(|item| item.token.id == token_id) else {
            return Ok(None);
        };
        item.token.last_used_at_unix_secs = Self::now_unix_secs();
        item.token.last_used_ip = last_used_ip.map(ToOwned::to_owned);
        item.token.usage_count = item.token.usage_count.saturating_add(1);
        item.token.updated_at_unix_secs = Self::now_unix_secs();
        Ok(Some(item.token.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryManagementTokenRepository;
    use crate::repository::management_tokens::{
        ActivateManagementTokenIfMatches, CreateManagementTokenRecord, ManagementTokenListQuery,
        ManagementTokenReadRepository, ManagementTokenWriteRepository,
        RegenerateManagementTokenSecret, StoredManagementToken, StoredManagementTokenUserSummary,
        StoredManagementTokenWithUser, UpdateManagementTokenRecord,
    };

    fn sample_token(id: &str, user_id: &str, is_active: bool) -> StoredManagementTokenWithUser {
        let token = StoredManagementToken::new(id.to_string(), user_id.to_string(), id.to_string())
            .expect("token should build")
            .with_runtime_fields(None, None, None, 2, is_active)
            .with_timestamps(Some(1_700_000_000), Some(1_700_000_100));
        let user = StoredManagementTokenUserSummary::new(
            user_id.to_string(),
            Some(format!("{user_id}@example.com")),
            format!("{user_id}-name"),
            "admin".to_string(),
        )
        .expect("user should build");
        StoredManagementTokenWithUser::new(token, user)
    }

    #[tokio::test]
    async fn lists_filters_and_mutates_management_tokens() {
        let repository = InMemoryManagementTokenRepository::seed_with_hashes(
            vec![
                sample_token("token-1", "user-1", true),
                sample_token("token-2", "user-2", false),
            ],
            vec![
                ("hash-1".to_string(), "token-1".to_string()),
                ("hash-2".to_string(), "token-2".to_string()),
            ],
        );

        let page = repository
            .list_management_tokens(&ManagementTokenListQuery {
                user_id: None,
                is_active: Some(true),
                offset: 0,
                limit: 10,
            })
            .await
            .expect("list should succeed");
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].token.id, "token-1");

        let toggled = repository
            .set_management_token_active("token-2", true)
            .await
            .expect("toggle should succeed")
            .expect("token should exist");
        assert!(toggled.is_active);

        let created = repository
            .create_management_token(&CreateManagementTokenRecord {
                id: "token-3".to_string(),
                user_id: "user-1".to_string(),
                user: StoredManagementTokenUserSummary::new(
                    "user-1".to_string(),
                    Some("user-1@example.com".to_string()),
                    "user-1-name".to_string(),
                    "user".to_string(),
                )
                .expect("user should build"),
                token_hash: "hash-3".to_string(),
                token_prefix: Some("ae_1234".to_string()),
                name: "created".to_string(),
                description: Some("created token".to_string()),
                allowed_ips: Some(serde_json::json!(["127.0.0.1"])),
                permissions: Some(serde_json::json!(["admin:usage:read"])),
                expires_at_unix_secs: Some(1_800_000_000),
                is_active: true,
            })
            .await
            .expect("create should succeed");
        assert_eq!(created.name, "created");
        assert_eq!(
            created.permissions,
            Some(serde_json::json!(["admin:usage:read"]))
        );

        let updated = repository
            .update_management_token(&UpdateManagementTokenRecord {
                token_id: "token-3".to_string(),
                name: Some("renamed".to_string()),
                description: None,
                clear_description: true,
                allowed_ips: Some(serde_json::json!(["10.0.0.1"])),
                clear_allowed_ips: false,
                permissions: Some(serde_json::json!(["admin:usage:read", "admin:usage:write"])),
                expires_at_unix_secs: None,
                clear_expires_at: true,
                is_active: Some(false),
            })
            .await
            .expect("update should succeed")
            .expect("token should exist");
        assert_eq!(updated.name, "renamed");
        assert_eq!(updated.description, None);
        assert_eq!(updated.allowed_ips, Some(serde_json::json!(["10.0.0.1"])));
        assert_eq!(
            updated.permissions,
            Some(serde_json::json!(["admin:usage:read", "admin:usage:write"]))
        );
        assert_eq!(updated.expires_at_unix_secs, None);
        assert!(!updated.is_active);

        let regenerated = repository
            .regenerate_management_token_secret(&RegenerateManagementTokenSecret {
                token_id: "token-3".to_string(),
                token_hash: "hash-3b".to_string(),
                token_prefix: Some("ae_5678".to_string()),
            })
            .await
            .expect("regenerate should succeed")
            .expect("token should exist");
        assert_eq!(regenerated.token_prefix.as_deref(), Some("ae_5678"));

        let by_hash = repository
            .get_management_token_with_user_by_hash("hash-3b")
            .await
            .expect("lookup by hash should succeed")
            .expect("token should exist");
        assert_eq!(by_hash.token.id, "token-3");
        assert_eq!(
            by_hash.token.permissions,
            Some(serde_json::json!(["admin:usage:read", "admin:usage:write"]))
        );

        let used = repository
            .record_management_token_usage("token-3", Some("127.0.0.1"))
            .await
            .expect("usage update should succeed")
            .expect("token should exist");
        assert_eq!(used.last_used_ip.as_deref(), Some("127.0.0.1"));
        assert_eq!(used.usage_count, 1);

        let deleted = repository
            .delete_management_token("token-1")
            .await
            .expect("delete should succeed");
        assert!(deleted);

        let deleted_by_hash = repository
            .get_management_token_with_user_by_hash("hash-1")
            .await
            .expect("hash lookup should succeed");
        assert!(deleted_by_hash.is_none());
    }

    #[tokio::test]
    async fn owner_scoped_mutations_never_cross_user_boundaries() {
        let repository = InMemoryManagementTokenRepository::seed_with_hashes(
            vec![sample_token("token-1", "user-1", true)],
            vec![("hash-1".to_string(), "token-1".to_string())],
        );
        let update = UpdateManagementTokenRecord {
            token_id: "token-1".to_string(),
            name: Some("hijacked".to_string()),
            description: None,
            clear_description: false,
            allowed_ips: None,
            clear_allowed_ips: false,
            permissions: None,
            expires_at_unix_secs: None,
            clear_expires_at: false,
            is_active: None,
        };

        assert!(repository
            .update_management_token_for_user(&update, "user-2")
            .await
            .expect("scoped update should execute")
            .is_none());
        assert!(repository
            .set_management_token_active_for_user("token-1", "user-2", false)
            .await
            .expect("scoped toggle should execute")
            .is_none());
        assert!(repository
            .regenerate_management_token_secret_for_user(
                &RegenerateManagementTokenSecret {
                    token_id: "token-1".to_string(),
                    token_hash: "hash-hijacked".to_string(),
                    token_prefix: Some("ae_hijacked".to_string()),
                },
                "user-2",
            )
            .await
            .expect("scoped regeneration should execute")
            .is_none());
        assert!(!repository
            .delete_management_token_for_user("token-1", "user-2")
            .await
            .expect("scoped delete should execute"));

        let unchanged = repository
            .get_management_token_with_user_by_hash("hash-1")
            .await
            .expect("original hash lookup should succeed")
            .expect("token should remain");
        assert_eq!(unchanged.token.name, "token-1");
        assert!(unchanged.token.is_active);
        assert!(repository
            .get_management_token_with_user_by_hash("hash-hijacked")
            .await
            .expect("replacement hash lookup should succeed")
            .is_none());
    }

    #[tokio::test]
    async fn install_activation_fails_closed_without_atomic_user_state() {
        let mut pending = sample_token("token-1", "user-1", false);
        pending.token.allowed_ips = Some(serde_json::json!(["127.0.0.1"]));
        pending.token.permissions = Some(serde_json::json!(["admin:proxy_nodes:write"]));
        pending.token.expires_at_unix_secs = Some(1_800_000_000);
        let expected_token = pending.token.clone();
        let repository = InMemoryManagementTokenRepository::seed_with_hashes(
            [pending],
            [("hash-1".to_string(), "token-1".to_string())],
        );
        let expected = ActivateManagementTokenIfMatches {
            expected_token,
            token_hash: "hash-1".to_string(),
            expected_user_security_version: 4,
            now_unix_secs: 1_700_000_000,
        };

        let mut mismatched = expected.clone();
        mismatched.expected_token.permissions =
            Some(serde_json::json!(["admin:proxy_nodes:admin"]));
        assert!(!repository
            .activate_management_token_if_matches(&mismatched)
            .await
            .expect("mismatched activation should execute"));
        assert!(!repository
            .activate_management_token_if_matches(&expected)
            .await
            .expect("memory activation should fail closed"));
        assert!(
            !repository
                .get_management_token_with_user("token-1")
                .await
                .expect("token lookup should execute")
                .expect("token should remain")
                .token
                .is_active
        );
        assert!(repository
            .delete_inactive_management_token_if_matches(&expected)
            .await
            .expect("exact inactive snapshot cleanup should execute"));
    }
}
