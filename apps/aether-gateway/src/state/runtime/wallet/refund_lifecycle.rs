use super::{
    AdminWalletMutationOutcome, AdminWalletRefundRecord, AdminWalletTransactionRecord, AppState,
    GatewayError,
};

impl AppState {
    pub(crate) async fn admin_process_wallet_refund(
        &self,
        wallet_id: &str,
        refund_id: &str,
        operator_id: Option<&str>,
    ) -> Result<
        AdminWalletMutationOutcome<(
            aether_data::repository::wallet::StoredWalletSnapshot,
            AdminWalletRefundRecord,
            AdminWalletTransactionRecord,
        )>,
        GatewayError,
    > {
        #[cfg(test)]
        if let (Some(wallet_store), Some(refund_store)) = (
            self.auth_wallet_store.as_ref(),
            self.admin_wallet_refund_store.as_ref(),
        ) {
            let Some(wallet) = wallet_store
                .lock()
                .expect("auth wallet store should lock")
                .get(wallet_id)
                .cloned()
            else {
                return Ok(AdminWalletMutationOutcome::NotFound);
            };
            let Some(refund) = refund_store
                .lock()
                .expect("admin wallet refund store should lock")
                .get(refund_id)
                .filter(|refund| refund.wallet_id == wallet_id)
                .cloned()
            else {
                return Ok(AdminWalletMutationOutcome::NotFound);
            };
            if !matches!(refund.status.as_str(), "approved" | "pending_approval") {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "refund status is not approvable".to_string(),
                ));
            }

