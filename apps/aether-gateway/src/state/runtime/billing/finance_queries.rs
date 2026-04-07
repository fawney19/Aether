use aether_data::repository::wallet::{
    AdminPaymentOrderListQuery, AdminWalletLedgerQuery, AdminWalletListQuery,
    AdminWalletRefundRequestListQuery, StoredAdminPaymentCallback, StoredAdminPaymentOrder,
    StoredAdminWalletLedgerItem, StoredAdminWalletListItem, StoredAdminWalletRefund,
    StoredAdminWalletRefundRequestItem, StoredAdminWalletTransaction,
};

use crate::{
    AdminWalletMutationOutcome, AdminWalletPaymentOrderRecord, AdminWalletRefundRecord, AppState,
    GatewayAdminPaymentCallbackView, GatewayError,
};

impl AppState {
    pub(crate) async fn list_admin_wallets(
        &self,
        status: Option<&str>,
        owner_type: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<StoredAdminWalletListItem>, u64), GatewayError> {
        let page = self
            .data
            .list_admin_wallets(&AdminWalletListQuery {
                status: status.map(ToOwned::to_owned),
                owner_type: owner_type.map(ToOwned::to_owned),
                limit,
                offset,
            })
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        Ok((page.items, page.total))
    }

    pub(crate) async fn list_admin_wallet_ledger(
        &self,
        category: Option<&str>,
        reason_code: Option<&str>,
        owner_type: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<StoredAdminWalletLedgerItem>, u64), GatewayError> {
        let page = self
            .data
            .list_admin_wallet_ledger(&AdminWalletLedgerQuery {
                category: category.map(ToOwned::to_owned),
                reason_code: reason_code.map(ToOwned::to_owned),
                owner_type: owner_type.map(ToOwned::to_owned),
                limit,
                offset,
            })
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        Ok((page.items, page.total))
    }

    pub(crate) async fn list_admin_payment_orders(
        &self,
        status: Option<&str>,
        payment_method: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Option<(Vec<AdminWalletPaymentOrderRecord>, u64)>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_wallet_payment_order_store.as_ref() {
            let now_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
            let mut items = store
                .lock()
                .expect("admin wallet payment order store should lock")
                .values()
                .filter(|order| {
                    payment_method.is_none_or(|expected| order.payment_method == expected)
                        && status.is_none_or(|expected| {
                            let effective_status = if order.status == "pending"
                                && order
                                    .expires_at_unix_secs
                                    .is_some_and(|value| value < now_unix_secs)
                            {
                                "expired"
                            } else {
                                order.status.as_str()
                            };
                            effective_status == expected
                        })
                })
                .cloned()
                .collect::<Vec<_>>();
            items.sort_by(|left, right| {
                right
                    .created_at_unix_secs
                    .cmp(&left.created_at_unix_secs)
                    .then_with(|| right.id.cmp(&left.id))
            });
            let total = items.len() as u64;
            let items = items
                .into_iter()
                .skip(offset)
                .take(limit)
                .collect::<Vec<_>>();
            return Ok(Some((items, total)));
        }

        let page = self
            .data
            .list_admin_payment_orders(&AdminPaymentOrderListQuery {
                status: status.map(ToOwned::to_owned),
                payment_method: payment_method.map(ToOwned::to_owned),
                limit,
                offset,
            })
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        Ok(Some((
            page.items
                .into_iter()
                .map(stored_admin_payment_order_to_gateway)
                .collect(),
            page.total,
        )))
    }

    pub(crate) async fn list_admin_payment_callbacks(
        &self,
        payment_method: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Option<(Vec<GatewayAdminPaymentCallbackView>, u64)>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_payment_callback_store.as_ref() {
            let mut items = store
                .lock()
                .expect("admin payment callback store should lock")
                .values()
                .filter(|callback| {
                    payment_method.is_none_or(|expected| callback.payment_method == expected)
                })
                .cloned()
                .collect::<Vec<_>>();
            items.sort_by(|left, right| {
                right
                    .created_at_unix_secs
                    .cmp(&left.created_at_unix_secs)
                    .then_with(|| right.id.cmp(&left.id))
            });
            let total = items.len() as u64;
            let items = items
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(Into::into)
                .collect::<Vec<_>>();
            return Ok(Some((items, total)));
        }

        let page = self
            .data
            .list_admin_payment_callbacks(payment_method, limit, offset)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        Ok(Some((
            page.items
                .into_iter()
                .map(stored_admin_payment_callback_to_gateway)
                .collect(),
            page.total,
        )))
    }

