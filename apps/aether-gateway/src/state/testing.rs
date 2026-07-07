use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use aether_contracts::{ExecutionPlan, ExecutionResult};
use aether_data_contracts::repository::candidates::RequestCandidateReadRepository;
use aether_data_contracts::repository::provider_catalog::ProviderCatalogReadRepository;
use aether_data_contracts::repository::usage::{UsageReadRepository, UsageRepository};
use aether_data_contracts::repository::video_tasks::{
    VideoTaskReadRepository, VideoTaskRepository,
};
use serde_json::json;

use super::{AppState, FrontdoorRuntimeGuardConfig, GatewayDataState};
use crate::{provider_transport, usage};

#[cfg(test)]
impl AppState {
    pub(crate) fn with_data_state_for_tests(mut self, data_state: GatewayDataState) -> Self {
        self.replace_data_state(Arc::new(data_state));
        self.request_candidate_queue = None;
        self
    }

    pub(crate) fn without_request_candidate_queue_for_tests(mut self) -> Self {
        self.request_candidate_queue = None;
        self
    }

    pub(crate) fn with_turnstile_siteverify_url_for_tests(mut self, url: &str) -> Self {
        self.turnstile_siteverify_url_override = Some(url.trim().to_string());
        self
    }

    pub(crate) fn with_turnstile_siteverify_timeout_for_tests(mut self, timeout: Duration) -> Self {
        self.turnstile_siteverify_timeout_override = Some(timeout);
        self
    }

    pub(crate) fn with_frontdoor_runtime_guard_config_for_tests(
        mut self,
        config: FrontdoorRuntimeGuardConfig,
    ) -> Self {
        self.frontdoor_runtime_guards = Arc::new(config);
        self
    }

    pub(crate) fn with_tunnel_identity_for_tests(
        mut self,
        instance_id: &str,
        relay_base_url: Option<&str>,
    ) -> Self {
        self.tunnel = crate::tunnel::EmbeddedTunnelState::with_data_and_directory(
            Arc::clone(&self.data),
            crate::tunnel::TunnelAttachmentDirectory::for_tests(instance_id, relay_base_url, 90),
        );
        self
    }

    pub(crate) fn with_video_task_data_reader_for_tests(
        mut self,
        repository: Arc<dyn VideoTaskReadRepository>,
    ) -> Self {
        self.replace_data_state(Arc::new(
            GatewayDataState::with_video_task_reader_for_tests(repository),
        ));
        self
    }

    pub(crate) fn with_video_task_data_repository_for_tests<T>(mut self, repository: Arc<T>) -> Self
    where
        T: VideoTaskRepository + 'static,
    {
        self.replace_data_state(Arc::new(
            GatewayDataState::with_video_task_repository_for_tests(repository),
        ));
        self
    }

    pub(crate) fn with_video_task_repository_and_provider_transport_for_tests<T>(
        mut self,
        repository: Arc<T>,
        provider_catalog_repository: Arc<dyn ProviderCatalogReadRepository>,
        encryption_key: impl Into<String>,
    ) -> Self
    where
        T: VideoTaskRepository + 'static,
    {
        self.replace_data_state(Arc::new(
            GatewayDataState::with_video_task_repository_and_provider_transport_for_tests(
                repository,
                provider_catalog_repository,
                encryption_key,
            ),
        ));
        self
    }

    pub(crate) fn with_request_candidate_data_reader_for_tests(
        mut self,
        repository: Arc<dyn RequestCandidateReadRepository>,
    ) -> Self {
        self.replace_data_state(Arc::new(
            GatewayDataState::with_request_candidate_reader_for_tests(repository),
        ));
        self
    }

    pub(crate) fn with_decision_trace_data_readers_for_tests(
        mut self,
        request_candidate_repository: Arc<dyn RequestCandidateReadRepository>,
        provider_catalog_repository: Arc<dyn ProviderCatalogReadRepository>,
    ) -> Self {
        self.replace_data_state(Arc::new(
            GatewayDataState::with_decision_trace_readers_for_tests(
                request_candidate_repository,
                provider_catalog_repository,
            ),
        ));
        self
    }

    pub(crate) fn with_request_audit_data_readers_for_tests(
        mut self,
        auth_api_key_repository: Arc<dyn aether_data::repository::auth::AuthApiKeyReadRepository>,
        request_candidate_repository: Arc<dyn RequestCandidateReadRepository>,
        provider_catalog_repository: Arc<dyn ProviderCatalogReadRepository>,
        usage_repository: Arc<dyn UsageReadRepository>,
    ) -> Self {
        self.replace_data_state(Arc::new(
            GatewayDataState::with_request_audit_readers_for_tests(
                auth_api_key_repository,
                request_candidate_repository,
                provider_catalog_repository,
                usage_repository,
            ),
        ));
        self
    }

