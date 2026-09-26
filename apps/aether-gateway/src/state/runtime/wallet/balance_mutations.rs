use crate::{AdminWalletPaymentOrderRecord, AdminWalletTransactionRecord, AppState, GatewayError};
use aether_data::repository::wallet::{
    AdjustWalletBalanceInBatchInput, AdminUserWalletBalanceBatchUserOutcome,
    PrepareAdminUserWalletBalanceBatchInput, PrepareAdminUserWalletBalanceBatchOutcome,
    StoredAdminUserWalletBalanceBatch,
};
use std::collections::BTreeMap;

use super::admin_wallet_build_order_no;

impl AppState {
    pub(crate) async fn prepare_admin_user_wallet_balance_batch(
        &self,
        input: PrepareAdminUserWalletBalanceBatchInput,
    ) -> Result<PrepareAdminUserWalletBalanceBatchOutcome, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_batch_store_for_tests.as_ref() {
            let mut batches = store.lock().expect("auth wallet batch store should lock");
            let key = (input.admin_user_id.clone(), input.idempotency_key.clone());
            if let Some(existing) = batches.get(&key) {
                if existing.request_fingerprint != input.request_fingerprint {
                    return Ok(PrepareAdminUserWalletBalanceBatchOutcome::Conflict);
                }
                return Ok(PrepareAdminUserWalletBalanceBatchOutcome::Ready(
                    existing.clone(),
                ));
            }
            let batch = StoredAdminUserWalletBalanceBatch {
                admin_user_id: input.admin_user_id,
                idempotency_key: input.idempotency_key,
                request_fingerprint: input.request_fingerprint,
                target_user_ids: input.target_user_ids,
                missing_user_ids: input.missing_user_ids,
                warnings: input.warnings,
                user_outcomes: BTreeMap::new(),
            };
            batches.insert(key, batch.clone());
            return Ok(PrepareAdminUserWalletBalanceBatchOutcome::Ready(batch));
        }

