use crate::{AppState, GatewayError};

impl AppState {
    pub(crate) async fn read_user_model_capability_settings(
        &self,
        user_id: &str,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_user_model_capability_store.as_ref() {
            if let Some(settings) = store
                .lock()
                .expect("auth user model capability store should lock")
                .get(user_id)
                .cloned()
            {
                return Ok(Some(settings));
            }
        }

        let users = self.list_non_admin_export_users().await?;
        Ok(users
            .into_iter()
            .find(|user| user.id == user_id)
            .and_then(|user| user.model_capability_settings))
    }

    pub(crate) async fn update_user_model_capability_settings(
        &self,
        user_id: &str,
        settings: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_user_model_capability_store.as_ref() {
            let mut guard = store
                .lock()
                .expect("auth user model capability store should lock");
            match settings {
                Some(value) => {
                    guard.insert(user_id.to_string(), value.clone());
                    return Ok(Some(value));
                }
                None => {
                    guard.remove(user_id);
                    return Ok(None);
                }
            }
        }

        self.data
            .update_user_model_capability_settings(user_id, settings)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn read_user_feature_settings(
        &self,
        user_id: &str,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        self.data
            .read_user_feature_settings(user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn update_user_feature_settings(
        &self,
        user_id: &str,
        settings: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        let updated = self
            .data
            .update_user_feature_settings(user_id, settings)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if updated.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(updated)
    }

    pub(crate) async fn find_active_provider_name(
        &self,
        provider_id: &str,
    ) -> Result<Option<String>, GatewayError> {
        self.data
            .find_active_provider_name(provider_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
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
    ) -> Result<Option<aether_data::repository::users::StoredUserAuthRecord>, GatewayError> {
        #[cfg(test)]
        if let (Some(user_store), Some(wallet_store)) = (
            self.auth_user_store.as_ref(),
            self.auth_wallet_store.as_ref(),
        ) {
            let mut users = user_store.lock().expect("auth user store should lock");
            let existing_id = users
                .values()
                .find(|user| {
                    user.email.as_deref() == Some(email.as_str())
                        || user.username == username
                        || ldap_username
                            .as_deref()
                            .is_some_and(|value| user.username == value)
                })
                .map(|user| user.id.clone());

            if let Some(existing_id) = existing_id {
                let Some(user) = users.get_mut(&existing_id) else {
                    return Ok(None);
                };
                if user.is_deleted || !user.is_active {
                    return Ok(None);
                }
                if !user.auth_source.eq_ignore_ascii_case("ldap") {
                    return Ok(None);
                }
                user.email = Some(email);
                user.email_verified = true;
                user.last_login_at = Some(logged_in_at);
                return Ok(Some(user.clone()));
            }

            let base_username = ldap_username
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(username.as_str())
                .trim()
                .to_string();
            let mut candidate_username = base_username.clone();
            while users
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
            }

            let user = aether_data::repository::users::StoredUserAuthRecord::new(
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
            )
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
            users.insert(user.id.clone(), user.clone());
            drop(users);

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
                logged_in_at.timestamp(),
            )
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
            wallet_store
                .lock()
                .expect("auth wallet store should lock")
                .insert(wallet.id.clone(), wallet);
            let _ = ldap_dn;
            return Ok(Some(user));
        }

        self.data
            .get_or_create_ldap_auth_user(
                email,
                username,
                ldap_dn,
                ldap_username,
                logged_in_at,
                initial_gift_usd,
                unlimited,
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn initialize_auth_user_wallet(
        &self,
        user_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            let gift_balance = if unlimited {
                0.0
            } else {
                initial_gift_usd.max(0.0)
            };
            let now_unix_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let wallet = aether_data::repository::wallet::StoredWalletSnapshot::new(
                uuid::Uuid::new_v4().to_string(),
                Some(user_id.to_string()),
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
                now_unix_secs,
            )
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
            store
                .lock()
                .expect("auth wallet store should lock")
                .insert(wallet.id.clone(), wallet.clone());
            self.invalidate_auth_context_cache();
            return Ok(Some(wallet));
        }

        let wallet = self
            .data
            .initialize_auth_user_wallet(user_id, initial_gift_usd, unlimited)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if wallet.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(wallet)
    }

    pub(crate) async fn initialize_auth_api_key_wallet(
        &self,
        api_key_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            let gift_balance = if unlimited {
                0.0
            } else {
                initial_gift_usd.max(0.0)
            };
            let now_unix_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let wallet = aether_data::repository::wallet::StoredWalletSnapshot::new(
                uuid::Uuid::new_v4().to_string(),
                None,
                Some(api_key_id.to_string()),
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
                now_unix_secs,
            )
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
            store
                .lock()
                .expect("auth wallet store should lock")
                .insert(wallet.id.clone(), wallet.clone());
            self.invalidate_auth_context_cache();
            return Ok(Some(wallet));
        }

        let wallet = self
            .data
            .initialize_auth_api_key_wallet(api_key_id, initial_gift_usd, unlimited)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if wallet.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(wallet)
    }

    pub(crate) async fn update_auth_user_wallet_limit_mode(
        &self,
        user_id: &str,
        limit_mode: &str,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            let mut guard = store.lock().expect("auth wallet store should lock");
            let Some((wallet_id, wallet)) = guard
                .iter_mut()
                .find(|(_, wallet)| wallet.user_id.as_deref() == Some(user_id))
            else {
                return Ok(None);
            };
            let _ = wallet_id;
            wallet.limit_mode = limit_mode.to_string();
            wallet.updated_at_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
            let wallet = wallet.clone();
            drop(guard);
            self.invalidate_auth_context_cache();
            return Ok(Some(wallet));
        }

        let wallet = self
            .data
            .update_auth_user_wallet_limit_mode(user_id, limit_mode)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if wallet.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(wallet)
    }

    pub(crate) async fn update_auth_api_key_wallet_limit_mode(
        &self,
        api_key_id: &str,
        limit_mode: &str,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            let mut guard = store.lock().expect("auth wallet store should lock");
            let Some((wallet_id, wallet)) = guard
                .iter_mut()
                .find(|(_, wallet)| wallet.api_key_id.as_deref() == Some(api_key_id))
            else {
                return Ok(None);
            };
            let _ = wallet_id;
            wallet.limit_mode = limit_mode.to_string();
            wallet.updated_at_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
            let wallet = wallet.clone();
            drop(guard);
            self.invalidate_auth_context_cache();
            return Ok(Some(wallet));
        }

        let wallet = self
            .data
            .update_auth_api_key_wallet_limit_mode(api_key_id, limit_mode)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if wallet.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(wallet)
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
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            let mut guard = store.lock().expect("auth wallet store should lock");
            let Some((_, wallet)) = guard
                .iter_mut()
                .find(|(_, wallet)| wallet.user_id.as_deref() == Some(user_id))
            else {
                return Ok(None);
            };
            wallet.balance = balance;
            wallet.gift_balance = gift_balance;
            wallet.limit_mode = limit_mode.to_string();
            wallet.currency = currency.to_string();
            wallet.status = status.to_string();
            wallet.total_recharged = total_recharged;
            wallet.total_consumed = total_consumed;
            wallet.total_refunded = total_refunded;
            wallet.total_adjusted = total_adjusted;
            if let Some(updated_at_unix_secs) = updated_at_unix_secs {
                wallet.updated_at_unix_secs = updated_at_unix_secs;
            }
            let wallet = wallet.clone();
            drop(guard);
            self.invalidate_auth_context_cache();
            return Ok(Some(wallet));
        }

        let wallet = self
            .data
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
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if wallet.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(wallet)
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
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            let mut guard = store.lock().expect("auth wallet store should lock");
            let Some((_, wallet)) = guard
                .iter_mut()
                .find(|(_, wallet)| wallet.api_key_id.as_deref() == Some(api_key_id))
            else {
                return Ok(None);
            };
            wallet.balance = balance;
            wallet.gift_balance = gift_balance;
            wallet.limit_mode = limit_mode.to_string();
            wallet.currency = currency.to_string();
            wallet.status = status.to_string();
            wallet.total_recharged = total_recharged;
            wallet.total_consumed = total_consumed;
            wallet.total_refunded = total_refunded;
            wallet.total_adjusted = total_adjusted;
            if let Some(updated_at_unix_secs) = updated_at_unix_secs {
                wallet.updated_at_unix_secs = updated_at_unix_secs;
            }
            let wallet = wallet.clone();
            drop(guard);
            self.invalidate_auth_context_cache();
            return Ok(Some(wallet));
        }

        let wallet = self
            .data
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
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if wallet.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(wallet)
    }
}
