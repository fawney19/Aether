use crate::{AppState, GatewayError};

impl AppState {
    pub(crate) async fn find_wallet(
        &self,
        lookup: aether_data::repository::wallet::WalletLookupKey<'_>,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        #[cfg(test)]
        if let Some(store) = self.auth_wallet_store.as_ref() {
            let wallet = {
                let wallets = store.lock().expect("auth wallet store should lock");
                match lookup {
                    aether_data::repository::wallet::WalletLookupKey::WalletId(wallet_id) => {
                        wallets.get(wallet_id).cloned()
                    }
                    aether_data::repository::wallet::WalletLookupKey::UserId(user_id) => wallets
                        .values()
                        .find(|wallet| wallet.user_id.as_deref() == Some(user_id))
                        .cloned(),
                    aether_data::repository::wallet::WalletLookupKey::ApiKeyId(api_key_id) => {
                        wallets
                            .values()
                            .find(|wallet| wallet.api_key_id.as_deref() == Some(api_key_id))
                            .cloned()
                    }
                }
            };
            if wallet.is_some() {
                return Ok(wallet);
            }
        }

        self.data
            .find_wallet(lookup)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn read_wallet_snapshot_for_auth(
        &self,
        user_id: &str,
        api_key_id: &str,
        api_key_is_standalone: bool,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        let user_id = user_id.trim();
        let api_key_id = api_key_id.trim();
        let lookup = if api_key_is_standalone {
            if api_key_id.is_empty() {
                None
            } else {
                Some((
                    format!("api_key:{api_key_id}"),
                    aether_data::repository::wallet::WalletLookupKey::ApiKeyId(api_key_id),
                ))
            }
        } else if !user_id.is_empty() {
            Some((
                format!("user:{user_id}"),
                aether_data::repository::wallet::WalletLookupKey::UserId(user_id),
            ))
        } else if !api_key_id.is_empty() {
            Some((
                format!("api_key:{api_key_id}"),
                aether_data::repository::wallet::WalletLookupKey::ApiKeyId(api_key_id),
            ))
        } else {
            None
        };

        let Some((cache_key, lookup)) = lookup else {
            return Ok(None);
        };

        let ttl = self.frontdoor_runtime_guards.auth_capacity_cache_ttl;
        if ttl.is_zero() {
            return self.find_wallet(lookup).await;
        }

        self.auth_wallet_snapshot_cache
            .get_or_load(
                cache_key,
                ttl,
                || async move { self.find_wallet(lookup).await },
            )
            .await
    }

    pub(crate) async fn read_wallet_snapshot_for_auth_uncached(
        &self,
        user_id: &str,
        api_key_id: &str,
        api_key_is_standalone: bool,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        let user_id = user_id.trim();
        let api_key_id = api_key_id.trim();
        let lookup = if api_key_is_standalone {
            if api_key_id.is_empty() {
                None
            } else {
                Some(aether_data::repository::wallet::WalletLookupKey::ApiKeyId(
                    api_key_id,
                ))
            }
        } else if !user_id.is_empty() {
            Some(aether_data::repository::wallet::WalletLookupKey::UserId(
                user_id,
            ))
        } else if !api_key_id.is_empty() {
            Some(aether_data::repository::wallet::WalletLookupKey::ApiKeyId(
                api_key_id,
            ))
        } else {
            None
        };

        let Some(lookup) = lookup else {
            return Ok(None);
        };

        self.find_wallet(lookup).await
    }

    pub(crate) async fn list_wallet_snapshots_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        self.data
            .list_wallets_by_user_ids(user_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_wallet_snapshots_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<aether_data::repository::wallet::StoredWalletSnapshot>, GatewayError> {
        self.data
            .list_wallets_by_api_key_ids(api_key_ids)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_wallet_today_usage(
        &self,
        wallet_id: &str,
        billing_timezone: &str,
    ) -> Result<Option<aether_data::repository::wallet::StoredWalletDailyUsageLedger>, GatewayError>
    {
        self.data
            .find_wallet_today_usage(wallet_id, billing_timezone)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_wallet_daily_usage_history(
        &self,
        wallet_id: &str,
        billing_timezone: &str,
        limit: usize,
    ) -> Result<aether_data::repository::wallet::StoredWalletDailyUsageLedgerPage, GatewayError>
    {
        self.data
            .list_wallet_daily_usage_history(wallet_id, billing_timezone, limit)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn list_wallet_payment_orders_by_user_id(
        &self,
        user_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<aether_data::repository::wallet::StoredAdminPaymentOrderPage, GatewayError> {
        self.data
            .list_wallet_payment_orders_by_user_id(user_id, limit, offset)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_wallet_payment_order_by_user_id(
        &self,
        user_id: &str,
        order_id: &str,
    ) -> Result<Option<aether_data::repository::wallet::StoredAdminPaymentOrder>, GatewayError>
    {
        self.data
            .find_wallet_payment_order_by_user_id(user_id, order_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_pending_plan_purchase_order_by_user_id(
        &self,
        user_id: &str,
        product_id: &str,
    ) -> Result<Option<aether_data::repository::wallet::StoredAdminPaymentOrder>, GatewayError>
    {
        self.data
            .find_pending_plan_purchase_order_by_user_id(user_id, product_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(crate) async fn find_wallet_refund(
        &self,
        wallet_id: &str,
        refund_id: &str,
    ) -> Result<Option<aether_data::repository::wallet::StoredAdminWalletRefund>, GatewayError>
    {
        #[cfg(test)]
        if let Some(store) = self.admin_wallet_refund_store.as_ref() {
            return Ok(store
                .lock()
                .expect("admin wallet refund store should lock")
                .get(refund_id)
                .filter(|refund| refund.wallet_id == wallet_id)
                .cloned()
                .map(test_admin_wallet_refund_to_stored));
        }

        self.data
            .find_wallet_refund(wallet_id, refund_id)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))
    }
}

#[cfg(test)]
fn test_admin_wallet_refund_to_stored(
    refund: crate::AdminWalletRefundRecord,
) -> aether_data::repository::wallet::StoredAdminWalletRefund {
    aether_data::repository::wallet::StoredAdminWalletRefund {
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
