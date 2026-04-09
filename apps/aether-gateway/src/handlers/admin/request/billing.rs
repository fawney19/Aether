use super::AdminAppState;
use crate::GatewayError;

impl<'a> AdminAppState<'a> {
    pub(crate) async fn list_admin_billing_collectors(
        &self,
        api_format: Option<&str>,
        task_type: Option<&str>,
        dimension_name: Option<&str>,
        is_enabled: Option<bool>,
        page: u32,
        page_size: u32,
    ) -> Result<Option<(Vec<crate::AdminBillingCollectorRecord>, u64)>, GatewayError> {
        self.app
            .list_admin_billing_collectors(
                api_format,
                task_type,
                dimension_name,
                is_enabled,
                page,
                page_size,
            )
            .await
    }

    pub(crate) async fn read_admin_billing_collector(
        &self,
        collector_id: &str,
    ) -> Result<Option<crate::AdminBillingCollectorRecord>, GatewayError> {
        self.app.read_admin_billing_collector(collector_id).await
    }

    pub(crate) async fn create_admin_billing_collector(
        &self,
        input: &crate::AdminBillingCollectorWriteInput,
    ) -> Result<crate::LocalMutationOutcome<crate::AdminBillingCollectorRecord>, GatewayError> {
        self.app.create_admin_billing_collector(input).await
    }

    pub(crate) async fn update_admin_billing_collector(
        &self,
        collector_id: &str,
        input: &crate::AdminBillingCollectorWriteInput,
    ) -> Result<crate::LocalMutationOutcome<crate::AdminBillingCollectorRecord>, GatewayError> {
        self.app
            .update_admin_billing_collector(collector_id, input)
            .await
    }

    pub(crate) async fn admin_billing_enabled_default_value_exists(
        &self,
        api_format: &str,
        task_type: &str,
        dimension_name: &str,
        existing_id: Option<&str>,
    ) -> Result<bool, GatewayError> {
        self.app
            .admin_billing_enabled_default_value_exists(
                api_format,
                task_type,
                dimension_name,
                existing_id,
            )
            .await
    }

    pub(crate) async fn apply_admin_billing_preset(
        &self,
        preset: &str,
        mode: &str,
        collectors: &[crate::AdminBillingCollectorWriteInput],
    ) -> Result<
        crate::LocalMutationOutcome<crate::state::AdminBillingPresetApplyResult>,
        GatewayError,
    > {
        self.app
            .apply_admin_billing_preset(preset, mode, collectors)
            .await
    }

    pub(crate) async fn list_admin_billing_rules(
        &self,
        task_type: Option<&str>,
        is_enabled: Option<bool>,
        page: u32,
        page_size: u32,
    ) -> Result<Option<(Vec<crate::AdminBillingRuleRecord>, u64)>, GatewayError> {
        self.app
            .list_admin_billing_rules(task_type, is_enabled, page, page_size)
            .await
    }

    pub(crate) async fn read_admin_billing_rule(
        &self,
        rule_id: &str,
    ) -> Result<Option<crate::AdminBillingRuleRecord>, GatewayError> {
        self.app.read_admin_billing_rule(rule_id).await
    }

    pub(crate) async fn create_admin_billing_rule(
        &self,
        input: &crate::AdminBillingRuleWriteInput,
    ) -> Result<crate::LocalMutationOutcome<crate::AdminBillingRuleRecord>, GatewayError> {
        self.app.create_admin_billing_rule(input).await
    }

    pub(crate) async fn update_admin_billing_rule(
        &self,
        rule_id: &str,
        input: &crate::AdminBillingRuleWriteInput,
    ) -> Result<crate::LocalMutationOutcome<crate::AdminBillingRuleRecord>, GatewayError> {
        self.app.update_admin_billing_rule(rule_id, input).await
    }

    pub(crate) async fn find_wallet(
        &self,
        lookup: aether_data::repository::wallet::WalletLookupKey<'_>,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        self.app.find_wallet(lookup).await
    }

