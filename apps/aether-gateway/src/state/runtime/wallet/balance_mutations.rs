use crate::{AdminWalletPaymentOrderRecord, AdminWalletTransactionRecord, AppState, GatewayError};

use super::admin_wallet_build_order_no;

impl AppState {
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
            AdminWalletTransactionRecord,
        )>,
        GatewayError,
    > {
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            let mut guard = store.lock().expect("auth wallet store should lock");
            let Some(wallet) = guard.get_mut(wallet_id) else {
                return Ok(None);
            };

            let before_recharge = wallet.balance;
            let before_gift = wallet.gift_balance;
            let before_total = before_recharge + before_gift;
            let mut after_recharge = before_recharge;
            let mut after_gift = before_gift;

            if amount_usd > 0.0 {
                if balance_type.eq_ignore_ascii_case("gift") {
                    after_gift += amount_usd;
                } else {
                    after_recharge += amount_usd;
                }
            } else {
                let mut remaining = -amount_usd;
                let consume_positive_bucket = |balance: &mut f64, to_consume: &mut f64| {
                    if *to_consume <= 0.0 {
                        return;
                    }
                    let available = (*balance).max(0.0);
                    let consumed = available.min(*to_consume);
                    *balance -= consumed;
                    *to_consume -= consumed;
                };
                if balance_type.eq_ignore_ascii_case("gift") {
                    consume_positive_bucket(&mut after_gift, &mut remaining);
                    consume_positive_bucket(&mut after_recharge, &mut remaining);
                } else {
                    consume_positive_bucket(&mut after_recharge, &mut remaining);
                    consume_positive_bucket(&mut after_gift, &mut remaining);
                }
                if remaining > 0.0 {
                    after_recharge -= remaining;
                }
            }

            wallet.balance = after_recharge;
            wallet.gift_balance = after_gift;
            wallet.total_adjusted += amount_usd;
            wallet.updated_at_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;

            let transaction = AdminWalletTransactionRecord {
                id: uuid::Uuid::new_v4().to_string(),
                wallet_id: wallet.id.clone(),
                category: "adjust".to_string(),
                reason_code: "adjust_admin".to_string(),
                amount: amount_usd,
                balance_before: before_total,
                balance_after: after_recharge + after_gift,
                recharge_balance_before: before_recharge,
                recharge_balance_after: after_recharge,
                gift_balance_before: before_gift,
                gift_balance_after: after_gift,
                link_type: Some("admin_action".to_string()),
                link_id: Some(wallet.id.clone()),
                operator_id: operator_id.map(ToOwned::to_owned),
                description: Some(
                    description
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or("管理员调账")
                        .to_string(),
                ),
                created_at_unix_ms: chrono::Utc::now().timestamp().max(0) as u64,
            };
            let updated_wallet = wallet.clone();
            drop(guard);
            self.invalidate_auth_context_cache();
            return Ok(Some((updated_wallet, transaction)));
        }

        Ok(self
            .adjust_wallet_balance(aether_data::repository::wallet::AdjustWalletBalanceInput {
                wallet_id: wallet_id.to_string(),
                amount_usd,
                balance_type: balance_type.to_string(),
                operator_id: operator_id.map(ToOwned::to_owned),
                description: description.map(ToOwned::to_owned),
            })
            .await?
            .map(|(wallet, transaction)| {
                (wallet, stored_wallet_transaction_to_gateway(transaction))
            }))
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
            AdminWalletPaymentOrderRecord,
        )>,
        GatewayError,
    > {
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            let mut guard = store.lock().expect("auth wallet store should lock");
            let Some(wallet) = guard.get_mut(wallet_id) else {
                return Ok(None);
            };
            wallet.balance += amount_usd;
            wallet.total_recharged += amount_usd;
            wallet.updated_at_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
            let now = chrono::Utc::now();
            let created_at = now.timestamp().max(0) as u64;
            let order = AdminWalletPaymentOrderRecord {
                id: uuid::Uuid::new_v4().to_string(),
                order_no: admin_wallet_build_order_no(now),
                wallet_id: wallet.id.clone(),
                user_id: wallet.user_id.clone(),
                amount_usd,
                pay_amount: None,
                pay_currency: None,
                exchange_rate: None,
                refunded_amount_usd: 0.0,
                refundable_amount_usd: amount_usd,
                payment_method: payment_method.to_string(),
                gateway_order_id: None,
                status: "credited".to_string(),
                gateway_response: Some(serde_json::json!({
                    "source": "manual",
                    "operator_id": operator_id,
                    "description": description,
                })),
                created_at_unix_ms: created_at,
                paid_at_unix_secs: Some(created_at),
                credited_at_unix_secs: Some(created_at),
                expires_at_unix_secs: None,
            };
            let updated_wallet = wallet.clone();
            drop(guard);
            self.invalidate_auth_context_cache();
            return Ok(Some((updated_wallet, order)));
        }

        let now = chrono::Utc::now();
        let order_no = admin_wallet_build_order_no(now);
        Ok(self
            .create_manual_wallet_recharge(
                aether_data::repository::wallet::CreateManualWalletRechargeInput {
                    wallet_id: wallet_id.to_string(),
                    amount_usd,
                    payment_method: payment_method.to_string(),
                    operator_id: operator_id.map(ToOwned::to_owned),
                    description: description.map(ToOwned::to_owned),
                    order_no,
                },
            )
            .await?
            .map(|(wallet, order)| (wallet, stored_admin_payment_order_to_gateway(order))))
    }
}

fn stored_wallet_transaction_to_gateway(
    transaction: aether_data::repository::wallet::StoredAdminWalletTransaction,
) -> AdminWalletTransactionRecord {
    AdminWalletTransactionRecord {
        id: transaction.id,
        wallet_id: transaction.wallet_id,
        category: transaction.category,
        reason_code: transaction.reason_code,
        amount: transaction.amount,
        balance_before: transaction.balance_before,
        balance_after: transaction.balance_after,
        recharge_balance_before: transaction.recharge_balance_before,
        recharge_balance_after: transaction.recharge_balance_after,
        gift_balance_before: transaction.gift_balance_before,
        gift_balance_after: transaction.gift_balance_after,
        link_type: transaction.link_type,
        link_id: transaction.link_id,
        operator_id: transaction.operator_id,
        description: transaction.description,
        created_at_unix_ms: transaction.created_at_unix_ms.unwrap_or_default(),
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
