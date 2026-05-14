use crate::{AppState, GatewayError};

impl AppState {
    pub(crate) async fn read_auth_api_key_force_capabilities(
        &self,
        user_id: &str,
        api_key_id: &str,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        Ok(self
            .list_auth_api_key_export_records_by_ids(&[api_key_id.to_string()])
            .await?
            .into_iter()
            .find(|record| record.api_key_id == api_key_id && record.user_id == user_id)
            .and_then(|record| record.force_capabilities))
    }

    pub(crate) async fn read_auth_api_key_feature_settings(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_standalone: bool,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        self.data
            .read_auth_api_key_feature_settings(user_id, api_key_id, is_standalone)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
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
