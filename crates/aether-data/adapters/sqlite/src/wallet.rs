use crate::error::SqlResultExt;
use crate::{sqlite_optional_real, sqlite_real, SqlitePool};
use aether_data_contracts::repository::wallet::{
    redeem_code_credits_recharge_balance, redeem_code_payment_method, redeem_code_refundable_amount,
};
use aether_data_contracts::repository::wallet::{
    AdjustWalletBalanceInput, AdminPaymentOrderListQuery, AdminRedeemCodeBatchListQuery,
    AdminRedeemCodeListQuery, AdminWalletLedgerQuery, AdminWalletListQuery,
    AdminWalletRefundRequestListQuery, CompleteAdminWalletRefundInput,
    CreateAdminRedeemCodeBatchInput, CreateAdminRedeemCodeBatchResult,
    CreateManualWalletRechargeInput, CreatePlanPurchaseOrderInput, CreatePlanPurchaseOrderOutcome,
    CreateWalletRechargeOrderInput, CreateWalletRechargeOrderOutcome,
    CreateWalletRefundRequestInput, CreateWalletRefundRequestOutcome,
    CreatedAdminRedeemCodePlaintext, CreditAdminPaymentOrderInput, DeleteAdminRedeemCodeBatchInput,
    DisableAdminRedeemCodeBatchInput, DisableAdminRedeemCodeInput, FailAdminWalletRefundInput,
    ProcessAdminWalletRefundInput, ProcessPaymentCallbackInput, ProcessPaymentCallbackOutcome,
    RedeemWalletCodeInput, RedeemWalletCodeOutcome, StoredAdminPaymentCallback,
    StoredAdminPaymentCallbackPage, StoredAdminPaymentOrder, StoredAdminPaymentOrderPage,
    StoredAdminRedeemCode, StoredAdminRedeemCodeBatch, StoredAdminRedeemCodeBatchPage,
    StoredAdminRedeemCodePage, StoredAdminWalletLedgerItem, StoredAdminWalletLedgerPage,
    StoredAdminWalletListItem, StoredAdminWalletListPage, StoredAdminWalletRefund,
    StoredAdminWalletRefundPage, StoredAdminWalletRefundRequestItem,
    StoredAdminWalletRefundRequestPage, StoredAdminWalletTransaction,
    StoredAdminWalletTransactionPage, StoredWalletDailyUsageLedger,
    StoredWalletDailyUsageLedgerPage, StoredWalletSnapshot, WalletLookupKey, WalletMutationOutcome,
    WalletReadRepository, WalletWriteRepository,
};
use aether_data_contracts::DataLayerError;
use async_trait::async_trait;
use chrono::Utc;
use sqlx::{sqlite::SqliteRow, QueryBuilder, Row, Sqlite};

#[derive(Debug, Clone)]
pub struct SqliteWalletReadRepository {
    pool: SqlitePool,
}

impl SqliteWalletReadRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl WalletReadRepository for SqliteWalletReadRepository {
    async fn find(
        &self,
        key: WalletLookupKey<'_>,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let (where_clause, bind) = match key {
            WalletLookupKey::WalletId(value) => ("WHERE id = ? LIMIT 1", value),
            WalletLookupKey::UserId(value) => ("WHERE user_id = ? LIMIT 1", value),
            WalletLookupKey::ApiKeyId(value) => ("WHERE api_key_id = ? LIMIT 1", value),
        };
        let sql = wallet_select_sql(where_clause);
        let row = sqlx::query(&sql)
            .bind(bind)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_wallet_row).transpose()
    }

    async fn update_auth_user_wallet_limit_mode(
        &self,
        user_id: &str,
        limit_mode: &str,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let result =
            sqlx::query("UPDATE wallets SET limit_mode = ?, updated_at = ? WHERE user_id = ?")
                .bind(limit_mode)
                .bind(current_unix_secs_i64())
                .bind(user_id)
                .execute(&self.pool)
                .await
                .map_sql_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.find(WalletLookupKey::UserId(user_id)).await
    }

    async fn update_auth_api_key_wallet_limit_mode(
        &self,
        api_key_id: &str,
        limit_mode: &str,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let result =
            sqlx::query("UPDATE wallets SET limit_mode = ?, updated_at = ? WHERE api_key_id = ?")
                .bind(limit_mode)
                .bind(current_unix_secs_i64())
                .bind(api_key_id)
                .execute(&self.pool)
                .await
                .map_sql_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.find(WalletLookupKey::ApiKeyId(api_key_id)).await
    }

    async fn initialize_auth_user_wallet(
        &self,
        user_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        initialize_sqlite_auth_wallet(&self.pool, Some(user_id), None, initial_gift_usd, unlimited)
            .await
    }

    async fn initialize_auth_api_key_wallet(
        &self,
        api_key_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        initialize_sqlite_auth_wallet(
            &self.pool,
            None,
            Some(api_key_id),
            initial_gift_usd,
            unlimited,
        )
        .await
    }

    async fn update_auth_user_wallet_snapshot(
        &self,
        user_id: &str,
        balance: f64,
        gift_balance: f64,
        limit_mode: &str,
        currency: &str,
        status: &str,
        total_recharged: f64,
        total_consumed: f64,
        total_refunded: f64,
        total_adjusted: f64,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        update_sqlite_wallet_snapshot(
            &self.pool,
            "user_id",
            user_id,
            balance,
            gift_balance,
            limit_mode,
            currency,
            status,
            total_recharged,
            total_consumed,
            total_refunded,
            total_adjusted,
            updated_at_unix_secs,
        )
        .await?;
        self.find(WalletLookupKey::UserId(user_id)).await
    }

    async fn update_auth_api_key_wallet_snapshot(
        &self,
        api_key_id: &str,
        balance: f64,
        gift_balance: f64,
        limit_mode: &str,
        currency: &str,
        status: &str,
        total_recharged: f64,
        total_consumed: f64,
        total_refunded: f64,
        total_adjusted: f64,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        update_sqlite_wallet_snapshot(
            &self.pool,
            "api_key_id",
            api_key_id,
            balance,
            gift_balance,
            limit_mode,
            currency,
            status,
            total_recharged,
            total_consumed,
            total_refunded,
            total_adjusted,
            updated_at_unix_secs,
        )
        .await?;
        self.find(WalletLookupKey::ApiKeyId(api_key_id)).await
    }

    async fn list_wallets_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredWalletSnapshot>, DataLayerError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
SELECT
  id, user_id, api_key_id, balance, gift_balance, limit_mode, currency,
  status, total_recharged, total_consumed, total_refunded, total_adjusted,
  updated_at AS updated_at_unix_secs
FROM wallets
WHERE user_id IN (
"#,
        );
        let mut separated = builder.separated(", ");
        for user_id in user_ids {
            separated.push_bind(user_id);
        }
        separated.push_unseparated(")");
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_wallet_row).collect()
    }

    async fn list_wallets_by_api_key_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredWalletSnapshot>, DataLayerError> {
        if api_key_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
SELECT
  id, user_id, api_key_id, balance, gift_balance, limit_mode, currency,
  status, total_recharged, total_consumed, total_refunded, total_adjusted,
  updated_at AS updated_at_unix_secs
FROM wallets
WHERE api_key_id IN (
"#,
        );
        let mut separated = builder.separated(", ");
        for api_key_id in api_key_ids {
            separated.push_bind(api_key_id);
        }
        separated.push_unseparated(")");
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_wallet_row).collect()
    }

    async fn list_admin_wallets(
        &self,
        query: &AdminWalletListQuery,
    ) -> Result<StoredAdminWalletListPage, DataLayerError> {
        let total = read_count_row(
            sqlx::query(
                r#"
SELECT COUNT(*) AS total
FROM wallets
WHERE (? IS NULL OR status = ?)
  AND (
    ? IS NULL
    OR (? = 'user' AND user_id IS NOT NULL)
    OR (? = 'api_key' AND api_key_id IS NOT NULL)
  )
"#,
            )
            .bind(query.status.as_deref())
            .bind(query.status.as_deref())
            .bind(query.owner_type.as_deref())
            .bind(query.owner_type.as_deref())
            .bind(query.owner_type.as_deref())
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?,
        )?;
        let rows = sqlx::query(
            r#"
SELECT
  w.id, w.user_id, w.api_key_id, w.balance, w.gift_balance, w.limit_mode,
  w.currency, w.status, w.total_recharged, w.total_consumed, w.total_refunded,
  w.total_adjusted, users.username AS user_name, api_keys.name AS api_key_name,
  w.created_at AS created_at_unix_ms, w.updated_at AS updated_at_unix_secs
FROM wallets w
LEFT JOIN users ON users.id = w.user_id
LEFT JOIN api_keys ON api_keys.id = w.api_key_id
WHERE (? IS NULL OR w.status = ?)
  AND (
    ? IS NULL
    OR (? = 'user' AND w.user_id IS NOT NULL)
    OR (? = 'api_key' AND w.api_key_id IS NOT NULL)
  )
ORDER BY w.updated_at DESC
LIMIT ? OFFSET ?
"#,
        )
        .bind(query.status.as_deref())
        .bind(query.status.as_deref())
        .bind(query.owner_type.as_deref())
        .bind(query.owner_type.as_deref())
        .bind(query.owner_type.as_deref())
        .bind(i64_from_usize(query.limit, "wallet limit")?)
        .bind(i64_from_usize(query.offset, "wallet offset")?)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_admin_wallet_list_item_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StoredAdminWalletListPage { items, total })
    }

    async fn list_admin_wallet_ledger(
        &self,
        query: &AdminWalletLedgerQuery,
    ) -> Result<StoredAdminWalletLedgerPage, DataLayerError> {
        let total = read_count_row(
            sqlx::query(
                r#"
SELECT COUNT(*) AS total
FROM wallet_transactions tx
JOIN wallets w ON w.id = tx.wallet_id
WHERE (? IS NULL OR tx.category = ?)
  AND (? IS NULL OR tx.reason_code = ?)
  AND (
    ? IS NULL
    OR (? = 'user' AND w.user_id IS NOT NULL)
    OR (? = 'api_key' AND w.api_key_id IS NOT NULL)
  )
"#,
            )
            .bind(query.category.as_deref())
            .bind(query.category.as_deref())
            .bind(query.reason_code.as_deref())
            .bind(query.reason_code.as_deref())
            .bind(query.owner_type.as_deref())
            .bind(query.owner_type.as_deref())
            .bind(query.owner_type.as_deref())
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?,
        )?;
        let rows = sqlx::query(
            r#"
SELECT
  tx.id, tx.wallet_id, tx.category, tx.reason_code, tx.amount,
  tx.balance_before, tx.balance_after, tx.recharge_balance_before,
  tx.recharge_balance_after, tx.gift_balance_before, tx.gift_balance_after,
  tx.link_type, tx.link_id, tx.operator_id, tx.description,
  w.user_id, w.api_key_id, w.status AS wallet_status,
  wallet_users.username AS wallet_user_name,
  api_keys.name AS api_key_name,
  operator_users.username AS operator_name,
  operator_users.email AS operator_email,
  tx.created_at AS created_at_unix_ms
FROM wallet_transactions tx
JOIN wallets w ON w.id = tx.wallet_id
LEFT JOIN users wallet_users ON wallet_users.id = w.user_id
LEFT JOIN api_keys ON api_keys.id = w.api_key_id
LEFT JOIN users operator_users ON operator_users.id = tx.operator_id
WHERE (? IS NULL OR tx.category = ?)
  AND (? IS NULL OR tx.reason_code = ?)
  AND (
    ? IS NULL
    OR (? = 'user' AND w.user_id IS NOT NULL)
    OR (? = 'api_key' AND w.api_key_id IS NOT NULL)
  )
ORDER BY tx.created_at DESC
LIMIT ? OFFSET ?
"#,
        )
        .bind(query.category.as_deref())
        .bind(query.category.as_deref())
        .bind(query.reason_code.as_deref())
        .bind(query.reason_code.as_deref())
        .bind(query.owner_type.as_deref())
        .bind(query.owner_type.as_deref())
        .bind(query.owner_type.as_deref())
        .bind(i64_from_usize(query.limit, "wallet ledger limit")?)
        .bind(i64_from_usize(query.offset, "wallet ledger offset")?)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_admin_wallet_ledger_item_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StoredAdminWalletLedgerPage { items, total })
    }

    async fn list_admin_wallet_refund_requests(
        &self,
        query: &AdminWalletRefundRequestListQuery,
    ) -> Result<StoredAdminWalletRefundRequestPage, DataLayerError> {
        let total = read_count_row(
            sqlx::query(
                r#"
SELECT COUNT(*) AS total
FROM refund_requests rr
JOIN wallets w ON w.id = rr.wallet_id
WHERE (? IS NULL OR rr.status = ?)
  AND w.user_id IS NOT NULL
"#,
            )
            .bind(query.status.as_deref())
            .bind(query.status.as_deref())
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?,
        )?;
        let rows = sqlx::query(
            r#"
SELECT
  rr.id, rr.refund_no, rr.wallet_id, rr.user_id, rr.payment_order_id,
  rr.source_type, rr.source_id, rr.refund_mode, rr.amount_usd, rr.status,
  rr.reason, rr.failure_reason, rr.gateway_refund_id, rr.payout_method,
  rr.payout_reference, rr.payout_proof, rr.requested_by, rr.approved_by,
  rr.processed_by, w.user_id AS wallet_user_id, w.api_key_id AS wallet_api_key_id,
  w.status AS wallet_status, wallet_users.username AS wallet_user_name,
  api_keys.name AS api_key_name, rr.created_at AS created_at_unix_ms,
  rr.updated_at AS updated_at_unix_secs,
  rr.processed_at AS processed_at_unix_secs,
  rr.completed_at AS completed_at_unix_secs
FROM refund_requests rr
JOIN wallets w ON w.id = rr.wallet_id
LEFT JOIN users wallet_users ON wallet_users.id = w.user_id
LEFT JOIN api_keys ON api_keys.id = w.api_key_id
WHERE (? IS NULL OR rr.status = ?)
  AND w.user_id IS NOT NULL
ORDER BY rr.created_at DESC
LIMIT ? OFFSET ?
"#,
        )
        .bind(query.status.as_deref())
        .bind(query.status.as_deref())
        .bind(i64_from_usize(query.limit, "wallet refund request limit")?)
        .bind(i64_from_usize(
            query.offset,
            "wallet refund request offset",
        )?)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_admin_wallet_refund_request_item_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StoredAdminWalletRefundRequestPage { items, total })
    }

    async fn list_admin_wallet_transactions(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminWalletTransactionPage, DataLayerError> {
        let total = read_count_row(
            sqlx::query("SELECT COUNT(*) AS total FROM wallet_transactions WHERE wallet_id = ?")
                .bind(wallet_id)
                .fetch_one(&self.pool)
                .await
                .map_sql_err()?,
        )?;
        let rows = sqlx::query(
            r#"
SELECT
  tx.id, tx.wallet_id, tx.category, tx.reason_code, tx.amount,
  tx.balance_before, tx.balance_after, tx.recharge_balance_before,
  tx.recharge_balance_after, tx.gift_balance_before, tx.gift_balance_after,
  tx.link_type, tx.link_id, tx.operator_id, tx.description,
  operator_users.username AS operator_name,
  operator_users.email AS operator_email,
  tx.created_at AS created_at_unix_ms
FROM wallet_transactions tx
LEFT JOIN users operator_users ON operator_users.id = tx.operator_id
WHERE tx.wallet_id = ?
ORDER BY tx.created_at DESC
LIMIT ? OFFSET ?
"#,
        )
        .bind(wallet_id)
        .bind(i64_from_usize(limit, "wallet transaction limit")?)
        .bind(i64_from_usize(offset, "wallet transaction offset")?)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_wallet_transaction_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StoredAdminWalletTransactionPage { items, total })
    }

    async fn find_wallet_today_usage(
        &self,
        wallet_id: &str,
        billing_timezone: &str,
    ) -> Result<Option<StoredWalletDailyUsageLedger>, DataLayerError> {
        let billing_date = current_billing_date(billing_timezone)?;
        let sql = daily_usage_select_sql("AND billing_date = ? LIMIT 1");
        let row = sqlx::query(&sql)
            .bind(wallet_id)
            .bind(billing_timezone)
            .bind(billing_date)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_daily_usage_row).transpose()
    }

    async fn list_wallet_daily_usage_history(
        &self,
        wallet_id: &str,
        billing_timezone: &str,
        limit: usize,
    ) -> Result<StoredWalletDailyUsageLedgerPage, DataLayerError> {
        let billing_date = current_billing_date(billing_timezone)?;
        let total: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM wallet_daily_usage_ledgers
WHERE wallet_id = ?
  AND billing_timezone = ?
  AND billing_date < ?
"#,
        )
        .bind(wallet_id)
        .bind(billing_timezone)
        .bind(&billing_date)
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;

        let sql = daily_usage_select_sql("AND billing_date < ? ORDER BY billing_date DESC LIMIT ?");
        let rows = sqlx::query(&sql)
            .bind(wallet_id)
            .bind(billing_timezone)
            .bind(billing_date)
            .bind(i64::try_from(limit).unwrap_or(i64::MAX))
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_daily_usage_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StoredWalletDailyUsageLedgerPage {
            items,
            total: total.max(0) as u64,
        })
    }

    async fn list_admin_wallet_refunds(
        &self,
        wallet_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminWalletRefundPage, DataLayerError> {
        let total = read_count_row(
            sqlx::query("SELECT COUNT(*) AS total FROM refund_requests WHERE wallet_id = ?")
                .bind(wallet_id)
                .fetch_one(&self.pool)
                .await
                .map_sql_err()?,
        )?;
        let sql =
            refund_select_sql("WHERE wallet_id = ? ORDER BY created_at DESC LIMIT ? OFFSET ?");
        let rows = sqlx::query(&sql)
            .bind(wallet_id)
            .bind(i64_from_usize(limit, "wallet refund limit")?)
            .bind(i64_from_usize(offset, "wallet refund offset")?)
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_refund_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StoredAdminWalletRefundPage { items, total })
    }

    async fn list_admin_payment_orders(
        &self,
        query: &AdminPaymentOrderListQuery,
    ) -> Result<StoredAdminPaymentOrderPage, DataLayerError> {
        let now = current_unix_secs_i64();
        let total = read_count_row(
            sqlx::query(
                r#"
SELECT COUNT(*) AS total
FROM payment_orders
WHERE (? IS NULL OR payment_method = ?)
  AND (
    ? IS NULL
    OR (
      CASE
        WHEN status = 'pending' AND expires_at IS NOT NULL AND expires_at < ? THEN 'expired'
        ELSE status
      END
    ) = ?
  )
"#,
            )
            .bind(query.payment_method.as_deref())
            .bind(query.payment_method.as_deref())
            .bind(query.status.as_deref())
            .bind(now)
            .bind(query.status.as_deref())
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?,
        )?;
        let rows = sqlx::query(
            r#"
SELECT
  id, order_no, wallet_id, user_id, amount_usd, pay_amount, pay_currency,
  exchange_rate, refunded_amount_usd, refundable_amount_usd, payment_method,
  payment_provider, payment_channel, order_kind, product_id, product_snapshot,
  gateway_order_id, gateway_response, status,
  created_at AS created_at_unix_ms,
  paid_at AS paid_at_unix_secs,
  credited_at AS credited_at_unix_secs,
  expires_at AS expires_at_unix_secs
FROM payment_orders
WHERE (? IS NULL OR payment_method = ?)
  AND (
    ? IS NULL
    OR (
      CASE
        WHEN status = 'pending' AND expires_at IS NOT NULL AND expires_at < ? THEN 'expired'
        ELSE status
      END
    ) = ?
  )
ORDER BY created_at DESC
LIMIT ? OFFSET ?
"#,
        )
        .bind(query.payment_method.as_deref())
        .bind(query.payment_method.as_deref())
        .bind(query.status.as_deref())
        .bind(now)
        .bind(query.status.as_deref())
        .bind(i64_from_usize(query.limit, "payment order limit")?)
        .bind(i64_from_usize(query.offset, "payment order offset")?)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_payment_order_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StoredAdminPaymentOrderPage { items, total })
    }

    async fn find_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        let sql = payment_order_select_sql("WHERE id = ? LIMIT 1");
        let row = sqlx::query(&sql)
            .bind(order_id)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_payment_order_row).transpose()
    }

    async fn list_wallet_payment_orders_by_user_id(
        &self,
        user_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminPaymentOrderPage, DataLayerError> {
        let total = read_count_row(
            sqlx::query("SELECT COUNT(*) AS total FROM payment_orders WHERE user_id = ?")
                .bind(user_id)
                .fetch_one(&self.pool)
                .await
                .map_sql_err()?,
        )?;
        let now = current_unix_secs_i64();
        let rows = sqlx::query(
            r#"
SELECT
  id, order_no, wallet_id, user_id, amount_usd, pay_amount, pay_currency,
  exchange_rate, refunded_amount_usd, refundable_amount_usd, payment_method,
  payment_provider, payment_channel, order_kind, product_id, product_snapshot,
  gateway_order_id, gateway_response,
  CASE
    WHEN status = 'pending' AND expires_at IS NOT NULL AND expires_at < ? THEN 'expired'
    ELSE status
  END AS status,
  created_at AS created_at_unix_ms,
  paid_at AS paid_at_unix_secs,
  credited_at AS credited_at_unix_secs,
  expires_at AS expires_at_unix_secs
FROM payment_orders
WHERE user_id = ?
ORDER BY created_at DESC
LIMIT ? OFFSET ?
"#,
        )
        .bind(now)
        .bind(user_id)
        .bind(i64_from_usize(limit, "wallet payment order limit")?)
        .bind(i64_from_usize(offset, "wallet payment order offset")?)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_payment_order_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StoredAdminPaymentOrderPage { items, total })
    }

    async fn count_pending_refunds_by_user_id(&self, user_id: &str) -> Result<u64, DataLayerError> {
        read_count_row(
            sqlx::query(
                r#"
SELECT COUNT(*) AS total
FROM refund_requests
WHERE user_id = ?
  AND status IN ('pending_approval', 'approved', 'processing')
"#,
            )
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?,
        )
    }

    async fn count_pending_payment_orders_by_user_id(
        &self,
        user_id: &str,
    ) -> Result<u64, DataLayerError> {
        read_count_row(
            sqlx::query(
                r#"
SELECT COUNT(*) AS total
FROM payment_orders
WHERE user_id = ?
  AND status IN ('pending', 'paid')
"#,
            )
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?,
        )
    }

    async fn find_wallet_payment_order_by_user_id(
        &self,
        user_id: &str,
        order_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  id, order_no, wallet_id, user_id, amount_usd, pay_amount, pay_currency,
  exchange_rate, refunded_amount_usd, refundable_amount_usd, payment_method,
  payment_provider, payment_channel, order_kind, product_id, product_snapshot,
  gateway_order_id, gateway_response,
  CASE
    WHEN status = 'pending' AND expires_at IS NOT NULL AND expires_at < ? THEN 'expired'
    ELSE status
  END AS status,
  created_at AS created_at_unix_ms,
  paid_at AS paid_at_unix_secs,
  credited_at AS credited_at_unix_secs,
  expires_at AS expires_at_unix_secs
FROM payment_orders
WHERE user_id = ?
  AND id = ?
LIMIT 1
"#,
        )
        .bind(current_unix_secs_i64())
        .bind(user_id)
        .bind(order_id)
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?;
        row.as_ref().map(map_payment_order_row).transpose()
    }

    async fn find_pending_plan_purchase_order_by_user_id(
        &self,
        user_id: &str,
        product_id: &str,
    ) -> Result<Option<StoredAdminPaymentOrder>, DataLayerError> {
        let sql = payment_order_select_sql(
            r#"
WHERE user_id = ?
  AND product_id = ?
  AND order_kind = 'plan_purchase'
  AND status = 'pending'
  AND expires_at > ?
ORDER BY created_at DESC
LIMIT 1
"#,
        );
        let row = sqlx::query(&sql)
            .bind(user_id)
            .bind(product_id)
            .bind(current_unix_secs_i64())
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_payment_order_row).transpose()
    }

    async fn find_wallet_refund(
        &self,
        wallet_id: &str,
        refund_id: &str,
    ) -> Result<Option<StoredAdminWalletRefund>, DataLayerError> {
        let sql = refund_select_sql("WHERE wallet_id = ? AND id = ? LIMIT 1");
        let row = sqlx::query(&sql)
            .bind(wallet_id)
            .bind(refund_id)
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_refund_row).transpose()
    }

    async fn list_admin_payment_callbacks(
        &self,
        payment_method: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<StoredAdminPaymentCallbackPage, DataLayerError> {
        let total = read_count_row(
            sqlx::query(
                r#"
SELECT COUNT(*) AS total
FROM payment_callbacks
WHERE (? IS NULL OR payment_method = ?)
"#,
            )
            .bind(payment_method)
            .bind(payment_method)
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?,
        )?;
        let rows = sqlx::query(
            r#"
SELECT
  id, payment_order_id, payment_method, callback_key, order_no,
  gateway_order_id, payload_hash, signature_valid, status, payload,
  error_message, created_at AS created_at_unix_ms,
  processed_at AS processed_at_unix_secs
FROM payment_callbacks
WHERE (? IS NULL OR payment_method = ?)
ORDER BY created_at DESC
LIMIT ? OFFSET ?
"#,
        )
        .bind(payment_method)
        .bind(payment_method)
        .bind(i64_from_usize(limit, "payment callback limit")?)
        .bind(i64_from_usize(offset, "payment callback offset")?)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_payment_callback_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StoredAdminPaymentCallbackPage { items, total })
    }

    async fn list_admin_redeem_code_batches(
        &self,
        query: &AdminRedeemCodeBatchListQuery,
    ) -> Result<StoredAdminRedeemCodeBatchPage, DataLayerError> {
        let total = read_count_row(
            sqlx::query(
                r#"
SELECT COUNT(*) AS total
FROM redeem_code_batches
WHERE (? IS NULL OR status = ?)
"#,
            )
            .bind(query.status.as_deref())
            .bind(query.status.as_deref())
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?,
        )?;
        let rows = sqlx::query(
            r#"
SELECT
  batches.id, batches.name, batches.amount_usd, batches.currency,
  batches.balance_bucket, batches.total_count,
  COALESCE(stats.redeemed_count, 0) AS redeemed_count,
  COALESCE(stats.active_count, 0) AS active_count,
  batches.status, batches.description, batches.created_by,
  batches.expires_at AS expires_at_unix_secs,
  batches.created_at AS created_at_unix_ms,
  batches.updated_at AS updated_at_unix_secs
FROM redeem_code_batches AS batches
LEFT JOIN (
  SELECT
    batch_id,
    SUM(CASE WHEN status = 'redeemed' THEN 1 ELSE 0 END) AS redeemed_count,
    SUM(CASE WHEN status = 'active' THEN 1 ELSE 0 END) AS active_count
  FROM redeem_codes
  GROUP BY batch_id
) AS stats ON stats.batch_id = batches.id
WHERE (? IS NULL OR batches.status = ?)
ORDER BY batches.created_at DESC, batches.id DESC
LIMIT ? OFFSET ?
"#,
        )
        .bind(query.status.as_deref())
        .bind(query.status.as_deref())
        .bind(i64_from_usize(query.limit, "redeem code batch limit")?)
        .bind(i64_from_usize(query.offset, "redeem code batch offset")?)
        .fetch_all(&self.pool)
        .await
        .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_redeem_batch_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StoredAdminRedeemCodeBatchPage { items, total })
    }

    async fn find_admin_redeem_code_batch(
        &self,
        batch_id: &str,
    ) -> Result<Option<StoredAdminRedeemCodeBatch>, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  batches.id, batches.name, batches.amount_usd, batches.currency,
  batches.balance_bucket, batches.total_count,
  COALESCE(stats.redeemed_count, 0) AS redeemed_count,
  COALESCE(stats.active_count, 0) AS active_count,
  batches.status, batches.description, batches.created_by,
  batches.expires_at AS expires_at_unix_secs,
  batches.created_at AS created_at_unix_ms,
  batches.updated_at AS updated_at_unix_secs