    pub(crate) fn with_auth_api_key_data_reader_for_tests(
        mut self,
        repository: Arc<dyn aether_data::repository::auth::AuthApiKeyReadRepository>,
    ) -> Self {
        self.replace_data_state(Arc::new(
            GatewayDataState::with_auth_api_key_reader_for_tests(repository),
        ));
        self
    }

    pub(crate) fn with_frontdoor_system_default_rpm_for_tests(mut self, limit: u32) -> Self {
        self.frontdoor_user_rpm = Arc::new(
            (*self.frontdoor_user_rpm)
                .clone()
                .with_system_default_limit_for_tests(limit),
        );
        self
    }

    pub(crate) fn with_usage_data_reader_for_tests(
        mut self,
        repository: Arc<dyn UsageReadRepository>,
    ) -> Self {
        self.replace_data_state(Arc::new(GatewayDataState::with_usage_reader_for_tests(
            repository,
        )));
        self
    }

    pub(crate) fn with_user_data_reader_for_tests(
        mut self,
        repository: Arc<dyn aether_data::repository::users::UserReadRepository>,
    ) -> Self {
        self.replace_data_state(Arc::new(GatewayDataState::with_user_reader_for_tests(
            repository,
        )));
        self
    }

    pub(crate) fn with_usage_data_repository_for_tests<T>(mut self, repository: Arc<T>) -> Self
    where
        T: UsageRepository + 'static,
    {
        self.replace_data_state(Arc::new(GatewayDataState::with_usage_repository_for_tests(
            repository,
        )));
        self
    }

    pub(crate) fn with_usage_runtime_for_tests(
        mut self,
        config: usage::UsageRuntimeConfig,
    ) -> Self {
        self.usage_runtime =
            Arc::new(usage::UsageRuntime::new(config).expect("usage runtime config should build"));
        self
    }

    pub(crate) fn with_execution_runtime_sync_override_for_tests<F>(
        mut self,
        override_fn: F,
    ) -> Self
    where
        F: Fn(&ExecutionPlan) -> Result<ExecutionResult, crate::GatewayError>
            + Send
            + Sync
            + 'static,
    {
        self.execution_runtime_sync_override = Some(super::app::TestExecutionRuntimeSyncOverride(
            Arc::new(override_fn),
        ));
        self
    }

    pub(crate) fn with_oauth_refresh_coordinator_for_tests(
        mut self,
        coordinator: provider_transport::LocalOAuthRefreshCoordinator,
    ) -> Self {
        self.oauth_refresh = Arc::new(coordinator);
        self
    }

    pub(crate) fn with_provider_oauth_state_entry_for_tests(
        mut self,
        nonce: &str,
        payload: serde_json::Value,
    ) -> Self {
        let store = self
            .provider_oauth_state_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        store
            .lock()
            .expect("provider oauth state store should lock")
            .insert(format!("provider_oauth_state:{nonce}"), payload.to_string());
        self.runtime_state.kv_set_local_nowait(
            &format!("provider_oauth_state:{nonce}"),
            payload.to_string(),
            Some(Duration::from_secs(
                aether_data::repository::provider_oauth::PROVIDER_OAUTH_STATE_TTL_SECS,
            )),
        );
        self
    }

    pub(crate) fn with_provider_oauth_device_session_entry_for_tests(
        mut self,
        session_id: &str,
        payload: serde_json::Value,
    ) -> Self {
        let store = self
            .provider_oauth_device_session_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        store
            .lock()
            .expect("provider oauth device session store should lock")
            .insert(
                format!("device_auth_session:{session_id}"),
                payload.to_string(),
            );
        self.runtime_state.kv_set_local_nowait(
            &format!("device_auth_session:{session_id}"),
            payload.to_string(),
            Some(Duration::from_secs(3600)),
        );
        self
    }

    pub(crate) fn with_provider_oauth_batch_task_entry_for_tests(
        mut self,
        task_id: &str,
        payload: serde_json::Value,
    ) -> Self {
        let store = self
            .provider_oauth_batch_task_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        store
            .lock()
            .expect("provider oauth batch task store should lock")
            .insert(
                format!("provider_oauth_batch_task:{task_id}"),
                payload.to_string(),
            );
        self.runtime_state.kv_set_local_nowait(
            &format!("provider_oauth_batch_task:{task_id}"),
            payload.to_string(),
            Some(Duration::from_secs(
                aether_data::repository::provider_oauth::PROVIDER_OAUTH_BATCH_TASK_TTL_SECS,
            )),
        );
        self
    }

    pub(crate) fn with_auth_session_for_tests(
        self,
        session: crate::data::state::StoredUserSessionRecord,
    ) -> Self {
        self.with_auth_sessions_for_tests([session])
    }

