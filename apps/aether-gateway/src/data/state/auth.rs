use super::{
    AuthApiKeyLookupKey, CreateManagementTokenRecord, DataLayerError, GatewayAuthApiKeySnapshot,
    GatewayDataState, ManagementTokenListQuery, ProxyNodeHeartbeatMutation,
    ProxyNodeManualCreateMutation, ProxyNodeManualUpdateMutation, ProxyNodeRegistrationMutation,
    ProxyNodeRemoteConfigMutation, ProxyNodeTrafficMutation, ProxyNodeTunnelStatusMutation,
    RegenerateManagementTokenSecret, StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
    StoredLdapModuleConfig, StoredManagementToken, StoredManagementTokenListPage,
    StoredManagementTokenWithUser, StoredOAuthProviderConfig, StoredOAuthProviderModuleConfig,
    StoredProxyFleetMetricsBucket, StoredProxyNode, StoredProxyNodeEvent,
    StoredProxyNodeMetricsBucket, StoredUserAuthRecord, StoredUserOAuthLinkSummary,
    StoredUserPreferenceRecord, StoredUserSessionRecord, StoredWalletSnapshot,
    UpdateManagementTokenRecord, UpsertOAuthProviderConfigRecord,
};
use crate::LocalMutationOutcome;
use aether_data::repository::auth::{
    read_resolved_auth_api_key_snapshot_by_key_hash,
    read_resolved_auth_api_key_snapshot_by_user_api_key_ids,
};