FROM redeem_code_batches AS batches
LEFT JOIN (
  SELECT
    batch_id,
    SUM(CASE WHEN status = 'redeemed' THEN 1 ELSE 0 END) AS redeemed_count,
    SUM(CASE WHEN status = 'active' THEN 1 ELSE 0 END) AS active_count
  FROM redeem_codes
  GROUP BY batch_id
) AS stats ON stats.batch_id = batches.id
WHERE batches.id = ?
LIMIT 1
"#,
        )
        .bind(batch_id)
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?;
        row.as_ref().map(map_redeem_batch_row).transpose()
    }

    async fn list_admin_redeem_codes(
        &self,
        query: &AdminRedeemCodeListQuery,
    ) -> Result<StoredAdminRedeemCodePage, DataLayerError> {
        let total = read_count_row(
            sqlx::query(
                r#"
SELECT COUNT(*) AS total
FROM redeem_codes
WHERE batch_id = ?
  AND (? IS NULL OR status = ?)
"#,
            )
            .bind(&query.batch_id)
            .bind(query.status.as_deref())
            .bind(query.status.as_deref())
            .fetch_one(&self.pool)
            .await
            .map_sql_err()?,
        )?;
        let sql = redeem_code_select_sql(
            "WHERE codes.batch_id = ? AND (? IS NULL OR codes.status = ?) ORDER BY codes.created_at DESC, codes.id DESC LIMIT ? OFFSET ?",
        );
        let rows = sqlx::query(&sql)
            .bind(&query.batch_id)
            .bind(query.status.as_deref())
            .bind(query.status.as_deref())
            .bind(i64_from_usize(query.limit, "redeem code limit")?)
            .bind(i64_from_usize(query.offset, "redeem code offset")?)
            .fetch_all(&self.pool)
            .await
            .map_sql_err()?;
        let items = rows
            .iter()
            .map(map_redeem_code_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(StoredAdminRedeemCodePage { items, total })
    }
}

#[async_trait]
impl WalletWriteRepository for SqliteWalletReadRepository {
    async fn create_wallet_recharge_order(
        &self,
        input: CreateWalletRechargeOrderInput,
    ) -> Result<CreateWalletRechargeOrderOutcome, DataLayerError> {
        let now = current_unix_secs_i64();
        let expires_at = i64::try_from(input.expires_at_unix_secs).map_err(|_| {
            DataLayerError::InvalidInput("wallet recharge expires_at overflow".to_string())
        })?;
        let gateway_response =
            json_string(&input.gateway_response, "payment_orders.gateway_response")?;
        let mut tx = self.pool.begin().await.map_sql_err()?;

        let wallet_row = sqlx::query(
            r#"
SELECT id, status
FROM wallets
WHERE user_id = ?
LIMIT 1
"#,
        )
        .bind(&input.user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_sql_err()?;
        let (wallet_id, wallet_status) = if let Some(row) = wallet_row {
            (get::<String>(&row, "id")?, get::<String>(&row, "status")?)
        } else {
            let wallet_id = input
                .preferred_wallet_id
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            sqlx::query(
                r#"
INSERT INTO wallets (
  id, user_id, balance, gift_balance, limit_mode, currency, status,
  total_recharged, total_consumed, total_refunded, total_adjusted,
  created_at, updated_at
)
VALUES (?, ?, 0, 0, 'finite', 'USD', 'active', 0, 0, 0, 0, ?, ?)
"#,
            )
            .bind(&wallet_id)
            .bind(&input.user_id)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
            (wallet_id, "active".to_string())
        };
        if wallet_status != "active" {
            tx.commit().await.map_sql_err()?;
            return Ok(CreateWalletRechargeOrderOutcome::WalletInactive);
        }

        let order_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            r#"
INSERT INTO payment_orders (
  id, order_no, wallet_id, user_id, amount_usd, pay_amount, pay_currency,
  exchange_rate, refunded_amount_usd, refundable_amount_usd, payment_method,
  payment_provider, payment_channel, order_kind, fulfillment_status,
  gateway_order_id, gateway_response, status, created_at, expires_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?, ?, ?, 'wallet_recharge', 'pending', ?, ?, 'pending', ?, ?)
"#,
        )
        .bind(&order_id)
        .bind(&input.order_no)
        .bind(&wallet_id)
        .bind(&input.user_id)
        .bind(input.amount_usd)
        .bind(input.pay_amount)
        .bind(input.pay_currency.as_deref())
        .bind(input.exchange_rate)
        .bind(&input.payment_method)
        .bind(input.payment_provider.as_deref())
        .bind(input.payment_channel.as_deref())
        .bind(&input.gateway_order_id)
        .bind(gateway_response)
        .bind(now)
        .bind(expires_at)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        let row = sqlite_payment_order_by_id(&mut tx, &order_id).await?;
        tx.commit().await.map_sql_err()?;
        Ok(CreateWalletRechargeOrderOutcome::Created(
            map_payment_order_row(&row)?,
        ))
    }

    async fn create_plan_purchase_order(
        &self,
        input: CreatePlanPurchaseOrderInput,
    ) -> Result<CreatePlanPurchaseOrderOutcome, DataLayerError> {
        let now = current_unix_secs_i64();
        let expires_at = i64::try_from(input.expires_at_unix_secs).map_err(|_| {
            DataLayerError::InvalidInput("plan purchase expires_at overflow".to_string())
        })?;
        let gateway_response =
            json_string(&input.gateway_response, "payment_orders.gateway_response")?;
        let product_snapshot =
            json_string(&input.product_snapshot, "payment_orders.product_snapshot")?;
        let mut tx = self.pool.begin().await.map_sql_err()?;

        let wallet_row = sqlx::query(
            r#"
SELECT id, status
FROM wallets
WHERE user_id = ?
LIMIT 1
"#,
        )
        .bind(&input.user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_sql_err()?;
        let (wallet_id, wallet_status) = if let Some(row) = wallet_row {
            (get::<String>(&row, "id")?, get::<String>(&row, "status")?)
        } else {
            let wallet_id = input
                .preferred_wallet_id
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            sqlx::query(
                r#"
INSERT INTO wallets (
  id, user_id, balance, gift_balance, limit_mode, currency, status,
  total_recharged, total_consumed, total_refunded, total_adjusted,
  created_at, updated_at
)
VALUES (?, ?, 0, 0, 'finite', 'USD', 'active', 0, 0, 0, 0, ?, ?)
"#,
            )
            .bind(&wallet_id)
            .bind(&input.user_id)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
            (wallet_id, "active".to_string())
        };
        if wallet_status != "active" {
            tx.commit().await.map_sql_err()?;
            return Ok(CreatePlanPurchaseOrderOutcome::WalletInactive);
        }

        let purchase_limit_scope = plan_purchase_limit_scope(&input.product_snapshot);
        if purchase_limit_scope != "unlimited" {
            let max_active_per_user = plan_max_active_per_user(&input.product_snapshot);
            let mut active_count = if purchase_limit_scope == "lifetime" {
                sqlx::query_scalar::<_, i64>(
                    r#"
SELECT COUNT(*)
FROM user_plan_entitlements
WHERE user_id = ?
  AND plan_id = ?
  AND status = 'active'
"#,
                )
                .bind(&input.user_id)
                .bind(&input.product_id)
                .fetch_one(&mut *tx)
                .await
                .map_sql_err()?
            } else {
                sqlx::query_scalar::<_, i64>(
                    r#"
SELECT COUNT(*)
FROM user_plan_entitlements
WHERE user_id = ?
  AND plan_id = ?
  AND status = 'active'
  AND expires_at > ?
"#,
                )
                .bind(&input.user_id)
                .bind(&input.product_id)
                .bind(now)
                .fetch_one(&mut *tx)
                .await
                .map_sql_err()?
            };
            active_count += sqlx::query_scalar::<_, i64>(
                r#"
	SELECT COUNT(*)
	FROM payment_orders
	WHERE user_id = ?
	  AND product_id = ?
	  AND order_kind = 'plan_purchase'
	  AND status = 'pending'
	  AND expires_at > ?
	"#,
            )
            .bind(&input.user_id)
            .bind(&input.product_id)
            .bind(now)
            .fetch_one(&mut *tx)
            .await
            .map_sql_err()?;
            if active_count >= max_active_per_user {
                tx.commit().await.map_sql_err()?;
                return Ok(CreatePlanPurchaseOrderOutcome::ActivePlanLimitReached);
            }
        }

        let order_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            r#"
INSERT INTO payment_orders (
  id, order_no, wallet_id, user_id, amount_usd, pay_amount, pay_currency,
  exchange_rate, refunded_amount_usd, refundable_amount_usd, payment_method,
  payment_provider, payment_channel, order_kind, product_id, product_snapshot,
  fulfillment_status, gateway_order_id, gateway_response, status, created_at, expires_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, 0, ?, ?, ?, 'plan_purchase', ?, ?, 'pending', ?, ?, 'pending', ?, ?)
"#,
        )
        .bind(&order_id)
        .bind(&input.order_no)
        .bind(&wallet_id)
        .bind(&input.user_id)
        .bind(input.amount_usd)
        .bind(input.pay_amount)
        .bind(&input.pay_currency)
        .bind(input.exchange_rate)
        .bind(&input.payment_method)
        .bind(input.payment_provider.as_deref())
        .bind(input.payment_channel.as_deref())
        .bind(&input.product_id)
        .bind(product_snapshot)
        .bind(&input.gateway_order_id)
        .bind(gateway_response)
        .bind(now)
        .bind(expires_at)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        let row = sqlite_payment_order_by_id(&mut tx, &order_id).await?;
        tx.commit().await.map_sql_err()?;
        Ok(CreatePlanPurchaseOrderOutcome::Created(
            map_payment_order_row(&row)?,
        ))
    }

