use super::MysqlWalletReadRepository;
use crate::run_migrations;
use aether_data_contracts::repository::wallet::{
    AdminPaymentOrderListQuery, AdminRedeemCodeListQuery, AdminWalletListQuery, WalletLookupKey,
    WalletReadRepository,
};

#[tokio::test]
async fn mysql_wallet_read_repository_reads_wallet_contract_views() {
    let Some(database_url) = std::env::var("AETHER_TEST_MYSQL_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipping mysql wallet read smoke test because AETHER_TEST_MYSQL_URL is unset");
        return;
    };

    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("mysql pool should connect");
    run_migrations(&pool)
        .await
        .expect("mysql migrations should run");
    cleanup_rows(&pool).await;
    seed_rows(&pool).await;

    let repository = MysqlWalletReadRepository::new(pool);
    let wallet = repository
        .find(WalletLookupKey::UserId("user-1"))
        .await
        .expect("wallet find should query")
        .expect("wallet should exist");
    assert_eq!(wallet.total_adjusted, 3.0);

    let page = repository
        .list_admin_wallets(&AdminWalletListQuery {
            status: Some("active".to_string()),
            owner_type: Some("user".to_string()),
            limit: 10,
            offset: 0,
        })
        .await
        .expect("admin wallets should list");
    let wallet_item = page
        .items
        .iter()
        .find(|item| item.id == "wallet-1")
        .expect("seeded wallet should be listed");
    assert!(page.total >= 1);
    assert_eq!(wallet_item.total_adjusted, 3.0);

    let orders = repository
        .list_admin_payment_orders(&AdminPaymentOrderListQuery {
            status: Some("credited".to_string()),
            payment_method: Some("redeem_code".to_string()),
            limit: 10,
            offset: 0,
        })
        .await
        .expect("payment orders should list");
    assert_eq!(orders.total, 1);
    assert_eq!(
        orders.items[0].gateway_response.as_ref().unwrap()["ok"],
        true
    );

    let refunds = repository
        .list_admin_wallet_refunds("wallet-1", 10, 0)
        .await
        .expect("refunds should list");
    assert_eq!(refunds.total, 1);
    assert_eq!(
        refunds.items[0].payout_proof.as_ref().unwrap()["proof"],
        "ok"
    );

    let callbacks = repository
        .list_admin_payment_callbacks(Some("redeem_code"), 10, 0)
        .await
        .expect("callbacks should list");
    assert_eq!(callbacks.total, 1);
    assert!(callbacks.items[0].signature_valid);

    let codes = repository
        .list_admin_redeem_codes(&AdminRedeemCodeListQuery {
            batch_id: "batch-1".to_string(),
            status: Some("redeemed".to_string()),
            limit: 10,
            offset: 0,
        })
        .await
        .expect("redeem codes should list");
    assert_eq!(codes.total, 1);
    assert_eq!(codes.items[0].masked_code, "ABCD****WXYZ");

    let today = super::current_billing_date("UTC").expect("UTC should parse");
    sqlx::query("UPDATE wallet_daily_usage_ledgers SET billing_date = ? WHERE id = 'daily-1'")
        .bind(today)
        .execute(repository.pool())
        .await
        .expect("daily row should update");
    let daily = repository
        .find_wallet_today_usage("wallet-1", "UTC")
        .await
        .expect("daily usage should query")
        .expect("daily usage should exist");
    assert_eq!(daily.total_requests, 2);
}

impl MysqlWalletReadRepository {
    fn pool(&self) -> &sqlx::MySqlPool {
        &self.pool
    }
}

async fn cleanup_rows(pool: &sqlx::MySqlPool) {
    for sql in [
        "DELETE FROM wallet_daily_usage_ledgers WHERE id = 'daily-1'",
        "DELETE FROM redeem_codes WHERE id = 'code-1'",
        "DELETE FROM redeem_code_batches WHERE id = 'batch-1'",
        "DELETE FROM wallet_transactions WHERE id = 'tx-1'",
        "DELETE FROM refund_requests WHERE id = 'refund-1'",
        "DELETE FROM payment_callbacks WHERE id = 'callback-1'",
        "DELETE FROM payment_orders WHERE id = 'order-1'",
        "DELETE FROM wallets WHERE id = 'wallet-1'",
        "DELETE FROM users WHERE id = 'user-1'",
    ] {
        sqlx::query(sql)
            .execute(pool)
            .await
            .expect("cleanup should succeed");
    }
}