    pub(crate) fn with_auth_sessions_for_tests<I>(mut self, sessions: I) -> Self
    where
        I: IntoIterator<Item = crate::data::state::StoredUserSessionRecord>,
    {
        let store = self
            .auth_session_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        let mut guard = store.lock().expect("auth session store should lock");
        for session in sessions {
            guard.insert(format!("{}:{}", session.user_id, session.id), session);
        }
        drop(guard);
        self
    }

    pub(crate) fn with_auth_users_for_tests<I>(mut self, users: I) -> Self
    where
        I: IntoIterator<Item = aether_data::repository::users::StoredUserAuthRecord>,
    {
        let store = self
            .auth_user_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        let mut guard = store.lock().expect("auth user store should lock");
        for user in users {
            guard.insert(user.id.clone(), user);
        }
        drop(guard);
        self
    }

    pub(crate) fn without_auth_user_store_for_tests(mut self) -> Self {
        self.auth_user_store = None;
        self
    }

    pub(crate) fn without_auth_user_model_capability_store_for_tests(mut self) -> Self {
        self.auth_user_model_capability_store = None;
        self
    }

    pub(crate) fn with_auth_wallets_for_tests<I>(mut self, wallets: I) -> Self
    where
        I: IntoIterator<Item = aether_data::repository::wallet::StoredWalletSnapshot>,
    {
        let store = self
            .auth_wallet_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        let mut guard = store.lock().expect("auth wallet store should lock");
        for wallet in wallets {
            guard.insert(wallet.id.clone(), wallet);
        }
        drop(guard);
        self
    }

    pub(crate) fn with_admin_wallet_payment_orders_for_tests<I>(mut self, orders: I) -> Self
    where
        I: IntoIterator<Item = crate::AdminWalletPaymentOrderRecord>,
    {
        let store = self
            .admin_wallet_payment_order_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        let mut guard = store
            .lock()
            .expect("admin wallet payment order store should lock");
        for order in orders {
            guard.insert(order.id.clone(), order);
        }
        drop(guard);
        self
    }

    pub(crate) fn with_admin_payment_callbacks_for_tests<I>(mut self, callbacks: I) -> Self
    where
        I: IntoIterator<Item = crate::state::AdminPaymentCallbackRecord>,
    {
        let store = self
            .admin_payment_callback_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        let mut guard = store
            .lock()
            .expect("admin payment callback store should lock");
        for callback in callbacks {
            guard.insert(callback.id.clone(), callback);
        }
        drop(guard);
        self
    }

    pub(crate) fn with_admin_wallet_transactions_for_tests<I>(mut self, transactions: I) -> Self
    where
        I: IntoIterator<Item = crate::AdminWalletTransactionRecord>,
    {
        let store = self
            .admin_wallet_transaction_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        let mut guard = store
            .lock()
            .expect("admin wallet transaction store should lock");
        for transaction in transactions {
            guard.insert(transaction.id.clone(), transaction);
        }
        drop(guard);
        self
    }

    pub(crate) fn with_admin_wallet_refunds_for_tests<I>(mut self, refunds: I) -> Self
    where
        I: IntoIterator<Item = crate::AdminWalletRefundRecord>,
    {
        let store = self
            .admin_wallet_refund_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        let mut guard = store.lock().expect("admin wallet refund store should lock");
        for refund in refunds {
            guard.insert(refund.id.clone(), refund);
        }
        drop(guard);
        self
    }

    pub(crate) fn with_admin_billing_rules_for_tests<I>(mut self, rules: I) -> Self
    where
        I: IntoIterator<Item = crate::AdminBillingRuleRecord>,
    {
        let store = self
            .admin_billing_rule_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        let mut guard = store.lock().expect("admin billing rule store should lock");
        for rule in rules {
            guard.insert(rule.id.clone(), rule);
        }
        drop(guard);
        self
    }

    pub(crate) fn with_admin_billing_collectors_for_tests<I>(mut self, collectors: I) -> Self
    where
        I: IntoIterator<Item = crate::AdminBillingCollectorRecord>,
    {
        let store = self
            .admin_billing_collector_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        let mut guard = store
            .lock()
            .expect("admin billing collector store should lock");
        for collector in collectors {
            guard.insert(collector.id.clone(), collector);
        }
        drop(guard);
        self
    }