    async fn create_wallet_refund_request(
        &self,
        input: CreateWalletRefundRequestInput,
    ) -> Result<CreateWalletRefundRequestOutcome, DataLayerError> {
        let now = current_unix_secs_i64();
        let mut tx = self.pool.begin().await.map_sql_err()?;

        if let Some(idempotency_key) = input.idempotency_key.as_deref() {
            let existing =
                sqlite_refund_by_idempotency(&mut tx, &input.user_id, idempotency_key).await?;
            if let Some(row) = existing {
                tx.commit().await.map_sql_err()?;
                return Ok(CreateWalletRefundRequestOutcome::Duplicate(map_refund_row(
                    &row,
                )?));
            }
        }

        let Some(wallet_row) = sqlx::query(
            r#"
SELECT id, balance
FROM wallets
WHERE id = ?
LIMIT 1
"#,
        )
        .bind(&input.wallet_id)
        .fetch_optional(&mut *tx)
        .await
        .map_sql_err()?
        else {
            tx.commit().await.map_sql_err()?;
            return Ok(CreateWalletRefundRequestOutcome::WalletMissing);
        };
        let wallet_recharge_balance = sqlite_real(&wallet_row, "balance")?;
        let wallet_reserved_amount: f64 = sqlx::query_scalar(
            r#"
SELECT COALESCE(SUM(amount_usd), 0.0)
FROM refund_requests
WHERE wallet_id = ?
  AND status IN ('pending_approval', 'approved')
"#,
        )
        .bind(&input.wallet_id)
        .fetch_one(&mut *tx)
        .await
        .map_sql_err()?;
        if input.amount_usd > (wallet_recharge_balance - wallet_reserved_amount) {
            tx.commit().await.map_sql_err()?;
            return Ok(CreateWalletRefundRequestOutcome::RefundAmountExceedsAvailableBalance);
        }

        let mut payment_order_id = None;
        let mut source_type = input
            .source_type
            .clone()
            .unwrap_or_else(|| "wallet_balance".to_string());
        let mut source_id = input.source_id.clone();
        let mut refund_mode = input
            .refund_mode
            .clone()
            .unwrap_or_else(|| "offline_payout".to_string());
        if let Some(order_id) = input.payment_order_id.as_deref() {
            let Some(order_row) = sqlx::query(
                r#"
SELECT id, status, payment_method, refundable_amount_usd
FROM payment_orders
WHERE id = ?
  AND wallet_id = ?
LIMIT 1
"#,
            )
            .bind(order_id)
            .bind(&input.wallet_id)
            .fetch_optional(&mut *tx)
            .await
            .map_sql_err()?
            else {
                tx.commit().await.map_sql_err()?;
                return Ok(CreateWalletRefundRequestOutcome::PaymentOrderNotFound);
            };
            let status: String = get(&order_row, "status")?;
            if status != "credited" {
                tx.commit().await.map_sql_err()?;
                return Ok(CreateWalletRefundRequestOutcome::PaymentOrderNotRefundable);
            }
            let order_reserved_amount: f64 = sqlx::query_scalar(
                r#"
SELECT COALESCE(SUM(amount_usd), 0.0)
FROM refund_requests
WHERE payment_order_id = ?
  AND status IN ('pending_approval', 'approved')
"#,
            )
            .bind(order_id)
            .fetch_one(&mut *tx)
            .await
            .map_sql_err()?;
            let refundable_amount = sqlite_real(&order_row, "refundable_amount_usd")?;
            if input.amount_usd > (refundable_amount - order_reserved_amount) {
                tx.commit().await.map_sql_err()?;
                return Ok(
                    CreateWalletRefundRequestOutcome::RefundAmountExceedsAvailableOrderAmount,
                );
            }
            payment_order_id = Some(order_id.to_string());
            source_type = "payment_order".to_string();
            source_id = Some(order_id.to_string());
            if input.refund_mode.is_none() {
                let payment_method: String = get(&order_row, "payment_method")?;
                refund_mode = default_refund_mode_for_payment_method(&payment_method).to_string();
            }
        }

        let refund_id = uuid::Uuid::new_v4().to_string();
        let insert = sqlx::query(
            r#"
INSERT INTO refund_requests (
  id, refund_no, wallet_id, user_id, payment_order_id, source_type, source_id,
  refund_mode, amount_usd, status, reason, requested_by, idempotency_key,
  created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending_approval', ?, ?, ?, ?, ?)
"#,
        )
        .bind(&refund_id)
        .bind(&input.refund_no)
        .bind(&input.wallet_id)
        .bind(&input.user_id)
        .bind(payment_order_id.as_deref())
        .bind(&source_type)
        .bind(source_id.as_deref())
        .bind(&refund_mode)
        .bind(input.amount_usd)
        .bind(input.reason.as_deref())
        .bind(&input.user_id)
        .bind(input.idempotency_key.as_deref())
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await;

        if let Err(err) = insert {
            if input.idempotency_key.is_some() {
                tx.rollback().await.map_sql_err()?;
                return Ok(CreateWalletRefundRequestOutcome::DuplicateRejected);
            }
            return Err(DataLayerError::sql(err));
        }

        let row = sqlite_refund_by_id(&mut tx, &refund_id).await?;
        tx.commit().await.map_sql_err()?;
        Ok(CreateWalletRefundRequestOutcome::Created(map_refund_row(
            &row,
        )?))
    }

    async fn process_payment_callback(
        &self,
        input: ProcessPaymentCallbackInput,
    ) -> Result<ProcessPaymentCallbackOutcome, DataLayerError> {
        let now = current_unix_secs_i64();
        let payload = json_string(&input.payload, "payment_callbacks.payload")?;
        let mut tx = self.pool.begin().await.map_sql_err()?;

        let existing_callback = sqlx::query(
            r#"
SELECT id, payment_order_id, status, order_no, gateway_order_id
FROM payment_callbacks
WHERE callback_key = ?
LIMIT 1
"#,
        )
        .bind(&input.callback_key)
        .fetch_optional(&mut *tx)
        .await
        .map_sql_err()?;
        let duplicate = existing_callback.is_some();
        let callback_id = if let Some(row) = existing_callback.as_ref() {
            let status: String = get(row, "status")?;
            if status == "processed" {
                let order_id: Option<String> = get(row, "payment_order_id")?;
                tx.commit().await.map_sql_err()?;
                return Ok(ProcessPaymentCallbackOutcome::DuplicateProcessed { order_id });
            }
            get(row, "id")?
        } else {
            let callback_id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                r#"
INSERT INTO payment_callbacks (
  id, payment_order_id, payment_method, callback_key, order_no, gateway_order_id,
  payload_hash, signature_valid, status, payload, error_message, created_at, processed_at
)
VALUES (?, NULL, ?, ?, ?, ?, ?, ?, 'received', ?, NULL, ?, NULL)
"#,
            )
            .bind(&callback_id)
            .bind(&input.payment_method)
            .bind(&input.callback_key)
            .bind(input.order_no.as_deref())
            .bind(input.gateway_order_id.as_deref())
            .bind(&input.payload_hash)
            .bind(sqlite_bool(input.signature_valid))
            .bind(&payload)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
            callback_id
        };

        if !input.signature_valid {
            update_sqlite_payment_callback_failure(
                &mut tx,
                &callback_id,
                &input,
                &payload,
                "invalid callback signature",
            )
            .await?;
            tx.commit().await.map_sql_err()?;
            return Ok(ProcessPaymentCallbackOutcome::Failed {
                duplicate,
                error: "invalid callback signature".to_string(),
            });
        }

        let lookup_order_no = input.order_no.clone().or_else(|| {
            existing_callback
                .as_ref()
                .and_then(|row| get(row, "order_no").ok())
        });
        let lookup_gateway_order_id = input.gateway_order_id.clone().or_else(|| {
            existing_callback
                .as_ref()
                .and_then(|row| get(row, "gateway_order_id").ok())
        });
        let order_row = if let Some(order_no) = lookup_order_no.as_deref() {
            sqlite_payment_order_by_order_no(&mut tx, order_no).await?
        } else if let Some(gateway_order_id) = lookup_gateway_order_id.as_deref() {
            sqlite_payment_order_by_gateway_order_id(&mut tx, gateway_order_id).await?
        } else {
            None
        };
        let Some(order_row) = order_row else {
            update_sqlite_payment_callback_failure(
                &mut tx,
                &callback_id,
                &input,
                &payload,
                "payment order not found",
            )
            .await?;
            tx.commit().await.map_sql_err()?;
            return Ok(ProcessPaymentCallbackOutcome::Failed {
                duplicate,
                error: "payment order not found".to_string(),
            });
        };

        let order_id: String = get(&order_row, "id")?;
        let order_no: String = get(&order_row, "order_no")?;
        let order_wallet_id: String = get(&order_row, "wallet_id")?;
        let order_payment_method: String = get(&order_row, "payment_method")?;
        let order_payment_provider: Option<String> = get(&order_row, "payment_provider")?;
        let order_payment_channel: Option<String> = get(&order_row, "payment_channel")?;
        let order_kind: String = get(&order_row, "order_kind")?;
        let order_amount_usd = sqlite_real(&order_row, "amount_usd")?;
        let order_pay_amount = sqlite_optional_real(&order_row, "pay_amount")?;
        let order_status: String = get(&order_row, "status")?;
        let expires_at_unix_secs: Option<i64> = get(&order_row, "expires_at_unix_secs")?;

        let amount_matches = if let (Some(callback_pay_amount), Some(order_pay_amount)) =
            (input.pay_amount, order_pay_amount)
        {
            (callback_pay_amount - order_pay_amount).abs() <= 0.01
        } else {
            (input.amount_usd - order_amount_usd).abs() <= f64::EPSILON
        };
        if !amount_matches {
            update_sqlite_payment_callback_failure(
                &mut tx,
                &callback_id,
                &input,
                &payload,
                "callback amount mismatch",
            )
            .await?;
            tx.commit().await.map_sql_err()?;
            return Ok(ProcessPaymentCallbackOutcome::Failed {
                duplicate,
                error: "callback amount mismatch".to_string(),
            });
        }
        if !order_payment_method.eq_ignore_ascii_case(&input.payment_method) {
            update_sqlite_payment_callback_failure(
                &mut tx,
                &callback_id,
                &input,
                &payload,
                "payment method mismatch",
            )
            .await?;
            tx.commit().await.map_sql_err()?;
            return Ok(ProcessPaymentCallbackOutcome::Failed {
                duplicate,
                error: "payment method mismatch".to_string(),
            });
        }
        if let Some(expected_provider) = input.payment_provider.as_deref() {
            if order_payment_provider
                .as_deref()
                .is_some_and(|value| !value.eq_ignore_ascii_case(expected_provider))
            {
                update_sqlite_payment_callback_failure(
                    &mut tx,
                    &callback_id,
                    &input,
                    &payload,
                    "payment provider mismatch",
                )
                .await?;
                tx.commit().await.map_sql_err()?;
                return Ok(ProcessPaymentCallbackOutcome::Failed {
                    duplicate,
                    error: "payment provider mismatch".to_string(),
                });
            }
        }
        if let Some(expected_channel) = input.payment_channel.as_deref() {
            if order_payment_channel
                .as_deref()
                .is_some_and(|value| !value.eq_ignore_ascii_case(expected_channel))
            {
                update_sqlite_payment_callback_failure(
                    &mut tx,
                    &callback_id,
                    &input,
                    &payload,
                    "payment channel mismatch",
                )
                .await?;
                tx.commit().await.map_sql_err()?;
                return Ok(ProcessPaymentCallbackOutcome::Failed {
                    duplicate,
                    error: "payment channel mismatch".to_string(),
                });
            }
        }
        if order_status == "credited" {
            mark_sqlite_payment_callback_processed(
                &mut tx,
                &callback_id,
                &input,
                &payload,
                &order_id,
                &order_no,
            )
            .await?;
            tx.commit().await.map_sql_err()?;
            return Ok(ProcessPaymentCallbackOutcome::AlreadyCredited {
                duplicate,
                order_id,
                order_no,
                wallet_id: order_wallet_id,
            });
        }
        if matches!(order_status.as_str(), "failed" | "expired" | "refunded") {
            let error = format!("payment order is not creditable: {order_status}");
            update_sqlite_payment_callback_failure(&mut tx, &callback_id, &input, &payload, &error)
                .await?;
            tx.commit().await.map_sql_err()?;
            return Ok(ProcessPaymentCallbackOutcome::Failed { duplicate, error });
        }
        if order_status == "pending" && expires_at_unix_secs.is_some_and(|value| value < now) {
            sqlx::query("UPDATE payment_orders SET status = 'expired' WHERE id = ?")
                .bind(&order_id)
                .execute(&mut *tx)
                .await
                .map_sql_err()?;
            update_sqlite_payment_callback_failure(
                &mut tx,
                &callback_id,
                &input,
                &payload,
                "payment order expired",
            )
            .await?;
            tx.commit().await.map_sql_err()?;
            return Ok(ProcessPaymentCallbackOutcome::Failed {
                duplicate,
                error: "payment order expired".to_string(),
            });
        }

        if order_kind == "plan_purchase" {
            let order_user_id: Option<String> = get(&order_row, "user_id")?;
            let Some(user_id) = order_user_id else {
                update_sqlite_payment_callback_failure(
                    &mut tx,
                    &callback_id,
                    &input,
                    &payload,
                    "payment order user missing",
                )
                .await?;
                tx.commit().await.map_sql_err()?;
                return Ok(ProcessPaymentCallbackOutcome::Failed {
                    duplicate,
                    error: "payment order user missing".to_string(),
                });
            };
            let product_id: Option<String> = get(&order_row, "product_id")?;
            let snapshot = optional_json(
                get::<Option<String>>(&order_row, "product_snapshot")?,
                "payment_orders.product_snapshot",
            )?
            .unwrap_or_else(|| serde_json::json!({}));
            let plan_id = product_id.unwrap_or_else(|| {
                snapshot
                    .get("id")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown")
                    .to_string()
            });
            let entitlements = plan_entitlements_snapshot(&snapshot);
            let existing_entitlement_id = sqlx::query_scalar::<_, String>(
                "SELECT id FROM user_plan_entitlements WHERE payment_order_id = ? LIMIT 1",
            )
            .bind(&order_id)
            .fetch_optional(&mut *tx)
            .await
            .map_sql_err()?;
            if existing_entitlement_id.is_none() {
                let purchase_limit_scope = plan_purchase_limit_scope(&snapshot);
                if purchase_limit_scope != "unlimited" {
                    let max_active_per_user = plan_max_active_per_user(&snapshot);
                    let active_count = if purchase_limit_scope == "lifetime" {
                        sqlx::query_scalar::<_, i64>(
                            r#"
SELECT COUNT(*)
FROM user_plan_entitlements
WHERE user_id = ?
  AND plan_id = ?
  AND status = 'active'
                    "#,
                        )
                        .bind(&user_id)
                        .bind(&plan_id)
                        .fetch_one(&mut *tx)
                        .await
                        .map_sql_err()?
                    } else {
                        sqlx::query_scalar::<_, i64>(
                            r#"
SELECT COUNT(*)
FROM user_plan_entitlements
WHERE user_id = ?
  AND plan_id = ?
  AND status = 'active'
  AND expires_at > ?
                    "#,
                        )
                        .bind(&user_id)
                        .bind(&plan_id)
                        .bind(now)
                        .fetch_one(&mut *tx)
                        .await
                        .map_sql_err()?
                    };
                    if active_count >= max_active_per_user {
                        update_sqlite_payment_callback_failure(
                            &mut tx,
                            &callback_id,
                            &input,
                            &payload,
                            "plan purchase limit reached",
                        )
                        .await?;
                        tx.commit().await.map_sql_err()?;
                        return Ok(ProcessPaymentCallbackOutcome::Failed {
                            duplicate,
                            error: "plan purchase limit reached".to_string(),
                        });
                    }
                }
                replace_matching_plan_entitlements_sqlite(&mut tx, &user_id, &snapshot, now)
                    .await?;
                sqlx::query(
                    r#"
INSERT INTO user_plan_entitlements (
  id, user_id, plan_id, payment_order_id, status, starts_at, expires_at,
  entitlements_snapshot, created_at, updated_at
)
VALUES (?, ?, ?, ?, 'active', ?, ?, ?, ?, ?)
                    "#,
                )
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(&user_id)
                .bind(&plan_id)
                .bind(&order_id)
                .bind(now)
                .bind(plan_expires_at_unix(&snapshot, now))
                .bind(json_string(
                    &entitlements,
                    "user_plan_entitlements.entitlements_snapshot",
                )?)
                .bind(now)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_sql_err()?;
                apply_plan_wallet_credit_sqlite(
                    &mut tx,
                    &order_wallet_id,
                    &order_id,
                    &input.payment_method,
                    &entitlements,
                    now,
                )
                .await?;
            }
            sqlx::query(
                r#"
UPDATE payment_orders
SET gateway_order_id = COALESCE(?, gateway_order_id),
    gateway_response = ?,
    pay_amount = COALESCE(?, pay_amount),
    pay_currency = COALESCE(?, pay_currency),
    exchange_rate = COALESCE(?, exchange_rate),
    status = 'credited',
    fulfillment_status = 'fulfilled',
    fulfillment_error = NULL,
    paid_at = COALESCE(paid_at, ?),
    credited_at = ?,
    refundable_amount_usd = 0
WHERE id = ?
                "#,
            )
            .bind(input.gateway_order_id.as_deref())
            .bind(&payload)
            .bind(input.pay_amount)
            .bind(input.pay_currency.as_deref())
            .bind(input.exchange_rate)
            .bind(now)
            .bind(now)
            .bind(&order_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
            let updated_order_row = sqlite_payment_order_by_id(&mut tx, &order_id).await?;
            mark_sqlite_payment_callback_processed(
                &mut tx,
                &callback_id,
                &input,
                &payload,
                &order_id,
                &order_no,
            )
            .await?;
            tx.commit().await.map_sql_err()?;
            return Ok(ProcessPaymentCallbackOutcome::Applied {
                duplicate,
                order_id,
                order_no,
                wallet_id: order_wallet_id,
                order: map_payment_order_row(&updated_order_row)?,
            });
        }

        let Some(wallet_row) = sqlx::query(
            r#"
SELECT id, status, balance, gift_balance
FROM wallets
WHERE id = ?
LIMIT 1
"#,
        )
        .bind(&order_wallet_id)
        .fetch_optional(&mut *tx)
        .await
        .map_sql_err()?
        else {
            update_sqlite_payment_callback_failure(
                &mut tx,
                &callback_id,
                &input,
                &payload,
                "wallet not found",
            )
            .await?;
            tx.commit().await.map_sql_err()?;
            return Ok(ProcessPaymentCallbackOutcome::Failed {
                duplicate,
                error: "wallet not found".to_string(),
            });
        };
        let wallet_status: String = get(&wallet_row, "status")?;
        if wallet_status != "active" {
            update_sqlite_payment_callback_failure(
                &mut tx,
                &callback_id,
                &input,
                &payload,
                "wallet is not active",
            )
            .await?;
            tx.commit().await.map_sql_err()?;
            return Ok(ProcessPaymentCallbackOutcome::Failed {
                duplicate,
                error: "wallet is not active".to_string(),
            });
        }

        let before_recharge = sqlite_real(&wallet_row, "balance")?;
        let before_gift = sqlite_real(&wallet_row, "gift_balance")?;
        let before_total = before_recharge + before_gift;
        let after_recharge = before_recharge + order_amount_usd;
        let after_total = after_recharge + before_gift;

        sqlx::query(
            r#"
UPDATE wallets
SET balance = ?,
    total_recharged = total_recharged + ?,
    updated_at = ?
WHERE id = ?
"#,
        )
        .bind(after_recharge)
        .bind(order_amount_usd)
        .bind(now)
        .bind(&order_wallet_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        sqlx::query(
            r#"
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount, balance_before, balance_after,
  recharge_balance_before, recharge_balance_after, gift_balance_before,
  gift_balance_after, link_type, link_id, operator_id, description, created_at
)
VALUES (?, ?, 'recharge', 'topup_gateway', ?, ?, ?, ?, ?, ?, ?, 'payment_order', ?, NULL, ?, ?)
"#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&order_wallet_id)
        .bind(order_amount_usd)
        .bind(before_total)
        .bind(after_total)
        .bind(before_recharge)
        .bind(after_recharge)
        .bind(before_gift)
        .bind(before_gift)
        .bind(&order_id)
        .bind(format!("充值到账({})", input.payment_method))
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        sqlx::query(
            r#"
UPDATE payment_orders
SET gateway_order_id = COALESCE(?, gateway_order_id),
    gateway_response = ?,
    pay_amount = COALESCE(?, pay_amount),
    pay_currency = COALESCE(?, pay_currency),
    exchange_rate = COALESCE(?, exchange_rate),
    status = 'credited',
    paid_at = COALESCE(paid_at, ?),
    credited_at = ?,
    refundable_amount_usd = amount_usd
WHERE id = ?
"#,
        )
        .bind(input.gateway_order_id.as_deref())
        .bind(&payload)
        .bind(input.pay_amount)
        .bind(input.pay_currency.as_deref())
        .bind(input.exchange_rate)
        .bind(now)
        .bind(now)
        .bind(&order_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        let updated_order_row = sqlite_payment_order_by_id(&mut tx, &order_id).await?;
        mark_sqlite_payment_callback_processed(
            &mut tx,
            &callback_id,
            &input,
            &payload,
            &order_id,
            &order_no,
        )
        .await?;
        tx.commit().await.map_sql_err()?;
        Ok(ProcessPaymentCallbackOutcome::Applied {
            duplicate,
            order_id,
            order_no,
            wallet_id: order_wallet_id,
            order: map_payment_order_row(&updated_order_row)?,
        })
    }

    async fn adjust_wallet_balance(
        &self,
        input: AdjustWalletBalanceInput,
    ) -> Result<Option<(StoredWalletSnapshot, StoredAdminWalletTransaction)>, DataLayerError> {
        let now = current_unix_secs_i64();
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let Some(row) = sqlite_wallet_by_id_optional(&mut tx, &input.wallet_id).await? else {
            tx.commit().await.map_sql_err()?;
            return Ok(None);
        };

        let before_recharge = sqlite_real(&row, "balance")?;
        let before_gift = sqlite_real(&row, "gift_balance")?;
        let before_total = before_recharge + before_gift;
        let mut after_recharge = before_recharge;
        let mut after_gift = before_gift;
        apply_admin_balance_adjustment(
            input.amount_usd,
            &input.balance_type,
            &mut after_recharge,
            &mut after_gift,
        );

        sqlx::query(
            r#"
UPDATE wallets
SET balance = ?,
    gift_balance = ?,
    total_adjusted = total_adjusted + ?,
    updated_at = ?
WHERE id = ?
"#,
        )
        .bind(after_recharge)
        .bind(after_gift)
        .bind(input.amount_usd)
        .bind(now)
        .bind(&input.wallet_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        let wallet = map_wallet_row(&sqlite_wallet_by_id(&mut tx, &input.wallet_id).await?)?;

        let transaction_id = uuid::Uuid::new_v4().to_string();
        let description = input
            .description
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("管理员调账")
            .to_string();
        sqlx::query(
            r#"
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount, balance_before, balance_after,
  recharge_balance_before, recharge_balance_after, gift_balance_before,
  gift_balance_after, link_type, link_id, operator_id, description, created_at
)
VALUES (?, ?, 'adjust', 'adjust_admin', ?, ?, ?, ?, ?, ?, ?, 'admin_action', ?, ?, ?, ?)
"#,
        )
        .bind(&transaction_id)
        .bind(&input.wallet_id)
        .bind(input.amount_usd)
        .bind(before_total)
        .bind(after_recharge + after_gift)
        .bind(before_recharge)
        .bind(after_recharge)
        .bind(before_gift)
        .bind(after_gift)
        .bind(&input.wallet_id)
        .bind(input.operator_id.as_deref())
        .bind(&description)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        tx.commit().await.map_sql_err()?;

        Ok(Some((
            wallet,
            StoredAdminWalletTransaction {
                id: transaction_id,
                wallet_id: input.wallet_id.clone(),
                category: "adjust".to_string(),
                reason_code: "adjust_admin".to_string(),
                amount: input.amount_usd,
                balance_before: before_total,
                balance_after: after_recharge + after_gift,
                recharge_balance_before: before_recharge,
                recharge_balance_after: after_recharge,
                gift_balance_before: before_gift,
                gift_balance_after: after_gift,
                link_type: Some("admin_action".to_string()),
                link_id: Some(input.wallet_id),
                operator_id: input.operator_id,
                operator_name: None,
                operator_email: None,
                description: Some(description),
                created_at_unix_ms: Some(timestamp(now, "wallet_transactions.created_at")?),
            },
        )))
    }

    async fn create_manual_wallet_recharge(
        &self,
        input: CreateManualWalletRechargeInput,
    ) -> Result<Option<(StoredWalletSnapshot, StoredAdminPaymentOrder)>, DataLayerError> {
        let now = current_unix_secs_i64();
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let Some(wallet_row) = sqlite_wallet_by_id_optional(&mut tx, &input.wallet_id).await?
        else {
            tx.commit().await.map_sql_err()?;
            return Ok(None);
        };

        let before_recharge = sqlite_real(&wallet_row, "balance")?;
        let before_gift = sqlite_real(&wallet_row, "gift_balance")?;
        let user_id: Option<String> = get(&wallet_row, "user_id")?;
        let order_id = uuid::Uuid::new_v4().to_string();
        let gateway_response = json_string(
            &serde_json::json!({
                "source": "manual",
                "operator_id": input.operator_id,
                "description": input.description,
            }),
            "payment_orders.gateway_response",
        )?;

        sqlx::query(
            r#"
INSERT INTO payment_orders (
  id, order_no, wallet_id, user_id, amount_usd, refunded_amount_usd,
  refundable_amount_usd, payment_method, status, gateway_response,
  created_at, paid_at, credited_at
)
VALUES (?, ?, ?, ?, ?, 0, ?, ?, 'credited', ?, ?, ?, ?)
"#,
        )
        .bind(&order_id)
        .bind(&input.order_no)
        .bind(&input.wallet_id)
        .bind(user_id.as_deref())
        .bind(input.amount_usd)
        .bind(input.amount_usd)
        .bind(&input.payment_method)
        .bind(&gateway_response)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        let after_recharge = before_recharge + input.amount_usd;
        sqlx::query(
            r#"
UPDATE wallets
SET balance = ?,
    total_recharged = total_recharged + ?,
    updated_at = ?
WHERE id = ?
"#,
        )
        .bind(after_recharge)
        .bind(input.amount_usd)
        .bind(now)
        .bind(&input.wallet_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        let reason_code = if matches!(
            input.payment_method.as_str(),
            "card_code" | "gift_code" | "card_recharge"
        ) {
            "topup_card_code"
        } else {
            "topup_admin_manual"
        };
        sqlx::query(
            r#"
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount, balance_before, balance_after,
  recharge_balance_before, recharge_balance_after, gift_balance_before,
  gift_balance_after, link_type, link_id, operator_id, description, created_at
)
VALUES (?, ?, 'recharge', ?, ?, ?, ?, ?, ?, ?, ?, 'payment_order', ?, ?, ?, ?)
"#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&input.wallet_id)
        .bind(reason_code)
        .bind(input.amount_usd)
        .bind(before_recharge + before_gift)
        .bind(after_recharge + before_gift)
        .bind(before_recharge)
        .bind(after_recharge)
        .bind(before_gift)
        .bind(before_gift)
        .bind(&order_id)
        .bind(input.operator_id.as_deref())
        .bind(
            input
                .description
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("管理员手动充值"),
        )
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        let wallet = map_wallet_row(&sqlite_wallet_by_id(&mut tx, &input.wallet_id).await?)?;
        let order = map_payment_order_row(&sqlite_payment_order_by_id(&mut tx, &order_id).await?)?;
        tx.commit().await.map_sql_err()?;
        Ok(Some((wallet, order)))
    }

    async fn process_admin_wallet_refund(
        &self,
        input: ProcessAdminWalletRefundInput,
    ) -> Result<
        WalletMutationOutcome<(
            StoredWalletSnapshot,
            StoredAdminWalletRefund,
            StoredAdminWalletTransaction,
        )>,
        DataLayerError,
    > {
        let now = current_unix_secs_i64();
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let Some(refund_row) =
            sqlite_refund_by_id_and_wallet(&mut tx, &input.refund_id, &input.wallet_id).await?
        else {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::NotFound);
        };
        let refund = map_refund_row(&refund_row)?;
        if !matches!(refund.status.as_str(), "approved" | "pending_approval") {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(
                "refund status is not approvable".to_string(),
            ));
        }

        let Some(wallet_row) = sqlite_wallet_by_id_optional(&mut tx, &input.wallet_id).await?
        else {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(
                "wallet not found".to_string(),
            ));
        };
        let before_recharge = sqlite_real(&wallet_row, "balance")?;
        let before_gift = sqlite_real(&wallet_row, "gift_balance")?;
        let before_total = before_recharge + before_gift;
        let amount_usd = refund.amount_usd;
        let after_recharge = before_recharge - amount_usd;
        if after_recharge < 0.0 {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(
                "refund amount exceeds refundable recharge balance".to_string(),
            ));
        }

        if let Some(payment_order_id) = refund.payment_order_id.as_deref() {
            let Some(order_row) =
                sqlite_payment_order_by_id_optional(&mut tx, payment_order_id).await?
            else {
                tx.commit().await.map_sql_err()?;
                return Ok(WalletMutationOutcome::Invalid(
                    "payment order not found".to_string(),
                ));
            };
            let refundable_amount = sqlite_real(&order_row, "refundable_amount_usd")?;
            if amount_usd > refundable_amount {
                tx.commit().await.map_sql_err()?;
                return Ok(WalletMutationOutcome::Invalid(
                    "refund amount exceeds refundable amount".to_string(),
                ));
            }
            sqlx::query(
                r#"
UPDATE payment_orders
SET refunded_amount_usd = refunded_amount_usd + ?,
    refundable_amount_usd = refundable_amount_usd - ?
WHERE id = ?
"#,
            )
            .bind(amount_usd)
            .bind(amount_usd)
            .bind(payment_order_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        }

        sqlx::query(
            r#"
UPDATE wallets
SET balance = ?,
    total_refunded = total_refunded + ?,
    updated_at = ?
WHERE id = ?
"#,
        )
        .bind(after_recharge)
        .bind(amount_usd)
        .bind(now)
        .bind(&input.wallet_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        let wallet = map_wallet_row(&sqlite_wallet_by_id(&mut tx, &input.wallet_id).await?)?;

        let transaction_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            r#"
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount, balance_before, balance_after,
  recharge_balance_before, recharge_balance_after, gift_balance_before,
  gift_balance_after, link_type, link_id, operator_id, description, created_at
)
VALUES (?, ?, 'refund', 'refund_out', ?, ?, ?, ?, ?, ?, ?, 'refund_request', ?, ?, '退款占款', ?)
"#,
        )
        .bind(&transaction_id)
        .bind(&input.wallet_id)
        .bind(-amount_usd)
        .bind(before_total)
        .bind(after_recharge + before_gift)
        .bind(before_recharge)
        .bind(after_recharge)
        .bind(before_gift)
        .bind(before_gift)
        .bind(&input.refund_id)
        .bind(input.operator_id.as_deref())
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        sqlx::query(
            r#"
UPDATE refund_requests
SET status = 'processing',
    approved_by = ?,
    processed_by = ?,
    processed_at = ?,
    updated_at = ?
WHERE id = ? AND wallet_id = ?
"#,
        )
        .bind(input.operator_id.as_deref())
        .bind(input.operator_id.as_deref())
        .bind(now)
        .bind(now)
        .bind(&input.refund_id)
        .bind(&input.wallet_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        let refund = map_refund_row(&sqlite_refund_by_id(&mut tx, &input.refund_id).await?)?;
        tx.commit().await.map_sql_err()?;
        Ok(WalletMutationOutcome::Applied((
            wallet,
            refund,
            StoredAdminWalletTransaction {
                id: transaction_id,
                wallet_id: input.wallet_id.clone(),
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
                link_id: Some(input.refund_id.clone()),
                operator_id: input.operator_id.clone(),
                operator_name: None,
                operator_email: None,
                description: Some("退款占款".to_string()),
                created_at_unix_ms: Some(timestamp(now, "wallet_transactions.created_at")?),
            },
        )))
    }

    async fn complete_admin_wallet_refund(
        &self,
        input: CompleteAdminWalletRefundInput,
    ) -> Result<WalletMutationOutcome<StoredAdminWalletRefund>, DataLayerError> {
        let now = current_unix_secs_i64();
        let payout_proof = input
            .payout_proof
            .as_ref()
            .map(|value| json_string(value, "refund_requests.payout_proof"))
            .transpose()?;
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let Some(current_refund) =
            sqlite_refund_by_id_and_wallet(&mut tx, &input.refund_id, &input.wallet_id).await?
        else {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::NotFound);
        };
        let status: String = get(&current_refund, "status")?;
        if status != "processing" {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(
                "refund status must be processing before completion".to_string(),
            ));
        }

