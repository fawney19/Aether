use std::time::Duration;

use crate::cache::{AuthApiKeyFeatureCacheKey, AuthApiKeyIdentityCacheKey};
use crate::{AppState, GatewayError};

const AUTH_API_KEY_RUNTIME_JSON_CACHE_TTL: Duration = Duration::from_secs(30);

impl AppState {
    pub(crate) async fn read_auth_api_key_force_capabilities(
        &self,
        user_id: &str,
        api_key_id: &str,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        let cache_key = AuthApiKeyIdentityCacheKey::new(user_id, api_key_id);
        if cache_key.is_empty() {
            return Ok(None);
        }
        self.auth_api_key_force_capabilities_cache
            .get_or_load(
                cache_key,
                AUTH_API_KEY_RUNTIME_JSON_CACHE_TTL,
                || async move {
                    let value = self
                        .list_auth_api_key_export_records_by_ids(&[api_key_id.to_string()])
                        .await?
                        .into_iter()
                        .find(|record| record.api_key_id == api_key_id && record.user_id == user_id)
                        .and_then(|record| record.force_capabilities);
                    Ok(value)
                },
            )
            .await
    }

    pub(crate) async fn read_auth_api_key_feature_settings(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_standalone: bool,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        let cache_key = AuthApiKeyFeatureCacheKey::new(user_id, api_key_id, is_standalone);
        if cache_key.is_empty() {
            return Ok(None);
        }
        self.auth_api_key_feature_settings_cache
            .get_or_load(
                cache_key,
                AUTH_API_KEY_RUNTIME_JSON_CACHE_TTL,
                || async move {
                    self.data
                        .read_auth_api_key_feature_settings(user_id, api_key_id, is_standalone)
                        .await
                        .map_err(|err| GatewayError::Internal(err.to_string()))
                },
            )
            .await
    }