        self.data
            .prepare_admin_user_wallet_balance_batch(input)
            .await
            .map_err(|error| GatewayError::Internal(error.to_string()))?
            .ok_or_else(|| {
                GatewayError::Internal("admin wallet batch storage is unavailable".to_string())
            })
    }

    pub(crate) async fn get_admin_user_wallet_balance_batch(
        &self,
        admin_user_id: &str,
        idempotency_key: &str,
        request_fingerprint: &str,
    ) -> Result<Option<PrepareAdminUserWalletBalanceBatchOutcome>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_batch_store_for_tests.as_ref() {
            let batches = store.lock().expect("auth wallet batch store should lock");
            return Ok(batches
                .get(&(admin_user_id.to_string(), idempotency_key.to_string()))
                .map(|existing| {
                    if existing.request_fingerprint != request_fingerprint {
                        PrepareAdminUserWalletBalanceBatchOutcome::Conflict
                    } else {
                        PrepareAdminUserWalletBalanceBatchOutcome::Ready(existing.clone())
                    }
                }));
        }

        self.data
            .get_admin_user_wallet_balance_batch(
                admin_user_id,
                idempotency_key,
                request_fingerprint,
            )
            .await
            .map_err(|error| GatewayError::Internal(error.to_string()))
    }

    pub(crate) async fn adjust_admin_user_wallet_balance_batch_user(
        &self,
        input: AdjustWalletBalanceInBatchInput,
    ) -> Result<AdminUserWalletBalanceBatchUserOutcome, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_batch_store_for_tests.as_ref() {
            let _operation = self.auth_wallet_batch_operation_lock_for_tests.lock().await;
            let key = (input.admin_user_id.clone(), input.idempotency_key.clone());
            if let Some(existing) = store
                .lock()
                .expect("auth wallet batch store should lock")
                .get(&key)
                .and_then(|batch| batch.user_outcomes.get(&input.user_id))
                .cloned()
            {
                return Ok(existing);
            }
            let target_exists = store
                .lock()
                .expect("auth wallet batch store should lock")
                .get(&key)
                .is_some_and(|batch| batch.target_user_ids.contains(&input.user_id));
            if !target_exists {
                return Err(GatewayError::Internal(
                    "user is outside the prepared admin wallet batch".to_string(),
                ));
            }
            let adjustment = input.adjustment;
            let result = self
                .admin_adjust_wallet_balance(
                    &adjustment.wallet_id,
                    adjustment.amount_usd,
                    &adjustment.balance_type,
                    adjustment.operator_id.as_deref(),
                    adjustment.description.as_deref(),
                    adjustment.clamp_deduction_to_available_balance,
                )
                .await?;
            let outcome = if result.is_some() {
                AdminUserWalletBalanceBatchUserOutcome::Succeeded
            } else {
                AdminUserWalletBalanceBatchUserOutcome::Failed("用户钱包不可用".to_string())
            };
            if let Some(batch) = store
                .lock()
                .expect("auth wallet batch store should lock")
                .get_mut(&key)
            {
                batch.user_outcomes.insert(input.user_id, outcome.clone());
            }
            return Ok(outcome);
        }

        self.data
            .adjust_admin_user_wallet_balance_batch_user(input)
            .await
            .map_err(|error| GatewayError::Internal(error.to_string()))?
            .ok_or_else(|| {
                GatewayError::Internal("admin wallet batch storage is unavailable".to_string())
            })
    }

    pub(crate) async fn record_admin_user_wallet_balance_batch_failure(
        &self,
        admin_user_id: &str,
        idempotency_key: &str,
        user_id: &str,
        reason: &str,
    ) -> Result<AdminUserWalletBalanceBatchUserOutcome, GatewayError> {
        #[cfg(test)]
        if self
            .auth_wallet_batch_failure_record_error_for_tests
            .as_deref()
            == Some(user_id)
        {
            return Err(GatewayError::Internal(
                "injected wallet batch failure-record error".to_string(),
            ));
        }

        #[cfg(test)]
        if let Some(store) = self.auth_wallet_batch_store_for_tests.as_ref() {
            let _operation = self.auth_wallet_batch_operation_lock_for_tests.lock().await;
            let key = (admin_user_id.to_string(), idempotency_key.to_string());
            let mut batches = store.lock().expect("auth wallet batch store should lock");
            let batch = batches.get_mut(&key).ok_or_else(|| {
                GatewayError::Internal("admin wallet batch was not prepared".to_string())
            })?;
            if !batch.target_user_ids.iter().any(|target| target == user_id) {
                return Err(GatewayError::Internal(
                    "user is outside the prepared admin wallet batch".to_string(),
                ));
            }
            return Ok(batch
                .user_outcomes
                .entry(user_id.to_string())
                .or_insert_with(|| {
                    AdminUserWalletBalanceBatchUserOutcome::Failed(reason.to_string())
                })
                .clone());
        }

        self.data
            .record_admin_user_wallet_balance_batch_failure(
                admin_user_id,
                idempotency_key,
                user_id,
                reason,
            )
            .await
            .map_err(|error| GatewayError::Internal(error.to_string()))?
            .ok_or_else(|| {
                GatewayError::Internal("admin wallet batch storage is unavailable".to_string())
            })
    }

    pub(crate) async fn admin_adjust_wallet_balance(
        &self,
        wallet_id: &str,
        amount_usd: f64,
        balance_type: &str,
        operator_id: Option<&str>,
        description: Option<&str>,
        clamp_deduction_to_available_balance: bool,
    ) -> Result<
        Option<(
            aether_data::repository::wallet::StoredWalletSnapshot,
            Option<AdminWalletTransactionRecord>,
        )>,
        GatewayError,
    > {
        #[cfg(test)]
        if self.auth_wallet_adjustment_error_for_tests.as_deref() == Some(wallet_id) {
            return Err(GatewayError::Internal(
                "injected test wallet adjustment failure".to_string(),
            ));
        }

        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            let mut guard = store.lock().expect("auth wallet store should lock");
            let Some(wallet) = guard.get_mut(wallet_id) else {
                return Ok(None);
            };

            let before_recharge = wallet.balance;
            let before_gift = wallet.gift_balance;
            let before_total = before_recharge + before_gift;
            let amount_usd = if clamp_deduction_to_available_balance && amount_usd < 0.0 {
                -(-amount_usd).min(before_total.max(0.0))
            } else {
                amount_usd
            };
            if amount_usd == 0.0 {
                return Ok(Some((wallet.clone(), None)));
            }
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
            return Ok(Some((updated_wallet, Some(transaction))));
        }

        Ok(self
            .adjust_wallet_balance(aether_data::repository::wallet::AdjustWalletBalanceInput {
                wallet_id: wallet_id.to_string(),
                amount_usd,
                balance_type: balance_type.to_string(),
                operator_id: operator_id.map(ToOwned::to_owned),
                description: description.map(ToOwned::to_owned),
                clamp_deduction_to_available_balance,
                batch_context: None,
            })
            .await?
            .map(|(wallet, transaction)| {
                (
                    wallet,
                    transaction.map(stored_wallet_transaction_to_gateway),
                )
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