        sqlx::query(
            r#"
UPDATE refund_requests
SET status = 'succeeded',
    gateway_refund_id = ?,
    payout_reference = ?,
    payout_proof = ?,
    completed_at = ?,
    updated_at = ?
WHERE id = ? AND wallet_id = ?
"#,
        )
        .bind(input.gateway_refund_id.as_deref())
        .bind(input.payout_reference.as_deref())
        .bind(payout_proof.as_deref())
        .bind(now)
        .bind(now)
        .bind(&input.refund_id)
        .bind(&input.wallet_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        let refund = map_refund_row(&sqlite_refund_by_id(&mut tx, &input.refund_id).await?)?;
        tx.commit().await.map_sql_err()?;
        Ok(WalletMutationOutcome::Applied(refund))
    }

    async fn fail_admin_wallet_refund(
        &self,
        input: FailAdminWalletRefundInput,
    ) -> Result<
        WalletMutationOutcome<(
            StoredWalletSnapshot,
            StoredAdminWalletRefund,
            Option<StoredAdminWalletTransaction>,
        )>,
        DataLayerError,
    > {
        let now = current_unix_secs_i64();
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let Some(refund_row) =
            sqlite_refund_by_id_and_wallet(&mut tx, &input.refund_id, &input.wallet_id).await?
        else {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::NotFound);
        };
        let refund = map_refund_row(&refund_row)?;