    pub(crate) async fn list_admin_wallets(
        &self,
        status: Option<&str>,
        owner_type: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<
        (
            Vec<aether_data::repository::wallet::StoredAdminWalletListItem>,
            u64,
        ),
        GatewayError,
    > {
        self.app
            .list_admin_wallets(status, owner_type, limit, offset)
            .await
    }

    pub(crate) async fn list_admin_wallet_ledger(
        &self,
        category: Option<&str>,
        reason_code: Option<&str>,
        owner_type: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<
        (
            Vec<aether_data::repository::wallet::StoredAdminWalletLedgerItem>,
            u64,
        ),
        GatewayError,
    > {
        self.app
            .list_admin_wallet_ledger(category, reason_code, owner_type, limit, offset)
            .await
    }

    pub(crate) async fn list_admin_wallet_refund_requests(
        &self,
        status: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<
        (
            Vec<aether_data::repository::wallet::StoredAdminWalletRefundRequestItem>,
            u64,
        ),
        GatewayError,
    > {
        self.app
            .list_admin_wallet_refund_requests(status, limit, offset)
            .await
    }

    pub(crate) async fn list_admin_wallet_transactions(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<
        (
            Vec<aether_data::repository::wallet::StoredAdminWalletTransaction>,
            u64,
        ),
        GatewayError,
    > {
        self.app
            .list_admin_wallet_transactions(wallet_id, limit, offset)
            .await
    }

    pub(crate) async fn list_admin_wallet_refunds(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<crate::AdminWalletRefundRecord>, u64), GatewayError> {
        self.app
            .list_admin_wallet_refunds(wallet_id, limit, offset)
            .await
    }

    pub(crate) async fn list_admin_payment_orders(
        &self,
        status: Option<&str>,
        payment_method: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Option<(Vec<crate::AdminWalletPaymentOrderRecord>, u64)>, GatewayError> {
        self.app
            .list_admin_payment_orders(status, payment_method, limit, offset)
            .await
    }

    pub(crate) async fn list_admin_payment_callbacks(
        &self,
        payment_method: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Option<(Vec<crate::GatewayAdminPaymentCallbackView>, u64)>, GatewayError> {
        self.app
            .list_admin_payment_callbacks(payment_method, limit, offset)
            .await
    }

    pub(crate) async fn read_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<crate::AdminWalletMutationOutcome<crate::AdminWalletPaymentOrderRecord>, GatewayError>
    {
        self.app.read_admin_payment_order(order_id).await
    }

    pub(crate) async fn admin_expire_payment_order(
        &self,
        order_id: &str,
    ) -> Result<
        crate::AdminWalletMutationOutcome<(crate::AdminWalletPaymentOrderRecord, bool)>,
        GatewayError,
    > {
        self.app.admin_expire_payment_order(order_id).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn admin_credit_payment_order(
        &self,
        order_id: &str,
        gateway_order_id: Option<&str>,
        pay_amount: Option<f64>,
        pay_currency: Option<&str>,
        exchange_rate: Option<f64>,
        gateway_response_patch: Option<serde_json::Value>,
        operator_id: Option<&str>,
    ) -> Result<
        crate::AdminWalletMutationOutcome<(crate::AdminWalletPaymentOrderRecord, bool)>,
        GatewayError,
    > {
        self.app
            .admin_credit_payment_order(
                order_id,
                gateway_order_id,
                pay_amount,
                pay_currency,
                exchange_rate,
                gateway_response_patch,
                operator_id,
            )
            .await
    }

    pub(crate) async fn admin_fail_payment_order(
        &self,
        order_id: &str,
    ) -> Result<crate::AdminWalletMutationOutcome<crate::AdminWalletPaymentOrderRecord>, GatewayError>
    {
        self.app.admin_fail_payment_order(order_id).await
    }

    pub(crate) async fn admin_adjust_wallet_balance(
        &self,
        wallet_id: &str,
        amount_usd: f64,
        balance_type: &str,
        operator_id: Option<&str>,
        description: Option<&str>,
    ) -> Result<
        Option<(
            aether_data::repository::wallet::StoredWalletSnapshot,
            crate::AdminWalletTransactionRecord,
        )>,
        GatewayError,
    > {
        self.app
            .admin_adjust_wallet_balance(
                wallet_id,
                amount_usd,
                balance_type,
                operator_id,
                description,
            )
            .await
    }

    pub(crate) async fn admin_create_manual_wallet_recharge(
        &self,
        wallet_id: &str,
        amount_usd: f64,
        payment_method: &str,
        operator_id: Option<&str>,
        description: Option<&str>,
    ) -> Result<
        Option<(
            aether_data::repository::wallet::StoredWalletSnapshot,
            crate::AdminWalletPaymentOrderRecord,
        )>,
        GatewayError,
    > {
        self.app
            .admin_create_manual_wallet_recharge(
                wallet_id,
                amount_usd,
                payment_method,
                operator_id,
                description,
            )
            .await
    }

    pub(crate) async fn admin_process_wallet_refund(
        &self,
        wallet_id: &str,
        refund_id: &str,
        operator_id: Option<&str>,
    ) -> Result<
        crate::AdminWalletMutationOutcome<(
            aether_data::repository::wallet::StoredWalletSnapshot,
            crate::AdminWalletRefundRecord,
            crate::AdminWalletTransactionRecord,
        )>,
        GatewayError,
    > {
        self.app
            .admin_process_wallet_refund(wallet_id, refund_id, operator_id)
            .await
    }

    pub(crate) async fn admin_complete_wallet_refund(
        &self,
        wallet_id: &str,
        refund_id: &str,
        gateway_refund_id: Option<&str>,
        payout_reference: Option<&str>,
        payout_proof: Option<serde_json::Value>,
    ) -> Result<crate::AdminWalletMutationOutcome<crate::AdminWalletRefundRecord>, GatewayError>
    {
        self.app
            .admin_complete_wallet_refund(
                wallet_id,
                refund_id,
                gateway_refund_id,
                payout_reference,
                payout_proof,
            )
            .await
    }

    pub(crate) async fn admin_fail_wallet_refund(
        &self,
        wallet_id: &str,
        refund_id: &str,
        reason: &str,
        operator_id: Option<&str>,
    ) -> Result<
        crate::AdminWalletMutationOutcome<(
            aether_data::repository::wallet::StoredWalletSnapshot,
            crate::AdminWalletRefundRecord,
            Option<crate::AdminWalletTransactionRecord>,
        )>,
        GatewayError,
    > {
        self.app
            .admin_fail_wallet_refund(wallet_id, refund_id, reason, operator_id)
            .await
    }
}
