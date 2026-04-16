use super::wallet::admin_payment_gateway_response_map;
use crate::{AdminWalletMutationOutcome, AdminWalletPaymentOrderRecord, AppState, GatewayError};

impl AppState {
    pub(crate) async fn admin_expire_payment_order(
        &self,
        order_id: &str,
    ) -> Result<AdminWalletMutationOutcome<(AdminWalletPaymentOrderRecord, bool)>, GatewayError>
    {
        #[cfg(test)]
        if let Some(store) = self.admin_wallet_payment_order_store.as_ref() {
            let mut guard = store
                .lock()
                .expect("admin wallet payment order store should lock");
            let Some(order) = guard.get_mut(order_id) else {
                return Ok(AdminWalletMutationOutcome::NotFound);
            };
            if order.status == "credited" {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "credited order cannot be expired".to_string(),
                ));
            }
            if order.status == "expired" {
                return Ok(AdminWalletMutationOutcome::Applied((order.clone(), false)));
            }
            if order.status != "pending" {
                return Ok(AdminWalletMutationOutcome::Invalid(format!(
                    "only pending order can be expired: {}",
                    order.status
                )));
            }
            let mut gateway_response =
                admin_payment_gateway_response_map(order.gateway_response.take());
            gateway_response.insert(
                "expire_reason".to_string(),
                serde_json::Value::String("admin_mark_expired".to_string()),
            );
            gateway_response.insert(
                "expired_at".to_string(),
                serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
            );
            order.status = "expired".to_string();
            order.gateway_response = Some(serde_json::Value::Object(gateway_response));
            return Ok(AdminWalletMutationOutcome::Applied((order.clone(), true)));
        }

        match self.expire_admin_payment_order(order_id).await? {
            Some(aether_data::repository::wallet::WalletMutationOutcome::Applied((
                order,
                changed,
            ))) => Ok(AdminWalletMutationOutcome::Applied((
                stored_admin_payment_order_to_gateway(order),
                changed,
            ))),
            Some(aether_data::repository::wallet::WalletMutationOutcome::NotFound) => {
                Ok(AdminWalletMutationOutcome::NotFound)
            }
            Some(aether_data::repository::wallet::WalletMutationOutcome::Invalid(detail)) => {
                Ok(AdminWalletMutationOutcome::Invalid(detail))
            }
            None => Ok(AdminWalletMutationOutcome::Unavailable),
        }
    }

    pub(crate) async fn admin_fail_payment_order(
        &self,
        order_id: &str,
    ) -> Result<AdminWalletMutationOutcome<AdminWalletPaymentOrderRecord>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_wallet_payment_order_store.as_ref() {
            let mut guard = store
                .lock()
                .expect("admin wallet payment order store should lock");
            let Some(order) = guard.get_mut(order_id) else {
                return Ok(AdminWalletMutationOutcome::NotFound);
            };
            if order.status == "credited" {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "credited order cannot be failed".to_string(),
                ));
            }
            let mut gateway_response =
                admin_payment_gateway_response_map(order.gateway_response.take());
            gateway_response.insert(
                "failure_reason".to_string(),
                serde_json::Value::String("admin_mark_failed".to_string()),
            );
            gateway_response.insert(
                "failed_at".to_string(),
                serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
            );
            order.status = "failed".to_string();
            order.gateway_response = Some(serde_json::Value::Object(gateway_response));
            return Ok(AdminWalletMutationOutcome::Applied(order.clone()));
        }

        match self.fail_admin_payment_order(order_id).await? {
            Some(aether_data::repository::wallet::WalletMutationOutcome::Applied(order)) => Ok(
                AdminWalletMutationOutcome::Applied(stored_admin_payment_order_to_gateway(order)),
            ),
            Some(aether_data::repository::wallet::WalletMutationOutcome::NotFound) => {
                Ok(AdminWalletMutationOutcome::NotFound)
            }
            Some(aether_data::repository::wallet::WalletMutationOutcome::Invalid(detail)) => {
                Ok(AdminWalletMutationOutcome::Invalid(detail))
            }
            None => Ok(AdminWalletMutationOutcome::Unavailable),
        }
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
    ) -> Result<AdminWalletMutationOutcome<(AdminWalletPaymentOrderRecord, bool)>, GatewayError>
    {
        #[cfg(test)]
        if let (Some(order_store), Some(wallet_store)) = (
            self.admin_wallet_payment_order_store.as_ref(),
            self.auth_wallet_store.as_ref(),
        ) {
            let mut orders = order_store
                .lock()
                .expect("admin wallet payment order store should lock");
            let Some(order) = orders.get_mut(order_id) else {
                return Ok(AdminWalletMutationOutcome::NotFound);
            };
            if order.status == "credited" {
                return Ok(AdminWalletMutationOutcome::Applied((order.clone(), false)));
            }
            if matches!(order.status.as_str(), "failed" | "expired" | "refunded") {
                return Ok(AdminWalletMutationOutcome::Invalid(format!(
                    "payment order is not creditable: {}",
                    order.status
                )));
            }
            if order
                .expires_at_unix_secs
                .is_some_and(|value| value < chrono::Utc::now().timestamp().max(0) as u64)
            {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "payment order expired".to_string(),
                ));
            }

            let mut wallets = wallet_store.lock().expect("auth wallet store should lock");
            let Some(wallet) = wallets.get_mut(&order.wallet_id) else {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "wallet not found".to_string(),
                ));
            };
            if wallet.status != "active" {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "wallet is not active".to_string(),
                ));
            }

            let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
            let mut gateway_response =
                admin_payment_gateway_response_map(order.gateway_response.take());
            if let Some(serde_json::Value::Object(map)) = gateway_response_patch {
                gateway_response.extend(map);
            }
            gateway_response.insert("manual_credit".to_string(), serde_json::Value::Bool(true));
            gateway_response.insert(
                "credited_by".to_string(),
                operator_id
                    .map(|value| serde_json::Value::String(value.to_string()))
                    .unwrap_or(serde_json::Value::Null),
            );

            wallet.balance += order.amount_usd;
            wallet.total_recharged += order.amount_usd;
            wallet.updated_at_unix_secs = now_unix_secs;

            if let Some(value) = gateway_order_id {
                order.gateway_order_id = Some(value.to_string());
            }
            if let Some(value) = pay_amount {
                order.pay_amount = Some(value);
            }
            if let Some(value) = pay_currency {
                order.pay_currency = Some(value.to_string());
            }
            if let Some(value) = exchange_rate {
                order.exchange_rate = Some(value);
            }
            order.status = "credited".to_string();
            order.paid_at_unix_secs = order.paid_at_unix_secs.or(Some(now_unix_secs));
            order.credited_at_unix_secs = Some(now_unix_secs);
            order.refundable_amount_usd = order.amount_usd;
            order.gateway_response = Some(serde_json::Value::Object(gateway_response));
            return Ok(AdminWalletMutationOutcome::Applied((order.clone(), true)));
        }

        match self
            .credit_admin_payment_order(
                aether_data::repository::wallet::CreditAdminPaymentOrderInput {
                    order_id: order_id.to_string(),
                    gateway_order_id: gateway_order_id.map(ToOwned::to_owned),
                    pay_amount,
                    pay_currency: pay_currency.map(ToOwned::to_owned),
                    exchange_rate,
                    gateway_response_patch,
                    operator_id: operator_id.map(ToOwned::to_owned),
                },
            )
            .await?
        {
            Some(aether_data::repository::wallet::WalletMutationOutcome::Applied((
                order,
                changed,
            ))) => Ok(AdminWalletMutationOutcome::Applied((
                stored_admin_payment_order_to_gateway(order),
                changed,
            ))),
            Some(aether_data::repository::wallet::WalletMutationOutcome::NotFound) => {
                Ok(AdminWalletMutationOutcome::NotFound)
            }
            Some(aether_data::repository::wallet::WalletMutationOutcome::Invalid(detail)) => {
                Ok(AdminWalletMutationOutcome::Invalid(detail))
            }
            None => Ok(AdminWalletMutationOutcome::Unavailable),
        }
    }

    pub(crate) async fn admin_create_redeem_code_batch(
        &self,
        input: aether_data::repository::wallet::CreateAdminRedeemCodeBatchInput,
    ) -> Result<
        Option<aether_data::repository::wallet::CreateAdminRedeemCodeBatchResult>,
        GatewayError,
    > {
        self.data
            .create_admin_redeem_code_batch(input)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn admin_disable_redeem_code_batch(
        &self,
        batch_id: &str,
        operator_id: Option<&str>,
    ) -> Result<
        AdminWalletMutationOutcome<aether_data::repository::wallet::StoredAdminRedeemCodeBatch>,
        GatewayError,
    > {
        match self
            .data
            .disable_admin_redeem_code_batch(
                aether_data::repository::wallet::DisableAdminRedeemCodeBatchInput {
                    batch_id: batch_id.to_string(),
                    operator_id: operator_id.map(ToOwned::to_owned),
                },
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
        {
            Some(aether_data::repository::wallet::WalletMutationOutcome::Applied(batch)) => {
                Ok(AdminWalletMutationOutcome::Applied(batch))
            }
            Some(aether_data::repository::wallet::WalletMutationOutcome::NotFound) => {
                Ok(AdminWalletMutationOutcome::NotFound)
            }
            Some(aether_data::repository::wallet::WalletMutationOutcome::Invalid(detail)) => {
                Ok(AdminWalletMutationOutcome::Invalid(detail))
            }
            None => Ok(AdminWalletMutationOutcome::Unavailable),
        }
    }

    pub(crate) async fn admin_delete_redeem_code_batch(
        &self,
        batch_id: &str,
        operator_id: Option<&str>,
    ) -> Result<
        AdminWalletMutationOutcome<aether_data::repository::wallet::StoredAdminRedeemCodeBatch>,
        GatewayError,
    > {
        match self
            .data
            .delete_admin_redeem_code_batch(
                aether_data::repository::wallet::DeleteAdminRedeemCodeBatchInput {
                    batch_id: batch_id.to_string(),
                    operator_id: operator_id.map(ToOwned::to_owned),
                },
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
        {
            Some(aether_data::repository::wallet::WalletMutationOutcome::Applied(batch)) => {
                Ok(AdminWalletMutationOutcome::Applied(batch))
            }
            Some(aether_data::repository::wallet::WalletMutationOutcome::NotFound) => {
                Ok(AdminWalletMutationOutcome::NotFound)
            }
            Some(aether_data::repository::wallet::WalletMutationOutcome::Invalid(detail)) => {
                Ok(AdminWalletMutationOutcome::Invalid(detail))
            }
            None => Ok(AdminWalletMutationOutcome::Unavailable),
        }
    }

    pub(crate) async fn admin_disable_redeem_code(
        &self,
        code_id: &str,
        operator_id: Option<&str>,
    ) -> Result<
        AdminWalletMutationOutcome<aether_data::repository::wallet::StoredAdminRedeemCode>,
        GatewayError,
    > {
        match self
            .data
            .disable_admin_redeem_code(
                aether_data::repository::wallet::DisableAdminRedeemCodeInput {
                    code_id: code_id.to_string(),
                    operator_id: operator_id.map(ToOwned::to_owned),
                },
            )
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?
        {
            Some(aether_data::repository::wallet::WalletMutationOutcome::Applied(code)) => {
                Ok(AdminWalletMutationOutcome::Applied(code))
            }
            Some(aether_data::repository::wallet::WalletMutationOutcome::NotFound) => {
                Ok(AdminWalletMutationOutcome::NotFound)
            }
            Some(aether_data::repository::wallet::WalletMutationOutcome::Invalid(detail)) => {
                Ok(AdminWalletMutationOutcome::Invalid(detail))
            }
            None => Ok(AdminWalletMutationOutcome::Unavailable),
        }
    }

    pub(crate) async fn redeem_wallet_code(
        &self,
        input: aether_data::repository::wallet::RedeemWalletCodeInput,
    ) -> Result<Option<aether_data::repository::wallet::RedeemWalletCodeOutcome>, GatewayError>
    {
        self.data
            .redeem_wallet_code(input)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }
}

fn stored_admin_payment_order_to_gateway(
    order: aether_data::repository::wallet::StoredAdminPaymentOrder,
) -> AdminWalletPaymentOrderRecord {
    AdminWalletPaymentOrderRecord {
        id: order.id,
        order_no: order.order_no,
        wallet_id: order.wallet_id,
        user_id: order.user_id,
        amount_usd: order.amount_usd,
        pay_amount: order.pay_amount,
        pay_currency: order.pay_currency,
        exchange_rate: order.exchange_rate,
        refunded_amount_usd: order.refunded_amount_usd,
        refundable_amount_usd: order.refundable_amount_usd,
        payment_method: order.payment_method,
        gateway_order_id: order.gateway_order_id,
        status: order.status,
        gateway_response: order.gateway_response,
        created_at_unix_ms: order.created_at_unix_ms,
        paid_at_unix_secs: order.paid_at_unix_secs,
        credited_at_unix_secs: order.credited_at_unix_secs,
        expires_at_unix_secs: order.expires_at_unix_secs,
    }
}
