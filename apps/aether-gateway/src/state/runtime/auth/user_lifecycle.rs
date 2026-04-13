use std::collections::{BTreeMap, BTreeSet};

use crate::{AppState, GatewayError};

impl AppState {
    pub(crate) async fn resolve_auth_user_summaries_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<BTreeMap<String, aether_data::repository::users::StoredUserSummary>, GatewayError>
    {
        let user_ids = user_ids
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if user_ids.is_empty() {
            return Ok(BTreeMap::new());
        }

        let mut users = BTreeMap::new();
        if self.has_user_data_reader() {
            for user in self.list_users_by_ids(&user_ids).await? {
                users.insert(user.id.clone(), user);
            }
        }

        for user_id in &user_ids {
            if users.contains_key(user_id) {
                continue;
            }
            let Some(user) = self.find_user_auth_by_id(user_id).await? else {
                continue;
            };
            let summary = user
                .to_summary()
                .map_err(|err| GatewayError::Internal(err.to_string()))?;
            users.insert(summary.id.clone(), summary);
        }

        Ok(users)
    }

    pub(crate) async fn find_user_auth_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<aether_data::repository::users::StoredUserAuthRecord>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_user_store.as_ref() {
            if let Some(user) = store
                .lock()
                .expect("auth user store should lock")
                .get(user_id)
                .cloned()
            {
                return Ok(Some(user));
            }
        }
        self.data
            .find_user_auth_by_id(user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_user_auth_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Option<aether_data::repository::users::StoredUserAuthRecord>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_user_store.as_ref() {
            let identifier = identifier.trim();
            if !identifier.is_empty() {
                if let Some(user) = store
                    .lock()
                    .expect("auth user store should lock")
                    .values()
                    .find(|user| {
                        user.username == identifier || user.email.as_deref() == Some(identifier)
                    })
                    .cloned()
                {
                    return Ok(Some(user));
                }
            }
        }
        self.data
            .find_user_auth_by_identifier(identifier)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn is_other_user_auth_email_taken(
        &self,
        email: &str,
        user_id: &str,
    ) -> Result<bool, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_user_store.as_ref() {
            if store
                .lock()
                .expect("auth user store should lock")
                .values()
                .any(|user| user.id != user_id && user.email.as_deref() == Some(email))
            {
                return Ok(true);
            }
        }

        self.data
            .is_other_user_auth_email_taken(email, user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn is_other_user_auth_username_taken(
        &self,
        username: &str,
        user_id: &str,
    ) -> Result<bool, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_user_store.as_ref() {
            if store
                .lock()
                .expect("auth user store should lock")
                .values()
                .any(|user| user.id != user_id && user.username == username)
            {
                return Ok(true);
            }
        }

        self.data
            .is_other_user_auth_username_taken(username, user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn update_local_auth_user_profile(
        &self,
        user_id: &str,
        email: Option<String>,
        username: Option<String>,
    ) -> Result<Option<aether_data::repository::users::StoredUserAuthRecord>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_user_store.as_ref() {
            let existing = {
                store
                    .lock()
                    .expect("auth user store should lock")
                    .get(user_id)
                    .cloned()
            };
            let existing = match existing {
                Some(user) => Some(user),
                None => self
                    .data
                    .find_user_auth_by_id(user_id)
                    .await
                    .map_err(|err| GatewayError::Internal(err.to_string()))?,
            };
            let Some(mut user) = existing else {
                return Ok(None);
            };
            if let Some(email) = email {
                user.email = Some(email);
            }
            if let Some(username) = username {
                user.username = username;
            }
            store
                .lock()
                .expect("auth user store should lock")
                .insert(user.id.clone(), user.clone());
            return Ok(Some(user));
        }

        self.data
            .update_local_auth_user_profile(user_id, email, username)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn update_local_auth_user_password_hash(
        &self,
        user_id: &str,
        password_hash: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<aether_data::repository::users::StoredUserAuthRecord>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_user_store.as_ref() {
            let existing = {
                store
                    .lock()
                    .expect("auth user store should lock")
                    .get(user_id)
                    .cloned()
            };
            let existing = match existing {
                Some(user) => Some(user),
                None => self
                    .data
                    .find_user_auth_by_id(user_id)
                    .await
                    .map_err(|err| GatewayError::Internal(err.to_string()))?,
            };
            let Some(mut user) = existing else {
                return Ok(None);
            };
            user.password_hash = Some(password_hash);
            store
                .lock()
                .expect("auth user store should lock")
                .insert(user.id.clone(), user.clone());
            return Ok(Some(user));
        }

        self.data
            .update_local_auth_user_password_hash(user_id, password_hash, updated_at)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn create_local_auth_user(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
    ) -> Result<Option<aether_data::repository::users::StoredUserAuthRecord>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_user_store.as_ref() {
            let now = chrono::Utc::now();
            let user = aether_data::repository::users::StoredUserAuthRecord::new(
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
            )
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
            store
                .lock()
                .expect("auth user store should lock")
                .insert(user.id.clone(), user.clone());
            return Ok(Some(user));
        }

        self.data
            .create_local_auth_user(email, email_verified, username, password_hash)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
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
        #[cfg(test)]
        if let Some(store) = self.auth_user_store.as_ref() {
            let now = chrono::Utc::now();
            let user = aether_data::repository::users::StoredUserAuthRecord::new(
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
            )
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
            store
                .lock()
                .expect("auth user store should lock")
                .insert(user.id.clone(), user.clone());
            let _ = rate_limit;
            return Ok(Some(user));
        }

        self.data
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
            .map_err(|err| GatewayError::Internal(err.to_string()))
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
        #[cfg(test)]
        if let Some(store) = self.auth_user_store.as_ref() {
            let mut guard = store.lock().expect("auth user store should lock");
            let Some(user) = guard.get_mut(user_id) else {
                return Ok(None);
            };
            if let Some(role) = role {
                user.role = role;
            }
            if allowed_providers_present {
                user.allowed_providers = allowed_providers;
            }
            if allowed_api_formats_present {
                user.allowed_api_formats = allowed_api_formats;
            }
            if allowed_models_present {
                user.allowed_models = allowed_models;
            }
            if let Some(is_active) = is_active {
                user.is_active = is_active;
            }
            let _ = rate_limit;
            return Ok(Some(user.clone()));
        }

        self.data
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
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn touch_auth_user_last_login(
        &self,
        user_id: &str,
        logged_in_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_user_store.as_ref() {
            let mut guard = store.lock().expect("auth user store should lock");
            if let Some(user) = guard.get_mut(user_id) {
                user.last_login_at = Some(logged_in_at);
                return Ok(true);
            }
            return Ok(false);
        }

        self.data
            .touch_auth_user_last_login(user_id, logged_in_at)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn delete_local_auth_user(&self, user_id: &str) -> Result<bool, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_user_store.as_ref() {
            let removed = store
                .lock()
                .expect("auth user store should lock")
                .remove(user_id)
                .is_some();
            if removed {
                if let Some(wallet_store) = self.auth_wallet_store.as_ref() {
                    wallet_store
                        .lock()
                        .expect("auth wallet store should lock")
                        .retain(|_, wallet| wallet.user_id.as_deref() != Some(user_id));
                }
                if let Some(session_store) = self.auth_session_store.as_ref() {
                    let prefix = format!("{user_id}:");
                    session_store
                        .lock()
                        .expect("auth session store should lock")
                        .retain(|key, _| !key.starts_with(&prefix));
                }
            }
            return Ok(removed);
        }

        self.data
            .delete_local_auth_user(user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn register_local_auth_user(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<
        Option<(
            aether_data::repository::users::StoredUserAuthRecord,
            aether_data::repository::wallet::StoredWalletSnapshot,
        )>,
        GatewayError,
    > {
        #[cfg(test)]
        if let (Some(user_store), Some(wallet_store)) = (
            self.auth_user_store.as_ref(),
            self.auth_wallet_store.as_ref(),
        ) {
            let now = chrono::Utc::now();
            let user = aether_data::repository::users::StoredUserAuthRecord::new(
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
            )
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
            let gift_balance = if unlimited {
                0.0
            } else {
                initial_gift_usd.max(0.0)
            };
            let wallet = aether_data::repository::wallet::StoredWalletSnapshot::new(
                uuid::Uuid::new_v4().to_string(),
                Some(user.id.clone()),
                None,
                0.0,
                gift_balance,
                if unlimited {
                    "unlimited".to_string()
                } else {
                    "finite".to_string()
                },
                "USD".to_string(),
                "active".to_string(),
                0.0,
                0.0,
                0.0,
                gift_balance,
                now.timestamp(),
            )
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
            user_store
                .lock()
                .expect("auth user store should lock")
                .insert(user.id.clone(), user.clone());
            wallet_store
                .lock()
                .expect("auth wallet store should lock")
                .insert(wallet.id.clone(), wallet.clone());
            return Ok(Some((user, wallet)));
        }

        self.data
            .register_local_auth_user(
                email,
                email_verified,
                username,
                password_hash,
                initial_gift_usd,
                unlimited,
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }
}