    pub(crate) fn with_admin_security_blacklist_for_tests<I>(mut self, entries: I) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let store = self
            .admin_security_blacklist_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        let mut guard = store
            .lock()
            .expect("admin security blacklist store should lock");
        for (ip_address, reason) in entries {
            self.runtime_state.kv_set_local_nowait(
                &format!("ip:blacklist:{ip_address}"),
                reason.clone(),
                None,
            );
            guard.insert(ip_address, reason);
        }
        drop(guard);
        self
    }

    pub(crate) fn with_admin_security_whitelist_for_tests<I>(mut self, entries: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        let store = self
            .admin_security_whitelist_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(std::collections::BTreeSet::new())));
        let mut guard = store
            .lock()
            .expect("admin security whitelist store should lock");
        for ip_address in entries {
            self.runtime_state
                .set_add_local_nowait("ip:whitelist", &ip_address);
            guard.insert(ip_address);
        }
        drop(guard);
        self
    }

    pub(crate) fn with_admin_monitoring_cache_affinity_entry_for_tests(
        mut self,
        cache_key: &str,
        payload: serde_json::Value,
    ) -> Self {
        let store = self
            .admin_monitoring_cache_affinity_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        store
            .lock()
            .expect("admin monitoring cache affinity store should lock")
            .insert(cache_key.to_string(), payload.to_string());
        self
    }

    pub(crate) fn list_admin_monitoring_cache_affinity_entries_for_tests(
        &self,
    ) -> Vec<(String, String)> {
        self.admin_monitoring_cache_affinity_store
            .as_ref()
            .map(|store| {
                store
                    .lock()
                    .expect("admin monitoring cache affinity store should lock")
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn remove_admin_monitoring_cache_affinity_entries_for_tests(
        &self,
        raw_keys: &[String],
    ) -> usize {
        let Some(store) = self.admin_monitoring_cache_affinity_store.as_ref() else {
            return 0;
        };
        let mut guard = store
            .lock()
            .expect("admin monitoring cache affinity store should lock");
        raw_keys
            .iter()
            .filter(|raw_key| guard.remove(raw_key.as_str()).is_some())
            .count()
    }

    pub(crate) fn with_admin_monitoring_redis_key_for_tests(
        mut self,
        cache_key: &str,
        payload: serde_json::Value,
    ) -> Self {
        let store = self
            .admin_monitoring_redis_key_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        store
            .lock()
            .expect("admin monitoring redis key store should lock")
            .insert(cache_key.to_string(), payload.to_string());
        self
    }

    pub(crate) fn list_admin_monitoring_redis_keys_for_tests(&self) -> Vec<String> {
        self.admin_monitoring_redis_key_store
            .as_ref()
            .map(|store| {
                store
                    .lock()
                    .expect("admin monitoring redis key store should lock")
                    .keys()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn remove_admin_monitoring_redis_keys_for_tests(
        &self,
        raw_keys: &[String],
    ) -> usize {
        let Some(store) = self.admin_monitoring_redis_key_store.as_ref() else {
            return 0;
        };
        let mut guard = store
            .lock()
            .expect("admin monitoring redis key store should lock");
        raw_keys
            .iter()
            .filter(|raw_key| guard.remove(raw_key.as_str()).is_some())
            .count()
    }

    pub(crate) fn with_auth_email_verification_pending_for_tests(
        mut self,
        email: &str,
        code: &str,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        let store = self
            .auth_email_verification_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        store
            .lock()
            .expect("auth email verification store should lock")
            .insert(
                format!("email:verification:{}", email.trim().to_ascii_lowercase()),
                json!({
                    "code": code,
                    "created_at": created_at.to_rfc3339(),
                })
                .to_string(),
            );
        self.runtime_state.kv_set_local_nowait(
            &format!("email:verification:{}", email.trim().to_ascii_lowercase()),
            json!({
                "code": code,
                "created_at": created_at.to_rfc3339(),
            })
            .to_string(),
            Some(Duration::from_secs(600)),
        );
        self
    }

    pub(crate) fn with_auth_email_verified_for_tests(mut self, email: &str) -> Self {
        let store = self
            .auth_email_verification_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        store
            .lock()
            .expect("auth email verification store should lock")
            .insert(
                format!("email:verified:{}", email.trim().to_ascii_lowercase()),
                "verified".to_string(),
            );
        self.runtime_state.kv_set_local_nowait(
            &format!("email:verified:{}", email.trim().to_ascii_lowercase()),
            "verified".to_string(),
            Some(Duration::from_secs(3600)),
        );
        self
    }

    pub(crate) fn with_auth_user_model_capability_settings_for_tests(
        mut self,
        user_id: &str,
        settings: serde_json::Value,
    ) -> Self {
        let store = self
            .auth_user_model_capability_store
            .get_or_insert_with(|| Arc::new(StdMutex::new(HashMap::new())));
        store
            .lock()
            .expect("auth user model capability store should lock")
            .insert(user_id.to_string(), settings);
        self
    }

    pub(crate) fn with_provider_oauth_token_url_for_tests(
        self,
        provider_type: &str,
        token_url: impl Into<String>,
    ) -> Self {
        self.provider_oauth_token_url_overrides
            .lock()
            .expect("provider oauth token url overrides should lock")
            .insert(provider_type.trim().to_ascii_lowercase(), token_url.into());
        self
    }
}