        if matches!(refund.status.as_str(), "pending_approval" | "approved") {
            sqlx::query(
                r#"
UPDATE refund_requests
SET status = 'failed',
    failure_reason = ?,
    updated_at = ?
WHERE id = ? AND wallet_id = ?
"#,
            )
            .bind(&input.reason)
            .bind(now)
            .bind(&input.refund_id)
            .bind(&input.wallet_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
            let wallet = map_wallet_row(&sqlite_wallet_by_id(&mut tx, &input.wallet_id).await?)?;
            let refund = map_refund_row(&sqlite_refund_by_id(&mut tx, &input.refund_id).await?)?;
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Applied((wallet, refund, None)));
        }

        if refund.status != "processing" {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(format!(
                "cannot fail refund in status: {}",
                refund.status
            )));
        }

        let Some(wallet_row) = sqlite_wallet_by_id_optional(&mut tx, &input.wallet_id).await?
        else {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(
                "wallet not found".to_string(),
            ));
        };
        let amount_usd = refund.amount_usd;
        let before_recharge = sqlite_real(&wallet_row, "balance")?;
        let before_gift = sqlite_real(&wallet_row, "gift_balance")?;
        let before_total = before_recharge + before_gift;
        let after_recharge = before_recharge + amount_usd;

        sqlx::query(
            r#"
UPDATE wallets
SET balance = ?,
    total_refunded = MAX(total_refunded - ?, 0),
    updated_at = ?
WHERE id = ?
"#,
        )
        .bind(after_recharge)
        .bind(amount_usd)
        .bind(now)
        .bind(&input.wallet_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        let wallet = map_wallet_row(&sqlite_wallet_by_id(&mut tx, &input.wallet_id).await?)?;

        let transaction_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            r#"
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount, balance_before, balance_after,
  recharge_balance_before, recharge_balance_after, gift_balance_before,
  gift_balance_after, link_type, link_id, operator_id, description, created_at
)
VALUES (?, ?, 'refund', 'refund_revert', ?, ?, ?, ?, ?, ?, ?, 'refund_request', ?, ?, '退款失败回补', ?)
"#,
        )
        .bind(&transaction_id)
        .bind(&input.wallet_id)
        .bind(amount_usd)
        .bind(before_total)
        .bind(after_recharge + before_gift)
        .bind(before_recharge)
        .bind(after_recharge)
        .bind(before_gift)
        .bind(before_gift)
        .bind(&input.refund_id)
        .bind(input.operator_id.as_deref())
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        if let Some(payment_order_id) = refund.payment_order_id.as_deref() {
            sqlx::query(
                r#"
UPDATE payment_orders
SET refunded_amount_usd = refunded_amount_usd - ?,
    refundable_amount_usd = refundable_amount_usd + ?
WHERE id = ?
"#,
            )
            .bind(amount_usd)
            .bind(amount_usd)
            .bind(payment_order_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        }

        sqlx::query(
            r#"
UPDATE refund_requests
SET status = 'failed',
    failure_reason = ?,
    updated_at = ?
WHERE id = ? AND wallet_id = ?
"#,
        )
        .bind(&input.reason)
        .bind(now)
        .bind(&input.refund_id)
        .bind(&input.wallet_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        let refund = map_refund_row(&sqlite_refund_by_id(&mut tx, &input.refund_id).await?)?;
        tx.commit().await.map_sql_err()?;
        Ok(WalletMutationOutcome::Applied((
            wallet,
            refund,
            Some(StoredAdminWalletTransaction {
                id: transaction_id,
                wallet_id: input.wallet_id.clone(),
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
                link_id: Some(input.refund_id.clone()),
                operator_id: input.operator_id.clone(),
                operator_name: None,
                operator_email: None,
                description: Some("退款失败回补".to_string()),
                created_at_unix_ms: Some(timestamp(now, "wallet_transactions.created_at")?),
            }),
        )))
    }

    async fn expire_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<WalletMutationOutcome<(StoredAdminPaymentOrder, bool)>, DataLayerError> {
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let Some(row) = sqlite_payment_order_by_id_optional(&mut tx, order_id).await? else {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::NotFound);
        };
        let order = map_payment_order_row(&row)?;
        if order.status == "credited" {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(
                "credited order cannot be expired".to_string(),
            ));
        }
        if order.status == "expired" {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Applied((order, false)));
        }
        if order.status != "pending" {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(format!(
                "only pending order can be expired: {}",
                order.status
            )));
        }
        let mut gateway_response = payment_gateway_response_map(order.gateway_response.clone());
        gateway_response.insert(
            "expire_reason".to_string(),
            serde_json::Value::String("admin_mark_expired".to_string()),
        );
        gateway_response.insert(
            "expired_at".to_string(),
            serde_json::Value::String(Utc::now().to_rfc3339()),
        );
        let gateway_response = json_string(
            &serde_json::Value::Object(gateway_response),
            "payment_orders.gateway_response",
        )?;
        sqlx::query(
            "UPDATE payment_orders SET status = 'expired', gateway_response = ? WHERE id = ?",
        )
        .bind(gateway_response)
        .bind(order_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        let updated = map_payment_order_row(&sqlite_payment_order_by_id(&mut tx, order_id).await?)?;
        tx.commit().await.map_sql_err()?;
        Ok(WalletMutationOutcome::Applied((updated, true)))
    }

    async fn fail_admin_payment_order(
        &self,
        order_id: &str,
    ) -> Result<WalletMutationOutcome<StoredAdminPaymentOrder>, DataLayerError> {
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let Some(row) = sqlite_payment_order_by_id_optional(&mut tx, order_id).await? else {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::NotFound);
        };
        let order = map_payment_order_row(&row)?;
        if order.status == "credited" {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(
                "credited order cannot be failed".to_string(),
            ));
        }
        let mut gateway_response = payment_gateway_response_map(order.gateway_response.clone());
        gateway_response.insert(
            "failure_reason".to_string(),
            serde_json::Value::String("admin_mark_failed".to_string()),
        );
        gateway_response.insert(
            "failed_at".to_string(),
            serde_json::Value::String(Utc::now().to_rfc3339()),
        );
        let gateway_response = json_string(
            &serde_json::Value::Object(gateway_response),
            "payment_orders.gateway_response",
        )?;
        sqlx::query(
            "UPDATE payment_orders SET status = 'failed', gateway_response = ? WHERE id = ?",
        )
        .bind(gateway_response)
        .bind(order_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        let updated = map_payment_order_row(&sqlite_payment_order_by_id(&mut tx, order_id).await?)?;
        tx.commit().await.map_sql_err()?;
        Ok(WalletMutationOutcome::Applied(updated))
    }

    async fn credit_admin_payment_order(
        &self,
        input: CreditAdminPaymentOrderInput,
    ) -> Result<WalletMutationOutcome<(StoredAdminPaymentOrder, bool)>, DataLayerError> {
        let now = current_unix_secs_i64();
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let Some(order_row) = sqlite_payment_order_by_id_optional(&mut tx, &input.order_id).await?
        else {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::NotFound);
        };
        let order = map_payment_order_row(&order_row)?;
        if order.status == "credited" {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Applied((order, false)));
        }
        if matches!(order.status.as_str(), "failed" | "expired" | "refunded") {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(format!(
                "payment order is not creditable: {}",
                order.status
            )));
        }
        if order
            .expires_at_unix_secs
            .is_some_and(|value| value < now as u64)
        {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(
                "payment order expired".to_string(),
            ));
        }

        let order_kind: String = get(&order_row, "order_kind")?;
        if order_kind == "plan_purchase" {
            let order_user_id: Option<String> = get(&order_row, "user_id")?;
            let Some(user_id) = order_user_id else {
                tx.commit().await.map_sql_err()?;
                return Ok(WalletMutationOutcome::Invalid(
                    "payment order user missing".to_string(),
                ));
            };
            let product_id: Option<String> = get(&order_row, "product_id")?;
            let snapshot = optional_json(
                get::<Option<String>>(&order_row, "product_snapshot")?,
                "payment_orders.product_snapshot",
            )?
            .unwrap_or_else(|| serde_json::json!({}));
            let plan_id = product_id.unwrap_or_else(|| {
                snapshot
                    .get("id")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown")
                    .to_string()
            });
            let entitlements = plan_entitlements_snapshot(&snapshot);
            let existing_entitlement_id = sqlx::query_scalar::<_, String>(
                "SELECT id FROM user_plan_entitlements WHERE payment_order_id = ? LIMIT 1",
            )
            .bind(&input.order_id)
            .fetch_optional(&mut *tx)
            .await
            .map_sql_err()?;
            if existing_entitlement_id.is_none() {
                let purchase_limit_scope = plan_purchase_limit_scope(&snapshot);
                if purchase_limit_scope != "unlimited" {
                    let max_active_per_user = plan_max_active_per_user(&snapshot);
                    let active_count = if purchase_limit_scope == "lifetime" {
                        sqlx::query_scalar::<_, i64>(
                            r#"
SELECT COUNT(*)
FROM user_plan_entitlements
WHERE user_id = ?
  AND plan_id = ?
  AND status = 'active'
                    "#,
                        )
                        .bind(&user_id)
                        .bind(&plan_id)
                        .fetch_one(&mut *tx)
                        .await
                        .map_sql_err()?
                    } else {
                        sqlx::query_scalar::<_, i64>(
                            r#"
SELECT COUNT(*)
FROM user_plan_entitlements
WHERE user_id = ?
  AND plan_id = ?
  AND status = 'active'
  AND expires_at > ?
                    "#,
                        )
                        .bind(&user_id)
                        .bind(&plan_id)
                        .bind(now)
                        .fetch_one(&mut *tx)
                        .await
                        .map_sql_err()?
                    };
                    if active_count >= max_active_per_user {
                        tx.commit().await.map_sql_err()?;
                        return Ok(WalletMutationOutcome::Invalid(
                            "plan purchase limit reached".to_string(),
                        ));
                    }
                }
                replace_matching_plan_entitlements_sqlite(&mut tx, &user_id, &snapshot, now)
                    .await?;
                sqlx::query(
                    r#"
INSERT INTO user_plan_entitlements (
  id, user_id, plan_id, payment_order_id, status, starts_at, expires_at,
  entitlements_snapshot, created_at, updated_at
)
VALUES (?, ?, ?, ?, 'active', ?, ?, ?, ?, ?)
                    "#,
                )
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(&user_id)
                .bind(&plan_id)
                .bind(&input.order_id)
                .bind(now)
                .bind(plan_expires_at_unix(&snapshot, now))
                .bind(json_string(
                    &entitlements,
                    "user_plan_entitlements.entitlements_snapshot",
                )?)
                .bind(now)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_sql_err()?;
                apply_plan_wallet_credit_sqlite(
                    &mut tx,
                    &order.wallet_id,
                    &input.order_id,
                    &order.payment_method,
                    &entitlements,
                    now,
                )
                .await?;
            }

            let mut gateway_response = payment_gateway_response_map(order.gateway_response.clone());
            if let Some(serde_json::Value::Object(map)) = input.gateway_response_patch.clone() {
                gateway_response.extend(map);
            }
            gateway_response.insert("manual_credit".to_string(), serde_json::Value::Bool(true));
            gateway_response.insert(
                "credited_by".to_string(),
                input
                    .operator_id
                    .clone()
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null),
            );
            let gateway_response = json_string(
                &serde_json::Value::Object(gateway_response),
                "payment_orders.gateway_response",
            )?;
            let next_gateway_order_id = input.gateway_order_id.clone().or(order.gateway_order_id);
            let next_pay_amount = input.pay_amount.or(order.pay_amount);
            let next_pay_currency = input.pay_currency.clone().or(order.pay_currency);
            let next_exchange_rate = input.exchange_rate.or(order.exchange_rate);
            let next_paid_at = order.paid_at_unix_secs.unwrap_or(now as u64) as i64;

            sqlx::query(
                r#"
UPDATE payment_orders
SET gateway_order_id = ?,
    gateway_response = ?,
    pay_amount = ?,
    pay_currency = ?,
    exchange_rate = ?,
    status = 'credited',
    fulfillment_status = 'fulfilled',
    fulfillment_error = NULL,
    paid_at = ?,
    credited_at = ?,
    refundable_amount_usd = 0
WHERE id = ?
"#,
            )
            .bind(next_gateway_order_id.as_deref())
            .bind(&gateway_response)
            .bind(next_pay_amount)
            .bind(next_pay_currency.as_deref())
            .bind(next_exchange_rate)
            .bind(next_paid_at)
            .bind(now)
            .bind(&input.order_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
            let order = map_payment_order_row(
                &sqlite_payment_order_by_id(&mut tx, &input.order_id).await?,
            )?;
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Applied((order, true)));
        }

        let Some(wallet_row) = sqlite_wallet_by_id_optional(&mut tx, &order.wallet_id).await?
        else {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(
                "wallet not found".to_string(),
            ));
        };
        let wallet_status: String = get(&wallet_row, "status")?;
        if wallet_status != "active" {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(
                "wallet is not active".to_string(),
            ));
        }

        let before_recharge = sqlite_real(&wallet_row, "balance")?;
        let before_gift = sqlite_real(&wallet_row, "gift_balance")?;
        let before_total = before_recharge + before_gift;
        let after_recharge = before_recharge + order.amount_usd;
        sqlx::query(
            r#"
UPDATE wallets
SET balance = ?,
    total_recharged = total_recharged + ?,
    updated_at = ?
WHERE id = ?
"#,
        )
        .bind(after_recharge)
        .bind(order.amount_usd)
        .bind(now)
        .bind(&order.wallet_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        sqlx::query(
            r#"
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount, balance_before, balance_after,
  recharge_balance_before, recharge_balance_after, gift_balance_before,
  gift_balance_after, link_type, link_id, operator_id, description, created_at
)
VALUES (?, ?, 'recharge', 'topup_gateway', ?, ?, ?, ?, ?, ?, ?, 'payment_order', ?, NULL, ?, ?)
"#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&order.wallet_id)
        .bind(order.amount_usd)
        .bind(before_total)
        .bind(after_recharge + before_gift)
        .bind(before_recharge)
        .bind(after_recharge)
        .bind(before_gift)
        .bind(before_gift)
        .bind(&input.order_id)
        .bind(format!("充值到账({})", order.payment_method))
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        let mut gateway_response = payment_gateway_response_map(order.gateway_response.clone());
        if let Some(serde_json::Value::Object(map)) = input.gateway_response_patch {
            gateway_response.extend(map);
        }
        gateway_response.insert("manual_credit".to_string(), serde_json::Value::Bool(true));
        gateway_response.insert(
            "credited_by".to_string(),
            input
                .operator_id
                .clone()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
        );
        let gateway_response = json_string(
            &serde_json::Value::Object(gateway_response),
            "payment_orders.gateway_response",
        )?;
        let next_gateway_order_id = input.gateway_order_id.or(order.gateway_order_id);
        let next_pay_amount = input.pay_amount.or(order.pay_amount);
        let next_pay_currency = input.pay_currency.or(order.pay_currency);
        let next_exchange_rate = input.exchange_rate.or(order.exchange_rate);
        let next_paid_at = order.paid_at_unix_secs.unwrap_or(now as u64) as i64;

        sqlx::query(
            r#"
UPDATE payment_orders
SET gateway_order_id = ?,
    gateway_response = ?,
    pay_amount = ?,
    pay_currency = ?,
    exchange_rate = ?,
    status = 'credited',
    paid_at = ?,
    credited_at = ?,
    refundable_amount_usd = amount_usd
WHERE id = ?
"#,
        )
        .bind(next_gateway_order_id.as_deref())
        .bind(&gateway_response)
        .bind(next_pay_amount)
        .bind(next_pay_currency.as_deref())
        .bind(next_exchange_rate)
        .bind(next_paid_at)
        .bind(now)
        .bind(&input.order_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        let order =
            map_payment_order_row(&sqlite_payment_order_by_id(&mut tx, &input.order_id).await?)?;
        tx.commit().await.map_sql_err()?;
        Ok(WalletMutationOutcome::Applied((order, true)))
    }

    async fn create_admin_redeem_code_batch(
        &self,
        input: CreateAdminRedeemCodeBatchInput,
    ) -> Result<CreateAdminRedeemCodeBatchResult, DataLayerError> {
        let now = current_unix_secs_i64();
        let batch_id = uuid::Uuid::new_v4().to_string();
        let expires_at = input.expires_at_unix_secs.map(|value| value as i64);
        let mut tx = self.pool.begin().await.map_sql_err()?;

        sqlx::query(
            r#"
INSERT INTO redeem_code_batches (
  id, name, amount_usd, currency, balance_bucket, total_count, status,
  description, created_by, expires_at, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, 'active', ?, ?, ?, ?, ?)
"#,
        )
        .bind(&batch_id)
        .bind(&input.name)
        .bind(input.amount_usd)
        .bind(&input.currency)
        .bind(&input.balance_bucket)
        .bind(i64::try_from(input.total_count).map_err(|_| {
            DataLayerError::InvalidInput(format!(
                "invalid redeem code count: {}",
                input.total_count
            ))
        })?)
        .bind(input.description.as_deref())
        .bind(input.created_by.as_deref())
        .bind(expires_at)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        let mut codes = Vec::with_capacity(input.total_count);
        for _ in 0..input.total_count {
            let (code_id, code, masked_code, code_hash, prefix, suffix) =
                generate_redeem_code_candidate();
            sqlx::query(
                r#"
INSERT INTO redeem_codes (
  id, batch_id, code_hash, code_prefix, code_suffix, status, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, 'active', ?, ?)
"#,
            )
            .bind(&code_id)
            .bind(&batch_id)
            .bind(&code_hash)
            .bind(&prefix)
            .bind(&suffix)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
            codes.push(CreatedAdminRedeemCodePlaintext {
                code_id,
                code,
                masked_code,
            });
        }

        let batch = StoredAdminRedeemCodeBatch {
            id: batch_id,
            name: input.name,
            amount_usd: input.amount_usd,
            currency: input.currency,
            balance_bucket: input.balance_bucket,
            total_count: input.total_count as u64,
            redeemed_count: 0,
            active_count: input.total_count as u64,
            status: "active".to_string(),
            description: input.description,
            created_by: input.created_by,
            expires_at_unix_secs: input.expires_at_unix_secs,
            created_at_unix_ms: timestamp(now, "redeem_code_batches.created_at")?,
            updated_at_unix_secs: timestamp(now, "redeem_code_batches.updated_at")?,
        };
        tx.commit().await.map_sql_err()?;
        Ok(CreateAdminRedeemCodeBatchResult { batch, codes })
    }

    async fn disable_admin_redeem_code_batch(
        &self,
        input: DisableAdminRedeemCodeBatchInput,
    ) -> Result<WalletMutationOutcome<StoredAdminRedeemCodeBatch>, DataLayerError> {
        let now = current_unix_secs_i64();
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let Some(current_batch) = sqlx::query(
            r#"
SELECT status
FROM redeem_code_batches
WHERE id = ?
LIMIT 1
"#,
        )
        .bind(&input.batch_id)
        .fetch_optional(&mut *tx)
        .await
        .map_sql_err()?
        else {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::NotFound);
        };
        let status: String = get(&current_batch, "status")?;
        if status != "disabled" {
            sqlx::query(
                r#"
UPDATE redeem_code_batches
SET status = 'disabled',
    updated_at = ?
WHERE id = ?
"#,
            )
            .bind(now)
            .bind(&input.batch_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;

            sqlx::query(
                r#"
UPDATE redeem_codes
SET status = 'disabled',
    disabled_by = COALESCE(?, disabled_by),
    updated_at = ?
WHERE batch_id = ?
  AND status = 'active'
"#,
            )
            .bind(input.operator_id.as_deref())
            .bind(now)
            .bind(&input.batch_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        }

        let batch =
            map_redeem_batch_row(&sqlite_redeem_batch_by_id(&mut tx, &input.batch_id).await?)?;
        tx.commit().await.map_sql_err()?;
        Ok(WalletMutationOutcome::Applied(batch))
    }

    async fn delete_admin_redeem_code_batch(
        &self,
        input: DeleteAdminRedeemCodeBatchInput,
    ) -> Result<WalletMutationOutcome<StoredAdminRedeemCodeBatch>, DataLayerError> {
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let Some(current_batch) = sqlx::query(
            r#"
SELECT status
FROM redeem_code_batches
WHERE id = ?
LIMIT 1
"#,
        )
        .bind(&input.batch_id)
        .fetch_optional(&mut *tx)
        .await
        .map_sql_err()?
        else {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::NotFound);
        };
        let status: String = get(&current_batch, "status")?;
        if status != "disabled" {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(
                "only disabled redeem code batch can be deleted".to_string(),
            ));
        }

        let redeemed_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM redeem_codes
WHERE batch_id = ?
  AND status = 'redeemed'
"#,
        )
        .bind(&input.batch_id)
        .fetch_one(&mut *tx)
        .await
        .map_sql_err()?;
        if redeemed_count > 0 {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(
                "redeemed batch cannot be deleted".to_string(),
            ));
        }

        let batch =
            map_redeem_batch_row(&sqlite_redeem_batch_by_id(&mut tx, &input.batch_id).await?)?;
        let _ = input.operator_id;
        sqlx::query("DELETE FROM redeem_codes WHERE batch_id = ?")
            .bind(&input.batch_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        sqlx::query("DELETE FROM redeem_code_batches WHERE id = ?")
            .bind(&input.batch_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        tx.commit().await.map_sql_err()?;
        Ok(WalletMutationOutcome::Applied(batch))
    }

    async fn disable_admin_redeem_code(
        &self,
        input: DisableAdminRedeemCodeInput,
    ) -> Result<WalletMutationOutcome<StoredAdminRedeemCode>, DataLayerError> {
        let now = current_unix_secs_i64();
        let mut tx = self.pool.begin().await.map_sql_err()?;
        let Some(current_code) = sqlx::query(
            r#"
SELECT batch_id, status
FROM redeem_codes
WHERE id = ?
LIMIT 1
"#,
        )
        .bind(&input.code_id)
        .fetch_optional(&mut *tx)
        .await
        .map_sql_err()?
        else {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::NotFound);
        };
        let batch_id: String = get(&current_code, "batch_id")?;
        let status: String = get(&current_code, "status")?;
        if status == "redeemed" {
            tx.commit().await.map_sql_err()?;
            return Ok(WalletMutationOutcome::Invalid(
                "redeemed code cannot be disabled".to_string(),
            ));
        }
        if status != "disabled" {
            sqlx::query(
                r#"
UPDATE redeem_codes
SET status = 'disabled',
    disabled_by = COALESCE(?, disabled_by),
    updated_at = ?
WHERE id = ?
"#,
            )
            .bind(input.operator_id.as_deref())
            .bind(now)
            .bind(&input.code_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        }

        sqlx::query("UPDATE redeem_code_batches SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(&batch_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        let code = map_redeem_code_row(&sqlite_redeem_code_by_id(&mut tx, &input.code_id).await?)?;
        tx.commit().await.map_sql_err()?;
        Ok(WalletMutationOutcome::Applied(code))
    }

    async fn redeem_wallet_code(
        &self,
        input: RedeemWalletCodeInput,
    ) -> Result<RedeemWalletCodeOutcome, DataLayerError> {
        let Some(normalized) = normalize_redeem_code(&input.code) else {
            return Ok(RedeemWalletCodeOutcome::InvalidCode);
        };
        let code_hash = hash_redeem_code(&normalized);
        let now = current_unix_secs_i64();
        let mut tx = self.pool.begin().await.map_sql_err()?;

        let Some(code_row) = sqlx::query(
            r#"
SELECT
  codes.id AS code_id,
  codes.status AS code_status,
  codes.batch_id,
  batches.name AS batch_name,
  batches.status AS batch_status,
  batches.balance_bucket,
  batches.amount_usd,
  batches.expires_at AS batch_expires_at
FROM redeem_codes AS codes
JOIN redeem_code_batches AS batches ON batches.id = codes.batch_id
WHERE codes.code_hash = ?
LIMIT 1
"#,
        )
        .bind(&code_hash)
        .fetch_optional(&mut *tx)
        .await
        .map_sql_err()?
        else {
            tx.commit().await.map_sql_err()?;
            return Ok(RedeemWalletCodeOutcome::CodeNotFound);
        };

        let code_status: String = get(&code_row, "code_status")?;
        match code_status.as_str() {
            "disabled" => {
                tx.commit().await.map_sql_err()?;
                return Ok(RedeemWalletCodeOutcome::CodeDisabled);
            }
            "redeemed" => {
                tx.commit().await.map_sql_err()?;
                return Ok(RedeemWalletCodeOutcome::CodeRedeemed);
            }
            _ => {}
        }
        let batch_status: String = get(&code_row, "batch_status")?;
        if batch_status != "active" {
            tx.commit().await.map_sql_err()?;
            return Ok(RedeemWalletCodeOutcome::BatchDisabled);
        }
        let batch_expires_at: Option<i64> = get(&code_row, "batch_expires_at")?;
        if batch_expires_at.is_some_and(|value| value <= now) {
            tx.commit().await.map_sql_err()?;
            return Ok(RedeemWalletCodeOutcome::CodeExpired);
        }

        let code_id: String = get(&code_row, "code_id")?;
        let batch_id: String = get(&code_row, "batch_id")?;
        let batch_name: String = get(&code_row, "batch_name")?;
        let balance_bucket: String = get(&code_row, "balance_bucket")?;
        let amount_usd = sqlite_real(&code_row, "amount_usd")?;
        let credits_recharge_balance = redeem_code_credits_recharge_balance(&balance_bucket);

        let wallet_row = sqlite_wallet_by_user_id(&mut tx, &input.user_id).await?;
        let wallet_id = if let Some(row) = wallet_row.as_ref() {
            let status: String = get(row, "status")?;
            if status != "active" {
                tx.commit().await.map_sql_err()?;
                return Ok(RedeemWalletCodeOutcome::WalletInactive);
            }
            get(row, "id")?
        } else {
            uuid::Uuid::new_v4().to_string()
        };

        let (before_recharge, before_gift) = if let Some(row) = wallet_row.as_ref() {
            (
                sqlite_real(row, "balance")?,
                sqlite_real(row, "gift_balance")?,
            )
        } else {
            sqlx::query(
                r#"
INSERT INTO wallets (
  id, user_id, balance, gift_balance, limit_mode, currency, status,
  total_recharged, total_consumed, total_refunded, total_adjusted,
  created_at, updated_at
)
VALUES (?, ?, 0.0, 0.0, 'finite', 'USD', 'active', 0.0, 0.0, 0.0, 0.0, ?, ?)
"#,
            )
            .bind(&wallet_id)
            .bind(&input.user_id)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
            (0.0, 0.0)
        };
        let after_recharge = if credits_recharge_balance {
            before_recharge + amount_usd
        } else {
            before_recharge
        };
        let after_gift = if credits_recharge_balance {
            before_gift
        } else {
            before_gift + amount_usd
        };
        sqlx::query(
            r#"
UPDATE wallets
SET balance = ?,
    gift_balance = ?,
    total_recharged = total_recharged + ?,
    updated_at = ?
WHERE id = ?
"#,
        )
        .bind(after_recharge)
        .bind(after_gift)
        .bind(amount_usd)
        .bind(now)
        .bind(&wallet_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        let payment_method = redeem_code_payment_method(&balance_bucket);
        let order_id = uuid::Uuid::new_v4().to_string();
        let gateway_order_id = format!("card_{}", uuid::Uuid::new_v4().simple());
        let gateway_response = json_string(
            &serde_json::json!({
                "source": "redeem_code",
                "batch_id": batch_id,
                "batch_name": batch_name,
                "balance_bucket": balance_bucket,
            }),
            "payment_orders.gateway_response",
        )?;
        sqlx::query(
            r#"
INSERT INTO payment_orders (
  id, order_no, wallet_id, user_id, amount_usd, pay_amount, pay_currency,
  exchange_rate, refunded_amount_usd, refundable_amount_usd, payment_method,
  gateway_order_id, gateway_response, status, created_at, paid_at, credited_at
)
VALUES (?, ?, ?, ?, ?, NULL, NULL, NULL, 0, ?, ?, ?, ?, 'credited', ?, ?, ?)
"#,
        )
        .bind(&order_id)
        .bind(&input.order_no)
        .bind(&wallet_id)
        .bind(&input.user_id)
        .bind(amount_usd)
        .bind(redeem_code_refundable_amount(&balance_bucket, amount_usd))
        .bind(payment_method)
        .bind(&gateway_order_id)
        .bind(&gateway_response)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        sqlx::query(
            r#"
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount, balance_before, balance_after,
  recharge_balance_before, recharge_balance_after, gift_balance_before,
  gift_balance_after, link_type, link_id, operator_id, description, created_at
)
VALUES (?, ?, 'recharge', 'topup_card_code', ?, ?, ?, ?, ?, ?, ?, 'payment_order', ?, NULL, ?, ?)
"#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&wallet_id)
        .bind(amount_usd)
        .bind(before_recharge + before_gift)
        .bind(after_recharge + after_gift)
        .bind(before_recharge)
        .bind(after_recharge)
        .bind(before_gift)
        .bind(after_gift)
        .bind(&order_id)
        .bind("兑换码充值")
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;

        sqlx::query(
            r#"
UPDATE redeem_codes
SET status = 'redeemed',
    redeemed_by_user_id = ?,
    redeemed_wallet_id = ?,
    redeemed_payment_order_id = ?,
    redeemed_at = ?,
    updated_at = ?
WHERE id = ?
"#,
        )
        .bind(&input.user_id)
        .bind(&wallet_id)
        .bind(&order_id)
        .bind(now)
        .bind(now)
        .bind(&code_id)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
        sqlx::query("UPDATE redeem_code_batches SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(&batch_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;

        let wallet = map_wallet_row(&sqlite_wallet_by_id(&mut tx, &wallet_id).await?)?;
        let order = map_payment_order_row(&sqlite_payment_order_by_id(&mut tx, &order_id).await?)?;
        tx.commit().await.map_sql_err()?;
        Ok(RedeemWalletCodeOutcome::Redeemed {
            wallet,
            order,
            amount_usd,
            batch_name,
        })
    }
}

fn daily_usage_select_sql(suffix: &'static str) -> String {
    format!(
        r#"
SELECT
  id, billing_date, billing_timezone, total_cost_usd, total_requests,
  input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
  first_finalized_at AS first_finalized_at_unix_secs,
  last_finalized_at AS last_finalized_at_unix_secs,
  aggregated_at AS aggregated_at_unix_secs
FROM wallet_daily_usage_ledgers
WHERE wallet_id = ?
  AND billing_timezone = ?
  {suffix}
"#
    )
}

fn current_billing_date(billing_timezone: &str) -> Result<String, DataLayerError> {
    let timezone = billing_timezone.parse::<chrono_tz::Tz>().map_err(|err| {
        DataLayerError::InvalidInput(format!("invalid wallet billing timezone: {err}"))
    })?;
    Ok(Utc::now().with_timezone(&timezone).date_naive().to_string())
}

fn map_wallet_row(row: &SqliteRow) -> Result<StoredWalletSnapshot, DataLayerError> {
    StoredWalletSnapshot::new(
        get(row, "id")?,
        get(row, "user_id")?,
        get(row, "api_key_id")?,
        sqlite_real(row, "balance")?,
        sqlite_real(row, "gift_balance")?,
        get(row, "limit_mode")?,
        get(row, "currency")?,
        get(row, "status")?,
        sqlite_real(row, "total_recharged")?,
        sqlite_real(row, "total_consumed")?,
        sqlite_real(row, "total_refunded")?,
        sqlite_real(row, "total_adjusted")?,
        get(row, "updated_at_unix_secs")?,
    )
}

fn map_admin_wallet_list_item_row(
    row: &SqliteRow,
) -> Result<StoredAdminWalletListItem, DataLayerError> {
    Ok(StoredAdminWalletListItem {
        id: get(row, "id")?,
        user_id: get(row, "user_id")?,
        api_key_id: get(row, "api_key_id")?,
        balance: sqlite_real(row, "balance")?,
        gift_balance: sqlite_real(row, "gift_balance")?,
        limit_mode: get(row, "limit_mode")?,
        currency: get(row, "currency")?,
        status: get(row, "status")?,
        total_recharged: sqlite_real(row, "total_recharged")?,
        total_consumed: sqlite_real(row, "total_consumed")?,
        total_refunded: sqlite_real(row, "total_refunded")?,
        total_adjusted: sqlite_real(row, "total_adjusted")?,
        user_name: get(row, "user_name")?,
        api_key_name: get(row, "api_key_name")?,
        created_at_unix_ms: optional_timestamp(
            get(row, "created_at_unix_ms")?,
            "wallets.created_at",
        )?,
        updated_at_unix_secs: optional_timestamp(
            get(row, "updated_at_unix_secs")?,
            "wallets.updated_at",
        )?,
    })
}

fn map_admin_wallet_ledger_item_row(
    row: &SqliteRow,
) -> Result<StoredAdminWalletLedgerItem, DataLayerError> {
    Ok(StoredAdminWalletLedgerItem {
        id: get(row, "id")?,
        wallet_id: get(row, "wallet_id")?,
        category: get(row, "category")?,
        reason_code: get(row, "reason_code")?,
        amount: sqlite_real(row, "amount")?,
        balance_before: sqlite_real(row, "balance_before")?,
        balance_after: sqlite_real(row, "balance_after")?,
        recharge_balance_before: sqlite_real(row, "recharge_balance_before")?,
        recharge_balance_after: sqlite_real(row, "recharge_balance_after")?,
        gift_balance_before: sqlite_real(row, "gift_balance_before")?,
        gift_balance_after: sqlite_real(row, "gift_balance_after")?,
        link_type: get(row, "link_type")?,
        link_id: get(row, "link_id")?,
        operator_id: get(row, "operator_id")?,
        operator_name: get(row, "operator_name")?,
        operator_email: get(row, "operator_email")?,
        description: get(row, "description")?,
        wallet_user_id: get(row, "user_id")?,
        wallet_user_name: get(row, "wallet_user_name")?,
        wallet_api_key_id: get(row, "api_key_id")?,
        api_key_name: get(row, "api_key_name")?,
        wallet_status: get(row, "wallet_status")?,
        created_at_unix_ms: optional_timestamp(
            get(row, "created_at_unix_ms")?,
            "wallet_transactions.created_at",
        )?,
    })
}

fn map_admin_wallet_refund_request_item_row(
    row: &SqliteRow,
) -> Result<StoredAdminWalletRefundRequestItem, DataLayerError> {
    Ok(StoredAdminWalletRefundRequestItem {
        id: get(row, "id")?,
        refund_no: get(row, "refund_no")?,
        wallet_id: get(row, "wallet_id")?,
        user_id: get(row, "user_id")?,
        payment_order_id: get(row, "payment_order_id")?,
        source_type: get(row, "source_type")?,
        source_id: get(row, "source_id")?,
        refund_mode: get(row, "refund_mode")?,
        amount_usd: sqlite_real(row, "amount_usd")?,
        status: get(row, "status")?,
        reason: get(row, "reason")?,
        failure_reason: get(row, "failure_reason")?,
        gateway_refund_id: get(row, "gateway_refund_id")?,
        payout_method: get(row, "payout_method")?,
        payout_reference: get(row, "payout_reference")?,
        payout_proof: optional_json(get(row, "payout_proof")?, "refund_requests.payout_proof")?,
        requested_by: get(row, "requested_by")?,
        approved_by: get(row, "approved_by")?,
        processed_by: get(row, "processed_by")?,
        wallet_user_id: get(row, "wallet_user_id")?,
        wallet_user_name: get(row, "wallet_user_name")?,
        wallet_api_key_id: get(row, "wallet_api_key_id")?,
        api_key_name: get(row, "api_key_name")?,
        wallet_status: get(row, "wallet_status")?,
        created_at_unix_ms: optional_timestamp(
            get(row, "created_at_unix_ms")?,
            "refund_requests.created_at",
        )?,
        updated_at_unix_secs: optional_timestamp(
            get(row, "updated_at_unix_secs")?,
            "refund_requests.updated_at",
        )?,
        processed_at_unix_secs: optional_timestamp(
            get(row, "processed_at_unix_secs")?,
            "refund_requests.processed_at",
        )?,
        completed_at_unix_secs: optional_timestamp(
            get(row, "completed_at_unix_secs")?,
            "refund_requests.completed_at",
        )?,
    })
}

fn current_unix_secs_i64() -> i64 {
    Utc::now().timestamp().max(0)
}

fn i64_from_usize(value: usize, field_name: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value).map_err(|_| DataLayerError::InvalidInput(format!("{field_name} overflow")))
}

fn sqlite_bool(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn json_string(value: &serde_json::Value, field_name: &str) -> Result<String, DataLayerError> {
    serde_json::to_string(value).map_err(|err| {
        DataLayerError::UnexpectedValue(format!("{field_name} could not be encoded: {err}"))
    })
}

fn plan_entitlements_snapshot(snapshot: &serde_json::Value) -> serde_json::Value {
    snapshot
        .get("entitlements")
        .or_else(|| snapshot.get("entitlements_json"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]))
}

fn plan_max_active_per_user(snapshot: &serde_json::Value) -> i64 {
    snapshot
        .get("max_active_per_user")
        .and_then(|value| value.as_i64())
        .unwrap_or(1)
        .max(1)
}

fn plan_purchase_limit_scope(snapshot: &serde_json::Value) -> &str {
    match snapshot
        .get("purchase_limit_scope")
        .and_then(|value| value.as_str())
    {
        Some("lifetime") => "lifetime",
        Some("unlimited") => "unlimited",
        _ => "active_period",
    }
}

fn plan_replacement_entitlement_types(snapshot: &serde_json::Value) -> Vec<&'static str> {
    let entitlements = plan_entitlements_snapshot(snapshot);
    let mut kinds = Vec::new();
    if entitlement_snapshot_has_type(&entitlements, "daily_quota") {
        kinds.push("daily_quota");
    }
    if entitlement_snapshot_has_type(&entitlements, "membership_group") {
        kinds.push("membership_group");
    }
    kinds
}

fn entitlement_snapshot_has_type(snapshot: &serde_json::Value, entitlement_type: &str) -> bool {
    snapshot.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.get("type").and_then(|value| value.as_str()) == Some(entitlement_type))
    })
}