            let amount_usd = refund.amount_usd;
            let mut updated_wallet = wallet.clone();
            let before_recharge = updated_wallet.balance;
            let before_gift = updated_wallet.gift_balance;
            let before_total = before_recharge + before_gift;
            let after_recharge = before_recharge - amount_usd;
            if after_recharge < 0.0 {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "refund amount exceeds refundable recharge balance".to_string(),
                ));
            }

            let mut updated_order = None;
            if let Some(payment_order_id) = refund.payment_order_id.clone() {
                let Some(order_store) = self.admin_wallet_payment_order_store.as_ref() else {
                    return Ok(AdminWalletMutationOutcome::Unavailable);
                };
                let Some(order) = order_store
                    .lock()
                    .expect("admin wallet payment order store should lock")
                    .get(&payment_order_id)
                    .cloned()
                else {
                    return Ok(AdminWalletMutationOutcome::Invalid(
                        "payment order not found".to_string(),
                    ));
                };
                if amount_usd > order.refundable_amount_usd {
                    return Ok(AdminWalletMutationOutcome::Invalid(
                        "refund amount exceeds refundable amount".to_string(),
                    ));
                }
                let mut order = order;
                order.refunded_amount_usd += amount_usd;
                order.refundable_amount_usd -= amount_usd;
                updated_order = Some(order);
            }

            let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
            updated_wallet.balance = after_recharge;
            updated_wallet.total_refunded = (updated_wallet.total_refunded + amount_usd).max(0.0);
            updated_wallet.updated_at_unix_secs = now_unix_secs;

            let transaction = AdminWalletTransactionRecord {
                id: uuid::Uuid::new_v4().to_string(),
                wallet_id: updated_wallet.id.clone(),
                category: "refund".to_string(),
                reason_code: "refund_out".to_string(),
                amount: -amount_usd,
                balance_before: before_total,
                balance_after: after_recharge + before_gift,
                recharge_balance_before: before_recharge,
                recharge_balance_after: after_recharge,
                gift_balance_before: before_gift,
                gift_balance_after: before_gift,
                link_type: Some("refund_request".to_string()),
                link_id: Some(refund.id.clone()),
                operator_id: operator_id.map(ToOwned::to_owned),
                description: Some("退款占款".to_string()),
                created_at_unix_ms: now_unix_secs,
            };

            let mut updated_refund = refund.clone();
            updated_refund.status = "processing".to_string();
            updated_refund.approved_by = operator_id.map(ToOwned::to_owned);
            updated_refund.processed_by = operator_id.map(ToOwned::to_owned);
            updated_refund.processed_at_unix_secs = Some(now_unix_secs);
            updated_refund.updated_at_unix_secs = now_unix_secs;

            wallet_store
                .lock()
                .expect("auth wallet store should lock")
                .insert(updated_wallet.id.clone(), updated_wallet.clone());
            refund_store
                .lock()
                .expect("admin wallet refund store should lock")
                .insert(updated_refund.id.clone(), updated_refund.clone());
            if let Some(updated_order) = updated_order {
                self.admin_wallet_payment_order_store
                    .as_ref()
                    .expect("admin wallet payment order store should exist")
                    .lock()
                    .expect("admin wallet payment order store should lock")
                    .insert(updated_order.id.clone(), updated_order);
            }

            self.invalidate_auth_context_cache();
            return Ok(AdminWalletMutationOutcome::Applied((
                updated_wallet,
                updated_refund,
                transaction,
            )));
        }

        match self
            .process_admin_wallet_refund(
                aether_data::repository::wallet::ProcessAdminWalletRefundInput {
                    wallet_id: wallet_id.to_string(),
                    refund_id: refund_id.to_string(),
                    operator_id: operator_id.map(ToOwned::to_owned),
                },
            )
            .await?
        {
            Some(aether_data::repository::wallet::WalletMutationOutcome::Applied((
                wallet,
                refund,
                transaction,
            ))) => Ok(AdminWalletMutationOutcome::Applied((
                wallet,
                stored_admin_wallet_refund_to_gateway(refund),
                stored_admin_wallet_transaction_to_gateway(transaction),
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

    pub(crate) async fn admin_complete_wallet_refund(
        &self,
        wallet_id: &str,
        refund_id: &str,
        gateway_refund_id: Option<&str>,
        payout_reference: Option<&str>,
        payout_proof: Option<serde_json::Value>,
    ) -> Result<AdminWalletMutationOutcome<AdminWalletRefundRecord>, GatewayError> {
        #[cfg(test)]
        if let Some(refund_store) = self.admin_wallet_refund_store.as_ref() {
            let Some(refund) = refund_store
                .lock()
                .expect("admin wallet refund store should lock")
                .get(refund_id)
                .filter(|refund| refund.wallet_id == wallet_id)
                .cloned()
            else {
                return Ok(AdminWalletMutationOutcome::NotFound);
            };
            if refund.status != "processing" {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "refund status must be processing before completion".to_string(),
                ));
            }
            let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
            let mut updated_refund = refund;
            updated_refund.status = "succeeded".to_string();
            updated_refund.gateway_refund_id = gateway_refund_id.map(ToOwned::to_owned);
            updated_refund.payout_reference = payout_reference.map(ToOwned::to_owned);
            updated_refund.payout_proof = payout_proof;
            updated_refund.completed_at_unix_secs = Some(now_unix_secs);
            updated_refund.updated_at_unix_secs = now_unix_secs;
            refund_store
                .lock()
                .expect("admin wallet refund store should lock")
                .insert(updated_refund.id.clone(), updated_refund.clone());
            return Ok(AdminWalletMutationOutcome::Applied(updated_refund));
        }

        match self
            .complete_admin_wallet_refund(
                aether_data::repository::wallet::CompleteAdminWalletRefundInput {
                    wallet_id: wallet_id.to_string(),
                    refund_id: refund_id.to_string(),
                    gateway_refund_id: gateway_refund_id.map(ToOwned::to_owned),
                    payout_reference: payout_reference.map(ToOwned::to_owned),
                    payout_proof,
                },
            )
            .await?
        {
            Some(aether_data::repository::wallet::WalletMutationOutcome::Applied(refund)) => Ok(
                AdminWalletMutationOutcome::Applied(stored_admin_wallet_refund_to_gateway(refund)),
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

    pub(crate) async fn admin_fail_wallet_refund(
        &self,
        wallet_id: &str,
        refund_id: &str,
        reason: &str,
        operator_id: Option<&str>,
    ) -> Result<
        AdminWalletMutationOutcome<(
            aether_data::repository::wallet::StoredWalletSnapshot,
            AdminWalletRefundRecord,
            Option<AdminWalletTransactionRecord>,
        )>,
        GatewayError,
    > {
        #[cfg(test)]
        if let (Some(wallet_store), Some(refund_store)) = (
            self.auth_wallet_store.as_ref(),
            self.admin_wallet_refund_store.as_ref(),
        ) {
            let Some(wallet) = wallet_store
                .lock()
                .expect("auth wallet store should lock")
                .get(wallet_id)
                .cloned()
            else {
                return Ok(AdminWalletMutationOutcome::NotFound);
            };
            let Some(refund) = refund_store
                .lock()
                .expect("admin wallet refund store should lock")
                .get(refund_id)
                .filter(|refund| refund.wallet_id == wallet_id)
                .cloned()
            else {
                return Ok(AdminWalletMutationOutcome::NotFound);
            };

            let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
            if matches!(refund.status.as_str(), "pending_approval" | "approved") {
                let mut updated_refund = refund;
                updated_refund.status = "failed".to_string();
                updated_refund.failure_reason = Some(reason.to_string());
                updated_refund.updated_at_unix_secs = now_unix_secs;
                refund_store
                    .lock()
                    .expect("admin wallet refund store should lock")
                    .insert(updated_refund.id.clone(), updated_refund.clone());
                return Ok(AdminWalletMutationOutcome::Applied((
                    wallet,
                    updated_refund,
                    None,
                )));
            }
            if refund.status != "processing" {
                return Ok(AdminWalletMutationOutcome::Invalid(format!(
                    "cannot fail refund in status: {}",
                    refund.status
                )));
            }

            let amount_usd = refund.amount_usd;
            let before_recharge = wallet.balance;
            let before_gift = wallet.gift_balance;
            let before_total = before_recharge + before_gift;
            let after_recharge = before_recharge + amount_usd;

            let mut updated_wallet = wallet.clone();
            updated_wallet.balance = after_recharge;
            updated_wallet.total_refunded = (updated_wallet.total_refunded - amount_usd).max(0.0);
            updated_wallet.updated_at_unix_secs = now_unix_secs;

            let transaction = AdminWalletTransactionRecord {
                id: uuid::Uuid::new_v4().to_string(),
                wallet_id: updated_wallet.id.clone(),
                category: "refund".to_string(),
                reason_code: "refund_revert".to_string(),
                amount: amount_usd,
                balance_before: before_total,
                balance_after: after_recharge + before_gift,
                recharge_balance_before: before_recharge,
                recharge_balance_after: after_recharge,
                gift_balance_before: before_gift,
                gift_balance_after: before_gift,
                link_type: Some("refund_request".to_string()),
                link_id: Some(refund.id.clone()),
                operator_id: operator_id.map(ToOwned::to_owned),
                description: Some("退款失败回补".to_string()),
                created_at_unix_ms: now_unix_secs,
            };

            if let Some(payment_order_id) = refund.payment_order_id.clone() {
                let Some(order_store) = self.admin_wallet_payment_order_store.as_ref() else {
                    return Ok(AdminWalletMutationOutcome::Unavailable);
                };
                let maybe_order = order_store
                    .lock()
                    .expect("admin wallet payment order store should lock")
                    .get(&payment_order_id)
                    .cloned();
                if let Some(mut order) = maybe_order {
                    order.refunded_amount_usd -= amount_usd;
                    order.refundable_amount_usd += amount_usd;
                    order_store
                        .lock()
                        .expect("admin wallet payment order store should lock")
                        .insert(order.id.clone(), order);
                }
            }

            let mut updated_refund = refund;
            updated_refund.status = "failed".to_string();
            updated_refund.failure_reason = Some(reason.to_string());
            updated_refund.updated_at_unix_secs = now_unix_secs;

            wallet_store
                .lock()
                .expect("auth wallet store should lock")
                .insert(updated_wallet.id.clone(), updated_wallet.clone());
            refund_store
                .lock()
                .expect("admin wallet refund store should lock")
                .insert(updated_refund.id.clone(), updated_refund.clone());

            self.invalidate_auth_context_cache();
            return Ok(AdminWalletMutationOutcome::Applied((
                updated_wallet,
                updated_refund,
                Some(transaction),
            )));
        }

        match self
            .fail_admin_wallet_refund(
                aether_data::repository::wallet::FailAdminWalletRefundInput {
                    wallet_id: wallet_id.to_string(),
                    refund_id: refund_id.to_string(),
                    reason: reason.to_string(),
                    operator_id: operator_id.map(ToOwned::to_owned),
                },
            )
            .await?
        {
            Some(aether_data::repository::wallet::WalletMutationOutcome::Applied((
                wallet,
                refund,
                transaction,
            ))) => Ok(AdminWalletMutationOutcome::Applied((
                wallet,
                stored_admin_wallet_refund_to_gateway(refund),
                transaction.map(stored_admin_wallet_transaction_to_gateway),
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
}

fn stored_admin_wallet_refund_to_gateway(
    refund: aether_data::repository::wallet::StoredAdminWalletRefund,
) -> AdminWalletRefundRecord {
    AdminWalletRefundRecord {
        id: refund.id,
        refund_no: refund.refund_no,
        wallet_id: refund.wallet_id,
        user_id: refund.user_id,
        payment_order_id: refund.payment_order_id,
        source_type: refund.source_type,
        source_id: refund.source_id,
        refund_mode: refund.refund_mode,
        amount_usd: refund.amount_usd,
        status: refund.status,
        reason: refund.reason,
        failure_reason: refund.failure_reason,
        gateway_refund_id: refund.gateway_refund_id,
        payout_method: refund.payout_method,
        payout_reference: refund.payout_reference,
        payout_proof: refund.payout_proof,
        requested_by: refund.requested_by,
        approved_by: refund.approved_by,
        processed_by: refund.processed_by,
        created_at_unix_ms: refund.created_at_unix_ms,
        updated_at_unix_secs: refund.updated_at_unix_secs,
        processed_at_unix_secs: refund.processed_at_unix_secs,
        completed_at_unix_secs: refund.completed_at_unix_secs,
    }
}

fn stored_admin_wallet_transaction_to_gateway(
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