impl GatewayDataState {
    pub(crate) async fn is_other_user_auth_email_taken(
        &self,
        email: &str,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(false);
        };
        Ok(repository
            .find_user_auth_by_email(email)
            .await?
            .is_some_and(|user| user.id != user_id))
    }

    pub(crate) async fn is_other_user_auth_username_taken(
        &self,
        username: &str,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(false);
        };
        Ok(repository
            .find_user_auth_by_username(username)
            .await?
            .is_some_and(|user| user.id != user_id))
    }

    pub(crate) async fn find_active_user_auth_by_email_ci(
        &self,
        email: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository.find_active_user_auth_by_email_ci(email).await
    }

    pub(crate) async fn find_user_auth_by_username(
        &self,
        username: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository.find_user_auth_by_username(username).await
    }

    pub(crate) async fn find_user_auth_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.find_user_auth_by_id(user_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn find_user_auth_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.find_user_auth_by_identifier(identifier).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_user_groups(
        &self,
    ) -> Result<Vec<aether_data::repository::users::StoredUserGroup>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.list_user_groups().await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn find_user_group_by_id(
        &self,
        group_id: &str,
    ) -> Result<Option<aether_data::repository::users::StoredUserGroup>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.find_user_group_by_id(group_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_user_groups_by_ids(
        &self,
        group_ids: &[String],
    ) -> Result<Vec<aether_data::repository::users::StoredUserGroup>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.list_user_groups_by_ids(group_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn create_user_group(
        &self,
        record: aether_data::repository::users::UpsertUserGroupRecord,
    ) -> Result<Option<aether_data::repository::users::StoredUserGroup>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.create_user_group(record).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn update_user_group(
        &self,
        group_id: &str,
        record: aether_data::repository::users::UpsertUserGroupRecord,
    ) -> Result<Option<aether_data::repository::users::StoredUserGroup>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.update_user_group(group_id, record).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_user_group(&self, group_id: &str) -> Result<bool, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.delete_user_group(group_id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn list_user_group_members(
        &self,
        group_id: &str,
    ) -> Result<Vec<aether_data::repository::users::StoredUserGroupMember>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.list_user_group_members(group_id).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn replace_user_group_members(
        &self,
        group_id: &str,
        user_ids: &[String],
    ) -> Result<Vec<aether_data::repository::users::StoredUserGroupMember>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => {
                repository
                    .replace_user_group_members(group_id, user_ids)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_user_groups_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<aether_data::repository::users::StoredUserGroup>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.list_user_groups_for_user(user_id).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_user_group_memberships_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<aether_data::repository::users::StoredUserGroupMembership>, DataLayerError>
    {
        match &self.user_reader {
            Some(repository) => {
                repository
                    .list_user_group_memberships_by_user_ids(user_ids)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn replace_user_groups_for_user(
        &self,
        user_id: &str,
        group_ids: &[String],
    ) -> Result<Vec<aether_data::repository::users::StoredUserGroup>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => {
                repository
                    .replace_user_groups_for_user(user_id, group_ids)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn add_user_to_group(
        &self,
        group_id: &str,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.add_user_to_group(group_id, user_id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn list_user_oauth_links(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserOAuthLinkSummary>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(Vec::new());
        };
        repository.list_user_oauth_links(user_id).await
    }

    pub(crate) async fn find_oauth_linked_user(
        &self,
        provider_type: &str,
        provider_user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository
            .find_oauth_linked_user(provider_type, provider_user_id)
            .await
    }

    pub(crate) async fn touch_oauth_link(
        &self,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<serde_json::Value>,
        touched_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(false);
        };
        repository
            .touch_oauth_link(
                provider_type,
                provider_user_id,
                provider_username,
                provider_email,
                extra_data,
                touched_at,
            )
            .await
    }

    pub(crate) async fn create_oauth_auth_user(
        &self,
        email: Option<String>,
        username: String,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository
            .create_oauth_auth_user(email, username, created_at)
            .await
    }

    pub(crate) async fn find_oauth_link_owner(
        &self,
        provider_type: &str,
        provider_user_id: &str,
    ) -> Result<Option<String>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository
            .find_oauth_link_owner(provider_type, provider_user_id)
            .await
    }

    pub(crate) async fn has_user_oauth_provider_link(
        &self,
        user_id: &str,
        provider_type: &str,
    ) -> Result<bool, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(false);
        };
        repository
            .has_user_oauth_provider_link(user_id, provider_type)
            .await
    }

    pub(crate) async fn count_user_oauth_links(
        &self,
        user_id: &str,
    ) -> Result<u64, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(0);
        };
        repository.count_user_oauth_links(user_id).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn upsert_user_oauth_link(
        &self,
        user_id: &str,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<serde_json::Value>,
        linked_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(());
        };
        repository
            .upsert_user_oauth_link(
                user_id,
                provider_type,
                provider_user_id,
                provider_username,
                provider_email,
                extra_data,
                linked_at,
            )
            .await
    }

    pub(crate) async fn delete_user_oauth_link(
        &self,
        user_id: &str,
        provider_type: &str,
    ) -> Result<bool, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(false);
        };
        repository
            .delete_user_oauth_link(user_id, provider_type)
            .await
    }

    pub(crate) async fn read_user_preferences(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserPreferenceRecord>, DataLayerError> {
        if let Some(store) = &self.user_preferences {
            return Ok(store
                .read()
                .expect("user preference store should lock")
                .get(user_id)
                .cloned());
        }

        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository.read_user_preferences(user_id).await
    }

    pub(crate) async fn write_user_preferences(
        &self,
        preferences: &StoredUserPreferenceRecord,
    ) -> Result<Option<StoredUserPreferenceRecord>, DataLayerError> {
        if let Some(store) = &self.user_preferences {
            store
                .write()
                .expect("user preference store should lock")
                .insert(preferences.user_id.clone(), preferences.clone());
            return Ok(Some(preferences.clone()));
        }

        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository.write_user_preferences(preferences).await
    }

    pub(crate) async fn find_active_provider_name(
        &self,
        provider_id: &str,
    ) -> Result<Option<String>, DataLayerError> {
        let providers = self.list_provider_catalog_providers(true).await?;
        Ok(providers
            .into_iter()
            .find(|provider| provider.id == provider_id)
            .map(|provider| provider.name))
    }

    pub(crate) async fn find_user_session(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> Result<Option<StoredUserSessionRecord>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository.find_user_session(user_id, session_id).await
    }

    pub(crate) async fn list_user_sessions(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserSessionRecord>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(Vec::new());
        };
        repository.list_user_sessions(user_id).await
    }

    pub(crate) async fn create_user_session(
        &self,
        session: &StoredUserSessionRecord,
    ) -> Result<Option<StoredUserSessionRecord>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository.create_user_session(session).await
    }

    pub(crate) async fn update_user_model_capability_settings(
        &self,
        user_id: &str,
        settings: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository
            .update_user_model_capability_settings(user_id, settings)
            .await
    }

    pub(crate) async fn update_user_feature_settings(
        &self,
        user_id: &str,
        settings: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository
            .update_user_feature_settings(user_id, settings)
            .await
    }

    pub(crate) async fn update_local_auth_user_profile(
        &self,
        user_id: &str,
        email: Option<String>,
        username: Option<String>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository
            .update_local_auth_user_profile(user_id, email, username)
            .await
    }

    pub(crate) async fn update_local_auth_user_password_hash(
        &self,
        user_id: &str,
        password_hash: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository
            .update_local_auth_user_password_hash(user_id, password_hash, updated_at)
            .await
    }

    #[allow(dead_code)]
    pub(crate) async fn create_local_auth_user(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository
            .create_local_auth_user(email, email_verified, username, password_hash)
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
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository
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
        rate_limit_present: bool,
        rate_limit: Option<i32>,
        is_active: Option<bool>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository
            .update_local_auth_user_admin_fields(
                user_id,
                role,
                allowed_providers_present,
                allowed_providers,
                allowed_api_formats_present,
                allowed_api_formats,
                allowed_models_present,
                allowed_models,
                rate_limit_present,
                rate_limit,
                is_active,
            )
            .await
    }

    pub(crate) async fn update_local_auth_user_policy_modes(
        &self,
        user_id: &str,
        allowed_providers_mode: Option<String>,
        allowed_api_formats_mode: Option<String>,
        allowed_models_mode: Option<String>,
        rate_limit_mode: Option<String>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        repository
            .update_local_auth_user_policy_modes(
                user_id,
                allowed_providers_mode,
                allowed_api_formats_mode,
                allowed_models_mode,
                rate_limit_mode,
            )
            .await
    }

    pub(crate) async fn touch_auth_user_last_login(
        &self,
        user_id: &str,
        logged_in_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(false);
        };
        repository
            .touch_auth_user_last_login(user_id, logged_in_at)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn get_or_create_ldap_auth_user(
        &self,
        email: String,
        username: String,
        ldap_dn: Option<String>,
        ldap_username: Option<String>,
        logged_in_at: chrono::DateTime<chrono::Utc>,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(None);
        };
        let Some(outcome) = repository
            .get_or_create_ldap_auth_user(email, username, ldap_dn, ldap_username, logged_in_at)
            .await?
        else {
            return Ok(None);
        };
        if outcome.created {
            match self
                .initialize_auth_user_wallet(&outcome.user.id, initial_gift_usd, unlimited)
                .await
            {
                Ok(Some(_wallet)) => {}
                Ok(None) => {
                    let _ = self.delete_local_auth_user(&outcome.user.id).await;
                    return Ok(None);
                }
                Err(err) => {
                    let _ = self.delete_local_auth_user(&outcome.user.id).await;
                    return Err(err);
                }
            }
        }
        Ok(Some(outcome.user))
    }

    #[allow(dead_code)]
    pub(crate) async fn initialize_auth_user_wallet(
        &self,
        user_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let Some(repository) = self.wallet_reader.as_ref() else {
            return Ok(None);
        };
        repository
            .initialize_auth_user_wallet(user_id, initial_gift_usd, unlimited)
            .await
    }

    pub(crate) async fn initialize_auth_api_key_wallet(
        &self,
        api_key_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let Some(repository) = self.wallet_reader.as_ref() else {
            return Ok(None);
        };
        repository
            .initialize_auth_api_key_wallet(api_key_id, initial_gift_usd, unlimited)
            .await
    }

    pub(crate) async fn update_auth_user_wallet_limit_mode(
        &self,
        user_id: &str,
        limit_mode: &str,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let Some(repository) = self.wallet_reader.as_ref() else {
            return Ok(None);
        };
        repository
            .update_auth_user_wallet_limit_mode(user_id, limit_mode)
            .await
    }

    pub(crate) async fn update_auth_api_key_wallet_limit_mode(
        &self,
        api_key_id: &str,
        limit_mode: &str,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let Some(repository) = self.wallet_reader.as_ref() else {
            return Ok(None);
        };
        repository
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
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let Some(repository) = self.wallet_reader.as_ref() else {
            return Ok(None);
        };
        repository
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
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let Some(repository) = self.wallet_reader.as_ref() else {
            return Ok(None);
        };
        repository
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

    pub(crate) async fn count_active_admin_users(&self) -> Result<u64, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(0);
        };
        repository.count_active_admin_users().await
    }

    pub(crate) async fn count_active_local_admin_users_with_valid_password(
        &self,
    ) -> Result<u64, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(0);
        };
        repository
            .count_active_local_admin_users_with_valid_password()
            .await
    }

    pub(crate) async fn count_user_pending_refunds(
        &self,
        user_id: &str,
    ) -> Result<u64, DataLayerError> {
        let Some(repository) = self.wallet_reader.as_ref() else {
            return Ok(0);
        };
        repository.count_pending_refunds_by_user_id(user_id).await
    }

    pub(crate) async fn count_user_pending_payment_orders(
        &self,
        user_id: &str,
    ) -> Result<u64, DataLayerError> {
        let Some(repository) = self.wallet_reader.as_ref() else {
            return Ok(0);
        };
        repository
            .count_pending_payment_orders_by_user_id(user_id)
            .await
    }

    pub(crate) async fn delete_local_auth_user(
        &self,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(false);
        };
        repository.delete_local_auth_user(user_id).await
    }

    pub(crate) async fn register_local_auth_user(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<(StoredUserAuthRecord, StoredWalletSnapshot)>, DataLayerError> {
        let Some(user) = self
            .create_local_auth_user(email, email_verified, username, password_hash)
            .await?
        else {
            return Ok(None);
        };

        match self
            .initialize_auth_user_wallet(&user.id, initial_gift_usd, unlimited)
            .await
        {
            Ok(Some(wallet)) => Ok(Some((user, wallet))),
            Ok(None) => {
                let _ = self.delete_local_auth_user(&user.id).await;
                Ok(None)
            }
            Err(err) => {
                let _ = self.delete_local_auth_user(&user.id).await;
                Err(err)
            }
        }
    }

    pub(crate) async fn touch_user_session(
        &self,
        user_id: &str,
        session_id: &str,
        touched_at: chrono::DateTime<chrono::Utc>,
        ip_address: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<bool, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(false);
        };
        repository
            .touch_user_session(user_id, session_id, touched_at, ip_address, user_agent)
            .await
    }

    pub(crate) async fn update_user_session_device_label(
        &self,
        user_id: &str,
        session_id: &str,
        device_label: &str,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(false);
        };
        repository
            .update_user_session_device_label(user_id, session_id, device_label, updated_at)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn rotate_user_session_refresh_token(
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
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(false);
        };
        repository
            .rotate_user_session_refresh_token(
                user_id,
                session_id,
                previous_refresh_token_hash,
                next_refresh_token_hash,
                rotated_at,
                expires_at,
                ip_address,
                user_agent,
            )
            .await
    }

    pub(crate) async fn revoke_user_session(
        &self,
        user_id: &str,
        session_id: &str,
        revoked_at: chrono::DateTime<chrono::Utc>,
        reason: &str,
    ) -> Result<bool, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(false);
        };
        repository
            .revoke_user_session(user_id, session_id, revoked_at, reason)
            .await
    }

    pub(crate) async fn revoke_all_user_sessions(
        &self,
        user_id: &str,
        revoked_at: chrono::DateTime<chrono::Utc>,
        reason: &str,
    ) -> Result<u64, DataLayerError> {
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(0);
        };
        repository
            .revoke_all_user_sessions(user_id, revoked_at, reason)
            .await
    }

    pub(crate) async fn list_enabled_oauth_module_providers(
        &self,
    ) -> Result<Vec<StoredOAuthProviderModuleConfig>, DataLayerError> {
        match &self.auth_module_reader {
            Some(repository) => repository.list_enabled_oauth_providers().await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn get_ldap_module_config(
        &self,
    ) -> Result<Option<StoredLdapModuleConfig>, DataLayerError> {
        match &self.auth_module_reader {
            Some(repository) => repository.get_ldap_config().await,
            None => Ok(None),
        }
    }

    pub(crate) async fn upsert_ldap_module_config(
        &self,
        config: &StoredLdapModuleConfig,
    ) -> Result<Option<StoredLdapModuleConfig>, DataLayerError> {
        match &self.auth_module_writer {
            Some(repository) => repository.upsert_ldap_config(config).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_oauth_provider_configs(
        &self,
    ) -> Result<Vec<StoredOAuthProviderConfig>, DataLayerError> {
        match &self.oauth_provider_reader {
            Some(repository) => repository.list_oauth_provider_configs().await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn get_oauth_provider_config(
        &self,
        provider_type: &str,
    ) -> Result<Option<StoredOAuthProviderConfig>, DataLayerError> {
        match &self.oauth_provider_reader {
            Some(repository) => repository.get_oauth_provider_config(provider_type).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn count_locked_users_if_oauth_provider_disabled(
        &self,
        provider_type: &str,
        ldap_exclusive: bool,
    ) -> Result<usize, DataLayerError> {
        match &self.oauth_provider_reader {
            Some(repository) => {
                repository
                    .count_locked_users_if_provider_disabled(provider_type, ldap_exclusive)
                    .await
            }
            None => Ok(0),
        }
    }

    pub(crate) async fn upsert_oauth_provider_config(
        &self,
        record: &UpsertOAuthProviderConfigRecord,
    ) -> Result<Option<StoredOAuthProviderConfig>, DataLayerError> {
        match &self.oauth_provider_writer {
            Some(repository) => repository
                .upsert_oauth_provider_config(record)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_oauth_provider_config(
        &self,
        provider_type: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.oauth_provider_writer {
            Some(repository) => repository.delete_oauth_provider_config(provider_type).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn list_management_tokens(
        &self,
        query: &ManagementTokenListQuery,
    ) -> Result<StoredManagementTokenListPage, DataLayerError> {
        match &self.management_token_reader {
            Some(repository) => repository.list_management_tokens(query).await,
            None => Ok(StoredManagementTokenListPage {
                items: Vec::new(),
                total: 0,
            }),
        }
    }

    pub(crate) async fn get_management_token_with_user(
        &self,
        token_id: &str,
    ) -> Result<Option<StoredManagementTokenWithUser>, DataLayerError> {
        match &self.management_token_reader {
            Some(repository) => repository.get_management_token_with_user(token_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn get_management_token_with_user_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<StoredManagementTokenWithUser>, DataLayerError> {
        match &self.management_token_reader {
            Some(repository) => {
                repository
                    .get_management_token_with_user_by_hash(token_hash)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn create_management_token(
        &self,
        record: &CreateManagementTokenRecord,
    ) -> Result<LocalMutationOutcome<StoredManagementToken>, DataLayerError> {
        match &self.management_token_writer {
            Some(repository) => match repository.create_management_token(record).await {
                Ok(token) => Ok(LocalMutationOutcome::Applied(token)),
                Err(DataLayerError::InvalidInput(detail)) => {
                    Ok(LocalMutationOutcome::Invalid(detail))
                }
                Err(err) => Err(err),
            },
            None => Ok(LocalMutationOutcome::Unavailable),
        }
    }

    pub(crate) async fn update_management_token(
        &self,
        record: &UpdateManagementTokenRecord,
    ) -> Result<LocalMutationOutcome<StoredManagementToken>, DataLayerError> {
        match &self.management_token_writer {
            Some(repository) => match repository.update_management_token(record).await {
                Ok(Some(token)) => Ok(LocalMutationOutcome::Applied(token)),
                Ok(None) => Ok(LocalMutationOutcome::NotFound),
                Err(DataLayerError::InvalidInput(detail)) => {
                    Ok(LocalMutationOutcome::Invalid(detail))
                }
                Err(err) => Err(err),
            },
            None => Ok(LocalMutationOutcome::Unavailable),
        }
    }

    pub(crate) async fn delete_management_token(
        &self,
        token_id: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.management_token_writer {
            Some(repository) => repository.delete_management_token(token_id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn record_management_token_usage(
        &self,
        token_id: &str,
        last_used_ip: Option<&str>,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        match &self.management_token_writer {
            Some(repository) => {
                repository
                    .record_management_token_usage(token_id, last_used_ip)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn find_proxy_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_reader {
            Some(repository) => repository.find_proxy_node(node_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_proxy_nodes(&self) -> Result<Vec<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_reader {
            Some(repository) => repository.list_proxy_nodes().await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_proxy_node_events(
        &self,
        node_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredProxyNodeEvent>, DataLayerError> {
        match &self.proxy_node_reader {
            Some(repository) => repository.list_proxy_node_events(node_id, limit).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_proxy_node_events_filtered(
        &self,
        node_id: &str,
        query: &super::ProxyNodeEventQuery,
    ) -> Result<Vec<StoredProxyNodeEvent>, DataLayerError> {
        match &self.proxy_node_reader {
            Some(repository) => {
                repository
                    .list_proxy_node_events_filtered(node_id, query)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_proxy_node_metrics(
        &self,
        node_id: &str,
        step: super::ProxyNodeMetricsStep,
        from_unix_secs: u64,
        to_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredProxyNodeMetricsBucket>, DataLayerError> {
        match &self.proxy_node_reader {
            Some(repository) => {
                repository
                    .list_proxy_node_metrics(node_id, step, from_unix_secs, to_unix_secs, limit)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_proxy_fleet_metrics(
        &self,
        step: super::ProxyNodeMetricsStep,
        from_unix_secs: u64,
        to_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredProxyFleetMetricsBucket>, DataLayerError> {
        match &self.proxy_node_reader {
            Some(repository) => {
                repository
                    .list_proxy_fleet_metrics(step, from_unix_secs, to_unix_secs, limit)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn register_proxy_node(
        &self,
        mutation: &ProxyNodeRegistrationMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.register_node(mutation).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn create_manual_proxy_node(
        &self,
        mutation: &ProxyNodeManualCreateMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.create_manual_node(mutation).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn update_manual_proxy_node(
        &self,
        mutation: &ProxyNodeManualUpdateMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.update_manual_node(mutation).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn reset_stale_proxy_node_tunnel_statuses(
        &self,
    ) -> Result<usize, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.reset_stale_tunnel_statuses().await,
            None => Ok(0),
        }
    }

    pub(crate) async fn cleanup_proxy_node_metrics(
        &self,
        retain_1m_from_unix_secs: u64,
        retain_1h_from_unix_secs: u64,
        delete_limit: usize,
    ) -> Result<super::ProxyNodeMetricsCleanupSummary, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => {
                repository
                    .cleanup_proxy_node_metrics(
                        retain_1m_from_unix_secs,
                        retain_1h_from_unix_secs,
                        delete_limit,
                    )
                    .await
            }
            None => Ok(super::ProxyNodeMetricsCleanupSummary::default()),
        }
    }

    pub(crate) async fn apply_proxy_node_heartbeat(
        &self,
        mutation: &ProxyNodeHeartbeatMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.apply_heartbeat(mutation).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn record_proxy_node_traffic(
        &self,
        mutation: &ProxyNodeTrafficMutation,
    ) -> Result<bool, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.record_traffic(mutation).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn update_proxy_node_tunnel_status(
        &self,
        mutation: &ProxyNodeTunnelStatusMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.update_tunnel_status(mutation).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn unregister_proxy_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.unregister_node(node_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_proxy_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.delete_node(node_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn update_proxy_node_remote_config(
        &self,
        mutation: &ProxyNodeRemoteConfigMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.update_remote_config(mutation).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn set_management_token_active(
        &self,
        token_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        match &self.management_token_writer {
            Some(repository) => {
                repository
                    .set_management_token_active(token_id, is_active)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn regenerate_management_token_secret(
        &self,
        mutation: &RegenerateManagementTokenSecret,
    ) -> Result<LocalMutationOutcome<StoredManagementToken>, DataLayerError> {
        match &self.management_token_writer {
            Some(repository) => match repository
                .regenerate_management_token_secret(mutation)
                .await
            {
                Ok(Some(token)) => Ok(LocalMutationOutcome::Applied(token)),
                Ok(None) => Ok(LocalMutationOutcome::NotFound),
                Err(DataLayerError::InvalidInput(detail)) => {
                    Ok(LocalMutationOutcome::Invalid(detail))
                }
                Err(err) => Err(err),
            },
            None => Ok(LocalMutationOutcome::Unavailable),
        }
    }

    pub(in crate::data) async fn find_auth_api_key_snapshot(
        &self,
        key: AuthApiKeyLookupKey<'_>,
    ) -> Result<Option<StoredAuthApiKeySnapshot>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => repository.find_api_key_snapshot(key).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_auth_api_key_snapshots_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeySnapshot>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => repository.list_api_key_snapshots_by_ids(api_key_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_auth_api_key_export_records_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => repository.list_export_api_keys_by_user_ids(user_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_auth_api_key_export_records_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => repository.list_export_api_keys_by_ids(api_key_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn read_auth_api_key_feature_settings(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_standalone: bool,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        if is_standalone {
            return Ok(self
                .find_auth_api_key_export_standalone_record_by_id(api_key_id)
                .await?
                .and_then(|record| record.feature_settings));
        }

        Ok(self
            .list_auth_api_key_export_records_by_ids(&[api_key_id.to_string()])
            .await?
            .into_iter()
            .find(|record| record.user_id == user_id && !record.is_standalone)
            .and_then(|record| record.feature_settings))
    }

    pub(crate) async fn list_auth_api_key_export_records_by_name_search(
        &self,
        name_search: &str,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => {
                repository
                    .list_export_api_keys_by_name_search(name_search)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_auth_api_key_export_standalone_records_page(
        &self,
        query: &aether_data::repository::auth::StandaloneApiKeyExportListQuery,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => repository.list_export_standalone_api_keys_page(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn count_auth_api_key_export_standalone_records(
        &self,
        is_active: Option<bool>,
    ) -> Result<u64, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => repository.count_export_standalone_api_keys(is_active).await,
            None => Ok(0),
        }
    }

    pub(crate) async fn summarize_auth_api_key_export_records_by_user_ids(
        &self,
        user_ids: &[String],
        now_unix_secs: u64,
    ) -> Result<aether_data::repository::auth::AuthApiKeyExportSummary, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => {
                repository
                    .summarize_export_api_keys_by_user_ids(user_ids, now_unix_secs)
                    .await
            }
            None => Ok(aether_data::repository::auth::AuthApiKeyExportSummary::default()),
        }
    }

    pub(crate) async fn summarize_auth_api_key_export_non_standalone_records(
        &self,
        now_unix_secs: u64,
    ) -> Result<aether_data::repository::auth::AuthApiKeyExportSummary, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => {
                repository
                    .summarize_export_non_standalone_api_keys(now_unix_secs)
                    .await
            }
            None => Ok(aether_data::repository::auth::AuthApiKeyExportSummary::default()),
        }
    }

    pub(crate) async fn list_auth_api_key_export_standalone_records(
        &self,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => repository.list_export_standalone_api_keys().await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn summarize_auth_api_key_export_standalone_records(
        &self,
        now_unix_secs: u64,
    ) -> Result<aether_data::repository::auth::AuthApiKeyExportSummary, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => {
                repository
                    .summarize_export_standalone_api_keys(now_unix_secs)
                    .await
            }
            None => Ok(aether_data::repository::auth::AuthApiKeyExportSummary::default()),
        }
    }

    pub(crate) async fn find_auth_api_key_export_standalone_record_by_id(
        &self,
        api_key_id: &str,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => {
                repository
                    .find_export_standalone_api_key_by_id(api_key_id)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn create_user_api_key(
        &self,
        record: aether_data::repository::auth::CreateUserApiKeyRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => repository.create_user_api_key(record).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn create_standalone_api_key(
        &self,
        record: aether_data::repository::auth::CreateStandaloneApiKeyRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => repository.create_standalone_api_key(record).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn update_user_api_key_basic(
        &self,
        record: aether_data::repository::auth::UpdateUserApiKeyBasicRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => repository.update_user_api_key_basic(record).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn update_standalone_api_key_basic(
        &self,
        record: aether_data::repository::auth::UpdateStandaloneApiKeyBasicRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => repository.update_standalone_api_key_basic(record).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn set_user_api_key_active(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => {
                repository
                    .set_user_api_key_active(user_id, api_key_id, is_active)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn set_standalone_api_key_active(
        &self,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => {
                repository
                    .set_standalone_api_key_active(api_key_id, is_active)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn set_user_api_key_locked(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_locked: bool,
    ) -> Result<bool, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => {
                repository
                    .set_user_api_key_locked(user_id, api_key_id, is_locked)
                    .await
            }
            None => Ok(false),
        }
    }

    pub(crate) async fn set_user_api_key_allowed_providers(
        &self,
        user_id: &str,
        api_key_id: &str,
        allowed_providers: Option<Vec<String>>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => {
                repository
                    .set_user_api_key_allowed_providers(user_id, api_key_id, allowed_providers)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn set_user_api_key_force_capabilities(
        &self,
        user_id: &str,
        api_key_id: &str,
        force_capabilities: Option<serde_json::Value>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => {
                repository
                    .set_user_api_key_force_capabilities(user_id, api_key_id, force_capabilities)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn set_user_api_key_feature_settings(
        &self,
        user_id: &str,
        api_key_id: &str,
        feature_settings: Option<serde_json::Value>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => {
                repository
                    .set_user_api_key_feature_settings(user_id, api_key_id, feature_settings)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn set_standalone_api_key_feature_settings(
        &self,
        api_key_id: &str,
        feature_settings: Option<serde_json::Value>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => {
                repository
                    .set_standalone_api_key_feature_settings(api_key_id, feature_settings)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_user_api_key(
        &self,
        user_id: &str,
        api_key_id: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => repository.delete_user_api_key(user_id, api_key_id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn delete_standalone_api_key(
        &self,
        api_key_id: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => repository.delete_standalone_api_key(api_key_id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn read_auth_api_key_snapshot(
        &self,
        user_id: &str,
        api_key_id: &str,
        now_unix_secs: u64,
    ) -> Result<Option<GatewayAuthApiKeySnapshot>, DataLayerError> {
        let snapshot = read_resolved_auth_api_key_snapshot_by_user_api_key_ids(
            self,
            user_id,
            api_key_id,
            now_unix_secs,
        )
        .await?;
        self.apply_user_group_effective_policies(snapshot).await
    }

    pub(crate) async fn read_auth_api_key_snapshot_by_key_hash(
        &self,
        key_hash: &str,
        now_unix_secs: u64,
    ) -> Result<Option<GatewayAuthApiKeySnapshot>, DataLayerError> {
        let snapshot =
            read_resolved_auth_api_key_snapshot_by_key_hash(self, key_hash, now_unix_secs).await?;
        self.apply_user_group_effective_policies(snapshot).await
    }

    async fn apply_user_group_effective_policies(
        &self,
        snapshot: Option<GatewayAuthApiKeySnapshot>,
    ) -> Result<Option<GatewayAuthApiKeySnapshot>, DataLayerError> {
        let Some(mut snapshot) = snapshot else {
            return Ok(None);
        };
        if snapshot.user_role.eq_ignore_ascii_case("admin") && !snapshot.api_key_is_standalone {
            apply_admin_unrestricted_auth_snapshot(&mut snapshot);
            return Ok(Some(snapshot));
        }
        let Some(repository) = self.user_reader.as_ref() else {
            return Ok(Some(snapshot));
        };
        let Some(user) = repository.find_user_auth_by_id(&snapshot.user_id).await? else {
            return Ok(Some(snapshot));
        };
        if user.role.eq_ignore_ascii_case("admin") && !snapshot.api_key_is_standalone {
            snapshot.user_role = user.role;
            apply_admin_unrestricted_auth_snapshot(&mut snapshot);
            return Ok(Some(snapshot));
        }
        let mut groups = repository
            .list_user_groups_for_user(&snapshot.user_id)
            .await?;
        groups.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut allowed_providers =
            resolve_effective_list_policy(None, "unrestricted", &groups, |group| {
                (
                    &group.allowed_providers_mode,
                    group.allowed_providers.clone(),
                )
            });
        let mut allowed_api_formats =
            resolve_effective_list_policy(None, "unrestricted", &groups, |group| {
                (
                    &group.allowed_api_formats_mode,
                    group.allowed_api_formats.clone(),
                )
            });
        let mut allowed_models =
            resolve_effective_list_policy(None, "unrestricted", &groups, |group| {
                (&group.allowed_models_mode, group.allowed_models.clone())
            });
        let user_rate_limit = resolve_effective_rate_limit_policy(None, "system", &groups);
        if !snapshot.api_key_is_standalone {
            constrain_api_key_list_policy_to_user_policy(
                &mut allowed_providers,
                &mut snapshot.api_key_allowed_providers,
            );
            constrain_api_key_list_policy_to_user_policy(
                &mut allowed_api_formats,
                &mut snapshot.api_key_allowed_api_formats,
            );
            constrain_api_key_list_policy_to_user_policy(
                &mut allowed_models,
                &mut snapshot.api_key_allowed_models,
            );
        }
        snapshot.apply_user_policy(
            allowed_providers,
            allowed_api_formats,
            allowed_models,
            user_rate_limit,
        );
        Ok(Some(snapshot))
    }
}

fn apply_admin_unrestricted_auth_snapshot(snapshot: &mut GatewayAuthApiKeySnapshot) {
    snapshot.user_allowed_providers = None;
    snapshot.user_allowed_api_formats = None;
    snapshot.user_allowed_models = None;
    snapshot.user_rate_limit = None;
    snapshot.api_key_allowed_providers = None;
    snapshot.api_key_allowed_api_formats = None;
    snapshot.api_key_allowed_models = None;
    snapshot.api_key_rate_limit = None;
    snapshot.api_key_concurrent_limit = None;
}

fn resolve_effective_list_policy(
    user_values: Option<Vec<String>>,
    user_mode: &str,
    groups: &[aether_data::repository::users::StoredUserGroup],
    group_field: impl Fn(
        &aether_data::repository::users::StoredUserGroup,
    ) -> (&str, Option<Vec<String>>),
) -> Option<Vec<String>> {
    let group_policy = groups.iter().fold(None, |effective, group| {
        let (mode, values) = group_field(group);
        intersect_list_policies(effective, list_restriction_from_mode(mode, values))
    });
    let user_policy = list_restriction_from_mode(user_mode, user_values);
    intersect_list_policies(group_policy, user_policy)
}

fn list_restriction_from_mode(mode: &str, values: Option<Vec<String>>) -> Option<Vec<String>> {
    match mode {
        "specific" => Some(values.unwrap_or_default()),
        "deny_all" => Some(Vec::new()),
        _ => None,
    }
}

fn resolve_effective_rate_limit_policy(
    user_rate_limit: Option<i32>,
    user_mode: &str,
    groups: &[aether_data::repository::users::StoredUserGroup],
) -> Option<i32> {
    let group_policy = groups.iter().fold(None, |effective, group| {
        intersect_rate_limit_policies(
            effective,
            rate_limit_restriction_from_mode(&group.rate_limit_mode, group.rate_limit),
        )
    });
    let user_policy = rate_limit_restriction_from_mode(user_mode, user_rate_limit);
    rate_limit_policy_value(intersect_rate_limit_policies(group_policy, user_policy))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RateLimitRestriction {
    Unlimited,
    Limited(i32),
}

fn rate_limit_restriction_from_mode(
    mode: &str,
    rate_limit: Option<i32>,
) -> Option<RateLimitRestriction> {
    match mode {
        "custom" => {
            let rate_limit = rate_limit.unwrap_or(0).max(0);
            if rate_limit == 0 {
                Some(RateLimitRestriction::Unlimited)
            } else {
                Some(RateLimitRestriction::Limited(rate_limit))
            }
        }
        _ => None,
    }
}

fn intersect_list_policies(
    left: Option<Vec<String>>,
    right: Option<Vec<String>>,
) -> Option<Vec<String>> {
    match (left, right) {
        (None, None) => None,
        (Some(values), None) | (None, Some(values)) => Some(values),
        (Some(left_values), Some(right_values)) => {
            let right_values = right_values
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>();
            Some(
                left_values
                    .into_iter()
                    .filter(|value| right_values.contains(value))
                    .collect(),
            )
        }
    }
}

fn intersect_rate_limit_policies(
    left: Option<RateLimitRestriction>,
    right: Option<RateLimitRestriction>,
) -> Option<RateLimitRestriction> {
    match (left, right) {
        (None, None) => None,
        (Some(value), None) | (None, Some(value)) => Some(value),
        (Some(RateLimitRestriction::Unlimited), Some(RateLimitRestriction::Unlimited)) => {
            Some(RateLimitRestriction::Unlimited)
        }
        (Some(RateLimitRestriction::Limited(value)), Some(RateLimitRestriction::Unlimited))
        | (Some(RateLimitRestriction::Unlimited), Some(RateLimitRestriction::Limited(value))) => {
            Some(RateLimitRestriction::Limited(value))
        }
        (Some(RateLimitRestriction::Limited(left)), Some(RateLimitRestriction::Limited(right))) => {
            Some(RateLimitRestriction::Limited(left.min(right)))
        }
    }
}

fn rate_limit_policy_value(policy: Option<RateLimitRestriction>) -> Option<i32> {
    match policy {
        None => None,
        Some(RateLimitRestriction::Unlimited) => Some(0),
        Some(RateLimitRestriction::Limited(value)) => Some(value),
    }
}

fn constrain_api_key_list_policy_to_user_policy(
    user_policy: &mut Option<Vec<String>>,
    api_key_policy: &mut Option<Vec<String>>,
) {
    let Some(api_key_values) = api_key_policy.as_ref().filter(|values| !values.is_empty()) else {
        return;
    };
    let Some(user_values) = user_policy.clone() else {
        return;
    };
    let effective = intersect_list_policies(Some(api_key_values.to_vec()), Some(user_values))
        .unwrap_or_default();
    *user_policy = Some(effective.clone());
    *api_key_policy = Some(effective);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use aether_data::repository::auth::{
        InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeyExportRecord,
        StoredAuthApiKeySnapshot,
    };
    use aether_data::repository::users::{
        InMemoryUserReadRepository, StoredUserAuthRecord, StoredUserGroup, UpsertUserGroupRecord,
        UserReadRepository,
    };

    use crate::data::GatewayDataState;

    fn sample_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
        sample_snapshot_with_role(api_key_id, user_id, "user")
    }

    fn sample_snapshot_with_role(
        api_key_id: &str,
        user_id: &str,
        role: &str,
    ) -> StoredAuthApiKeySnapshot {
        StoredAuthApiKeySnapshot::new(
            user_id.to_string(),
            "alice".to_string(),
            Some("alice@example.com".to_string()),
            role.to_string(),
            "local".to_string(),
            true,
            false,
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-5"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(200),
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-5"])),
        )
        .expect("snapshot should build")
    }

    fn sample_auth_user(user_id: &str, role: &str) -> StoredUserAuthRecord {
        StoredUserAuthRecord::new(
            user_id.to_string(),
            Some("alice@example.com".to_string()),
            true,
            "alice".to_string(),
            Some("hash".to_string()),
            role.to_string(),
            "local".to_string(),
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-5"])),
            true,
            false,
            None,
            None,
        )
        .expect("auth user should build")
    }

    fn sample_group(
        id: &str,
        priority: i32,
        allowed_models: Option<Vec<&str>>,
        allowed_models_mode: &str,
        rate_limit: Option<i32>,
        rate_limit_mode: &str,
    ) -> StoredUserGroup {
        StoredUserGroup {
            id: id.to_string(),
            name: id.to_string(),
            normalized_name: id.to_string(),
            description: None,
            priority,
            allowed_providers: None,
            allowed_providers_mode: "unrestricted".to_string(),
            allowed_api_formats: None,
            allowed_api_formats_mode: "unrestricted".to_string(),
            allowed_models: allowed_models.map(|values| {
                values
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            }),
            allowed_models_mode: allowed_models_mode.to_string(),
            rate_limit,
            rate_limit_mode: rate_limit_mode.to_string(),
            created_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn list_policy_intersects_group_and_user_restrictions() {
        let groups = vec![
            sample_group("default", 0, None, "unrestricted", None, "system"),
            sample_group(
                "restricted",
                10,
                Some(vec!["gpt-5", "gpt-4.1"]),
                "specific",
                None,
                "system",
            ),
        ];

        let policy = resolve_effective_list_policy(
            Some(vec!["gpt-4.1".to_string(), "gemini-2.5-pro".to_string()]),
            "specific",
            &groups,
            |group| (&group.allowed_models_mode, group.allowed_models.clone()),
        );

        assert_eq!(policy, Some(vec!["gpt-4.1".to_string()]));
    }

    #[test]
    fn list_policy_intersects_multiple_group_restrictions() {
        let groups = vec![
            sample_group(
                "team-a",
                10,
                Some(vec!["gpt-5", "gpt-4.1"]),
                "specific",
                None,
                "system",
            ),
            sample_group(
                "team-b",
                20,
                Some(vec!["gpt-4.1", "gemini-2.5-pro"]),
                "specific",
                None,
                "system",
            ),
        ];

        let policy = resolve_effective_list_policy(None, "unrestricted", &groups, |group| {
            (&group.allowed_models_mode, group.allowed_models.clone())
        });

        assert_eq!(policy, Some(vec!["gpt-4.1".to_string()]));
    }

    #[test]
    fn user_unrestricted_does_not_bypass_group_restrictions() {
        let groups = vec![sample_group(
            "restricted",
            10,
            Some(vec!["gpt-5"]),
            "specific",
            None,
            "system",
        )];

        let policy = resolve_effective_list_policy(None, "unrestricted", &groups, |group| {
            (&group.allowed_models_mode, group.allowed_models.clone())
        });

        assert_eq!(policy, Some(vec!["gpt-5".to_string()]));
    }

    #[test]
    fn rate_limit_policy_uses_most_restrictive_custom_limit() {
        let groups = vec![sample_group(
            "restricted",
            10,
            None,
            "unrestricted",
            Some(60),
            "custom",
        )];

        assert_eq!(
            resolve_effective_rate_limit_policy(Some(120), "custom", &groups),
            Some(60)
        );
    }

    #[test]
    fn rate_limit_unlimited_does_not_bypass_limited_group() {
        let groups = vec![sample_group(
            "restricted",
            10,
            None,
            "unrestricted",
            Some(60),
            "custom",
        )];

        assert_eq!(
            resolve_effective_rate_limit_policy(Some(0), "custom", &groups),
            Some(60)
        );
    }

    #[test]
    fn api_key_specific_policy_cannot_expand_user_policy() {
        let mut user_policy = Some(vec!["gpt-5".to_string()]);
        let mut api_key_policy = Some(vec!["gpt-4.1".to_string()]);

        constrain_api_key_list_policy_to_user_policy(&mut user_policy, &mut api_key_policy);

        assert_eq!(user_policy, Some(Vec::<String>::new()));
        assert_eq!(api_key_policy, Some(Vec::<String>::new()));
    }

    #[tokio::test]
    async fn admin_non_standalone_snapshot_bypasses_group_and_key_policies() {
        let mut snapshot = sample_snapshot_with_role("key-admin", "admin-1", "admin")
            .with_user_rate_limit(Some(120));
        snapshot.api_key_allowed_providers = Some(vec!["anthropic".to_string()]);
        snapshot.api_key_allowed_api_formats = Some(vec!["anthropic:messages".to_string()]);
        snapshot.api_key_allowed_models = Some(vec!["claude-sonnet-4-5".to_string()]);
        snapshot.api_key_rate_limit = Some(5);
        snapshot.api_key_concurrent_limit = Some(1);

        let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-admin".to_string()),
            snapshot,
        )]));
        let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
            sample_auth_user("admin-1", "admin"),
        ]));
        let group = user_repository
            .create_user_group(UpsertUserGroupRecord {
                name: "Restricted".to_string(),
                description: None,
                priority: 10,
                allowed_providers: Some(vec!["openai".to_string()]),
                allowed_providers_mode: "specific".to_string(),
                allowed_api_formats: Some(vec!["openai:chat".to_string()]),
                allowed_api_formats_mode: "specific".to_string(),
                allowed_models: Some(vec!["gpt-4.1".to_string()]),
                allowed_models_mode: "specific".to_string(),
                rate_limit: Some(1),
                rate_limit_mode: "custom".to_string(),
            })
            .await
            .expect("group should create")
            .expect("group should exist");
        user_repository
            .add_user_to_group(&group.id, "admin-1")
            .await
            .expect("group membership should create");

        let state = GatewayDataState::with_auth_api_key_reader_for_tests(auth_repository)
            .with_user_reader(user_repository);
        let resolved = state
            .read_auth_api_key_snapshot_by_key_hash("hash-admin", 100)
            .await
            .expect("snapshot should resolve")
            .expect("snapshot should exist");

        assert_eq!(resolved.effective_allowed_providers(), None);
        assert_eq!(resolved.effective_allowed_api_formats(), None);
        assert_eq!(resolved.effective_allowed_models(), None);
        assert_eq!(resolved.user_rate_limit, None);
        assert_eq!(resolved.api_key_rate_limit, None);
        assert_eq!(resolved.api_key_concurrent_limit, None);
    }

    #[tokio::test]
    async fn user_personal_policy_fields_are_ignored_when_groups_are_applied() {
        let mut snapshot = sample_snapshot("key-user", "user-1").with_user_rate_limit(Some(200));
        snapshot.api_key_allowed_providers = None;
        snapshot.api_key_allowed_api_formats = None;
        snapshot.api_key_allowed_models = None;

        let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
            Some("hash-user".to_string()),
            snapshot,
        )]));
        let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
            sample_auth_user("user-1", "user"),
        ]));
        let group = user_repository
            .create_user_group(UpsertUserGroupRecord {
                name: "Group Policy".to_string(),
                description: None,
                priority: 10,
                allowed_providers: Some(vec!["anthropic".to_string()]),
                allowed_providers_mode: "specific".to_string(),
                allowed_api_formats: Some(vec!["claude:messages".to_string()]),
                allowed_api_formats_mode: "specific".to_string(),
                allowed_models: Some(vec!["claude-sonnet-4-5".to_string()]),
                allowed_models_mode: "specific".to_string(),
                rate_limit: Some(30),
                rate_limit_mode: "custom".to_string(),
            })
            .await
            .expect("group should create")
            .expect("group should exist");
        user_repository
            .add_user_to_group(&group.id, "user-1")
            .await
            .expect("group membership should create");

        let state = GatewayDataState::with_auth_api_key_reader_for_tests(auth_repository)
            .with_user_reader(user_repository);
        let resolved = state
            .read_auth_api_key_snapshot_by_key_hash("hash-user", 100)
            .await
            .expect("snapshot should resolve")
            .expect("snapshot should exist");

        assert_eq!(
            resolved.effective_allowed_providers(),
            Some(&["anthropic".to_string()][..])
        );
        assert_eq!(
            resolved.effective_allowed_api_formats(),
            Some(&["claude:messages".to_string()][..])
        );
        assert_eq!(
            resolved.effective_allowed_models(),
            Some(&["claude-sonnet-4-5".to_string()][..])
        );
        assert_eq!(resolved.user_rate_limit, Some(30));
    }

    #[tokio::test]
    async fn data_state_lists_auth_api_key_export_records() {
        let repository = Arc::new(
            InMemoryAuthApiKeySnapshotRepository::seed(vec![
                (
                    Some("hash-user".to_string()),
                    sample_snapshot("key-user", "user-1"),
                ),
                (
                    Some("hash-standalone".to_string()),
                    sample_snapshot("key-standalone", "admin-1"),
                ),
            ])
            .with_export_records(vec![
                StoredAuthApiKeyExportRecord::new(
                    "user-1".to_string(),
                    "key-user".to_string(),
                    "hash-user".to_string(),
                    Some("enc-user".to_string()),
                    Some("default".to_string()),
                    None,
                    None,
                    Some(serde_json::json!(["gpt-5"])),
                    Some(60),
                    Some(5),
                    Some(serde_json::json!({"cache_1h": true})),
                    true,
                    Some(200),
                    false,
                    9,
                    0,
                    1.75,
                    false,
                )
                .expect("user export record should build"),
                StoredAuthApiKeyExportRecord::new(
                    "admin-1".to_string(),
                    "key-standalone".to_string(),
                    "hash-standalone".to_string(),
                    Some("enc-standalone".to_string()),
                    Some("standalone".to_string()),
                    None,
                    None,
                    None,
                    None,
                    Some(1),
                    None,
                    true,
                    None,
                    true,
                    2,
                    0,
                    0.5,
                    true,
                )
                .expect("standalone export record should build"),
            ]),
        );

        let state = GatewayDataState::with_auth_api_key_reader_for_tests(repository);

        let user_records = state
            .list_auth_api_key_export_records_by_user_ids(&["user-1".to_string()])
            .await
            .expect("user export records should load");
        assert_eq!(user_records.len(), 1);
        assert_eq!(user_records[0].api_key_id, "key-user");
        assert_eq!(user_records[0].total_requests, 9);

        let selected_records = state
            .list_auth_api_key_export_records_by_ids(&[
                "key-standalone".to_string(),
                "missing".to_string(),
                "key-user".to_string(),
            ])
            .await
            .expect("selected export records should load");
        assert_eq!(selected_records.len(), 2);
        assert_eq!(selected_records[0].api_key_id, "key-standalone");
        assert_eq!(selected_records[1].api_key_id, "key-user");

        let paged_records = state
            .list_auth_api_key_export_standalone_records_page(
                &aether_data::repository::auth::StandaloneApiKeyExportListQuery {
                    skip: 0,
                    limit: 10,
                    is_active: Some(true),
                },
            )
            .await
            .expect("paged standalone export records should load");
        assert_eq!(paged_records.len(), 1);
        assert_eq!(paged_records[0].api_key_id, "key-standalone");
        assert_eq!(
            state
                .count_auth_api_key_export_standalone_records(Some(true))
                .await
                .expect("standalone export count should load"),
            1
        );

        let standalone_records = state
            .list_auth_api_key_export_standalone_records()
            .await
            .expect("standalone export records should load");
        assert_eq!(standalone_records.len(), 1);
        assert_eq!(standalone_records[0].api_key_id, "key-standalone");
        assert!(standalone_records[0].is_standalone);
    }
}