    pub(crate) async fn list_admin_wallet_transactions(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<StoredAdminWalletTransaction>, u64), GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_wallet_transaction_store.as_ref() {
            let mut items = store
                .lock()
                .expect("admin wallet transaction store should lock")
                .values()
                .filter(|transaction| transaction.wallet_id == wallet_id)
                .cloned()
                .collect::<Vec<_>>();
            items.sort_by(|left, right| {
                right
                    .created_at_unix_secs
                    .cmp(&left.created_at_unix_secs)
                    .then_with(|| right.id.cmp(&left.id))
            });
            let total = items.len() as u64;
            let items = items
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|record| StoredAdminWalletTransaction {
                    id: record.id,
                    wallet_id: record.wallet_id,
                    category: record.category,
                    reason_code: record.reason_code,
                    amount: record.amount,
                    balance_before: record.balance_before,
                    balance_after: record.balance_after,
                    recharge_balance_before: record.recharge_balance_before,
                    recharge_balance_after: record.recharge_balance_after,
                    gift_balance_before: record.gift_balance_before,
                    gift_balance_after: record.gift_balance_after,
                    link_type: record.link_type,
                    link_id: record.link_id,
                    operator_id: record.operator_id,
                    operator_name: None,
                    operator_email: None,
                    description: record.description,
                    created_at_unix_secs: Some(record.created_at_unix_secs),
                })
                .collect::<Vec<_>>();
            return Ok((items, total));
        }

        let page = self
            .data
            .list_admin_wallet_transactions(wallet_id, limit, offset)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        Ok((page.items, page.total))
    }

    pub(crate) async fn list_admin_wallet_refunds(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<AdminWalletRefundRecord>, u64), GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_wallet_refund_store.as_ref() {
            let mut items = store
                .lock()
                .expect("admin wallet refund store should lock")
                .values()
                .filter(|refund| refund.wallet_id == wallet_id)
                .cloned()
                .collect::<Vec<_>>();
            items.sort_by(|left, right| {
                right
                    .created_at_unix_secs
                    .cmp(&left.created_at_unix_secs)
                    .then_with(|| right.id.cmp(&left.id))
            });
            let total = items.len() as u64;
            let items = items
                .into_iter()
                .skip(offset)
                .take(limit)
                .collect::<Vec<_>>();
            return Ok((items, total));
        }

        let page = self
            .data
            .list_admin_wallet_refunds(wallet_id, limit, offset)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        Ok((
            page.items
                .into_iter()
                .map(stored_admin_wallet_refund_to_gateway)
                .collect(),
            page.total,
        ))
    }

    pub(crate) async fn list_admin_wallet_refund_requests(
        &self,
        status: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<StoredAdminWalletRefundRequestItem>, u64), GatewayError> {
        #[cfg(test)]
        if let (Some(wallet_store), Some(refund_store)) = (
            self.auth_wallet_store.as_ref(),
            self.admin_wallet_refund_store.as_ref(),
        ) {
            let wallets = wallet_store
                .lock()
                .expect("auth wallet store should lock")
                .clone();
            let mut items = refund_store
                .lock()
                .expect("admin wallet refund store should lock")
                .values()
                .filter(|refund| status.is_none_or(|expected| refund.status == expected))
                .filter(|refund| {
                    wallets
                        .get(&refund.wallet_id)
                        .is_some_and(|wallet| wallet.user_id.is_some())
                })
                .cloned()
                .collect::<Vec<_>>();
            items.sort_by(|left, right| {
                right
                    .created_at_unix_secs
                    .cmp(&left.created_at_unix_secs)
                    .then_with(|| right.id.cmp(&left.id))
            });
            let total = items.len() as u64;
            let items = items
                .into_iter()
                .skip(offset)
                .take(limit)
                .filter_map(|refund| {
                    wallets.get(&refund.wallet_id).map(|wallet| {
                        StoredAdminWalletRefundRequestItem {
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
                            wallet_user_id: wallet.user_id.clone(),
                            wallet_user_name: None,
                            wallet_api_key_id: wallet.api_key_id.clone(),
                            api_key_name: None,
                            wallet_status: wallet.status.clone(),
                            created_at_unix_secs: Some(refund.created_at_unix_secs),
                            updated_at_unix_secs: Some(refund.updated_at_unix_secs),
                            processed_at_unix_secs: refund.processed_at_unix_secs,
                            completed_at_unix_secs: refund.completed_at_unix_secs,
                        }
                    })
                })
                .collect::<Vec<_>>();
            return Ok((items, total));
        }

        let page = self
            .data
            .list_admin_wallet_refund_requests(&AdminWalletRefundRequestListQuery {
                status: status.map(ToOwned::to_owned),
                limit,
                offset,
            })
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        Ok((page.items, page.total))
    }

    pub(crate) async fn read_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<AdminWalletMutationOutcome<AdminWalletPaymentOrderRecord>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.admin_wallet_payment_order_store.as_ref() {
            return Ok(store
                .lock()
                .expect("admin wallet payment order store should lock")
                .get(order_id)
                .cloned()
                .map(AdminWalletMutationOutcome::Applied)
                .unwrap_or(AdminWalletMutationOutcome::NotFound));
        }

        if !self.has_wallet_data_reader() {
            return Ok(AdminWalletMutationOutcome::Unavailable);
        }

        let order = self
            .data
            .find_admin_payment_order(order_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        match order {
            Some(record) => Ok(AdminWalletMutationOutcome::Applied(
                stored_admin_payment_order_to_gateway(record),
            )),
            None => Ok(AdminWalletMutationOutcome::NotFound),
        }
    }
}