async fn replace_matching_plan_entitlements_sqlite(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    user_id: &str,
    snapshot: &serde_json::Value,
    now: i64,
) -> Result<(), DataLayerError> {
    let replacement_types = plan_replacement_entitlement_types(snapshot);
    if replacement_types.is_empty() {
        return Ok(());
    }

    let rows = sqlx::query(
        r#"
SELECT id, entitlements_snapshot
FROM user_plan_entitlements
WHERE user_id = ?
  AND status = 'active'
  AND expires_at > ?
        "#,
    )
    .bind(user_id)
    .bind(now)
    .fetch_all(&mut **tx)
    .await
    .map_sql_err()?;

    for row in rows {
        let entitlements = optional_json(
            get::<Option<String>>(&row, "entitlements_snapshot")?,
            "user_plan_entitlements.entitlements_snapshot",
        )?
        .unwrap_or_else(|| serde_json::json!([]));
        let should_replace = replacement_types
            .iter()
            .any(|kind| entitlement_snapshot_has_type(&entitlements, kind));
        if !should_replace {
            continue;
        }
        let entitlement_id: String = get(&row, "id")?;
        sqlx::query(
            r#"
UPDATE user_plan_entitlements
SET status = 'replaced',
    expires_at = CASE WHEN expires_at > ? THEN ? ELSE expires_at END,
    updated_at = ?
WHERE id = ?
  AND status = 'active'
  AND expires_at > ?
        "#,
        )
        .bind(now)
        .bind(now)
        .bind(now)
        .bind(entitlement_id)
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_sql_err()?;
    }
    Ok(())
}

