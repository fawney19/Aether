use super::{
    AdminWalletMutationOutcome, AdminWalletRefundRecord, AdminWalletTransactionRecord, AppState,
    GatewayError,
};
use aether_data::repository::wallet::{
    payment_order_refund_amounts_are_consistent, wallet_refund_proof_is_success,
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
            if !refund.amount_usd.is_finite() || refund.amount_usd <= 0.0 {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "refund amount must be finite and greater than zero".to_string(),
                ));
            }
            if !matches!(refund.status.as_str(), "approved" | "pending_approval") {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "refund status is not approvable".to_string(),
                ));
            }

            let amount_usd = refund.amount_usd;
            let mut updated_wallet = wallet.clone();
            let before_recharge = updated_wallet.balance;
            let before_gift = updated_wallet.gift_balance;
            let before_total_refunded = updated_wallet.total_refunded;
            let before_total = before_recharge + before_gift;
            let after_recharge = before_recharge - amount_usd;
            let after_total = after_recharge + before_gift;
            let after_total_refunded = before_total_refunded + amount_usd;
            if !before_recharge.is_finite()
                || before_recharge < 0.0
                || !before_gift.is_finite()
                || before_gift < 0.0
                || !before_total_refunded.is_finite()
                || before_total_refunded < 0.0
                || !before_total.is_finite()
                || !after_recharge.is_finite()
                || !after_total.is_finite()
                || !after_total_refunded.is_finite()
            {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "wallet balance is invalid".to_string(),
                ));
            }
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
                if order.wallet_id != wallet_id || order.status != "credited" {
                    return Ok(AdminWalletMutationOutcome::Invalid(
                        "payment order is not refundable for this wallet".to_string(),
                    ));
                }
                let order_amount = order.amount_usd;
                let refunded_before = order.refunded_amount_usd;
                let refundable_before = order.refundable_amount_usd;
                let refunded_after = refunded_before + amount_usd;
                let refundable_after = refundable_before - amount_usd;
                if !payment_order_refund_amounts_are_consistent(
                    order_amount,
                    refunded_before,
                    refundable_before,
                ) || !refunded_after.is_finite()
                    || !refundable_after.is_finite()
                    || amount_usd > refundable_before
                    || refunded_after < 0.0
                    || refunded_after > order_amount
                    || refundable_after < 0.0
                    || refundable_after > order_amount
                {
                    return Ok(AdminWalletMutationOutcome::Invalid(
                        "payment order refund amounts are invalid".to_string(),
                    ));
                }
                let mut order = order;
                order.refunded_amount_usd = refunded_after;
                order.refundable_amount_usd = refundable_after;
                updated_order = Some(order);
            }

            let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
            updated_wallet.balance = after_recharge;
            updated_wallet.total_refunded = after_total_refunded;
            updated_wallet.updated_at_unix_secs = now_unix_secs;

            let transaction = AdminWalletTransactionRecord {
                id: uuid::Uuid::new_v4().to_string(),
                wallet_id: updated_wallet.id.clone(),
                category: "refund".to_string(),
                reason_code: "refund_out".to_string(),
                amount: -amount_usd,
                balance_before: before_total,
                balance_after: after_total,
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
            if let Some(transaction_store) = self.admin_wallet_transaction_store.as_ref() {
                transaction_store
                    .lock()
                    .expect("admin wallet transaction store should lock")
                    .insert(transaction.id.clone(), transaction.clone());
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
            if !refund.amount_usd.is_finite() || refund.amount_usd <= 0.0 {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "refund amount must be finite and greater than zero".to_string(),
                ));
            }
            if let (Some(existing_id), Some(incoming_id)) =
                (refund.gateway_refund_id.as_deref(), gateway_refund_id)
            {
                if existing_id != incoming_id {
                    return Ok(AdminWalletMutationOutcome::Invalid(
                        "gateway refund identifier conflicts with existing evidence".to_string(),
                    ));
                }
            }
            if refund.status == "succeeded" {
                return Ok(AdminWalletMutationOutcome::Applied(refund));
            }
            if refund.status != "processing" {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "refund status must be processing before completion".to_string(),
                ));
            }
            let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
            let mut updated_refund = refund;
            updated_refund.status = "succeeded".to_string();
            updated_refund.gateway_refund_id = gateway_refund_id
                .map(ToOwned::to_owned)
                .or_else(|| updated_refund.gateway_refund_id.clone());
            updated_refund.payout_reference = payout_reference
                .map(ToOwned::to_owned)
                .or_else(|| updated_refund.payout_reference.clone());
            // Keep the durable provider response on ordinary retries.  A
            // terminal success proof is the only completion payload allowed
            // to upgrade an earlier processing proof.
            if updated_refund.payout_proof.is_none()
                || payout_proof
                    .as_ref()
                    .is_some_and(wallet_refund_proof_is_success)
            {
                updated_refund.payout_proof = payout_proof;
            }
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

            if !refund.amount_usd.is_finite() || refund.amount_usd <= 0.0 {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "refund amount must be finite and greater than zero".to_string(),
                ));
            }

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

            // The in-memory implementation mirrors the database contract:
            // only an explicitly offline payout without external evidence may
            // release its reservation.
            if refund.gateway_refund_id.is_some()
                || refund.payout_proof.is_some()
                || !refund
                    .refund_mode
                    .trim()
                    .eq_ignore_ascii_case("offline_payout")
            {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "cannot fail refund while gateway settlement is processing".to_string(),
                ));
            }

            let amount_usd = refund.amount_usd;
            let before_recharge = wallet.balance;
            let before_gift = wallet.gift_balance;
            let before_total_refunded = wallet.total_refunded;
            let before_total = before_recharge + before_gift;
            let after_recharge = before_recharge + amount_usd;
            let after_total = after_recharge + before_gift;
            let after_total_refunded = before_total_refunded - amount_usd;
            if !before_recharge.is_finite()
                || before_recharge < 0.0
                || !before_gift.is_finite()
                || before_gift < 0.0
                || !before_total_refunded.is_finite()
                || before_total_refunded < 0.0
                || before_total_refunded < amount_usd
                || !before_total.is_finite()
                || !after_recharge.is_finite()
                || !after_total.is_finite()
                || !after_total_refunded.is_finite()
                || after_total_refunded < 0.0
            {
                return Ok(AdminWalletMutationOutcome::Invalid(
                    "wallet balance is invalid for refund recovery".to_string(),
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
                if order.wallet_id != wallet_id || order.status != "credited" {
                    return Ok(AdminWalletMutationOutcome::Invalid(
                        "payment order is not refundable for this wallet".to_string(),
                    ));
                }
                let refunded_before = order.refunded_amount_usd;
                let refundable_before = order.refundable_amount_usd;
                let refunded_after = refunded_before - amount_usd;
                let refundable_after = refundable_before + amount_usd;
                if !payment_order_refund_amounts_are_consistent(
                    order.amount_usd,
                    refunded_before,
                    refundable_before,
                ) || !refunded_before.is_finite()
                    || refunded_before < amount_usd
                    || !refunded_after.is_finite()
                    || refunded_after < 0.0
                    || refundable_after < 0.0
                    || refundable_after > order.amount_usd
                {
                    return Ok(AdminWalletMutationOutcome::Invalid(
                        "payment order refund amounts are invalid".to_string(),
                    ));
                }
                let mut order = order;
                order.refunded_amount_usd = refunded_after;
                order.refundable_amount_usd = refundable_after;
                updated_order = Some(order);
            }

            let mut updated_wallet = wallet.clone();
            updated_wallet.balance = after_recharge;
            updated_wallet.total_refunded = after_total_refunded;
            updated_wallet.updated_at_unix_secs = now_unix_secs;

            let transaction = AdminWalletTransactionRecord {
                id: uuid::Uuid::new_v4().to_string(),
                wallet_id: updated_wallet.id.clone(),
                category: "refund".to_string(),
                reason_code: "refund_revert".to_string(),
                amount: amount_usd,
                balance_before: before_total,
                balance_after: after_total,
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

            if let Some(updated_order) = updated_order {
                self.admin_wallet_payment_order_store
                    .as_ref()
                    .expect("admin wallet payment order store should exist")
                    .lock()
                    .expect("admin wallet payment order store should lock")
                    .insert(updated_order.id.clone(), updated_order);
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
            if let Some(transaction_store) = self.admin_wallet_transaction_store.as_ref() {
                transaction_store
                    .lock()
                    .expect("admin wallet transaction store should lock")
                    .insert(transaction.id.clone(), transaction.clone());
            }

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn refund_with_proof(proof: serde_json::Value) -> AdminWalletRefundRecord {
        AdminWalletRefundRecord {
            id: "refund-proof-lifecycle".to_string(),
            refund_no: "rf-proof-lifecycle".to_string(),
            wallet_id: "wallet-proof-lifecycle".to_string(),
            user_id: Some("user-proof-lifecycle".to_string()),
            payment_order_id: None,
            source_type: "manual".to_string(),
            source_id: None,
            refund_mode: "original_channel".to_string(),
            amount_usd: 4.0,
            status: "processing".to_string(),
            reason: Some("proof lifecycle regression".to_string()),
            failure_reason: None,
            gateway_refund_id: Some("gateway-proof-lifecycle".to_string()),
            payout_method: Some("wxpay".to_string()),
            payout_reference: None,
            payout_proof: Some(proof),
            requested_by: Some("user-proof-lifecycle".to_string()),
            approved_by: Some("admin-proof-lifecycle".to_string()),
            processed_by: Some("admin-proof-lifecycle".to_string()),
            created_at_unix_ms: 1_710_000_000,
            updated_at_unix_secs: 1_710_000_000,
            processed_at_unix_secs: Some(1_710_000_010),
            completed_at_unix_secs: None,
        }
    }

    #[tokio::test]
    async fn completion_preserves_processing_proof_on_non_terminal_retry() {
        let processing_proof = json!({
            "gateway": "wxpay",
            "id": "gateway-proof-lifecycle",
            "status": "processing"
        });
        let state = AppState::new()
            .expect("gateway state should build")
            .with_admin_wallet_refunds_for_tests([refund_with_proof(processing_proof.clone())]);

        let outcome = state
            .admin_complete_wallet_refund(
                "wallet-proof-lifecycle",
                "refund-proof-lifecycle",
                Some("gateway-proof-lifecycle"),
                None,
                Some(json!({
                    "gateway": "wxpay",
                    "id": "gateway-proof-lifecycle",
                    "status": "pending",
                    "attempt": 2
                })),
            )
            .await
            .expect("completion should resolve");
        let AdminWalletMutationOutcome::Applied(refund) = outcome else {
            panic!("completion should apply");
        };
        assert_eq!(refund.status, "succeeded");
        assert_eq!(refund.payout_proof, Some(processing_proof));
    }

    #[tokio::test]
    async fn completion_allows_terminal_success_proof_to_upgrade_processing_evidence() {
        let state = AppState::new()
            .expect("gateway state should build")
            .with_admin_wallet_refunds_for_tests([refund_with_proof(json!({
                "gateway": "wxpay",
                "id": "gateway-proof-lifecycle",
                "status": "processing"
            }))]);
        let success_proof = json!({
            "gateway": "wxpay",
            "id": "gateway-proof-lifecycle",
            "status": "succeeded",
            "processed_at": "2026-08-29T00:00:00Z"
        });

        let outcome = state
            .admin_complete_wallet_refund(
                "wallet-proof-lifecycle",
                "refund-proof-lifecycle",
                Some("gateway-proof-lifecycle"),
                None,
                Some(success_proof.clone()),
            )
            .await
            .expect("completion should resolve");
        let AdminWalletMutationOutcome::Applied(refund) = outcome else {
            panic!("completion should apply");
        };
        assert_eq!(refund.status, "succeeded");
        assert_eq!(refund.payout_proof, Some(success_proof));
    }
}