    pub(crate) async fn list_auth_api_key_export_records_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.data
            .list_auth_api_key_export_records_by_user_ids(user_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_auth_api_key_export_records_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.data
            .list_auth_api_key_export_records_by_ids(api_key_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_auth_api_key_export_records_by_name_search(
        &self,
        name_search: &str,
    ) -> Result<Vec<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.data
            .list_auth_api_key_export_records_by_name_search(name_search)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_auth_api_key_export_standalone_records_page(
        &self,
        query: &aether_data::repository::auth::StandaloneApiKeyExportListQuery,
    ) -> Result<Vec<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.data
            .list_auth_api_key_export_standalone_records_page(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_auth_api_key_export_standalone_records(
        &self,
        is_active: Option<bool>,
    ) -> Result<u64, GatewayError> {
        self.data
            .count_auth_api_key_export_standalone_records(is_active)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_auth_api_key_export_records_by_user_ids(
        &self,
        user_ids: &[String],
        now_unix_secs: u64,
    ) -> Result<aether_data::repository::auth::AuthApiKeyExportSummary, GatewayError> {
        self.data
            .summarize_auth_api_key_export_records_by_user_ids(user_ids, now_unix_secs)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_auth_api_key_export_non_standalone_records(
        &self,
        now_unix_secs: u64,
    ) -> Result<aether_data::repository::auth::AuthApiKeyExportSummary, GatewayError> {
        self.data
            .summarize_auth_api_key_export_non_standalone_records(now_unix_secs)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_auth_api_key_export_standalone_records(
        &self,
    ) -> Result<Vec<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.data
            .list_auth_api_key_export_standalone_records()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_auth_api_key_export_standalone_records(
        &self,
        now_unix_secs: u64,
    ) -> Result<aether_data::repository::auth::AuthApiKeyExportSummary, GatewayError> {
        self.data
            .summarize_auth_api_key_export_standalone_records(now_unix_secs)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_auth_api_key_export_standalone_record_by_id(
        &self,
        api_key_id: &str,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.data
            .find_auth_api_key_export_standalone_record_by_id(api_key_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_non_admin_export_users(
        &self,
    ) -> Result<Vec<aether_data::repository::users::StoredUserExportRow>, GatewayError> {
        self.data
            .list_non_admin_export_users()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_export_users(
        &self,
    ) -> Result<Vec<aether_data::repository::users::StoredUserExportRow>, GatewayError> {
        self.data
            .list_export_users()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn summarize_export_users(
        &self,
    ) -> Result<aether_data::repository::users::UserExportSummary, GatewayError> {
        self.data
            .summarize_export_users()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_export_users_page(
        &self,
        query: &aether_data::repository::users::UserExportListQuery,
    ) -> Result<Vec<aether_data::repository::users::StoredUserExportRow>, GatewayError> {
        self.data
            .list_export_users_page(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_export_users(
        &self,
        query: &aether_data::repository::users::UserExportListQuery,
    ) -> Result<u64, GatewayError> {
        self.data
            .count_export_users(query)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_export_user_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<aether_data::repository::users::StoredUserExportRow>, GatewayError> {
        self.data
            .find_export_user_by_id(user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_user_auth_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<aether_data::repository::users::StoredUserAuthRecord>, GatewayError> {
        self.data
            .list_user_auth_by_ids(user_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn create_user_api_key(
        &self,
        record: aether_data::repository::auth::CreateUserApiKeyRecord,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        #[cfg(test)]
        {
            // Unit-test AppState instances keep users and API keys in separate in-memory
            // repositories.  Bridge them with the authoritative user record only; an unknown,
            // inactive, or deleted user must never be synthesized from the key request.
            let Some(user) = self.find_user_auth_by_id(&record.user_id).await? else {
                return Ok(None);
            };
            if !user.is_active || user.is_deleted {
                return Ok(None);
            }
            self.data
                .synchronize_user_api_key_owner_for_tests(&user)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
        }

        let api_key = self
            .data
            .create_user_api_key(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn create_standalone_api_key(
        &self,
        record: aether_data::repository::auth::CreateStandaloneApiKeyRecord,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        let api_key = self
            .data
            .create_standalone_api_key(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn update_user_api_key_basic(
        &self,
        record: aether_data::repository::auth::UpdateUserApiKeyBasicRecord,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        let api_key = self
            .data
            .update_user_api_key_basic(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn compare_and_swap_api_key_ciphertext(
        &self,
        mutation: &aether_data::repository::auth::CompareAndSwapAuthApiKeyCiphertext,
    ) -> Result<bool, GatewayError> {
        self.data
            .compare_and_swap_api_key_ciphertext(mutation)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn update_user_api_key_basic_if_unlocked(
        &self,
        record: aether_data::repository::auth::UpdateUserApiKeyBasicRecord,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        let api_key = self
            .data
            .update_user_api_key_basic_if_unlocked(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn update_standalone_api_key_basic(
        &self,
        record: aether_data::repository::auth::UpdateStandaloneApiKeyBasicRecord,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        let api_key = self
            .data
            .update_standalone_api_key_basic(record)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn restore_api_key_if_matches(
        &self,
        expected: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
        restored: &aether_data::repository::auth::StoredAuthApiKeyExportRecord,
    ) -> Result<bool, GatewayError> {
        let restored = self
            .data
            .restore_api_key_if_matches(expected, restored)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if restored {
            self.invalidate_auth_context_cache();
        }
        Ok(restored)
    }

    pub(crate) async fn set_user_api_key_active(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        let api_key = self
            .data
            .set_user_api_key_active(user_id, api_key_id, is_active)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn set_user_api_key_active_if_unlocked(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        let api_key = self
            .data
            .set_user_api_key_active_if_unlocked(user_id, api_key_id, is_active)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn set_standalone_api_key_active(
        &self,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        let api_key = self
            .data
            .set_standalone_api_key_active(api_key_id, is_active)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn set_user_api_key_locked(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_locked: bool,
    ) -> Result<bool, GatewayError> {
        let updated = self
            .data
            .set_user_api_key_locked(user_id, api_key_id, is_locked)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if updated {
            self.invalidate_auth_context_cache();
        }
        Ok(updated)
    }

    pub(crate) async fn set_user_api_key_allowed_providers(
        &self,
        user_id: &str,
        api_key_id: &str,
        allowed_providers: Option<Vec<String>>,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        let api_key = self
            .data
            .set_user_api_key_allowed_providers(user_id, api_key_id, allowed_providers)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn set_user_api_key_allowed_providers_if_unlocked(
        &self,
        user_id: &str,
        api_key_id: &str,
        allowed_providers: Option<Vec<String>>,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        let api_key = self
            .data
            .set_user_api_key_allowed_providers_if_unlocked(user_id, api_key_id, allowed_providers)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn set_user_api_key_force_capabilities(
        &self,
        user_id: &str,
        api_key_id: &str,
        force_capabilities: Option<serde_json::Value>,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        let api_key = self
            .data
            .set_user_api_key_force_capabilities(user_id, api_key_id, force_capabilities)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn set_user_api_key_force_capabilities_if_unlocked(
        &self,
        user_id: &str,
        api_key_id: &str,
        force_capabilities: Option<serde_json::Value>,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        let api_key = self
            .data
            .set_user_api_key_force_capabilities_if_unlocked(
                user_id,
                api_key_id,
                force_capabilities,
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn set_user_api_key_feature_settings(
        &self,
        user_id: &str,
        api_key_id: &str,
        feature_settings: Option<serde_json::Value>,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        let api_key = self
            .data
            .set_user_api_key_feature_settings(user_id, api_key_id, feature_settings)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn set_user_api_key_feature_settings_if_unlocked(
        &self,
        user_id: &str,
        api_key_id: &str,
        feature_settings: Option<serde_json::Value>,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        let api_key = self
            .data
            .set_user_api_key_feature_settings_if_unlocked(user_id, api_key_id, feature_settings)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn set_api_key_usage_totals(
        &self,
        api_key_id: &str,
        total_requests: u64,
        total_tokens: u64,
        total_cost_usd: f64,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        let api_key = self
            .data
            .set_api_key_usage_totals(api_key_id, total_requests, total_tokens, total_cost_usd)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn set_standalone_api_key_feature_settings(
        &self,
        api_key_id: &str,
        feature_settings: Option<serde_json::Value>,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        let api_key = self
            .data
            .set_standalone_api_key_feature_settings(api_key_id, feature_settings)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if api_key.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(api_key)
    }

    pub(crate) async fn delete_user_api_key(
        &self,
        user_id: &str,
        api_key_id: &str,
    ) -> Result<bool, GatewayError> {
        let deleted = self
            .data
            .delete_user_api_key(user_id, api_key_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if deleted {
            self.invalidate_auth_context_cache();
        }
        Ok(deleted)
    }

    pub(crate) async fn delete_user_api_key_if_unlocked(
        &self,
        user_id: &str,
        api_key_id: &str,
    ) -> Result<bool, GatewayError> {
        let deleted = self
            .data
            .delete_user_api_key_if_unlocked(user_id, api_key_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if deleted {
            self.invalidate_auth_context_cache();
        }
        Ok(deleted)
    }

    pub(crate) async fn delete_standalone_api_key(
        &self,
        api_key_id: &str,
    ) -> Result<bool, GatewayError> {
        let deleted = self
            .data
            .delete_standalone_api_key(api_key_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if deleted {
            self.invalidate_auth_context_cache();
        }
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_data::repository::auth::{
        AuthApiKeyLookupKey, AuthApiKeyReadRepository, CreateUserApiKeyRecord,
        InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
    };
    use aether_data::repository::users::StoredUserAuthRecord;

    use crate::data::GatewayDataState;
    use crate::AppState;

    fn authoritative_user(
        user_id: &str,
        is_active: bool,
        is_deleted: bool,
    ) -> StoredUserAuthRecord {
        StoredUserAuthRecord::new(
            user_id.to_string(),
            Some(format!("{user_id}@example.com")),
            true,
            format!("owner-{user_id}"),
            Some("server-managed-password-hash".to_string()),
            "admin".to_string(),
            "oauth".to_string(),
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-5"])),
            is_active,
            is_deleted,
            None,
            None,
        )
        .expect("authoritative user should build")
        .with_security_version(41)
        .expect("security version should be valid")
    }

    fn create_record(user_id: &str, api_key_id: &str) -> CreateUserApiKeyRecord {
        CreateUserApiKeyRecord {
            user_id: user_id.to_string(),
            api_key_id: api_key_id.to_string(),
            key_hash: format!("hash-{api_key_id}"),
            key_encrypted: Some(format!("encrypted-{api_key_id}")),
            name: Some("first key".to_string()),
            allowed_providers: Some(vec!["anthropic".to_string()]),
            allowed_api_formats: None,
            allowed_models: None,
            ip_rules: None,
            rate_limit: 0,
            concurrent_limit: None,
            force_capabilities: None,
            feature_settings: None,
            is_active: true,
            expires_at_unix_secs: None,
            auto_delete_on_expiry: false,
            total_requests: 0,
            total_tokens: 0,
            total_cost_usd: 0.0,
        }
    }

    fn state_with_users<I>(
        repository: Arc<InMemoryAuthApiKeySnapshotRepository>,
        users: I,
    ) -> AppState
    where
        I: IntoIterator<Item = StoredUserAuthRecord>,
    {
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(GatewayDataState::with_auth_api_key_repository_for_tests(
                repository,
            ))
            .with_auth_users_for_tests(users)
    }

    #[tokio::test]
    async fn test_gateway_first_user_key_requires_authoritative_active_owner() {
        let unknown_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::default());
        let unknown_state = state_with_users(
            Arc::clone(&unknown_repository),
            Vec::<StoredUserAuthRecord>::new(),
        );
        assert!(unknown_state
            .create_user_api_key(create_record("missing-user", "missing-key"))
            .await
            .expect("unknown owner creation should resolve")
            .is_none());
        assert!(unknown_repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("missing-key"))
            .await
            .expect("unknown key lookup should resolve")
            .is_none());

        for (user_id, is_active, is_deleted) in [
            ("inactive-user", false, false),
            ("deleted-user", true, true),
        ] {
            let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::default());
            let state = state_with_users(
                Arc::clone(&repository),
                [authoritative_user(user_id, is_active, is_deleted)],
            );
            let api_key_id = format!("{user_id}-key");
            assert!(state
                .create_user_api_key(create_record(user_id, &api_key_id))
                .await
                .expect("ineligible owner creation should resolve")
                .is_none());
            assert!(repository
                .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId(&api_key_id))
                .await
                .expect("ineligible key lookup should resolve")
                .is_none());
        }
    }

    #[tokio::test]
    async fn test_gateway_first_user_key_syncs_owner_without_mutating_authority() {
        let stale_owner = StoredAuthApiKeySnapshot::new(
            "active-user".to_string(),
            "request-derived-owner".to_string(),
            None,
            "user".to_string(),
            "local".to_string(),
            false,
            false,
            None,
            None,
            None,
            "ignored-owner-fixture".to_string(),
            None,
            true,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("stale owner fixture should build");
        let repository = Arc::new(
            InMemoryAuthApiKeySnapshotRepository::default().with_owner_snapshots([stale_owner]),
        );
        let authoritative = authoritative_user("active-user", true, false);
        let state = state_with_users(Arc::clone(&repository), [authoritative.clone()]);

        state
            .create_user_api_key(create_record("active-user", "active-key"))
            .await
            .expect("active owner creation should resolve")
            .expect("active authoritative owner should allow its first key");

        let snapshot = repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("active-key"))
            .await
            .expect("created key lookup should resolve")
            .expect("created key should exist");
        assert_eq!(snapshot.user_role, "admin");
        assert!(snapshot.user_is_active);
        assert!(!snapshot.user_is_deleted);
        assert_eq!(
            snapshot.user_allowed_providers,
            Some(vec!["openai".to_string()])
        );
        assert_eq!(
            snapshot.api_key_allowed_providers,
            Some(vec!["anthropic".to_string()])
        );

        let unchanged = state
            .find_user_auth_by_id("active-user")
            .await
            .expect("authoritative owner lookup should resolve")
            .expect("authoritative owner should remain");
        assert_eq!(unchanged, authoritative);
        assert_eq!(unchanged.role, authoritative.role);
        assert_eq!(unchanged.is_active, authoritative.is_active);
        assert_eq!(unchanged.is_deleted, authoritative.is_deleted);
        assert_eq!(unchanged.security_version, 41);
    }
}