fn plan_expires_at_unix(snapshot: &serde_json::Value, starts_at_unix_secs: i64) -> i64 {
    let duration_value = snapshot
        .get("duration_value")
        .and_then(|value| value.as_i64())
        .unwrap_or(1)
        .max(1);
    let days = match snapshot
        .get("duration_unit")
        .and_then(|value| value.as_str())
        .unwrap_or("month")
    {
        "day" | "custom" => duration_value,
        "year" => 365 * duration_value,
        _ => 30 * duration_value,
    };
    starts_at_unix_secs.saturating_add(days.saturating_mul(86_400))
}

async fn apply_plan_wallet_credit_sqlite(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    wallet_id: &str,
    order_id: &str,
    payment_method: &str,
    entitlements: &serde_json::Value,
    now: i64,
) -> Result<(), DataLayerError> {
    let credits = entitlements
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(|value| value.as_str()) == Some("wallet_credit"))
        .filter_map(|item| {
            let amount = item.get("amount_usd").and_then(|value| value.as_f64())?;
            if amount <= 0.0 || !amount.is_finite() {
                return None;
            }
            let bucket = item
                .get("balance_bucket")
                .and_then(|value| value.as_str())
                .unwrap_or("gift")
                .to_ascii_lowercase();
            Some((amount, bucket))
        })
        .collect::<Vec<_>>();
    if credits.is_empty() {
        return Ok(());
    }
    let Some(wallet_row) =
        sqlx::query("SELECT id, status, balance, gift_balance FROM wallets WHERE id = ? LIMIT 1")
            .bind(wallet_id)
            .fetch_optional(&mut **tx)
            .await
            .map_sql_err()?
    else {
        return Err(DataLayerError::UnexpectedValue(
            "wallet not found for plan wallet_credit".to_string(),
        ));
    };
    let status: String = get(&wallet_row, "status")?;
    if status != "active" {
        return Err(DataLayerError::UnexpectedValue(
            "wallet is not active for plan wallet_credit".to_string(),
        ));
    }
    let mut recharge_balance = sqlite_real(&wallet_row, "balance")?;
    let mut gift_balance = sqlite_real(&wallet_row, "gift_balance")?;
    for (amount, bucket) in credits {
        let before_recharge = recharge_balance;
        let before_gift = gift_balance;
        let before_total = before_recharge + before_gift;
        let credits_recharge = bucket == "recharge";
        if credits_recharge {
            recharge_balance += amount;
        } else {
            gift_balance += amount;
        }
        let after_total = recharge_balance + gift_balance;
        sqlx::query(
            r#"
UPDATE wallets
SET balance = ?,
    gift_balance = ?,
    total_recharged = total_recharged + ?,
    updated_at = ?
WHERE id = ?
            "#,
        )
        .bind(recharge_balance)
        .bind(gift_balance)
        .bind(if credits_recharge { amount } else { 0.0 })
        .bind(now)
        .bind(wallet_id)
        .execute(&mut **tx)
        .await
        .map_sql_err()?;
        sqlx::query(
            r#"
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount, balance_before, balance_after,
  recharge_balance_before, recharge_balance_after, gift_balance_before,
  gift_balance_after, link_type, link_id, operator_id, description, created_at
)
VALUES (?, ?, 'recharge', 'plan_wallet_credit', ?, ?, ?, ?, ?, ?, ?, 'payment_order', ?, NULL, ?, ?)
            "#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(wallet_id)
        .bind(amount)
        .bind(before_total)
        .bind(after_total)
        .bind(before_recharge)
        .bind(recharge_balance)
        .bind(before_gift)
        .bind(gift_balance)
        .bind(order_id)
        .bind(format!("套餐附赠余额({payment_method})"))
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_sql_err()?;
    }
    Ok(())
}

fn default_refund_mode_for_payment_method(payment_method: &str) -> &'static str {
    if matches!(
        payment_method,
        "admin_manual" | "card_recharge" | "card_code" | "gift_code"
    ) {
        return "offline_payout";
    }
    "original_channel"
}

fn payment_gateway_response_map(
    value: Option<serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    match value {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    }
}

fn normalize_redeem_code(value: &str) -> Option<String> {
    let normalized = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_uppercase())
        .collect::<String>();
    if normalized.len() < 16 {
        None
    } else {
        Some(normalized)
    }
}

fn hash_redeem_code(normalized: &str) -> String {
    use sha2::Digest;

    format!("{:x}", sha2::Sha256::digest(normalized.as_bytes()))
}

fn format_redeem_code(normalized: &str) -> String {
    normalized
        .as_bytes()
        .chunks(8)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("-")
}

fn generate_redeem_code_candidate() -> (String, String, String, String, String, String) {
    let normalized = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .to_ascii_uppercase();
    let code = format_redeem_code(&normalized);
    let code_id = uuid::Uuid::new_v4().to_string();
    let prefix = normalized.chars().take(4).collect::<String>();
    let suffix = normalized
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let masked_code = mask_redeem_code(&prefix, &suffix);
    let code_hash = hash_redeem_code(&normalized);
    (code_id, code, masked_code, code_hash, prefix, suffix)
}

fn wallet_select_sql(where_clause: &str) -> String {
    format!(
        r#"
SELECT
  id, user_id, api_key_id, balance, gift_balance, limit_mode, currency,
  status, total_recharged, total_consumed, total_refunded, total_adjusted,
  updated_at AS updated_at_unix_secs
FROM wallets
{where_clause}
"#
    )
}

fn apply_admin_balance_adjustment(
    amount_usd: f64,
    balance_type: &str,
    recharge_balance: &mut f64,
    gift_balance: &mut f64,
) {
    if amount_usd > 0.0 {
        if balance_type.eq_ignore_ascii_case("gift") {
            *gift_balance += amount_usd;
        } else {
            *recharge_balance += amount_usd;
        }
        return;
    }

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
        consume_positive_bucket(gift_balance, &mut remaining);
        consume_positive_bucket(recharge_balance, &mut remaining);
    } else {
        consume_positive_bucket(recharge_balance, &mut remaining);
        consume_positive_bucket(gift_balance, &mut remaining);
    }
    if remaining > 0.0 {
        *recharge_balance -= remaining;
    }
}

async fn sqlite_wallet_by_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    wallet_id: &str,
) -> Result<SqliteRow, DataLayerError> {
    let sql = wallet_select_sql("WHERE id = ? LIMIT 1");
    sqlx::query(&sql)
        .bind(wallet_id)
        .fetch_one(&mut **tx)
        .await
        .map_sql_err()
}

async fn sqlite_wallet_by_id_optional(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    wallet_id: &str,
) -> Result<Option<SqliteRow>, DataLayerError> {
    let sql = wallet_select_sql("WHERE id = ? LIMIT 1");
    sqlx::query(&sql)
        .bind(wallet_id)
        .fetch_optional(&mut **tx)
        .await
        .map_sql_err()
}

async fn sqlite_wallet_by_user_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    user_id: &str,
) -> Result<Option<SqliteRow>, DataLayerError> {
    let sql = wallet_select_sql("WHERE user_id = ? LIMIT 1");
    sqlx::query(&sql)
        .bind(user_id)
        .fetch_optional(&mut **tx)
        .await
        .map_sql_err()
}

fn payment_order_select_sql(where_clause: &str) -> String {
    format!(
        r#"
SELECT
  id, order_no, wallet_id, user_id, amount_usd, pay_amount, pay_currency,
  exchange_rate, refunded_amount_usd, refundable_amount_usd, payment_method,
  payment_provider, payment_channel, order_kind, product_id, product_snapshot,
  gateway_order_id, gateway_response, status,
  created_at AS created_at_unix_ms,
  paid_at AS paid_at_unix_secs,
  credited_at AS credited_at_unix_secs,
  expires_at AS expires_at_unix_secs
FROM payment_orders
{where_clause}
"#
    )
}

async fn sqlite_payment_order_by_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    order_id: &str,
) -> Result<SqliteRow, DataLayerError> {
    let sql = payment_order_select_sql("WHERE id = ? LIMIT 1");
    sqlx::query(&sql)
        .bind(order_id)
        .fetch_one(&mut **tx)
        .await
        .map_sql_err()
}

async fn sqlite_payment_order_by_id_optional(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    order_id: &str,
) -> Result<Option<SqliteRow>, DataLayerError> {
    let sql = payment_order_select_sql("WHERE id = ? LIMIT 1");
    sqlx::query(&sql)
        .bind(order_id)
        .fetch_optional(&mut **tx)
        .await
        .map_sql_err()
}

async fn sqlite_payment_order_by_order_no(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    order_no: &str,
) -> Result<Option<SqliteRow>, DataLayerError> {
    let sql = payment_order_select_sql("WHERE order_no = ? LIMIT 1");
    sqlx::query(&sql)
        .bind(order_no)
        .fetch_optional(&mut **tx)
        .await
        .map_sql_err()
}

async fn sqlite_payment_order_by_gateway_order_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    gateway_order_id: &str,
) -> Result<Option<SqliteRow>, DataLayerError> {
    let sql = payment_order_select_sql("WHERE gateway_order_id = ? LIMIT 1");
    sqlx::query(&sql)
        .bind(gateway_order_id)
        .fetch_optional(&mut **tx)
        .await
        .map_sql_err()
}

fn refund_select_sql(where_clause: &str) -> String {
    format!(
        r#"
SELECT
  id, refund_no, wallet_id, user_id, payment_order_id, source_type,
  source_id, refund_mode, amount_usd, status, reason, failure_reason,
  gateway_refund_id, payout_method, payout_reference, payout_proof,
  requested_by, approved_by, processed_by,
  created_at AS created_at_unix_ms,
  updated_at AS updated_at_unix_secs,
  processed_at AS processed_at_unix_secs,
  completed_at AS completed_at_unix_secs
FROM refund_requests
{where_clause}
"#
    )
}

async fn sqlite_refund_by_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    refund_id: &str,
) -> Result<SqliteRow, DataLayerError> {
    let sql = refund_select_sql("WHERE id = ? LIMIT 1");
    sqlx::query(&sql)
        .bind(refund_id)
        .fetch_one(&mut **tx)
        .await
        .map_sql_err()
}

async fn sqlite_refund_by_id_and_wallet(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    refund_id: &str,
    wallet_id: &str,
) -> Result<Option<SqliteRow>, DataLayerError> {
    let sql = refund_select_sql("WHERE id = ? AND wallet_id = ? LIMIT 1");
    sqlx::query(&sql)
        .bind(refund_id)
        .bind(wallet_id)
        .fetch_optional(&mut **tx)
        .await
        .map_sql_err()
}

async fn sqlite_refund_by_idempotency(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    user_id: &str,
    idempotency_key: &str,
) -> Result<Option<SqliteRow>, DataLayerError> {
    let sql = refund_select_sql("WHERE user_id = ? AND idempotency_key = ? LIMIT 1");
    sqlx::query(&sql)
        .bind(user_id)
        .bind(idempotency_key)
        .fetch_optional(&mut **tx)
        .await
        .map_sql_err()
}

fn redeem_batch_select_sql(where_clause: &str) -> String {
    format!(
        r#"
SELECT
  batches.id, batches.name, batches.amount_usd, batches.currency,
  batches.balance_bucket, batches.total_count,
  COALESCE(SUM(CASE WHEN codes.status = 'redeemed' THEN 1 ELSE 0 END), 0) AS redeemed_count,
  COALESCE(SUM(CASE WHEN codes.status = 'active' THEN 1 ELSE 0 END), 0) AS active_count,
  batches.status, batches.description, batches.created_by,
  batches.expires_at AS expires_at_unix_secs,
  batches.created_at AS created_at_unix_ms,
  batches.updated_at AS updated_at_unix_secs
FROM redeem_code_batches AS batches
LEFT JOIN redeem_codes AS codes ON codes.batch_id = batches.id
{where_clause}
GROUP BY
  batches.id, batches.name, batches.amount_usd, batches.currency,
  batches.balance_bucket, batches.total_count, batches.status,
  batches.description, batches.created_by, batches.expires_at,
  batches.created_at, batches.updated_at
"#
    )
}

async fn sqlite_redeem_batch_by_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    batch_id: &str,
) -> Result<SqliteRow, DataLayerError> {
    let sql = redeem_batch_select_sql("WHERE batches.id = ?");
    sqlx::query(&sql)
        .bind(batch_id)
        .fetch_one(&mut **tx)
        .await
        .map_sql_err()
}

fn redeem_code_select_sql(where_clause: &str) -> String {
    format!(
        r#"
SELECT
  codes.id, codes.batch_id, batches.name AS batch_name, codes.code_prefix,
  codes.code_suffix, codes.status, codes.redeemed_by_user_id,
  redeemed_users.username AS redeemed_by_user_name,
  codes.redeemed_wallet_id, codes.redeemed_payment_order_id,
  orders.order_no AS redeemed_order_no,
  codes.redeemed_at AS redeemed_at_unix_secs,
  codes.disabled_by,
  batches.expires_at AS expires_at_unix_secs,
  codes.created_at AS created_at_unix_ms,
  codes.updated_at AS updated_at_unix_secs
FROM redeem_codes AS codes
JOIN redeem_code_batches AS batches ON batches.id = codes.batch_id
LEFT JOIN users AS redeemed_users ON redeemed_users.id = codes.redeemed_by_user_id
LEFT JOIN payment_orders AS orders ON orders.id = codes.redeemed_payment_order_id
{where_clause}
"#
    )
}

