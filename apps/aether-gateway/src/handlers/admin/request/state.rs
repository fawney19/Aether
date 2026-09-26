use crate::{AppState, GatewayError};

#[derive(Clone, Copy)]
pub(crate) struct AdminAppState<'a> {
    pub(super) app: &'a AppState,
}

impl<'a> AdminAppState<'a> {
    pub(crate) fn new(app: &'a AppState) -> Self {
        Self { app }
    }

    pub(crate) fn app(&self) -> &AppState {
        self.app
    }

    pub(crate) fn cloned_app(&self) -> AppState {
        self.app.clone()
    }

    pub(crate) async fn get_admin_user_wallet_balance_batch(
        &self,
        admin_user_id: &str,
        idempotency_key: &str,
        request_fingerprint: &str,
    ) -> Result<
        Option<aether_data::repository::wallet::PrepareAdminUserWalletBalanceBatchOutcome>,
        GatewayError,
    > {
        self.app
            .get_admin_user_wallet_balance_batch(
                admin_user_id,
                idempotency_key,
                request_fingerprint,
            )
            .await
    }

    pub(crate) async fn prepare_admin_user_wallet_balance_batch(
        &self,
        input: aether_data::repository::wallet::PrepareAdminUserWalletBalanceBatchInput,
    ) -> Result<
        aether_data::repository::wallet::PrepareAdminUserWalletBalanceBatchOutcome,
        GatewayError,
    > {
        self.app
            .prepare_admin_user_wallet_balance_batch(input)
            .await
    }

    pub(crate) async fn record_admin_user_wallet_balance_batch_failure(
        &self,
        admin_user_id: &str,
        idempotency_key: &str,
        user_id: &str,
        reason: &str,
    ) -> Result<aether_data::repository::wallet::AdminUserWalletBalanceBatchUserOutcome, GatewayError>
    {
        self.app
            .record_admin_user_wallet_balance_batch_failure(
                admin_user_id,
                idempotency_key,
                user_id,
                reason,
            )
            .await
    }

    pub(crate) async fn adjust_admin_user_wallet_balance_batch_user(
        &self,
        input: aether_data::repository::wallet::AdjustWalletBalanceInBatchInput,
    ) -> Result<aether_data::repository::wallet::AdminUserWalletBalanceBatchUserOutcome, GatewayError>
    {
        self.app
            .adjust_admin_user_wallet_balance_batch_user(input)
            .await
    }
}

impl<'a> AsRef<AppState> for AdminAppState<'a> {
    fn as_ref(&self) -> &AppState {
        self.app
    }
}

pub(crate) type AdminRouteResponse = axum::http::Response<axum::body::Body>;
pub(crate) type AdminRouteResult = Result<Option<AdminRouteResponse>, GatewayError>;
