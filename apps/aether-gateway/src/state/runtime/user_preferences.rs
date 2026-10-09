use crate::{AppState, GatewayError, GatewayUserPreferenceView};

impl AppState {
    pub(crate) async fn read_user_preferences(
        &self,
        user_id: &str,
    ) -> Result<Option<GatewayUserPreferenceView>, GatewayError> {
        self.data
            .read_user_preferences(user_id)
            .await
            .map(|value| value.map(Into::into))
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn write_user_preferences<T>(
        &self,
        preferences: T,
    ) -> Result<Option<GatewayUserPreferenceView>, GatewayError>
    where
        T: Into<GatewayUserPreferenceView>,
    {
        let preferences = preferences.into();
        let raw_preferences: crate::data::state::StoredUserPreferenceRecord = preferences.into();
        let persisted = self
            .data
            .write_user_preferences(&raw_preferences)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if persisted.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(persisted.map(Into::into))
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use aether_data::repository::users::InMemoryUserReadRepository;

    use super::*;
    use crate::data::GatewayDataState;

    #[tokio::test]
    async fn wallet_overage_preference_changes_clear_cached_billing_access() {
        let repository = Arc::new(InMemoryUserReadRepository::default());
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(GatewayDataState::with_user_reader_for_tests(repository));
        let user_id = "user-wallet-overage".to_string();
        let ttl = Duration::from_secs(60);
        let mut preferences = GatewayUserPreferenceView::default_for_user(&user_id);

        for enabled in [true, false] {
            state
                .auth_daily_quota_availability_cache
                .insert(user_id.clone(), None, ttl);
            state
                .auth_plan_usage_policy_cache
                .insert(user_id.clone(), None, ttl);
            state
                .auth_wallet_snapshot_cache
                .insert(user_id.clone(), None, ttl);
            assert!(state
                .auth_daily_quota_availability_cache
                .get(&user_id, ttl)
                .is_some());
            assert!(state
                .auth_plan_usage_policy_cache
                .get(&user_id, ttl)
                .is_some());
            assert!(state
                .auth_wallet_snapshot_cache
                .get(&user_id, ttl)
                .is_some());

            preferences.allow_wallet_overage = enabled;
            let persisted = state
                .write_user_preferences(&preferences)
                .await
                .expect("preferences should persist")
                .expect("preferences should exist");
            assert_eq!(persisted.allow_wallet_overage, enabled);
            assert!(state
                .auth_daily_quota_availability_cache
                .get(&user_id, ttl)
                .is_none());
            assert!(state
                .auth_plan_usage_policy_cache
                .get(&user_id, ttl)
                .is_none());
            assert!(state
                .auth_wallet_snapshot_cache
                .get(&user_id, ttl)
                .is_none());
        }
    }
}