async fn sqlite_redeem_code_by_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    code_id: &str,
) -> Result<SqliteRow, DataLayerError> {
    let sql = redeem_code_select_sql("WHERE codes.id = ? LIMIT 1");
    sqlx::query(&sql)
        .bind(code_id)
        .fetch_one(&mut **tx)
        .await
        .map_sql_err()
}

async fn update_sqlite_payment_callback_failure(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    callback_id: &str,
    input: &ProcessPaymentCallbackInput,
    payload: &str,
    error: &str,
) -> Result<(), DataLayerError> {
    sqlx::query(
        r#"
UPDATE payment_callbacks
SET signature_valid = ?,
    status = 'failed',
    error_message = ?,
    payload_hash = ?,
    payload = ?,
    processed_at = ?,
    order_no = COALESCE(?, order_no),
    gateway_order_id = COALESCE(?, gateway_order_id)
WHERE id = ?
"#,
    )
    .bind(sqlite_bool(input.signature_valid))
    .bind(error)
    .bind(&input.payload_hash)
    .bind(payload)
    .bind(current_unix_secs_i64())
    .bind(input.order_no.as_deref())
    .bind(input.gateway_order_id.as_deref())
    .bind(callback_id)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(())
}

async fn mark_sqlite_payment_callback_processed(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    callback_id: &str,
    input: &ProcessPaymentCallbackInput,
    payload: &str,
    order_id: &str,
    order_no: &str,
) -> Result<(), DataLayerError> {
    sqlx::query(
        r#"
UPDATE payment_callbacks
SET payment_order_id = ?,
    signature_valid = 1,
    status = 'processed',
    error_message = NULL,
    payload_hash = ?,
    payload = ?,
    processed_at = ?,
    order_no = ?,
    gateway_order_id = COALESCE(?, gateway_order_id)
WHERE id = ?
"#,
    )
    .bind(order_id)
    .bind(&input.payload_hash)
    .bind(payload)
    .bind(current_unix_secs_i64())
    .bind(order_no)
    .bind(input.gateway_order_id.as_deref())
    .bind(callback_id)
    .execute(&mut **tx)
    .await
    .map_sql_err()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn update_sqlite_wallet_snapshot(
    pool: &SqlitePool,
    owner_column: &str,
    owner_id: &str,
    balance: f64,
    gift_balance: f64,
    limit_mode: &str,
    currency: &str,
    status: &str,
    total_recharged: f64,
    total_consumed: f64,
    total_refunded: f64,
    total_adjusted: f64,
    updated_at_unix_secs: Option<u64>,
) -> Result<(), DataLayerError> {
    let owner_predicate = match owner_column {
        "user_id" => "user_id = ?",
        "api_key_id" => "api_key_id = ?",
        _ => {
            return Err(DataLayerError::UnexpectedValue(format!(
                "unsupported wallet owner column: {owner_column}"
            )));
        }
    };
    let sql = format!(
        r#"
UPDATE wallets
SET balance = ?,
    gift_balance = ?,
    limit_mode = ?,
    currency = ?,
    status = ?,
    total_recharged = ?,
    total_consumed = ?,
    total_refunded = ?,
    total_adjusted = ?,
    updated_at = ?
WHERE {owner_predicate}
"#
    );
    sqlx::query(&sql)
        .bind(balance)
        .bind(gift_balance)
        .bind(limit_mode)
        .bind(currency)
        .bind(status)
        .bind(total_recharged)
        .bind(total_consumed)
        .bind(total_refunded)
        .bind(total_adjusted)
        .bind(
            updated_at_unix_secs
                .map(|value| value as i64)
                .unwrap_or_else(current_unix_secs_i64),
        )
        .bind(owner_id)
        .execute(pool)
        .await
        .map_sql_err()?;
    Ok(())
}

async fn initialize_sqlite_auth_wallet(
    pool: &SqlitePool,
    user_id: Option<&str>,
    api_key_id: Option<&str>,
    initial_gift_usd: f64,
    unlimited: bool,
) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
    let gift_amount = if unlimited {
        0.0
    } else {
        initial_gift_usd.max(0.0)
    };
    let now = current_unix_secs_i64();
    let wallet = StoredWalletSnapshot::new(
        uuid::Uuid::new_v4().to_string(),
        user_id.map(str::to_string),
        api_key_id.map(str::to_string),
        0.0,
        gift_amount,
        if unlimited { "unlimited" } else { "finite" }.to_string(),
        "USD".to_string(),
        "active".to_string(),
        0.0,
        0.0,
        0.0,
        gift_amount,
        now,
    )?;
    let mut tx = pool.begin().await.map_sql_err()?;
    sqlx::query(
        r#"
INSERT INTO wallets (
  id, user_id, api_key_id, balance, gift_balance, limit_mode, currency,
  status, total_recharged, total_consumed, total_refunded, total_adjusted,
  created_at, updated_at
)
VALUES (?, ?, ?, 0, ?, ?, 'USD', 'active', 0, 0, 0, ?, ?, ?)
"#,
    )
    .bind(&wallet.id)
    .bind(user_id)
    .bind(api_key_id)
    .bind(gift_amount)
    .bind(&wallet.limit_mode)
    .bind(gift_amount)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_sql_err()?;
    if gift_amount > 0.0 {
        let link_id = user_id.or(api_key_id).unwrap_or_default();
        let description = if api_key_id.is_some() {
            "独立余额 Key 初始赠款"
        } else {
            "用户初始赠款"
        };
        sqlx::query(
            r#"
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount, balance_before, balance_after,
  recharge_balance_before, recharge_balance_after, gift_balance_before,
  gift_balance_after, link_type, link_id, operator_id, description, created_at
)
VALUES (?, ?, 'gift', 'gift_initial', ?, 0, ?, 0, 0, 0, ?, 'system_task', ?, NULL, ?, ?)
"#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&wallet.id)
        .bind(gift_amount)
        .bind(gift_amount)
        .bind(gift_amount)
        .bind(link_id)
        .bind(description)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_sql_err()?;
    }
    tx.commit().await.map_sql_err()?;
    Ok(Some(wallet))
}

fn map_payment_order_row(row: &SqliteRow) -> Result<StoredAdminPaymentOrder, DataLayerError> {
    Ok(StoredAdminPaymentOrder {
        id: get(row, "id")?,
        order_no: get(row, "order_no")?,
        wallet_id: get(row, "wallet_id")?,
        user_id: get(row, "user_id")?,
        amount_usd: sqlite_real(row, "amount_usd")?,
        pay_amount: sqlite_optional_real(row, "pay_amount")?,
        pay_currency: get(row, "pay_currency")?,
        exchange_rate: sqlite_optional_real(row, "exchange_rate")?,
        refunded_amount_usd: sqlite_real(row, "refunded_amount_usd")?,
        refundable_amount_usd: sqlite_real(row, "refundable_amount_usd")?,
        payment_method: get(row, "payment_method")?,
        gateway_order_id: get(row, "gateway_order_id")?,
        gateway_response: optional_json(
            get(row, "gateway_response")?,
            "payment_orders.gateway_response",
        )?,
        status: get(row, "status")?,
        created_at_unix_ms: timestamp(
            get(row, "created_at_unix_ms")?,
            "payment_orders.created_at",
        )?,
        paid_at_unix_secs: optional_timestamp(
            get(row, "paid_at_unix_secs")?,
            "payment_orders.paid_at",
        )?,
        credited_at_unix_secs: optional_timestamp(
            get(row, "credited_at_unix_secs")?,
            "payment_orders.credited_at",
        )?,
        expires_at_unix_secs: optional_timestamp(
            get(row, "expires_at_unix_secs")?,
            "payment_orders.expires_at",
        )?,
    })
}

fn map_payment_callback_row(row: &SqliteRow) -> Result<StoredAdminPaymentCallback, DataLayerError> {
    Ok(StoredAdminPaymentCallback {
        id: get(row, "id")?,
        payment_order_id: get(row, "payment_order_id")?,
        payment_method: get(row, "payment_method")?,
        callback_key: get(row, "callback_key")?,
        order_no: get(row, "order_no")?,
        gateway_order_id: get(row, "gateway_order_id")?,
        payload_hash: get(row, "payload_hash")?,
        signature_valid: get::<i64>(row, "signature_valid")? != 0,
        status: get(row, "status")?,
        payload: optional_json(get(row, "payload")?, "payment_callbacks.payload")?,
        error_message: get(row, "error_message")?,
        created_at_unix_ms: timestamp(
            get(row, "created_at_unix_ms")?,
            "payment_callbacks.created_at",
        )?,
        processed_at_unix_secs: optional_timestamp(
            get(row, "processed_at_unix_secs")?,
            "payment_callbacks.processed_at",
        )?,
    })
}

fn map_wallet_transaction_row(
    row: &SqliteRow,
) -> Result<StoredAdminWalletTransaction, DataLayerError> {
    Ok(StoredAdminWalletTransaction {
        id: get(row, "id")?,
        wallet_id: get(row, "wallet_id")?,
        category: get(row, "category")?,
        reason_code: get(row, "reason_code")?,
        amount: sqlite_real(row, "amount")?,
        balance_before: sqlite_real(row, "balance_before")?,
        balance_after: sqlite_real(row, "balance_after")?,
        recharge_balance_before: sqlite_real(row, "recharge_balance_before")?,
        recharge_balance_after: sqlite_real(row, "recharge_balance_after")?,
        gift_balance_before: sqlite_real(row, "gift_balance_before")?,
        gift_balance_after: sqlite_real(row, "gift_balance_after")?,
        link_type: get(row, "link_type")?,
        link_id: get(row, "link_id")?,
        operator_id: get(row, "operator_id")?,
        operator_name: get(row, "operator_name")?,
        operator_email: get(row, "operator_email")?,
        description: get(row, "description")?,
        created_at_unix_ms: optional_timestamp(
            get(row, "created_at_unix_ms")?,
            "wallet_transactions.created_at",
        )?,
    })
}

fn map_refund_row(row: &SqliteRow) -> Result<StoredAdminWalletRefund, DataLayerError> {
    Ok(StoredAdminWalletRefund {
        id: get(row, "id")?,
        refund_no: get(row, "refund_no")?,
        wallet_id: get(row, "wallet_id")?,
        user_id: get(row, "user_id")?,
        payment_order_id: get(row, "payment_order_id")?,
        source_type: get(row, "source_type")?,
        source_id: get(row, "source_id")?,
        refund_mode: get(row, "refund_mode")?,
        amount_usd: sqlite_real(row, "amount_usd")?,
        status: get(row, "status")?,
        reason: get(row, "reason")?,
        failure_reason: get(row, "failure_reason")?,
        gateway_refund_id: get(row, "gateway_refund_id")?,
        payout_method: get(row, "payout_method")?,
        payout_reference: get(row, "payout_reference")?,
        payout_proof: optional_json(get(row, "payout_proof")?, "refund_requests.payout_proof")?,
        requested_by: get(row, "requested_by")?,
        approved_by: get(row, "approved_by")?,
        processed_by: get(row, "processed_by")?,
        created_at_unix_ms: timestamp(
            get(row, "created_at_unix_ms")?,
            "refund_requests.created_at",
        )?,
        updated_at_unix_secs: timestamp(
            get(row, "updated_at_unix_secs")?,
            "refund_requests.updated_at",
        )?,
        processed_at_unix_secs: optional_timestamp(
            get(row, "processed_at_unix_secs")?,
            "refund_requests.processed_at",
        )?,
        completed_at_unix_secs: optional_timestamp(
            get(row, "completed_at_unix_secs")?,
            "refund_requests.completed_at",
        )?,
    })
}

fn map_redeem_batch_row(row: &SqliteRow) -> Result<StoredAdminRedeemCodeBatch, DataLayerError> {
    Ok(StoredAdminRedeemCodeBatch {
        id: get(row, "id")?,
        name: get(row, "name")?,
        amount_usd: sqlite_real(row, "amount_usd")?,
        currency: get(row, "currency")?,
        balance_bucket: get(row, "balance_bucket")?,
        total_count: nonnegative_u64(get(row, "total_count")?, "redeem_code_batches.total_count")?,
        redeemed_count: nonnegative_u64(
            get(row, "redeemed_count")?,
            "redeem_codes.redeemed_count",
        )?,
        active_count: nonnegative_u64(get(row, "active_count")?, "redeem_codes.active_count")?,
        status: get(row, "status")?,
        description: get(row, "description")?,
        created_by: get(row, "created_by")?,
        expires_at_unix_secs: optional_timestamp(
            get(row, "expires_at_unix_secs")?,
            "redeem_code_batches.expires_at",
        )?,
        created_at_unix_ms: timestamp(
            get(row, "created_at_unix_ms")?,
            "redeem_code_batches.created_at",
        )?,
        updated_at_unix_secs: timestamp(
            get(row, "updated_at_unix_secs")?,
            "redeem_code_batches.updated_at",
        )?,
    })
}

fn map_redeem_code_row(row: &SqliteRow) -> Result<StoredAdminRedeemCode, DataLayerError> {
    let code_prefix: String = get(row, "code_prefix")?;
    let code_suffix: String = get(row, "code_suffix")?;
    Ok(StoredAdminRedeemCode {
        id: get(row, "id")?,
        batch_id: get(row, "batch_id")?,
        batch_name: get(row, "batch_name")?,
        masked_code: mask_redeem_code(&code_prefix, &code_suffix),
        code_prefix,
        code_suffix,
        status: get(row, "status")?,
        redeemed_by_user_id: get(row, "redeemed_by_user_id")?,
        redeemed_by_user_name: get(row, "redeemed_by_user_name")?,
        redeemed_wallet_id: get(row, "redeemed_wallet_id")?,
        redeemed_payment_order_id: get(row, "redeemed_payment_order_id")?,
        redeemed_order_no: get(row, "redeemed_order_no")?,
        redeemed_at_unix_secs: optional_timestamp(
            get(row, "redeemed_at_unix_secs")?,
            "redeem_codes.redeemed_at",
        )?,
        disabled_by: get(row, "disabled_by")?,
        expires_at_unix_secs: optional_timestamp(
            get(row, "expires_at_unix_secs")?,
            "redeem_code_batches.expires_at",
        )?,
        created_at_unix_ms: timestamp(get(row, "created_at_unix_ms")?, "redeem_codes.created_at")?,
        updated_at_unix_secs: timestamp(
            get(row, "updated_at_unix_secs")?,
            "redeem_codes.updated_at",
        )?,
    })
}

fn map_daily_usage_row(row: &SqliteRow) -> Result<StoredWalletDailyUsageLedger, DataLayerError> {
    Ok(StoredWalletDailyUsageLedger {
        id: get(row, "id")?,
        billing_date: get(row, "billing_date")?,
        billing_timezone: get(row, "billing_timezone")?,
        total_cost_usd: sqlite_real(row, "total_cost_usd")?,
        total_requests: nonnegative_u64(
            get(row, "total_requests")?,
            "wallet_daily_usage_ledgers.total_requests",
        )?,
        input_tokens: nonnegative_u64(
            get(row, "input_tokens")?,
            "wallet_daily_usage_ledgers.input_tokens",
        )?,
        output_tokens: nonnegative_u64(
            get(row, "output_tokens")?,
            "wallet_daily_usage_ledgers.output_tokens",
        )?,
        cache_creation_tokens: nonnegative_u64(
            get(row, "cache_creation_tokens")?,
            "wallet_daily_usage_ledgers.cache_creation_tokens",
        )?,
        cache_read_tokens: nonnegative_u64(
            get(row, "cache_read_tokens")?,
            "wallet_daily_usage_ledgers.cache_read_tokens",
        )?,
        first_finalized_at_unix_secs: optional_timestamp(
            get(row, "first_finalized_at_unix_secs")?,
            "wallet_daily_usage_ledgers.first_finalized_at",
        )?,
        last_finalized_at_unix_secs: optional_timestamp(
            get(row, "last_finalized_at_unix_secs")?,
            "wallet_daily_usage_ledgers.last_finalized_at",
        )?,
        aggregated_at_unix_secs: optional_timestamp(
            get(row, "aggregated_at_unix_secs")?,
            "wallet_daily_usage_ledgers.aggregated_at",
        )?,
    })
}

fn get<T>(row: &SqliteRow, field: &str) -> Result<T, DataLayerError>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    row.try_get(field).map_sql_err()
}

fn read_count_row(row: SqliteRow) -> Result<u64, DataLayerError> {
    nonnegative_u64(get(&row, "total")?, "count total")
}

fn optional_json(
    value: Option<String>,
    field_name: &str,
) -> Result<Option<serde_json::Value>, DataLayerError> {
    value
        .map(|value| {
            serde_json::from_str(&value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "{field_name} contains invalid JSON: {err}"
                ))
            })
        })
        .transpose()
}

fn timestamp(value: i64, field_name: &str) -> Result<u64, DataLayerError> {
    u64::try_from(value).map_err(|_| {
        DataLayerError::UnexpectedValue(format!("{field_name} contains a negative timestamp"))
    })
}

fn optional_timestamp(value: Option<i64>, field_name: &str) -> Result<Option<u64>, DataLayerError> {
    value.map(|value| timestamp(value, field_name)).transpose()
}

fn nonnegative_u64(value: i64, field_name: &str) -> Result<u64, DataLayerError> {
    u64::try_from(value).map_err(|_| {
        DataLayerError::UnexpectedValue(format!("{field_name} contains a negative value"))
    })
}

fn mask_redeem_code(prefix: &str, suffix: &str) -> String {
    format!("{prefix}****{suffix}")
}

#[cfg(test)]
mod tests;
