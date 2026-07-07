use aether_data::repository::wallet::{
    AdjustWalletBalanceInput, CompleteAdminWalletRefundInput, CreateManualWalletRechargeInput,
    CreatePlanPurchaseOrderInput, CreatePlanPurchaseOrderOutcome, CreateWalletRechargeOrderInput,
    CreateWalletRechargeOrderOutcome, CreateWalletRefundRequestInput,
    CreateWalletRefundRequestOutcome, CreditAdminPaymentOrderInput, FailAdminWalletRefundInput,
    ProcessAdminWalletRefundInput, ProcessPaymentCallbackInput, ProcessPaymentCallbackOutcome,
    WalletMutationOutcome,
};

use crate::{AppState, GatewayError};

impl AppState {
    pub(crate) async fn create_wallet_recharge_order(
        &self,
        input: CreateWalletRechargeOrderInput,
    ) -> Result<Option<CreateWalletRechargeOrderOutcome>, GatewayError> {
        self.data
            .create_wallet_recharge_order(input)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn create_plan_purchase_order(
        &self,
        input: CreatePlanPurchaseOrderInput,
    ) -> Result<Option<CreatePlanPurchaseOrderOutcome>, GatewayError> {
        self.data
            .create_plan_purchase_order(input)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn create_wallet_refund_request(
        &self,
        input: CreateWalletRefundRequestInput,
    ) -> Result<Option<CreateWalletRefundRequestOutcome>, GatewayError> {
        self.data
            .create_wallet_refund_request(input)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn process_payment_callback(
        &self,
        input: ProcessPaymentCallbackInput,
    ) -> Result<Option<ProcessPaymentCallbackOutcome>, GatewayError> {
        let outcome = self
            .data
            .process_payment_callback(input)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if matches!(outcome, Some(ProcessPaymentCallbackOutcome::Applied { .. })) {
            self.invalidate_auth_context_cache();
        }
        Ok(outcome)
    }

    pub(crate) async fn adjust_wallet_balance(
        &self,
        input: AdjustWalletBalanceInput,
    ) -> Result<
        Option<(
            aether_data::repository::wallet::StoredWalletSnapshot,
            aether_data::repository::wallet::StoredAdminWalletTransaction,
        )>,
        GatewayError,
    > {
        let result = self
            .data
            .adjust_wallet_balance(input)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if result.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(result)
    }

    pub(crate) async fn create_manual_wallet_recharge(
        &self,
        input: CreateManualWalletRechargeInput,
    ) -> Result<
        Option<(
            aether_data::repository::wallet::StoredWalletSnapshot,
            aether_data::repository::wallet::StoredAdminPaymentOrder,
        )>,
        GatewayError,
    > {
        let result = self
            .data
            .create_manual_wallet_recharge(input)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if result.is_some() {
            self.invalidate_auth_context_cache();
        }
        Ok(result)
    }

    pub(crate) async fn process_admin_wallet_refund(
        &self,
        input: ProcessAdminWalletRefundInput,
    ) -> Result<
        Option<
            WalletMutationOutcome<(
                aether_data::repository::wallet::StoredWalletSnapshot,
                aether_data::repository::wallet::StoredAdminWalletRefund,
                aether_data::repository::wallet::StoredAdminWalletTransaction,
            )>,
        >,
        GatewayError,
    > {
        let outcome = self
            .data
            .process_admin_wallet_refund(input)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if matches!(outcome, Some(WalletMutationOutcome::Applied(_))) {
            self.invalidate_auth_context_cache();
        }
        Ok(outcome)
    }

    pub(crate) async fn complete_admin_wallet_refund(
        &self,
        input: CompleteAdminWalletRefundInput,
    ) -> Result<
        Option<WalletMutationOutcome<aether_data::repository::wallet::StoredAdminWalletRefund>>,
        GatewayError,
    > {
        self.data
            .complete_admin_wallet_refund(input)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn fail_admin_wallet_refund(
        &self,
        input: FailAdminWalletRefundInput,
    ) -> Result<
        Option<
            WalletMutationOutcome<(
                aether_data::repository::wallet::StoredWalletSnapshot,
                aether_data::repository::wallet::StoredAdminWalletRefund,
                Option<aether_data::repository::wallet::StoredAdminWalletTransaction>,
            )>,
        >,
        GatewayError,
    > {
        let outcome = self
            .data
            .fail_admin_wallet_refund(input)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if matches!(
            outcome,
            Some(WalletMutationOutcome::Applied((_, _, Some(_))))
        ) {
            self.invalidate_auth_context_cache();
        }
        Ok(outcome)
    }

    pub(crate) async fn expire_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<
        Option<
            WalletMutationOutcome<(
                aether_data::repository::wallet::StoredAdminPaymentOrder,
                bool,
            )>,
        >,
        GatewayError,
    > {
        self.data
            .expire_admin_payment_order(order_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn fail_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<
        Option<WalletMutationOutcome<aether_data::repository::wallet::StoredAdminPaymentOrder>>,
        GatewayError,
    > {
        self.data
            .fail_admin_payment_order(order_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn credit_admin_payment_order(
        &self,
        input: CreditAdminPaymentOrderInput,
    ) -> Result<
        Option<
            WalletMutationOutcome<(
                aether_data::repository::wallet::StoredAdminPaymentOrder,
                bool,
            )>,
        >,
        GatewayError,
    > {
        let outcome = self
            .data
            .credit_admin_payment_order(input)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        if matches!(outcome, Some(WalletMutationOutcome::Applied((_, true)))) {
            self.invalidate_auth_context_cache();
        }
        Ok(outcome)
    }
}
