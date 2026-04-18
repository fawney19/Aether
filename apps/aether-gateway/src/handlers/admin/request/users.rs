use super::AdminAppState;
use crate::GatewayError;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn find_user_auth_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<aether_data::repository::users::StoredUserAuthRecord>, GatewayError> {
        self.app.find_user_auth_by_id(user_id).await
    }

    pub(crate) async fn list_users_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<aether_data::repository::users::StoredUserSummary>, GatewayError> {
        self.app.list_users_by_ids(user_ids).await
    }

    pub(crate) async fn resolve_auth_user_summaries_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<
        std::collections::BTreeMap<String, aether_data::repository::users::StoredUserSummary>,
        GatewayError,
    > {
        self.app.resolve_auth_user_summaries_by_ids(user_ids).await
    }

    pub(crate) async fn search_auth_user_summaries_by_username(
        &self,
        username_search: &str,
    ) -> Result<Vec<aether_data::repository::users::StoredUserSummary>, GatewayError> {
        self.app
            .search_auth_user_summaries_by_username(username_search)
            .await
    }

    pub(crate) async fn list_export_users_page(
        &self,
        query: &aether_data::repository::users::UserExportListQuery,
    ) -> Result<Vec<aether_data::repository::users::StoredUserExportRow>, GatewayError> {
        self.app.list_export_users_page(query).await
    }

    pub(crate) async fn find_export_user_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<aether_data::repository::users::StoredUserExportRow>, GatewayError> {
        self.app.find_export_user_by_id(user_id).await
    }

    pub(crate) async fn list_user_auth_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<aether_data::repository::users::StoredUserAuthRecord>, GatewayError> {
        self.app.list_user_auth_by_ids(user_ids).await
    }

    pub(crate) async fn find_user_auth_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Option<aether_data::repository::users::StoredUserAuthRecord>, GatewayError> {
        self.app.find_user_auth_by_identifier(identifier).await
    }

    pub(crate) async fn is_other_user_auth_email_taken(
        &self,
        email: &str,
        user_id: &str,
    ) -> Result<bool, GatewayError> {
        self.app
            .is_other_user_auth_email_taken(email, user_id)
            .await
    }

    pub(crate) async fn is_other_user_auth_username_taken(
        &self,
        username: &str,
        user_id: &str,
    ) -> Result<bool, GatewayError> {
        self.app
            .is_other_user_auth_username_taken(username, user_id)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_local_auth_user_with_settings(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
        role: String,
        allowed_providers: Option<Vec<String>>,
        allowed_api_formats: Option<Vec<String>>,
        allowed_models: Option<Vec<String>>,
        rate_limit: Option<i32>,
    ) -> Result<Option<aether_data::repository::users::StoredUserAuthRecord>, GatewayError> {
        self.app
            .create_local_auth_user_with_settings(
                email,
                email_verified,
                username,
                password_hash,
                role,
                allowed_providers,
                allowed_api_formats,
                allowed_models,
                rate_limit,
            )
            .await
    }

    pub(crate) async fn initialize_auth_user_wallet(
        &self,
        user_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        self.app
            .initialize_auth_user_wallet(user_id, initial_gift_usd, unlimited)
            .await
    }

    pub(crate) async fn initialize_auth_api_key_wallet(
        &self,
        api_key_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        self.app
            .initialize_auth_api_key_wallet(api_key_id, initial_gift_usd, unlimited)
            .await
    }

    pub(crate) async fn update_local_auth_user_profile(
        &self,
        user_id: &str,
        email: Option<String>,
        username: Option<String>,
    ) -> Result<Option<aether_data::repository::users::StoredUserAuthRecord>, GatewayError> {
        self.app
            .update_local_auth_user_profile(user_id, email, username)
            .await
    }

    pub(crate) async fn update_local_auth_user_password_hash(
        &self,
        user_id: &str,
        password_hash: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<aether_data::repository::users::StoredUserAuthRecord>, GatewayError> {
        self.app
            .update_local_auth_user_password_hash(user_id, password_hash, updated_at)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn update_local_auth_user_admin_fields(
        &self,
        user_id: &str,
        role: Option<String>,
        allowed_providers_present: bool,
        allowed_providers: Option<Vec<String>>,
        allowed_api_formats_present: bool,
        allowed_api_formats: Option<Vec<String>>,
        allowed_models_present: bool,
        allowed_models: Option<Vec<String>>,
        rate_limit: Option<i32>,
        is_active: Option<bool>,
    ) -> Result<Option<aether_data::repository::users::StoredUserAuthRecord>, GatewayError> {
        self.app
            .update_local_auth_user_admin_fields(
                user_id,
                role,
                allowed_providers_present,
                allowed_providers,
                allowed_api_formats_present,
                allowed_api_formats,
                allowed_models_present,
                allowed_models,
                rate_limit,
                is_active,
            )
            .await
    }

    pub(crate) async fn update_auth_user_wallet_limit_mode(
        &self,
        user_id: &str,
        limit_mode: &str,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        self.app
            .update_auth_user_wallet_limit_mode(user_id, limit_mode)
            .await
    }

    pub(crate) async fn update_auth_api_key_wallet_limit_mode(
        &self,
        api_key_id: &str,
        limit_mode: &str,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        self.app
            .update_auth_api_key_wallet_limit_mode(api_key_id, limit_mode)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn update_auth_user_wallet_snapshot(
        &self,
        user_id: &str,
        balance: f64,
        gift_balance: f64,
        limit_mode: &str,
        currency: &str,
        status: &str,
        total_recharged: f64,
        total_consumed: f64,
        total_refunded: f64,
        total_adjusted: f64,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        self.app
            .update_auth_user_wallet_snapshot(
                user_id,
                balance,
                gift_balance,
                limit_mode,
                currency,
                status,
                total_recharged,
                total_consumed,
                total_refunded,
                total_adjusted,
                updated_at_unix_secs,
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn update_auth_api_key_wallet_snapshot(
        &self,
        api_key_id: &str,
        balance: f64,
        gift_balance: f64,
        limit_mode: &str,
        currency: &str,
        status: &str,
        total_recharged: f64,
        total_consumed: f64,
        total_refunded: f64,
        total_adjusted: f64,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        self.app
            .update_auth_api_key_wallet_snapshot(
                api_key_id,
                balance,
                gift_balance,
                limit_mode,
                currency,
                status,
                total_recharged,
                total_consumed,
                total_refunded,
                total_adjusted,
                updated_at_unix_secs,
            )
            .await
    }

    pub(crate) async fn count_active_admin_users(&self) -> Result<u64, GatewayError> {
        self.app.count_active_admin_users().await
    }

    pub(crate) async fn update_user_model_capability_settings(
        &self,
        user_id: &str,
        settings: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        self.app
            .update_user_model_capability_settings(user_id, settings)
            .await
    }

    pub(crate) async fn count_user_pending_refunds(
        &self,
        user_id: &str,
    ) -> Result<u64, GatewayError> {
        self.app.count_user_pending_refunds(user_id).await
    }

    pub(crate) async fn count_user_pending_payment_orders(
        &self,
        user_id: &str,
    ) -> Result<u64, GatewayError> {
        self.app.count_user_pending_payment_orders(user_id).await
    }

    pub(crate) async fn delete_local_auth_user(&self, user_id: &str) -> Result<bool, GatewayError> {
        self.app.delete_local_auth_user(user_id).await
    }

    pub(crate) async fn list_user_sessions(
        &self,
        user_id: &str,
    ) -> Result<Vec<crate::GatewayUserSessionView>, GatewayError> {
        self.app.list_user_sessions(user_id).await
    }

    pub(crate) async fn find_user_session(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> Result<Option<crate::GatewayUserSessionView>, GatewayError> {
        self.app.find_user_session(user_id, session_id).await
    }

    pub(crate) async fn revoke_user_session(
        &self,
        user_id: &str,
        session_id: &str,
        revoked_at: chrono::DateTime<chrono::Utc>,
        reason: &str,
    ) -> Result<bool, GatewayError> {
        self.app
            .revoke_user_session(user_id, session_id, revoked_at, reason)
            .await
    }

    pub(crate) async fn revoke_all_user_sessions(
        &self,
        user_id: &str,
        revoked_at: chrono::DateTime<chrono::Utc>,
        reason: &str,
    ) -> Result<u64, GatewayError> {
        self.app
            .revoke_all_user_sessions(user_id, revoked_at, reason)
            .await
    }

    pub(crate) async fn list_auth_api_key_snapshots_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<aether_data::repository::auth::StoredAuthApiKeySnapshot>, GatewayError> {
        self.app
            .data
            .list_auth_api_key_snapshots_by_ids(api_key_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn resolve_auth_api_key_snapshots_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<aether_data::repository::auth::StoredAuthApiKeySnapshot>, GatewayError> {
        self.app
            .resolve_auth_api_key_snapshots_by_ids(api_key_ids)
            .await
    }

    pub(crate) async fn resolve_auth_api_key_names_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, String>, GatewayError> {
        self.app
            .resolve_auth_api_key_names_by_ids(api_key_ids)
            .await
    }

    pub(crate) async fn list_auth_api_key_export_records_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.app
            .data
            .list_auth_api_key_export_records_by_user_ids(user_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_auth_api_key_export_records_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.app
            .data
            .list_auth_api_key_export_records_by_ids(api_key_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_auth_api_key_export_records_by_name_search(
        &self,
        name_search: &str,
    ) -> Result<Vec<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.app
            .list_auth_api_key_export_records_by_name_search(name_search)
            .await
    }

    pub(crate) async fn list_auth_api_key_export_standalone_records_page(
        &self,
        query: &aether_data::repository::auth::StandaloneApiKeyExportListQuery,
    ) -> Result<Vec<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.app
            .list_auth_api_key_export_standalone_records_page(query)
            .await
    }

    pub(crate) async fn count_auth_api_key_export_standalone_records(
        &self,
        is_active: Option<bool>,
    ) -> Result<u64, GatewayError> {
        self.app
            .count_auth_api_key_export_standalone_records(is_active)
            .await
    }

    pub(crate) async fn list_auth_api_key_export_standalone_records(
        &self,
    ) -> Result<Vec<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.app.list_auth_api_key_export_standalone_records().await
    }

    pub(crate) async fn find_auth_api_key_export_standalone_record_by_id(
        &self,
        api_key_id: &str,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.app
            .find_auth_api_key_export_standalone_record_by_id(api_key_id)
            .await
    }

    pub(crate) async fn create_user_api_key(
        &self,
        record: aether_data::repository::auth::CreateUserApiKeyRecord,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.app.create_user_api_key(record).await
    }

    pub(crate) async fn create_standalone_api_key(
        &self,
        record: aether_data::repository::auth::CreateStandaloneApiKeyRecord,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.app.create_standalone_api_key(record).await
    }

    pub(crate) async fn update_user_api_key_basic(
        &self,
        record: aether_data::repository::auth::UpdateUserApiKeyBasicRecord,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.app.update_user_api_key_basic(record).await
    }

    pub(crate) async fn update_standalone_api_key_basic(
        &self,
        record: aether_data::repository::auth::UpdateStandaloneApiKeyBasicRecord,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.app.update_standalone_api_key_basic(record).await
    }

    pub(crate) async fn set_standalone_api_key_active(
        &self,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.app
            .set_standalone_api_key_active(api_key_id, is_active)
            .await
    }

    pub(crate) async fn set_user_api_key_active(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.app
            .set_user_api_key_active(user_id, api_key_id, is_active)
            .await
    }

    pub(crate) async fn set_user_api_key_locked(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_locked: bool,
    ) -> Result<bool, GatewayError> {
        self.app
            .set_user_api_key_locked(user_id, api_key_id, is_locked)
            .await
    }

    pub(crate) async fn set_user_api_key_allowed_providers(
        &self,
        user_id: &str,
        api_key_id: &str,
        allowed_providers: Option<Vec<String>>,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.app
            .set_user_api_key_allowed_providers(user_id, api_key_id, allowed_providers)
            .await
    }

    pub(crate) async fn set_user_api_key_force_capabilities(
        &self,
        user_id: &str,
        api_key_id: &str,
        force_capabilities: Option<serde_json::Value>,
    ) -> Result<Option<aether_data::repository::auth::StoredAuthApiKeyExportRecord>, GatewayError>
    {
        self.app
            .set_user_api_key_force_capabilities(user_id, api_key_id, force_capabilities)
            .await
    }

    pub(crate) async fn delete_user_api_key(
        &self,
        user_id: &str,
        api_key_id: &str,
    ) -> Result<bool, GatewayError> {
        self.app.delete_user_api_key(user_id, api_key_id).await
    }

    pub(crate) async fn delete_standalone_api_key(
        &self,
        api_key_id: &str,
    ) -> Result<bool, GatewayError> {
        self.app.delete_standalone_api_key(api_key_id).await
    }

    pub(crate) async fn list_wallet_snapshots_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        self.app
            .list_wallet_snapshots_by_api_key_ids(api_key_ids)
            .await
    }

    pub(crate) async fn list_wallet_snapshots_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        self.app.list_wallet_snapshots_by_user_ids(user_ids).await
    }

    pub(crate) async fn summarize_usage_total_tokens_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<std::collections::BTreeMap<String, u64>, GatewayError> {
        self.app
            .summarize_usage_total_tokens_by_api_key_ids(api_key_ids)
            .await
    }

    pub(crate) async fn summarize_usage_totals_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<aether_data_contracts::repository::usage::StoredUsageUserTotals>, GatewayError>
    {
        self.app.summarize_usage_totals_by_user_ids(user_ids).await
    }

    pub(crate) async fn list_non_admin_export_users(
        &self,
    ) -> Result<Vec<aether_data::repository::users::StoredUserExportRow>, GatewayError> {
        self.app.list_non_admin_export_users().await
    }
}
