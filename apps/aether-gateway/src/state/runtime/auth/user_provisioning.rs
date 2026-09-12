use std::time::Duration;

use crate::{AppState, GatewayError};

const USER_RUNTIME_JSON_CACHE_TTL: Duration = Duration::from_secs(30);

fn normalize_ldap_identity_email(value: &str) -> Option<String> {
    let normalized = value.trim().to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

/// LDAP synchronization may create a wallet as a side effect.  Keep the
/// wallet id only when this invocation created that row so later compensation
/// can never delete a pre-existing wallet.
pub(crate) struct LdapAuthProvisioningResult {
    pub(crate) user: aether_data::repository::users::StoredUserAuthRecord,
    pub(crate) owned_wallet_id: Option<String>,
}

#[cfg(test)]
pub(super) fn record_test_initial_gift_transaction(
    state: &AppState,
    wallet: &aether_data::repository::wallet::StoredWalletSnapshot,
    owner_id: &str,
    description: &str,
) {
    if wallet.gift_balance <= 0.0 {
        return;
    }
    let Some(store) = state.admin_wallet_transaction_store.as_ref() else {
        return;
    };
    let created_at_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64;
    let transaction = crate::AdminWalletTransactionRecord {
        id: uuid::Uuid::new_v4().to_string(),
        wallet_id: wallet.id.clone(),
        category: "gift".to_string(),
        reason_code: "gift_initial".to_string(),
        amount: wallet.gift_balance,
        balance_before: 0.0,
        balance_after: wallet.gift_balance,
        recharge_balance_before: 0.0,
        recharge_balance_after: 0.0,
        gift_balance_before: 0.0,
        gift_balance_after: wallet.gift_balance,
        link_type: Some("system_task".to_string()),
        link_id: Some(owner_id.to_string()),
        operator_id: None,
        description: Some(description.to_string()),
        created_at_unix_ms,
    };
    store
        .lock()
        .expect("admin wallet transaction store should lock")
        .insert(transaction.id.clone(), transaction);
}

#[cfg(test)]
impl AppState {
    /// The test-only auth stores keep users and API keys in separate
    /// repositories, just like the production data layer.  Check both wallet
    /// owner forms before a compensating user delete so an API-key wallet can
    /// never be orphaned by the in-memory path.
    async fn test_api_key_wallet_exists_for_user(
        &self,
        user_id: &str,
    ) -> Result<bool, GatewayError> {
        let Some(store) = self.auth_wallet_store.as_ref() else {
            return Ok(false);
        };

        if self.data.has_auth_api_key_reader() {
            let user_ids = vec![user_id.to_string()];
            let api_key_ids = self
                .data
                .list_auth_api_key_export_records_by_user_ids(&user_ids)
                .await
                .map_err(|err| GatewayError::Internal(err.to_string()))?
                .into_iter()
                .map(|record| record.api_key_id)
                .collect::<std::collections::BTreeSet<_>>();
            if api_key_ids.is_empty() {
                return Ok(false);
            }
            let wallets = store.lock().expect("auth wallet store should lock");
            return Ok(wallets.values().any(|wallet| {
                wallet
                    .api_key_id
                    .as_deref()
                    .is_some_and(|api_key_id| api_key_ids.contains(api_key_id))
            }));
        }

        // No key repository means ownership cannot be resolved.  Treat any
        // API-key wallet as a reference and fail closed.
        let wallets = store.lock().expect("auth wallet store should lock");
        Ok(wallets.values().any(|wallet| wallet.api_key_id.is_some()))
    }
}

impl AppState {
    pub(crate) async fn delete_wallet_if_unreferenced(
        &self,
        wallet_id: &str,
        owner: aether_data::repository::wallet::WalletLookupKey<'_>,
    ) -> Result<bool, GatewayError> {
        if wallet_id.trim().is_empty() {
            return Ok(false);
        }

        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            let owner_matches =
                |wallet: &aether_data::repository::wallet::StoredWalletSnapshot| match owner {
                    aether_data::repository::wallet::WalletLookupKey::UserId(user_id) => {
                        !user_id.trim().is_empty()
                            && wallet.id == wallet_id
                            && wallet.user_id.as_deref() == Some(user_id)
                            && wallet.api_key_id.is_none()
                    }
                    aether_data::repository::wallet::WalletLookupKey::ApiKeyId(api_key_id) => {
                        !api_key_id.trim().is_empty()
                            && wallet.id == wallet_id
                            && wallet.api_key_id.as_deref() == Some(api_key_id)
                            && wallet.user_id.is_none()
                    }
                    aether_data::repository::wallet::WalletLookupKey::WalletId(_) => false,
                };
            if matches!(
                owner,
                aether_data::repository::wallet::WalletLookupKey::WalletId(_)
            ) {
                return Err(GatewayError::Internal(
                    "wallet compensation requires an explicit user or API-key owner".to_string(),
                ));
            }
            let wallet = store
                .lock()
                .expect("auth wallet store should lock")
                .values()
                .find(|wallet| owner_matches(wallet))
                .cloned();
            let Some(wallet) = wallet else {
                return Ok(false);
            };
            let untouched = wallet.balance == 0.0
                && wallet.gift_balance == 0.0
                && wallet.total_recharged == 0.0
                && wallet.total_consumed == 0.0
                && wallet.total_refunded == 0.0
                && wallet.total_adjusted == 0.0
                && matches!(wallet.limit_mode.as_str(), "finite" | "unlimited")
                && wallet.currency == "USD"
                && wallet.status == "active";
            if !untouched {
                return Ok(false);
            }
            let has_order = self
                .admin_wallet_payment_order_store
                .as_ref()
                .is_some_and(|orders| {
                    orders
                        .lock()
                        .expect("admin wallet payment order store should lock")
                        .values()
                        .any(|order| order.wallet_id == wallet.id)
                });
            let has_transaction =
                self.admin_wallet_transaction_store
                    .as_ref()
                    .is_some_and(|transactions| {
                        transactions
                            .lock()
                            .expect("admin wallet transaction store should lock")
                            .values()
                            .any(|transaction| transaction.wallet_id == wallet.id)
                    });
            let has_refund = self
                .admin_wallet_refund_store
                .as_ref()
                .is_some_and(|refunds| {
                    refunds
                        .lock()
                        .expect("admin wallet refund store should lock")
                        .values()
                        .any(|refund| refund.wallet_id == wallet.id)
                });
            if has_order || has_transaction || has_refund {
                return Ok(false);
            }
            let mut wallets = store.lock().expect("auth wallet store should lock");
            if !wallets
                .get(&wallet.id)
                .is_some_and(|current| owner_matches(current))
            {
                return Ok(false);
            }
            let removed = wallets.remove(&wallet.id).is_some();
            if removed {
                self.invalidate_auth_context_cache();
            }
            return Ok(removed);
        }

        self.data
            .delete_wallet_if_unreferenced(wallet_id, owner)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn delete_wallet_if_snapshot_matches_and_unreferenced(
        &self,
        expected: &aether_data::repository::wallet::StoredWalletSnapshot,
        owner: aether_data::repository::wallet::WalletLookupKey<'_>,
    ) -> Result<bool, GatewayError> {
        if expected.id.trim().is_empty() {
            return Ok(false);
        }

        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            let owner_matches =
                |wallet: &aether_data::repository::wallet::StoredWalletSnapshot| match owner {
                    aether_data::repository::wallet::WalletLookupKey::UserId(user_id) => {
                        !user_id.trim().is_empty()
                            && wallet.id == expected.id
                            && wallet.user_id.as_deref() == Some(user_id)
                            && wallet.api_key_id.is_none()
                    }
                    aether_data::repository::wallet::WalletLookupKey::ApiKeyId(api_key_id) => {
                        !api_key_id.trim().is_empty()
                            && wallet.id == expected.id
                            && wallet.api_key_id.as_deref() == Some(api_key_id)
                            && wallet.user_id.is_none()
                    }
                    aether_data::repository::wallet::WalletLookupKey::WalletId(_) => false,
                };
            if matches!(
                owner,
                aether_data::repository::wallet::WalletLookupKey::WalletId(_)
            ) {
                return Err(GatewayError::Internal(
                    "wallet compensation requires an explicit user or API-key owner".to_string(),
                ));
            }
            let current = store
                .lock()
                .expect("auth wallet store should lock")
                .values()
                .find(|wallet| owner_matches(wallet))
                .cloned();
            if current.as_ref() != Some(expected) {
                return Ok(false);
            }
            let has_order = self
                .admin_wallet_payment_order_store
                .as_ref()
                .is_some_and(|orders| {
                    orders
                        .lock()
                        .expect("admin wallet payment order store should lock")
                        .values()
                        .any(|order| order.wallet_id == expected.id)
                });
            let has_transaction =
                self.admin_wallet_transaction_store
                    .as_ref()
                    .is_some_and(|transactions| {
                        transactions
                            .lock()
                            .expect("admin wallet transaction store should lock")
                            .values()
                            .any(|transaction| transaction.wallet_id == expected.id)
                    });
            let has_refund = self
                .admin_wallet_refund_store
                .as_ref()
                .is_some_and(|refunds| {
                    refunds
                        .lock()
                        .expect("admin wallet refund store should lock")
                        .values()
                        .any(|refund| refund.wallet_id == expected.id)
                });
            if has_order || has_transaction || has_refund {
                return Ok(false);
            }
            let mut wallets = store.lock().expect("auth wallet store should lock");
            if wallets
                .get(&expected.id)
                .is_some_and(|current| owner_matches(current) && current == expected)
            {
                let removed = wallets.remove(&expected.id).is_some();
                if removed {
                    self.invalidate_auth_context_cache();
                }
                return Ok(removed);
            }
            return Ok(false);
        }

        self.data
            .delete_wallet_if_snapshot_matches_and_unreferenced(expected, owner)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn restore_wallet_if_snapshot_matches(
        &self,
        before: &aether_data::repository::wallet::StoredWalletSnapshot,
        after: &aether_data::repository::wallet::StoredWalletSnapshot,
        owner: aether_data::repository::wallet::WalletLookupKey<'_>,
    ) -> Result<bool, GatewayError> {
        if before.id.trim().is_empty() || after.id.trim().is_empty() {
            return Ok(false);
        }
        if before.id != after.id {
            return Err(GatewayError::Internal(
                "wallet restore snapshots must reference the same wallet".to_string(),
            ));
        }

        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            let owner_matches =
                |wallet: &aether_data::repository::wallet::StoredWalletSnapshot| match owner {
                    aether_data::repository::wallet::WalletLookupKey::UserId(user_id) => {
                        !user_id.trim().is_empty()
                            && wallet.user_id.as_deref() == Some(user_id)
                            && wallet.api_key_id.is_none()
                    }
                    aether_data::repository::wallet::WalletLookupKey::ApiKeyId(api_key_id) => {
                        !api_key_id.trim().is_empty()
                            && wallet.api_key_id.as_deref() == Some(api_key_id)
                            && wallet.user_id.is_none()
                    }
                    aether_data::repository::wallet::WalletLookupKey::WalletId(_) => false,
                };
            if matches!(
                owner,
                aether_data::repository::wallet::WalletLookupKey::WalletId(_)
            ) {
                return Err(GatewayError::Internal(
                    "wallet restore requires an explicit user or API-key owner".to_string(),
                ));
            }
            if !owner_matches(before) || !owner_matches(after) {
                return Ok(false);
            }
            let mut guard = store.lock().expect("auth wallet store should lock");
            let Some(current) = guard.get(&after.id) else {
                return Ok(false);
            };
            if current != after {
                return Ok(false);
            }
            guard.insert(before.id.clone(), before.clone());
            drop(guard);
            self.invalidate_auth_context_cache();
            return Ok(true);
        }

        let restored = self
            .data
            .restore_wallet_if_snapshot_matches(before, after, owner)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if restored {
            self.invalidate_auth_context_cache();
        }
        Ok(restored)
    }

    pub(crate) async fn rollback_provisional_auth_user(
        &self,
        user_id: &str,
    ) -> Result<(), GatewayError> {
        self.rollback_provisional_auth_user_with_wallet(user_id, None)
            .await
    }

    pub(crate) async fn rollback_provisional_auth_user_with_wallet(
        &self,
        user_id: &str,
        wallet_id: Option<&str>,
    ) -> Result<(), GatewayError> {
        if wallet_id.is_some_and(|wallet_id| wallet_id.trim().is_empty()) {
            return Err(GatewayError::Internal(
                "wallet compensation wallet id cannot be empty".to_string(),
            ));
        }
        // Keep this ordering: wallets reference the user with SET NULL, so
        // purge the guarded provisioning wallet before deleting its owner.
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            if self.test_api_key_wallet_exists_for_user(user_id).await? {
                return Err(GatewayError::Internal(format!(
                    "refusing to delete provisional auth user {user_id}: wallet ownership is unknown"
                )));
            }
            let wallet = store
                .lock()
                .expect("auth wallet store should lock")
                .values()
                .find(|wallet| {
                    wallet.user_id.as_deref() == Some(user_id)
                        && wallet_id.is_none_or(|wallet_id| wallet.id == wallet_id)
                })
                .cloned();
            if wallet_id.is_none() && wallet.is_some() {
                return Err(GatewayError::Internal(format!(
                    "refusing to delete provisional auth user {user_id}: wallet ownership is unknown"
                )));
            }
            if wallet_id.is_some() && wallet.is_none() {
                let supplied_wallet_exists = store
                    .lock()
                    .expect("auth wallet store should lock")
                    .values()
                    .any(|wallet| wallet_id.is_some_and(|expected| wallet.id == expected));
                if supplied_wallet_exists {
                    return Err(GatewayError::Internal(format!(
                        "refusing to delete provisional auth user {user_id}: supplied wallet still exists"
                    )));
                }
                let any_wallet = store
                    .lock()
                    .expect("auth wallet store should lock")
                    .values()
                    .any(|wallet| wallet.user_id.as_deref() == Some(user_id));
                if any_wallet {
                    return Err(GatewayError::Internal(format!(
                        "refusing to delete provisional auth user {user_id}: wallet ownership is unknown"
                    )));
                }
            }
            if let Some(wallet) = wallet {
                let structurally_removable = wallet.api_key_id.is_none()
                    && wallet.balance == 0.0
                    && wallet.gift_balance >= 0.0
                    && wallet.total_recharged == 0.0
                    && wallet.total_consumed == 0.0
                    && wallet.total_refunded == 0.0
                    && wallet.total_adjusted == wallet.gift_balance
                    && wallet.status == "active"
                    && matches!(wallet.limit_mode.as_str(), "finite" | "unlimited")
                    && wallet.currency == "USD";
                if !structurally_removable {
                    return Err(GatewayError::Internal(format!(
                        "refusing to delete provisional auth user {user_id}: wallet is not eligible for rollback"
                    )));
                }
                let wallet_id = wallet.id;
                let has_order =
                    self.admin_wallet_payment_order_store
                        .as_ref()
                        .is_some_and(|orders| {
                            orders
                                .lock()
                                .expect("admin wallet payment order store should lock")
                                .values()
                                .any(|order| order.wallet_id == wallet_id)
                        });
                let has_transaction =
                    self.admin_wallet_transaction_store
                        .as_ref()
                        .is_some_and(|transactions| {
                            let wallet_transactions = transactions
                                .lock()
                                .expect("admin wallet transaction store should lock")
                                .values()
                                .filter(|transaction| transaction.wallet_id == wallet_id)
                                .cloned()
                                .collect::<Vec<_>>();
                            if wallet_transactions.is_empty() {
                                return false;
                            }
                            let gift_balance = store
                                .lock()
                                .expect("auth wallet store should lock")
                                .get(&wallet_id)
                                .map(|wallet| wallet.gift_balance)
                                .unwrap_or_default();
                            !(wallet_transactions.len() == 1
                                && gift_balance > 0.0
                                && wallet_transactions[0].category == "gift"
                                && wallet_transactions[0].reason_code == "gift_initial"
                                && wallet_transactions[0].amount == gift_balance
                                && wallet_transactions[0].balance_before == 0.0
                                && wallet_transactions[0].balance_after == gift_balance
                                && wallet_transactions[0].recharge_balance_before == 0.0
                                && wallet_transactions[0].recharge_balance_after == 0.0
                                && wallet_transactions[0].gift_balance_before == 0.0
                                && wallet_transactions[0].gift_balance_after == gift_balance
                                && wallet_transactions[0].link_type.as_deref()
                                    == Some("system_task")
                                && wallet_transactions[0].link_id.as_deref() == Some(user_id)
                                && wallet_transactions[0].operator_id.is_none())
                        });
                let has_refund = self
                    .admin_wallet_refund_store
                    .as_ref()
                    .is_some_and(|refunds| {
                        refunds
                            .lock()
                            .expect("admin wallet refund store should lock")
                            .values()
                            .any(|refund| refund.wallet_id == wallet_id)
                    });
                if has_order || has_transaction || has_refund {
                    return Err(GatewayError::Internal(format!(
                        "refusing to delete provisional auth user {user_id}: wallet has financial activity"
                    )));
                }
                if let Some(transactions) = self.admin_wallet_transaction_store.as_ref() {
                    transactions
                        .lock()
                        .expect("admin wallet transaction store should lock")
                        .retain(|_, transaction| transaction.wallet_id != wallet_id);
                }
                store
                    .lock()
                    .expect("auth wallet store should lock")
                    .remove(&wallet_id);
            }
            self.delete_local_auth_user(user_id).await?;
            return Ok(());
        }

        self.data
            .rollback_provisional_auth_user_with_wallet(user_id, wallet_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        Ok(())
    }

    pub(crate) async fn read_user_model_capability_settings(
        &self,
        user_id: &str,
    ) -> Result<Option<serde_json::Value>, GatewayError> {
        let user_id = user_id.trim();
        if user_id.is_empty() {
            return Ok(None);
        }
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

        let cache_key = user_id.to_string();
        self.user_model_capability_settings_cache
            .get_or_load(cache_key, USER_RUNTIME_JSON_CACHE_TTL, || async move {
                Ok(self
                    .data
                    .find_export_user_by_id(user_id)
                    .await
                    .map_err(|err| GatewayError::Internal(err.to_string()))?
                    .and_then(|user| user.model_capability_settings))
            })
            .await
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
        let user_id = user_id.trim();
        if user_id.is_empty() {
            return Ok(None);
        }
        let cache_key = user_id.to_string();
        self.user_feature_settings_cache
            .get_or_load(cache_key, USER_RUNTIME_JSON_CACHE_TTL, || async move {
                self.data
                    .read_user_feature_settings(user_id)
                    .await
                    .map_err(|err| GatewayError::Internal(err.to_string()))
            })
            .await
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
        Ok(self
            .get_or_create_ldap_auth_user_with_wallet_outcome(
                email,
                username,
                ldap_dn,
                ldap_username,
                logged_in_at,
                initial_gift_usd,
                unlimited,
            )
            .await?
            .map(|result| result.user))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn get_or_create_ldap_auth_user_with_wallet_outcome(
        &self,
        email: String,
        username: String,
        ldap_dn: Option<String>,
        ldap_username: Option<String>,
        logged_in_at: chrono::DateTime<chrono::Utc>,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<LdapAuthProvisioningResult>, GatewayError> {
        let Some(email) = normalize_ldap_identity_email(&email) else {
            return Ok(None);
        };
        #[cfg(test)]
        if let (Some(user_store), Some(_wallet_store)) = (
            self.auth_user_store.as_ref(),
            self.auth_wallet_store.as_ref(),
        ) {
            if !initial_gift_usd.is_finite() {
                return Err(GatewayError::Internal(
                    "initial gift amount must be finite".to_string(),
                ));
            }
            let user = {
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
                    return Ok(Some(LdapAuthProvisioningResult {
                        user: user.clone(),
                        owned_wallet_id: None,
                    }));
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
                user
            };
            let _ = ldap_dn;

            let initialized = match self
                .initialize_auth_user_wallet_with_outcome(&user.id, initial_gift_usd, unlimited)
                .await
            {
                Ok(Some(initialized)) => initialized,
                Ok(None) => {
                    let _ = self
                        .rollback_provisional_auth_user_with_wallet(&user.id, None)
                        .await;
                    return Ok(None);
                }
                Err(err) => {
                    let _ = self
                        .rollback_provisional_auth_user_with_wallet(&user.id, None)
                        .await;
                    return Err(err);
                }
            };
            let wallet_is_user_owned = initialized.wallet.user_id.as_deref()
                == Some(user.id.as_str())
                && initialized.wallet.api_key_id.is_none();
            if !wallet_is_user_owned {
                let owned_wallet_id = initialized.created.then(|| initialized.wallet.id.clone());
                let _ = self
                    .rollback_provisional_auth_user_with_wallet(
                        &user.id,
                        owned_wallet_id.as_deref(),
                    )
                    .await;
                return Err(GatewayError::Internal(
                    "LDAP user wallet owner does not match the provisioned user".to_string(),
                ));
            }
            return Ok(Some(LdapAuthProvisioningResult {
                user,
                owned_wallet_id: initialized.created.then_some(initialized.wallet.id),
            }));
        }

        let result = self
            .data
            .get_or_create_ldap_auth_user_with_wallet_outcome(
                email,
                username,
                ldap_dn,
                ldap_username,
                logged_in_at,
                initial_gift_usd,
                unlimited,
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        Ok(result.map(|result| LdapAuthProvisioningResult {
            user: result.user,
            owned_wallet_id: result.owned_wallet_id,
        }))
    }

    pub(crate) async fn initialize_auth_user_wallet(
        &self,
        user_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            if user_id.trim().is_empty() {
                return Err(GatewayError::Internal(
                    "user id is required to initialize a wallet".to_string(),
                ));
            }
            if !initial_gift_usd.is_finite() {
                return Err(GatewayError::Internal(
                    "initial gift amount must be finite".to_string(),
                ));
            }
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
            let mut wallets = store.lock().expect("auth wallet store should lock");
            if let Some(existing) = wallets.values().find(|existing| {
                existing.user_id.as_deref() == Some(user_id) && existing.api_key_id.is_none()
            }) {
                let existing = existing.clone();
                drop(wallets);
                return Ok(Some(existing));
            }
            wallets.insert(wallet.id.clone(), wallet.clone());
            drop(wallets);
            record_test_initial_gift_transaction(self, &wallet, user_id, "用户初始赠款");
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

    pub(crate) async fn initialize_auth_user_wallet_with_outcome(
        &self,
        user_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<aether_data::repository::wallet::InitializeAuthWalletOutcome>, GatewayError>
    {
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            if user_id.trim().is_empty() {
                return Err(GatewayError::Internal(
                    "user id is required to initialize a wallet".to_string(),
                ));
            }
            if !initial_gift_usd.is_finite() {
                return Err(GatewayError::Internal(
                    "initial gift amount must be finite".to_string(),
                ));
            }
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
            let mut wallets = store.lock().expect("auth wallet store should lock");
            if let Some(existing) = wallets.values().find(|existing| {
                existing.user_id.as_deref() == Some(user_id) && existing.api_key_id.is_none()
            }) {
                return Ok(Some(
                    aether_data::repository::wallet::InitializeAuthWalletOutcome {
                        wallet: existing.clone(),
                        created: false,
                    },
                ));
            }
            wallets.insert(wallet.id.clone(), wallet.clone());
            drop(wallets);
            record_test_initial_gift_transaction(self, &wallet, user_id, "用户初始赠款");
            self.invalidate_auth_context_cache();
            return Ok(Some(
                aether_data::repository::wallet::InitializeAuthWalletOutcome {
                    wallet,
                    created: true,
                },
            ));
        }

        self.data
            .initialize_auth_user_wallet_with_outcome(user_id, initial_gift_usd, unlimited)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn initialize_auth_api_key_wallet(
        &self,
        api_key_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            if api_key_id.trim().is_empty() {
                return Err(GatewayError::Internal(
                    "api key id is required to initialize a wallet".to_string(),
                ));
            }
            if !initial_gift_usd.is_finite() {
                return Err(GatewayError::Internal(
                    "initial gift amount must be finite".to_string(),
                ));
            }
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
            let mut wallets = store.lock().expect("auth wallet store should lock");
            if let Some(existing) = wallets.values().find(|existing| {
                existing.api_key_id.as_deref() == Some(api_key_id) && existing.user_id.is_none()
            }) {
                let existing = existing.clone();
                drop(wallets);
                return Ok(Some(existing));
            }
            wallets.insert(wallet.id.clone(), wallet.clone());
            drop(wallets);
            record_test_initial_gift_transaction(
                self,
                &wallet,
                api_key_id,
                "独立余额 Key 初始赠款",
            );
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

    pub(crate) async fn initialize_auth_api_key_wallet_with_outcome(
        &self,
        api_key_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<aether_data::repository::wallet::InitializeAuthWalletOutcome>, GatewayError>
    {
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            if api_key_id.trim().is_empty() {
                return Err(GatewayError::Internal(
                    "api key id is required to initialize a wallet".to_string(),
                ));
            }
            if !initial_gift_usd.is_finite() {
                return Err(GatewayError::Internal(
                    "initial gift amount must be finite".to_string(),
                ));
            }
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
            let mut wallets = store.lock().expect("auth wallet store should lock");
            if let Some(existing) = wallets.values().find(|existing| {
                existing.api_key_id.as_deref() == Some(api_key_id) && existing.user_id.is_none()
            }) {
                return Ok(Some(
                    aether_data::repository::wallet::InitializeAuthWalletOutcome {
                        wallet: existing.clone(),
                        created: false,
                    },
                ));
            }
            wallets.insert(wallet.id.clone(), wallet.clone());
            drop(wallets);
            record_test_initial_gift_transaction(
                self,
                &wallet,
                api_key_id,
                "独立余额 Key 初始赠款",
            );
            self.invalidate_auth_context_cache();
            return Ok(Some(
                aether_data::repository::wallet::InitializeAuthWalletOutcome {
                    wallet,
                    created: true,
                },
            ));
        }

        self.data
            .initialize_auth_api_key_wallet_with_outcome(api_key_id, initial_gift_usd, unlimited)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use aether_data::repository::users::StoredUserAuthRecord;
    use aether_data::repository::wallet::{StoredWalletSnapshot, WalletLookupKey};

    #[test]
    fn ldap_identity_email_uses_the_canonical_account_namespace() {
        assert_eq!(
            normalize_ldap_identity_email("  Alice@Example.COM  ").as_deref(),
            Some("alice@example.com")
        );
        assert_eq!(normalize_ldap_identity_email("   "), None);
    }

    fn provisional_user(user_id: &str) -> StoredUserAuthRecord {
        let now = chrono::Utc::now();
        StoredUserAuthRecord::new(
            user_id.to_string(),
            Some(format!("{user_id}@example.com")),
            true,
            user_id.to_string(),
            Some("$2b$12$4qL4tdcsFwVaDTw5Ck3xzu8GpNdre56DiNR6Dnw7t6gCXaEnqAe7G".to_string()),
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
        .expect("user should build")
    }

    #[tokio::test]
    async fn ldap_provisioning_preserves_only_new_wallet_id() {
        let state = AppState::new()
            .expect("state should build")
            .with_auth_users_for_tests(Vec::<StoredUserAuthRecord>::new())
            .with_auth_wallets_for_tests(Vec::<StoredWalletSnapshot>::new());
        let logged_in_at = chrono::Utc::now();

        let first = state
            .get_or_create_ldap_auth_user_with_wallet_outcome(
                "  Alice@Example.COM ".to_string(),
                "alice".to_string(),
                Some("uid=alice,dc=example".to_string()),
                Some("alice".to_string()),
                logged_in_at,
                10.0,
                false,
            )
            .await
            .expect("LDAP provisioning should succeed")
            .expect("new LDAP user should be returned");
        let first_wallet_id = first
            .owned_wallet_id
            .clone()
            .expect("new provisioning should report its wallet id");
        assert_eq!(first.user.email.as_deref(), Some("alice@example.com"));
        assert!(state
            .find_wallet(WalletLookupKey::WalletId(&first_wallet_id))
            .await
            .expect("wallet lookup should succeed")
            .is_some());

        let replay = state
            .get_or_create_ldap_auth_user_with_wallet_outcome(
                "ALICE@example.com".to_string(),
                "alice".to_string(),
                None,
                Some("alice".to_string()),
                logged_in_at,
                99.0,
                false,
            )
            .await
            .expect("LDAP replay should succeed")
            .expect("existing LDAP user should be returned");
        assert_eq!(replay.user.id, first.user.id);
        assert_eq!(replay.owned_wallet_id, None);
        assert_eq!(
            state
                .find_wallet(WalletLookupKey::WalletId(&first_wallet_id))
                .await
                .expect("wallet lookup should succeed")
                .expect("existing wallet should remain")
                .gift_balance,
            10.0
        );
    }

    #[tokio::test]
    async fn test_store_rollback_preserves_user_with_active_wallet() {
        let user_id = "active-provisional-user";
        let wallet = StoredWalletSnapshot::new(
            "active-provisional-wallet".to_string(),
            Some(user_id.to_string()),
            None,
            1.0,
            0.0,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            1.0,
            0.0,
            0.0,
            0.0,
            chrono::Utc::now().timestamp(),
        )
        .expect("wallet should build");
        let wallet_id = wallet.id.clone();
        let state = AppState::new()
            .expect("state should build")
            .with_auth_users_for_tests([provisional_user(user_id)])
            .with_auth_wallets_for_tests([wallet]);

        assert!(state
            .rollback_provisional_auth_user_with_wallet(user_id, Some(wallet_id.as_str()))
            .await
            .is_err());
        assert!(state
            .find_user_auth_by_id(user_id)
            .await
            .expect("user lookup should succeed")
            .is_some());
        assert!(state
            .find_wallet(WalletLookupKey::UserId(user_id))
            .await
            .expect("wallet lookup should succeed")
            .is_some());
    }

    #[tokio::test]
    async fn test_store_rollback_rejects_wallet_id_owned_by_another_user() {
        let target_user_id = "target-provisional-user";
        let other_user_id = "other-user";
        let other_wallet = StoredWalletSnapshot::new(
            "other-user-wallet".to_string(),
            Some(other_user_id.to_string()),
            None,
            0.0,
            0.0,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            0.0,
            0.0,
            0.0,
            0.0,
            chrono::Utc::now().timestamp(),
        )
        .expect("wallet should build");
        let state = AppState::new()
            .expect("state should build")
            .with_auth_users_for_tests([provisional_user(target_user_id)])
            .with_auth_wallets_for_tests([other_wallet]);

        assert!(state
            .rollback_provisional_auth_user_with_wallet(target_user_id, Some("other-user-wallet"),)
            .await
            .is_err());
        assert!(state
            .find_user_auth_by_id(target_user_id)
            .await
            .expect("user lookup should succeed")
            .is_some());
        assert!(state
            .find_wallet(WalletLookupKey::WalletId("other-user-wallet"))
            .await
            .expect("wallet lookup should succeed")
            .is_some());
    }

    #[tokio::test]
    async fn test_store_rollback_removes_user_when_wallet_is_absent() {
        let user_id = "no-wallet-provisional-user";
        let state = AppState::new()
            .expect("state should build")
            .with_auth_users_for_tests([provisional_user(user_id)]);

        state
            .rollback_provisional_auth_user_with_wallet(user_id, Some("missing-wallet"))
            .await
            .expect("confirmed wallet absence should allow rollback");
        assert!(state
            .find_user_auth_by_id(user_id)
            .await
            .expect("user lookup should succeed")
            .is_none());
    }

    #[tokio::test]
    async fn test_store_rollback_preserves_user_when_api_key_wallet_exists() {
        let user_id = "api-key-wallet-provisional-user";
        let api_key_wallet = StoredWalletSnapshot::new(
            "api-key-wallet-provisional".to_string(),
            None,
            Some("api-key-owned-by-user".to_string()),
            0.0,
            0.0,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            0.0,
            0.0,
            0.0,
            0.0,
            chrono::Utc::now().timestamp(),
        )
        .expect("api-key wallet should build");
        let state = AppState::new()
            .expect("state should build")
            .with_auth_users_for_tests([provisional_user(user_id)])
            .with_auth_wallets_for_tests([api_key_wallet.clone()]);

        assert!(state
            .rollback_provisional_auth_user_with_wallet(user_id, None)
            .await
            .is_err());
        assert!(state
            .find_user_auth_by_id(user_id)
            .await
            .expect("user lookup should succeed")
            .is_some());
        assert!(state
            .find_wallet(WalletLookupKey::ApiKeyId("api-key-owned-by-user"))
            .await
            .expect("wallet lookup should succeed")
            .is_some());
    }

    #[tokio::test]
    async fn test_store_wallet_initialization_records_initial_gift_once_and_rolls_back() {
        let user_id = "gift-provisional-user";
        let state = AppState::new()
            .expect("state should build")
            .with_auth_users_for_tests([provisional_user(user_id)]);

        let first = state
            .initialize_auth_user_wallet_with_outcome(user_id, 7.5, false)
            .await
            .expect("wallet initialization should resolve")
            .expect("wallet should be available");
        assert!(first.created);

        let transaction_store = state
            .admin_wallet_transaction_store
            .as_ref()
            .expect("transaction store should be available")
            .clone();
        {
            let transactions = transaction_store
                .lock()
                .expect("transaction store should lock");
            assert_eq!(transactions.len(), 1);
            let transaction = transactions
                .values()
                .next()
                .expect("initial gift transaction should exist");
            assert_eq!(transaction.wallet_id, first.wallet.id);
            assert_eq!(transaction.category, "gift");
            assert_eq!(transaction.reason_code, "gift_initial");
            assert_eq!(transaction.amount, 7.5);
            assert_eq!(transaction.balance_before, 0.0);
            assert_eq!(transaction.balance_after, 7.5);
            assert_eq!(transaction.gift_balance_before, 0.0);
            assert_eq!(transaction.gift_balance_after, 7.5);
            assert_eq!(transaction.link_type.as_deref(), Some("system_task"));
            assert_eq!(transaction.link_id.as_deref(), Some(user_id));
            assert!(transaction.operator_id.is_none());
        }

        let replay = state
            .initialize_auth_user_wallet_with_outcome(user_id, 99.0, false)
            .await
            .expect("wallet replay should resolve")
            .expect("existing wallet should be returned");
        assert!(!replay.created);
        assert_eq!(replay.wallet.id, first.wallet.id);
        assert_eq!(
            transaction_store
                .lock()
                .expect("transaction store should lock")
                .len(),
            1
        );

        state
            .rollback_provisional_auth_user_with_wallet(user_id, Some(first.wallet.id.as_str()))
            .await
            .expect("rollback should remove the untouched wallet");
        assert!(state
            .find_wallet(WalletLookupKey::UserId(user_id))
            .await
            .expect("wallet lookup should resolve")
            .is_none());
        assert!(state
            .find_user_auth_by_id(user_id)
            .await
            .expect("user lookup should resolve")
            .is_none());
        assert!(transaction_store
            .lock()
            .expect("transaction store should lock")
            .is_empty());
    }
}