fn stored_admin_payment_order_to_gateway(
    record: StoredAdminPaymentOrder,
) -> AdminWalletPaymentOrderRecord {
    AdminWalletPaymentOrderRecord {
        id: record.id,
        order_no: record.order_no,
        wallet_id: record.wallet_id,
        user_id: record.user_id,
        amount_usd: record.amount_usd,
        pay_amount: record.pay_amount,
        pay_currency: record.pay_currency,
        exchange_rate: record.exchange_rate,
        refunded_amount_usd: record.refunded_amount_usd,
        refundable_amount_usd: record.refundable_amount_usd,
        payment_method: record.payment_method,
        gateway_order_id: record.gateway_order_id,
        status: record.status,
        gateway_response: record.gateway_response,
        created_at_unix_secs: record.created_at_unix_secs,
        paid_at_unix_secs: record.paid_at_unix_secs,
        credited_at_unix_secs: record.credited_at_unix_secs,
        expires_at_unix_secs: record.expires_at_unix_secs,
    }
}

fn stored_admin_payment_callback_to_gateway(
    record: StoredAdminPaymentCallback,
) -> GatewayAdminPaymentCallbackView {
    GatewayAdminPaymentCallbackView {
        id: record.id,
        payment_order_id: record.payment_order_id,
        payment_method: record.payment_method,
        callback_key: record.callback_key,
        order_no: record.order_no,
        gateway_order_id: record.gateway_order_id,
        payload_hash: record.payload_hash,
        signature_valid: record.signature_valid,
        status: record.status,
        payload: record.payload,
        error_message: record.error_message,
        created_at_unix_secs: record.created_at_unix_secs,
        processed_at_unix_secs: record.processed_at_unix_secs,
    }
}

fn stored_admin_wallet_refund_to_gateway(
    record: StoredAdminWalletRefund,
) -> AdminWalletRefundRecord {
    AdminWalletRefundRecord {
        id: record.id,
        refund_no: record.refund_no,
        wallet_id: record.wallet_id,
        user_id: record.user_id,
        payment_order_id: record.payment_order_id,
        source_type: record.source_type,
        source_id: record.source_id,
        refund_mode: record.refund_mode,
        amount_usd: record.amount_usd,
        status: record.status,
        reason: record.reason,
        failure_reason: record.failure_reason,
        gateway_refund_id: record.gateway_refund_id,
        payout_method: record.payout_method,
        payout_reference: record.payout_reference,
        payout_proof: record.payout_proof,
        requested_by: record.requested_by,
        approved_by: record.approved_by,
        processed_by: record.processed_by,
        created_at_unix_secs: record.created_at_unix_secs,
        updated_at_unix_secs: record.updated_at_unix_secs,
        processed_at_unix_secs: record.processed_at_unix_secs,
        completed_at_unix_secs: record.completed_at_unix_secs,
    }
}