async fn seed_rows(pool: &sqlx::MySqlPool) {
    sqlx::query(
        r#"
INSERT INTO users (id, username, email, auth_source, created_at, updated_at)
VALUES ('user-1', 'Alice', 'alice@example.com', 'local', 1, 1)
"#,
    )
    .execute(pool)
    .await
    .expect("user should seed");

    sqlx::query(
        r#"
INSERT INTO wallets (
  id, user_id, balance, gift_balance, total_recharged, total_consumed,
  total_refunded, total_adjusted, created_at, updated_at
) VALUES (
  'wallet-1', 'user-1', 10.0, 2.0, 20.0, 4.0, 1.0, 3.0, 1, 2
)
"#,
    )
    .execute(pool)
    .await
    .expect("wallet should seed");

    sqlx::query(
        r#"
INSERT INTO payment_orders (
  id, order_no, wallet_id, user_id, amount_usd, refunded_amount_usd,
  refundable_amount_usd, payment_method, gateway_response, status, created_at
) VALUES (
  'order-1', 'order-no-1', 'wallet-1', 'user-1', 5.0, 1.0, 4.0,
  'redeem_code', '{"ok":true}', 'credited', 3
)
"#,
    )
    .execute(pool)
    .await
    .expect("payment order should seed");

    sqlx::query(
        r#"
INSERT INTO payment_callbacks (
  id, payment_order_id, payment_method, callback_key, order_no,
  signature_valid, payload, created_at
) VALUES (
  'callback-1', 'order-1', 'redeem_code', 'callback-key-1',
  'order-no-1', 1, '{"event":"paid"}', 4
)
"#,
    )
    .execute(pool)
    .await
    .expect("callback should seed");

    sqlx::query(
        r#"
INSERT INTO refund_requests (
  id, refund_no, wallet_id, user_id, payment_order_id, source_type,
  refund_mode, amount_usd, status, payout_proof, created_at, updated_at
) VALUES (
  'refund-1', 'refund-no-1', 'wallet-1', 'user-1', 'order-1',
  'payment_order', 'offline_payout', 1.0, 'completed',
  '{"proof":"ok"}', 5, 6
)
"#,
    )
    .execute(pool)
    .await
    .expect("refund should seed");

    sqlx::query(
        r#"
INSERT INTO wallet_transactions (
  id, wallet_id, category, reason_code, amount, balance_before,
  balance_after, recharge_balance_before, recharge_balance_after,
  gift_balance_before, gift_balance_after, created_at
) VALUES (
  'tx-1', 'wallet-1', 'credit', 'manual_adjustment', 3.0, 7.0, 10.0,
  5.0, 8.0, 2.0, 2.0, 7
)
"#,
    )
    .execute(pool)
    .await
    .expect("transaction should seed");

    sqlx::query(
        r#"
INSERT INTO redeem_code_batches (
  id, name, amount_usd, total_count, created_at, updated_at
) VALUES (
  'batch-1', 'Batch One', 5.0, 1, 8, 9
)
"#,
    )
    .execute(pool)
    .await
    .expect("redeem batch should seed");

    sqlx::query(
        r#"
INSERT INTO redeem_codes (
  id, batch_id, code_hash, code_prefix, code_suffix, status,
  redeemed_by_user_id, redeemed_wallet_id, redeemed_payment_order_id,
  redeemed_at, created_at, updated_at
) VALUES (
  'code-1', 'batch-1', 'hash-1', 'ABCD', 'WXYZ', 'redeemed',
  'user-1', 'wallet-1', 'order-1', 10, 8, 10
)
"#,
    )
    .execute(pool)
    .await
    .expect("redeem code should seed");

    sqlx::query(
        r#"
INSERT INTO wallet_daily_usage_ledgers (
  id, wallet_id, billing_date, billing_timezone, total_cost_usd,
  total_requests, input_tokens, output_tokens, cache_creation_tokens,
  cache_read_tokens, aggregated_at, created_at, updated_at
) VALUES (
  'daily-1', 'wallet-1', '2000-01-01', 'UTC', 1.25, 2, 10, 20, 3, 4, 11, 11, 11
)
"#,
    )
    .execute(pool)
    .await
    .expect("daily usage should seed");
}
