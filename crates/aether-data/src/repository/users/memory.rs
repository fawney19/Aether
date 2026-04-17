use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;

use super::types::{
    StoredUserAuthRecord, StoredUserExportRow, StoredUserSummary, UserExportListQuery,
    UserExportSummary, UserReadRepository,
};
use crate::DataLayerError;

#[derive(Debug, Default)]
pub struct InMemoryUserReadRepository {
    by_id: RwLock<BTreeMap<String, StoredUserSummary>>,
    auth_by_id: RwLock<BTreeMap<String, StoredUserAuthRecord>>,
    auth_by_identifier: RwLock<BTreeMap<String, String>>,
    export_rows: RwLock<Vec<StoredUserExportRow>>,
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
            export_rows: RwLock::new(Vec::new()),
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
            export_rows: RwLock::new(Vec::new()),
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
            export_rows: RwLock::new(items.into_iter().collect()),
        }
    }

    pub fn with_export_users<I>(self, items: I) -> Self
    where
        I: IntoIterator<Item = StoredUserExportRow>,
    {
        let rows = items.into_iter().collect();
        *self.export_rows.write().expect("user repository lock") = rows;
        self
    }
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
        Ok(self
            .export_rows
            .read()
            .expect("user repository lock")
            .iter()
            .filter(|row| !row.role.eq_ignore_ascii_case("admin"))
            .cloned()
            .collect())
    }

    async fn list_export_users(&self) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        Ok(self
            .export_rows
            .read()
            .expect("user repository lock")
            .clone())
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

        assert_eq!(by_id, Some(user.clone()));
        assert_eq!(by_email, Some(user.clone()));
        assert_eq!(by_username, Some(user));
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
            })
            .await
            .expect("paged export should succeed");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "user-1");
    }
}
