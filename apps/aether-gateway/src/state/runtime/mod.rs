use super::super::error::GatewayError;
use super::super::{scheduler, usage};
use super::{
    AdminBillingCollectorRecord, AdminBillingCollectorWriteInput, AdminBillingPresetApplyResult,
    AdminBillingRuleRecord, AdminBillingRuleWriteInput, AdminPaymentCallbackRecord,
    AdminSecurityBlacklistEntry, AdminWalletMutationOutcome, AdminWalletPaymentOrderRecord,
    AdminWalletRefundRecord, AdminWalletTransactionRecord, AppState, LocalMutationOutcome,
};

mod announcements;
mod api_key_exports;
mod auth;
mod billing;
mod candidate_queries;
mod gemini_files;
mod payments;
mod security;
mod shadow_results;
mod usage_queries;
mod user_preferences;
mod wallet;

impl AppState {
    pub fn has_announcement_data_reader(&self) -> bool {
        self.data.has_announcement_reader()
    }

    pub fn has_announcement_data_writer(&self) -> bool {
        self.data.has_announcement_writer()
    }

    pub fn has_video_task_data_reader(&self) -> bool {
        self.data.has_video_task_reader()
    }

    pub fn has_video_task_data_writer(&self) -> bool {
        self.data.has_video_task_writer()
    }

    pub fn has_request_candidate_data_reader(&self) -> bool {
        self.data.has_request_candidate_reader()
    }

    pub fn has_request_candidate_data_writer(&self) -> bool {
        self.data.has_request_candidate_writer()
    }

    pub fn has_usage_data_reader(&self) -> bool {
        self.data.has_usage_reader()
    }

    pub fn has_user_data_reader(&self) -> bool {
        self.data.has_user_reader()
    }

    pub fn has_auth_user_data_reader(&self) -> bool {
        #[cfg(test)]
        if self.auth_user_store.is_some() {
            return true;
        }

        self.data.has_user_reader()
    }

    pub fn has_usage_data_writer(&self) -> bool {
        self.data.has_usage_writer()
    }

    pub fn has_usage_worker_backend(&self) -> bool {
        self.data.has_usage_worker_runner()
    }

    pub fn has_wallet_data_reader(&self) -> bool {
        self.data.has_wallet_reader()
    }

    pub fn has_wallet_data_writer(&self) -> bool {
        self.data.has_wallet_writer()
    }

    pub fn has_auth_user_write_capability(&self) -> bool {
        #[cfg(test)]
        if self.auth_user_store.is_some() {
            return true;
        }

        self.postgres_pool().is_some()
    }

    pub fn has_auth_wallet_write_capability(&self) -> bool {
        #[cfg(test)]
        if self.auth_wallet_store.is_some() {
            return true;
        }

        self.postgres_pool().is_some()
    }

    pub fn has_provider_quota_data_writer(&self) -> bool {
        self.data.has_provider_quota_writer()
    }

    pub fn has_shadow_result_data_writer(&self) -> bool {
        self.data.has_shadow_result_writer()
    }

    pub fn has_shadow_result_data_reader(&self) -> bool {
        self.data.has_shadow_result_reader()
    }

    pub(crate) async fn count_active_admin_users(&self) -> Result<u64, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_user_store.as_ref() {
            let total = store
                .lock()
                .expect("auth user store should lock")
                .values()
                .filter(|user| {
                    user.role.eq_ignore_ascii_case("admin") && user.is_active && !user.is_deleted
                })
                .count() as u64;
            return Ok(total);
        }

        self.data
            .count_active_admin_users()
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_user_pending_refunds(
        &self,
        user_id: &str,
    ) -> Result<u64, GatewayError> {
        #[cfg(test)]
        {
            let _ = user_id;
            if self.auth_user_store.is_some() {
                return Ok(0);
            }
        }

        self.data
            .count_user_pending_refunds(user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn count_user_pending_payment_orders(
        &self,
        user_id: &str,
    ) -> Result<u64, GatewayError> {
        #[cfg(test)]
        {
            let _ = user_id;
            if self.auth_user_store.is_some() {
                return Ok(0);
            }
        }

        self.data
            .count_user_pending_payment_orders(user_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }
}
