use super::SqliteWalletReadRepository;
use crate::run_migrations;
use aether_data_contracts::repository::wallet::{
    AdjustWalletBalanceInput, AdminPaymentOrderListQuery, AdminRedeemCodeListQuery,
    AdminWalletListQuery, CompleteAdminWalletRefundInput, CreateAdminRedeemCodeBatchInput,
    CreateManualWalletRechargeInput, CreatePlanPurchaseOrderInput, CreatePlanPurchaseOrderOutcome,
    CreateWalletRechargeOrderInput, CreateWalletRechargeOrderOutcome,
    CreateWalletRefundRequestInput, CreateWalletRefundRequestOutcome, CreditAdminPaymentOrderInput,
    DeleteAdminRedeemCodeBatchInput, DisableAdminRedeemCodeBatchInput, DisableAdminRedeemCodeInput,
    FailAdminWalletRefundInput, ProcessAdminWalletRefundInput, ProcessPaymentCallbackInput,
    ProcessPaymentCallbackOutcome, RedeemWalletCodeInput, RedeemWalletCodeOutcome, WalletLookupKey,
    WalletMutationOutcome, WalletReadRepository, WalletWriteRepository,
};
use serde_json::json;

#[tokio::test]
async fn sqlite_wallet_read_repository_reads_wallet_contract_views() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");
    seed_rows(&pool).await;

    let repository = SqliteWalletReadRepository::new(pool);
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
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].total_adjusted, 3.0);

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

#[tokio::test]
async fn sqlite_wallet_write_repository_handles_public_recharge_callback_and_refund() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");

    let repository = SqliteWalletReadRepository::new(pool);
    let order = match repository
        .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
            preferred_wallet_id: Some("wallet-write-1".to_string()),
            user_id: "user-write-1".to_string(),
            amount_usd: 12.5,
            pay_amount: Some(12.5),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            payment_method: "alipay".to_string(),
            payment_provider: None,
            payment_channel: None,
            gateway_order_id: "gateway-order-write-1".to_string(),
            gateway_response: json!({ "checkout": true }),
            order_no: "order-no-write-1".to_string(),
            expires_at_unix_secs: 4_102_444_800,
        })
        .await
        .expect("recharge order should be created")
    {
        CreateWalletRechargeOrderOutcome::Created(order) => order,
        CreateWalletRechargeOrderOutcome::WalletInactive => {
            panic!("new wallet should be active")
        }
    };
    assert_eq!(order.status, "pending");

    let callback = repository
        .process_payment_callback(ProcessPaymentCallbackInput {
            payment_method: "alipay".to_string(),
            payment_provider: None,
            payment_channel: None,
            callback_key: "callback-write-1".to_string(),
            order_no: Some("order-no-write-1".to_string()),
            gateway_order_id: Some("gateway-order-write-1".to_string()),
            amount_usd: 12.5,
            pay_amount: Some(12.5),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            payload_hash: "payload-hash-write-1".to_string(),
            payload: json!({ "status": "paid" }),
            signature_valid: true,
        })
        .await
        .expect("payment callback should process");
    let ProcessPaymentCallbackOutcome::Applied {
        wallet_id, order, ..
    } = callback
    else {
        panic!("callback should credit the order");
    };
    assert_eq!(wallet_id, "wallet-write-1");
    assert_eq!(order.status, "credited");

    let wallet = repository
        .find(WalletLookupKey::UserId("user-write-1"))
        .await
        .expect("wallet should query")
        .expect("wallet should exist");
    assert_eq!(wallet.balance, 12.5);
    assert_eq!(wallet.total_recharged, 12.5);

    let refund = repository
        .create_wallet_refund_request(CreateWalletRefundRequestInput {
            wallet_id: wallet.id.clone(),
            user_id: "user-write-1".to_string(),
            amount_usd: 4.0,
            payment_order_id: Some(order.id.clone()),
            source_type: None,
            source_id: None,
            refund_mode: None,
            reason: Some("requested".to_string()),
            idempotency_key: Some("idem-refund-write-1".to_string()),
            refund_no: "refund-no-write-1".to_string(),
        })
        .await
        .expect("refund request should create");
    let CreateWalletRefundRequestOutcome::Created(refund) = refund else {
        panic!("refund request should be created");
    };

    let duplicate = repository
        .create_wallet_refund_request(CreateWalletRefundRequestInput {
            wallet_id: wallet.id.clone(),
            user_id: "user-write-1".to_string(),
            amount_usd: 4.0,
            payment_order_id: Some(order.id.clone()),
            source_type: None,
            source_id: None,
            refund_mode: None,
            reason: Some("requested".to_string()),
            idempotency_key: Some("idem-refund-write-1".to_string()),
            refund_no: "refund-no-write-duplicate".to_string(),
        })
        .await
        .expect("duplicate refund request should resolve");
    assert!(matches!(
        duplicate,
        CreateWalletRefundRequestOutcome::Duplicate(_)
    ));

    let processed = repository
        .process_admin_wallet_refund(ProcessAdminWalletRefundInput {
            wallet_id: wallet.id.clone(),
            refund_id: refund.id.clone(),
            operator_id: Some("admin-1".to_string()),
        })
        .await
        .expect("refund should process");
    let WalletMutationOutcome::Applied((wallet, refund, transaction)) = processed else {
        panic!("refund should be processed");
    };
    assert_eq!(wallet.balance, 8.5);
    assert_eq!(refund.status, "processing");
    assert_eq!(transaction.reason_code, "refund_out");

    let completed = repository
        .complete_admin_wallet_refund(CompleteAdminWalletRefundInput {
            wallet_id: wallet.id.clone(),
            refund_id: refund.id.clone(),
            gateway_refund_id: Some("gateway-refund-write-1".to_string()),
            payout_reference: Some("payout-ref-write-1".to_string()),
            payout_proof: Some(json!({ "proof": "ok" })),
        })
        .await
        .expect("refund should complete");
    let WalletMutationOutcome::Applied(completed_refund) = completed else {
        panic!("refund should be completed");
    };
    assert_eq!(completed_refund.status, "succeeded");
    assert_eq!(
        completed_refund.payout_proof.as_ref().unwrap()["proof"],
        "ok"
    );

    let refund_to_fail = repository
        .create_wallet_refund_request(CreateWalletRefundRequestInput {
            wallet_id: wallet.id.clone(),
            user_id: "user-write-1".to_string(),
            amount_usd: 1.5,
            payment_order_id: Some(order.id.clone()),
            source_type: None,
            source_id: None,
            refund_mode: None,
            reason: Some("requested again".to_string()),
            idempotency_key: Some("idem-refund-write-2".to_string()),
            refund_no: "refund-no-write-2".to_string(),
        })
        .await
        .expect("second refund request should create");
    let CreateWalletRefundRequestOutcome::Created(refund_to_fail) = refund_to_fail else {
        panic!("second refund request should be created");
    };
    let processed_to_fail = repository
        .process_admin_wallet_refund(ProcessAdminWalletRefundInput {
            wallet_id: wallet.id.clone(),
            refund_id: refund_to_fail.id.clone(),
            operator_id: Some("admin-1".to_string()),
        })
        .await
        .expect("second refund should process");
    assert!(matches!(
        processed_to_fail,
        WalletMutationOutcome::Applied(_)
    ));
    let failed = repository
        .fail_admin_wallet_refund(FailAdminWalletRefundInput {
            wallet_id: wallet.id.clone(),
            refund_id: refund_to_fail.id.clone(),
            reason: "manual failure".to_string(),
            operator_id: Some("admin-1".to_string()),
        })
        .await
        .expect("second refund should fail");
    let WalletMutationOutcome::Applied((wallet, failed_refund, revert_transaction)) = failed else {
        panic!("second refund should fail with wallet restoration");
    };
    assert_eq!(wallet.balance, 8.5);
    assert_eq!(failed_refund.status, "failed");
    assert_eq!(
        revert_transaction.as_ref().unwrap().reason_code,
        "refund_revert"
    );

    let batch = repository
        .create_admin_redeem_code_batch(CreateAdminRedeemCodeBatchInput {
            name: "Write Batch".to_string(),
            amount_usd: 3.5,
            currency: "USD".to_string(),
            balance_bucket: "gift".to_string(),
            total_count: 1,
            expires_at_unix_secs: None,
            description: Some("write smoke".to_string()),
            created_by: Some("admin-1".to_string()),
        })
        .await
        .expect("redeem batch should create");
    assert_eq!(batch.batch.active_count, 1);
    let redeem_code = batch.codes[0].code.clone();

    let redeem = repository
        .redeem_wallet_code(RedeemWalletCodeInput {
            code: redeem_code,
            user_id: "user-write-1".to_string(),
            order_no: "redeem-order-write-1".to_string(),
        })
        .await
        .expect("redeem should apply");
    let RedeemWalletCodeOutcome::Redeemed {
        wallet,
        order,
        amount_usd,
        batch_name,
    } = redeem
    else {
        panic!("redeem should succeed");
    };
    assert_eq!(wallet.gift_balance, 3.5);
    assert_eq!(order.payment_method, "gift_code");
    assert_eq!(amount_usd, 3.5);
    assert_eq!(batch_name, "Write Batch");

    let disabled_batch = repository
        .create_admin_redeem_code_batch(CreateAdminRedeemCodeBatchInput {
            name: "Disabled Batch".to_string(),
            amount_usd: 1.25,
            currency: "USD".to_string(),
            balance_bucket: "gift".to_string(),
            total_count: 2,
            expires_at_unix_secs: None,
            description: Some("disable smoke".to_string()),
            created_by: Some("admin-1".to_string()),
        })
        .await
        .expect("disable batch should create");
    let disabled_code = repository
        .disable_admin_redeem_code(DisableAdminRedeemCodeInput {
            code_id: disabled_batch.codes[0].code_id.clone(),
            operator_id: Some("admin-1".to_string()),
        })
        .await
        .expect("redeem code should disable");
    let WalletMutationOutcome::Applied(disabled_code) = disabled_code else {
        panic!("redeem code should be disabled");
    };
    assert_eq!(disabled_code.status, "disabled");

    let disabled_batch = repository
        .disable_admin_redeem_code_batch(DisableAdminRedeemCodeBatchInput {
            batch_id: disabled_batch.batch.id.clone(),
            operator_id: Some("admin-1".to_string()),
        })
        .await
        .expect("redeem batch should disable");
    let WalletMutationOutcome::Applied(disabled_batch) = disabled_batch else {
        panic!("redeem batch should be disabled");
    };
    assert_eq!(disabled_batch.status, "disabled");
    assert_eq!(disabled_batch.active_count, 0);

    let deleted_batch = repository
        .delete_admin_redeem_code_batch(DeleteAdminRedeemCodeBatchInput {
            batch_id: disabled_batch.id,
            operator_id: Some("admin-1".to_string()),
        })
        .await
        .expect("disabled unredeemed batch should delete");
    assert!(matches!(deleted_batch, WalletMutationOutcome::Applied(_)));

    let (wallet, adjustment) = repository
        .adjust_wallet_balance(AdjustWalletBalanceInput {
            wallet_id: wallet.id.clone(),
            amount_usd: -2.0,
            balance_type: "gift".to_string(),
            operator_id: Some("admin-1".to_string()),
            description: Some("trim gift".to_string()),
        })
        .await
        .expect("adjustment should run")
        .expect("wallet should exist");
    assert_eq!(wallet.gift_balance, 1.5);
    assert_eq!(adjustment.reason_code, "adjust_admin");
    assert_eq!(
        adjustment.balance_after,
        wallet.balance + wallet.gift_balance
    );

    let (wallet, order) = repository
        .create_manual_wallet_recharge(CreateManualWalletRechargeInput {
            wallet_id: wallet.id,
            amount_usd: 5.0,
            payment_method: "admin_manual".to_string(),
            operator_id: Some("admin-1".to_string()),
            description: Some("manual topup".to_string()),
            order_no: "manual-order-write-1".to_string(),
        })
        .await
        .expect("manual recharge should run")
        .expect("wallet should exist");
    assert_eq!(wallet.balance, 13.5);
    assert_eq!(order.status, "credited");
    assert_eq!(order.payment_method, "admin_manual");

    let credit_order = match repository
        .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
            preferred_wallet_id: Some("wallet-credit-1".to_string()),
            user_id: "user-credit-1".to_string(),
            amount_usd: 2.25,
            pay_amount: None,
            pay_currency: None,
            exchange_rate: None,
            payment_method: "manual_gateway".to_string(),
            payment_provider: None,
            payment_channel: None,
            gateway_order_id: "gateway-order-credit-1".to_string(),
            gateway_response: json!({ "checkout": true }),
            order_no: "order-no-credit-1".to_string(),
            expires_at_unix_secs: 4_102_444_800,
        })
        .await
        .expect("credit order should be created")
    {
        CreateWalletRechargeOrderOutcome::Created(order) => order,
        CreateWalletRechargeOrderOutcome::WalletInactive => {
            panic!("new credit wallet should be active")
        }
    };
    let credited = repository
        .credit_admin_payment_order(CreditAdminPaymentOrderInput {
            order_id: credit_order.id.clone(),
            gateway_order_id: Some("gateway-order-credit-paid-1".to_string()),
            pay_amount: Some(2.25),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            gateway_response_patch: Some(json!({ "settled": true })),
            operator_id: Some("admin-1".to_string()),
        })
        .await
        .expect("credit order should apply");
    let WalletMutationOutcome::Applied((credited_order, applied)) = credited else {
        panic!("credit order should be applied");
    };
    assert!(applied);
    assert_eq!(credited_order.status, "credited");
    assert_eq!(
        credited_order.gateway_response.as_ref().unwrap()["manual_credit"],
        true
    );
    let credited_again = repository
        .credit_admin_payment_order(CreditAdminPaymentOrderInput {
            order_id: credit_order.id,
            gateway_order_id: None,
            pay_amount: None,
            pay_currency: None,
            exchange_rate: None,
            gateway_response_patch: None,
            operator_id: Some("admin-1".to_string()),
        })
        .await
        .expect("credit order should be idempotent");
    assert!(matches!(
        credited_again,
        WalletMutationOutcome::Applied((_, false))
    ));

    let expiring_order = match repository
        .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
            preferred_wallet_id: Some("wallet-expire-1".to_string()),
            user_id: "user-expire-1".to_string(),
            amount_usd: 1.0,
            pay_amount: None,
            pay_currency: None,
            exchange_rate: None,
            payment_method: "alipay".to_string(),
            payment_provider: None,
            payment_channel: None,
            gateway_order_id: "gateway-order-expire-1".to_string(),
            gateway_response: json!({ "checkout": true }),
            order_no: "order-no-expire-1".to_string(),
            expires_at_unix_secs: 4_102_444_800,
        })
        .await
        .expect("expiring order should be created")
    {
        CreateWalletRechargeOrderOutcome::Created(order) => order,
        CreateWalletRechargeOrderOutcome::WalletInactive => {
            panic!("new expire wallet should be active")
        }
    };
    let expired = repository
        .expire_admin_payment_order(&expiring_order.id)
        .await
        .expect("expire should run");
    assert!(matches!(expired, WalletMutationOutcome::Applied((_, true))));
    let expired_again = repository
        .expire_admin_payment_order(&expiring_order.id)
        .await
        .expect("expire should be idempotent");
    assert!(matches!(
        expired_again,
        WalletMutationOutcome::Applied((_, false))
    ));

    let failing_order = match repository
        .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
            preferred_wallet_id: Some("wallet-fail-1".to_string()),
            user_id: "user-fail-1".to_string(),
            amount_usd: 1.0,
            pay_amount: None,
            pay_currency: None,
            exchange_rate: None,
            payment_method: "alipay".to_string(),
            payment_provider: None,
            payment_channel: None,
            gateway_order_id: "gateway-order-fail-1".to_string(),
            gateway_response: json!({ "checkout": true }),
            order_no: "order-no-fail-1".to_string(),
            expires_at_unix_secs: 4_102_444_800,
        })
        .await
        .expect("failing order should be created")
    {
        CreateWalletRechargeOrderOutcome::Created(order) => order,
        CreateWalletRechargeOrderOutcome::WalletInactive => {
            panic!("new fail wallet should be active")
        }
    };
    let failed_order = repository
        .fail_admin_payment_order(&failing_order.id)
        .await
        .expect("fail should run");
    let WalletMutationOutcome::Applied(failed_order) = failed_order else {
        panic!("payment order should fail");
    };
    assert_eq!(failed_order.status, "failed");
}

#[tokio::test]
async fn sqlite_plan_purchase_blocks_duplicate_pending_active_period_order_and_manual_credit_fulfills(
) {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");

    let repository = SqliteWalletReadRepository::new(pool);
    sqlx::query(
            "INSERT INTO users (id, username, email, auth_source, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind("user-active-period-1")
        .bind("Active Period Buyer")
        .bind("active-period@example.com")
        .bind("local")
        .bind(1_i64)
        .bind(1_i64)
        .execute(repository.pool())
        .await
        .expect("user should seed");

    let _wallet_order = match repository
        .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
            preferred_wallet_id: Some("wallet-active-period-1".to_string()),
            user_id: "user-active-period-1".to_string(),
            amount_usd: 1.0,
            pay_amount: Some(1.0),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            payment_method: "bootstrap".to_string(),
            payment_provider: None,
            payment_channel: None,
            gateway_order_id: "gateway-bootstrap-active-period-1".to_string(),
            gateway_response: json!({ "bootstrap": true }),
            order_no: "order-bootstrap-active-period-1".to_string(),
            expires_at_unix_secs: 4_102_444_800,
        })
        .await
        .expect("wallet should be created")
    {
        CreateWalletRechargeOrderOutcome::Created(order) => order,
        CreateWalletRechargeOrderOutcome::WalletInactive => {
            panic!("new wallet should be active")
        }
    };

    let plan_snapshot = json!({
        "id": "active-period-plan",
        "title": "每日额度月卡",
        "duration_unit": "month",
        "duration_value": 1,
        "max_active_per_user": 1,
        "purchase_limit_scope": "active_period",
        "entitlements": [
            {
                "type": "daily_quota",
                "daily_quota_usd": 50.0,
                "reset_timezone": "Asia/Shanghai",
                "allow_wallet_overage": false
            }
        ]
    });
    sqlx::query(
        r#"
INSERT INTO billing_plans (
  id, title, price_amount, price_currency, duration_unit, duration_value,
  max_active_per_user, purchase_limit_scope, entitlements_json, created_at, updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
    )
    .bind("active-period-plan")
    .bind("每日额度月卡")
    .bind(100.0_f64)
    .bind("CNY")
    .bind("month")
    .bind(1_i64)
    .bind(1_i64)
    .bind("active_period")
    .bind(plan_snapshot["entitlements"].to_string())
    .bind(1_i64)
    .bind(1_i64)
    .execute(repository.pool())
    .await
    .expect("billing plan should seed");

    let first_order = match repository
        .create_plan_purchase_order(CreatePlanPurchaseOrderInput {
            preferred_wallet_id: None,
            user_id: "user-active-period-1".to_string(),
            amount_usd: 13.8,
            pay_amount: 100.0,
            pay_currency: "CNY".to_string(),
            exchange_rate: 7.24637681,
            payment_method: "alipay".to_string(),
            payment_provider: Some("epay".to_string()),
            payment_channel: Some("alipay".to_string()),
            gateway_order_id: "gateway-plan-active-period-1".to_string(),
            gateway_response: json!({ "checkout": true }),
            order_no: "order-plan-active-period-1".to_string(),
            product_id: "active-period-plan".to_string(),
            product_snapshot: plan_snapshot.clone(),
            expires_at_unix_secs: 4_102_444_800,
        })
        .await
        .expect("first active period order should create")
    {
        CreatePlanPurchaseOrderOutcome::Created(order) => order,
        other => panic!("first active period order should be created, got {other:?}"),
    };
    let duplicate_pending = repository
        .create_plan_purchase_order(CreatePlanPurchaseOrderInput {
            preferred_wallet_id: None,
            user_id: "user-active-period-1".to_string(),
            amount_usd: 13.8,
            pay_amount: 100.0,
            pay_currency: "CNY".to_string(),
            exchange_rate: 7.24637681,
            payment_method: "alipay".to_string(),
            payment_provider: Some("epay".to_string()),
            payment_channel: Some("alipay".to_string()),
            gateway_order_id: "gateway-plan-active-period-2".to_string(),
            gateway_response: json!({ "checkout": true }),
            order_no: "order-plan-active-period-2".to_string(),
            product_id: "active-period-plan".to_string(),
            product_snapshot: plan_snapshot.clone(),
            expires_at_unix_secs: 4_102_444_800,
        })
        .await
        .expect("duplicate active period order should resolve");
    assert!(matches!(
        duplicate_pending,
        CreatePlanPurchaseOrderOutcome::ActivePlanLimitReached
    ));

    let credited = repository
        .credit_admin_payment_order(CreditAdminPaymentOrderInput {
            order_id: first_order.id.clone(),
            gateway_order_id: Some("gateway-plan-active-period-paid-1".to_string()),
            pay_amount: Some(100.0),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.24637681),
            gateway_response_patch: Some(json!({ "settled": true })),
            operator_id: Some("admin-1".to_string()),
        })
        .await
        .expect("manual plan credit should run");
    let WalletMutationOutcome::Applied((credited_order, applied)) = credited else {
        panic!("manual plan credit should be applied");
    };
    assert!(applied);
    assert_eq!(credited_order.status, "credited");
    assert_eq!(credited_order.refundable_amount_usd, 0.0);

    let entitlement_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_plan_entitlements WHERE user_id = ? AND plan_id = ?",
    )
    .bind("user-active-period-1")
    .bind("active-period-plan")
    .fetch_one(repository.pool())
    .await
    .expect("entitlement count should query");
    assert_eq!(entitlement_count, 1);

    let wallet_balance: f64 = sqlx::query_scalar("SELECT balance FROM wallets WHERE id = ?")
        .bind("wallet-active-period-1")
        .fetch_one(repository.pool())
        .await
        .expect("wallet balance should query");
    assert_eq!(wallet_balance, 0.0);
}

#[tokio::test]
async fn sqlite_finds_reusable_pending_plan_purchase_order() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");

    let repository = SqliteWalletReadRepository::new(pool);
    sqlx::query(
            "INSERT INTO users (id, username, email, auth_source, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind("user-pending-plan-1")
        .bind("Pending Buyer")
        .bind("pending-plan@example.com")
        .bind("local")
        .bind(1_i64)
        .bind(1_i64)
        .execute(repository.pool())
        .await
        .expect("user should seed");

    let _wallet_order = match repository
        .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
            preferred_wallet_id: Some("wallet-pending-plan-1".to_string()),
            user_id: "user-pending-plan-1".to_string(),
            amount_usd: 1.0,
            pay_amount: Some(1.0),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            payment_method: "bootstrap".to_string(),
            payment_provider: None,
            payment_channel: None,
            gateway_order_id: "gateway-bootstrap-pending-plan-1".to_string(),
            gateway_response: json!({ "bootstrap": true }),
            order_no: "order-bootstrap-pending-plan-1".to_string(),
            expires_at_unix_secs: 4_102_444_800,
        })
        .await
        .expect("wallet should be created")
    {
        CreateWalletRechargeOrderOutcome::Created(order) => order,
        CreateWalletRechargeOrderOutcome::WalletInactive => {
            panic!("new wallet should be active")
        }
    };

    let plan_snapshot = json!({
        "id": "pending-plan",
        "title": "每日额度月卡",
        "duration_unit": "month",
        "duration_value": 1,
        "max_active_per_user": 1,
        "purchase_limit_scope": "active_period",
        "entitlements": [
            {
                "type": "daily_quota",
                "daily_quota_usd": 50.0,
                "reset_timezone": "Asia/Shanghai",
                "allow_wallet_overage": false
            }
        ]
    });
    let pending_order = match repository
        .create_plan_purchase_order(CreatePlanPurchaseOrderInput {
            preferred_wallet_id: None,
            user_id: "user-pending-plan-1".to_string(),
            amount_usd: 13.8,
            pay_amount: 100.0,
            pay_currency: "CNY".to_string(),
            exchange_rate: 7.24637681,
            payment_method: "alipay".to_string(),
            payment_provider: Some("epay".to_string()),
            payment_channel: Some("alipay".to_string()),
            gateway_order_id: "gateway-pending-plan-1".to_string(),
            gateway_response: json!({ "checkout": true }),
            order_no: "order-pending-plan-1".to_string(),
            product_id: "pending-plan".to_string(),
            product_snapshot: plan_snapshot.clone(),
            expires_at_unix_secs: 4_102_444_800,
        })
        .await
        .expect("pending plan order should create")
    {
        CreatePlanPurchaseOrderOutcome::Created(order) => order,
        other => panic!("pending plan order should be created, got {other:?}"),
    };
    let now = chrono::Utc::now().timestamp().max(0);
    for (id, order_no, status, product_id, user_id, expires_at, created_at) in [
        (
            "expired-pending-plan-order",
            "order-expired-pending-plan",
            "pending",
            "pending-plan",
            "user-pending-plan-1",
            now - 10,
            now + 10,
        ),
        (
            "credited-pending-plan-order",
            "order-credited-pending-plan",
            "credited",
            "pending-plan",
            "user-pending-plan-1",
            now + 3_600,
            now + 20,
        ),
        (
            "other-user-pending-plan-order",
            "order-other-user-pending-plan",
            "pending",
            "pending-plan",
            "other-user",
            now + 3_600,
            now + 30,
        ),
    ] {
        sqlx::query(
            r#"
INSERT INTO payment_orders (
  id, order_no, wallet_id, user_id, amount_usd, pay_amount, pay_currency,
  exchange_rate, refunded_amount_usd, refundable_amount_usd, payment_method,
  payment_provider, payment_channel, order_kind, product_id, product_snapshot,
  fulfillment_status, gateway_order_id, gateway_response, status, created_at, expires_at
) VALUES (?, ?, ?, ?, 13.8, 100.0, 'CNY', 7.24637681, 0, 0, 'alipay',
  'epay', 'alipay', 'plan_purchase', ?, ?, 'pending', ?, ?, ?, ?, ?)
                "#,
        )
        .bind(id)
        .bind(order_no)
        .bind("wallet-pending-plan-1")
        .bind(user_id)
        .bind(product_id)
        .bind(plan_snapshot.to_string())
        .bind(format!("gateway-{id}"))
        .bind(json!({ "checkout": id }).to_string())
        .bind(status)
        .bind(created_at)
        .bind(expires_at)
        .execute(repository.pool())
        .await
        .expect("extra payment order should seed");
    }

    let found = repository
        .find_pending_plan_purchase_order_by_user_id("user-pending-plan-1", "pending-plan")
        .await
        .expect("pending plan lookup should run")
        .expect("pending plan order should be found");
    assert_eq!(found.id, pending_order.id);
    assert_eq!(
        repository
            .find_pending_plan_purchase_order_by_user_id("user-pending-plan-1", "missing-plan")
            .await
            .expect("missing plan lookup should run"),
        None
    );
    assert_eq!(
        repository
            .find_pending_plan_purchase_order_by_user_id("missing-user", "pending-plan")
            .await
            .expect("missing user lookup should run"),
        None
    );
}

#[tokio::test]
async fn sqlite_plan_purchase_replaces_same_class_entitlements_on_manual_credit() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");

    let repository = SqliteWalletReadRepository::new(pool);
    sqlx::query(
            "INSERT INTO users (id, username, email, auth_source, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind("user-upgrade-1")
        .bind("Upgrade Buyer")
        .bind("upgrade@example.com")
        .bind("local")
        .bind(1_i64)
        .bind(1_i64)
        .execute(repository.pool())
        .await
        .expect("user should seed");

    let _wallet_order = match repository
        .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
            preferred_wallet_id: Some("wallet-upgrade-1".to_string()),
            user_id: "user-upgrade-1".to_string(),
            amount_usd: 1.0,
            pay_amount: Some(1.0),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            payment_method: "bootstrap".to_string(),
            payment_provider: None,
            payment_channel: None,
            gateway_order_id: "gateway-bootstrap-upgrade-1".to_string(),
            gateway_response: json!({ "bootstrap": true }),
            order_no: "order-bootstrap-upgrade-1".to_string(),
            expires_at_unix_secs: 4_102_444_800,
        })
        .await
        .expect("wallet should be created")
    {
        CreateWalletRechargeOrderOutcome::Created(order) => order,
        CreateWalletRechargeOrderOutcome::WalletInactive => {
            panic!("new wallet should be active")
        }
    };

    let low_snapshot = json!({
        "id": "pro-basic",
        "title": "Pro Basic",
        "duration_unit": "month",
        "duration_value": 1,
        "max_active_per_user": 1,
        "purchase_limit_scope": "active_period",
        "entitlements": [{"type": "daily_quota", "daily_quota_usd": 10.0}]
    });
    let high_snapshot = json!({
        "id": "pro-plus",
        "title": "Pro Plus",
        "duration_unit": "month",
        "duration_value": 1,
        "max_active_per_user": 1,
        "purchase_limit_scope": "active_period",
        "entitlements": [{"type": "daily_quota", "daily_quota_usd": 50.0}]
    });
    for (id, title, snapshot) in [
        ("pro-basic", "Pro Basic", &low_snapshot),
        ("pro-plus", "Pro Plus", &high_snapshot),
    ] {
        sqlx::query(
            r#"
INSERT INTO billing_plans (
  id, title, price_amount, price_currency, duration_unit, duration_value,
  max_active_per_user, purchase_limit_scope, entitlements_json, created_at, updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
        )
        .bind(id)
        .bind(title)
        .bind(100.0_f64)
        .bind("CNY")
        .bind("month")
        .bind(1_i64)
        .bind(1_i64)
        .bind("active_period")
        .bind(snapshot["entitlements"].to_string())
        .bind(1_i64)
        .bind(1_i64)
        .execute(repository.pool())
        .await
        .expect("billing plan should seed");
    }

    let low_order = match repository
        .create_plan_purchase_order(CreatePlanPurchaseOrderInput {
            preferred_wallet_id: None,
            user_id: "user-upgrade-1".to_string(),
            amount_usd: 13.8,
            pay_amount: 100.0,
            pay_currency: "CNY".to_string(),
            exchange_rate: 7.24637681,
            payment_method: "alipay".to_string(),
            payment_provider: Some("epay".to_string()),
            payment_channel: Some("alipay".to_string()),
            gateway_order_id: "gateway-pro-basic-1".to_string(),
            gateway_response: json!({ "checkout": true }),
            order_no: "order-pro-basic-1".to_string(),
            product_id: "pro-basic".to_string(),
            product_snapshot: low_snapshot,
            expires_at_unix_secs: 4_102_444_800,
        })
        .await
        .expect("low order should create")
    {
        CreatePlanPurchaseOrderOutcome::Created(order) => order,
        other => panic!("low order should be created, got {other:?}"),
    };
    let WalletMutationOutcome::Applied((_, true)) = repository
        .credit_admin_payment_order(CreditAdminPaymentOrderInput {
            order_id: low_order.id,
            gateway_order_id: Some("gateway-pro-basic-paid-1".to_string()),
            pay_amount: Some(100.0),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.24637681),
            gateway_response_patch: Some(json!({ "settled": true })),
            operator_id: Some("admin-1".to_string()),
        })
        .await
        .expect("low plan credit should run")
    else {
        panic!("low plan credit should apply");
    };

    let high_order = match repository
        .create_plan_purchase_order(CreatePlanPurchaseOrderInput {
            preferred_wallet_id: None,
            user_id: "user-upgrade-1".to_string(),
            amount_usd: 13.8,
            pay_amount: 100.0,
            pay_currency: "CNY".to_string(),
            exchange_rate: 7.24637681,
            payment_method: "alipay".to_string(),
            payment_provider: Some("epay".to_string()),
            payment_channel: Some("alipay".to_string()),
            gateway_order_id: "gateway-pro-plus-1".to_string(),
            gateway_response: json!({ "checkout": true }),
            order_no: "order-pro-plus-1".to_string(),
            product_id: "pro-plus".to_string(),
            product_snapshot: high_snapshot,
            expires_at_unix_secs: 4_102_444_800,
        })
        .await
        .expect("high order should create")
    {
        CreatePlanPurchaseOrderOutcome::Created(order) => order,
        other => panic!("high order should be created, got {other:?}"),
    };
    let WalletMutationOutcome::Applied((_, true)) = repository
        .credit_admin_payment_order(CreditAdminPaymentOrderInput {
            order_id: high_order.id,
            gateway_order_id: Some("gateway-pro-plus-paid-1".to_string()),
            pay_amount: Some(100.0),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.24637681),
            gateway_response_patch: Some(json!({ "settled": true })),
            operator_id: Some("admin-1".to_string()),
        })
        .await
        .expect("high plan credit should run")
    else {
        panic!("high plan credit should apply");
    };

    let low_status: String = sqlx::query_scalar(
        "SELECT status FROM user_plan_entitlements WHERE user_id = ? AND plan_id = ?",
    )
    .bind("user-upgrade-1")
    .bind("pro-basic")
    .fetch_one(repository.pool())
    .await
    .expect("low entitlement status should query");
    assert_eq!(low_status, "replaced");
    let active_high_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM user_plan_entitlements WHERE user_id = ? AND plan_id = ? AND status = 'active'",
        )
        .bind("user-upgrade-1")
        .bind("pro-plus")
        .fetch_one(repository.pool())
        .await
        .expect("high entitlement count should query");
    assert_eq!(active_high_count, 1);
}

#[tokio::test]
async fn sqlite_plan_purchase_respects_lifetime_purchase_limit() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite pool should connect");
    run_migrations(&pool)
        .await
        .expect("sqlite migrations should run");

    let repository = SqliteWalletReadRepository::new(pool);
    sqlx::query(
            "INSERT INTO users (id, username, email, auth_source, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind("user-lifetime-1")
        .bind("Lifetime Buyer")
        .bind("lifetime@example.com")
        .bind("local")
        .bind(1_i64)
        .bind(1_i64)
        .execute(repository.pool())
        .await
        .expect("user should seed");

    let _wallet_order = match repository
        .create_wallet_recharge_order(CreateWalletRechargeOrderInput {
            preferred_wallet_id: Some("wallet-lifetime-1".to_string()),
            user_id: "user-lifetime-1".to_string(),
            amount_usd: 1.0,
            pay_amount: Some(1.0),
            pay_currency: Some("USD".to_string()),
            exchange_rate: Some(1.0),
            payment_method: "bootstrap".to_string(),
            payment_provider: None,
            payment_channel: None,
            gateway_order_id: "gateway-bootstrap-lifetime-1".to_string(),
            gateway_response: json!({ "bootstrap": true }),
            order_no: "order-bootstrap-lifetime-1".to_string(),
            expires_at_unix_secs: 4_102_444_800,
        })
        .await
        .expect("wallet should be created")
    {
        CreateWalletRechargeOrderOutcome::Created(order) => order,
        CreateWalletRechargeOrderOutcome::WalletInactive => {
            panic!("new wallet should be active")
        }
    };

    let plan_snapshot = json!({
        "id": "first-plan",
        "title": "首购特惠包",
        "duration_unit": "month",
        "duration_value": 1,
        "max_active_per_user": 1,
        "purchase_limit_scope": "lifetime",
        "entitlements": [
            {
                "type": "wallet_credit",
                "amount_usd": 1.0,
                "balance_bucket": "gift"
            }
        ]
    });
    sqlx::query(
        r#"
INSERT INTO billing_plans (
  id, title, price_amount, price_currency, duration_unit, duration_value,
  max_active_per_user, purchase_limit_scope, entitlements_json, created_at, updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
    )
    .bind("first-plan")
    .bind("首购特惠包")
    .bind(7.2_f64)
    .bind("CNY")
    .bind("month")
    .bind(1_i64)
    .bind(1_i64)
    .bind("lifetime")
    .bind(plan_snapshot["entitlements"].to_string())
    .bind(1_i64)
    .bind(1_i64)
    .execute(repository.pool())
    .await
    .expect("billing plan should seed");

    let first_order = match repository
        .create_plan_purchase_order(CreatePlanPurchaseOrderInput {
            preferred_wallet_id: None,
            user_id: "user-lifetime-1".to_string(),
            amount_usd: 1.0,
            pay_amount: 7.2,
            pay_currency: "CNY".to_string(),
            exchange_rate: 7.2,
            payment_method: "alipay".to_string(),
            payment_provider: Some("epay".to_string()),
            payment_channel: Some("alipay".to_string()),
            gateway_order_id: "gateway-plan-lifetime-1".to_string(),
            gateway_response: json!({ "checkout": true }),
            order_no: "order-plan-lifetime-1".to_string(),
            product_id: "first-plan".to_string(),
            product_snapshot: plan_snapshot.clone(),
            expires_at_unix_secs: 4_102_444_800,
        })
        .await
        .expect("first plan order should create")
    {
        CreatePlanPurchaseOrderOutcome::Created(order) => order,
        other => panic!("first plan order should be created, got {other:?}"),
    };
    assert_eq!(first_order.status, "pending");

    let callback = repository
        .process_payment_callback(ProcessPaymentCallbackInput {
            payment_method: "alipay".to_string(),
            payment_provider: Some("epay".to_string()),
            payment_channel: Some("alipay".to_string()),
            callback_key: "callback-plan-lifetime-1".to_string(),
            order_no: Some("order-plan-lifetime-1".to_string()),
            gateway_order_id: Some("gateway-plan-lifetime-1".to_string()),
            amount_usd: 1.0,
            pay_amount: Some(7.2),
            pay_currency: Some("CNY".to_string()),
            exchange_rate: Some(7.2),
            payload_hash: "payload-plan-lifetime-1".to_string(),
            payload: json!({ "trade_status": "TRADE_SUCCESS" }),
            signature_valid: true,
        })
        .await
        .expect("plan payment callback should process");
    assert!(matches!(
        callback,
        ProcessPaymentCallbackOutcome::Applied { .. }
    ));

    let entitlement_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_plan_entitlements WHERE user_id = ? AND plan_id = ?",
    )
    .bind("user-lifetime-1")
    .bind("first-plan")
    .fetch_one(repository.pool())
    .await
    .expect("entitlement count should query");
    assert_eq!(entitlement_count, 1);

    let second_order = repository
        .create_plan_purchase_order(CreatePlanPurchaseOrderInput {
            preferred_wallet_id: None,
            user_id: "user-lifetime-1".to_string(),
            amount_usd: 1.0,
            pay_amount: 7.2,
            pay_currency: "CNY".to_string(),
            exchange_rate: 7.2,
            payment_method: "alipay".to_string(),
            payment_provider: Some("epay".to_string()),
            payment_channel: Some("alipay".to_string()),
            gateway_order_id: "gateway-plan-lifetime-2".to_string(),
            gateway_response: json!({ "checkout": true }),
            order_no: "order-plan-lifetime-2".to_string(),
            product_id: "first-plan".to_string(),
            product_snapshot: plan_snapshot.clone(),
            expires_at_unix_secs: 4_102_444_800,
        })
        .await
        .expect("second plan order should resolve");
    assert!(matches!(
        second_order,
        CreatePlanPurchaseOrderOutcome::ActivePlanLimitReached
    ));

    let unlimited_snapshot = json!({
        "id": "unlimited-plan",
        "title": "不限购余额包",
        "duration_unit": "month",
        "duration_value": 1,
        "max_active_per_user": 1,
        "purchase_limit_scope": "unlimited",
        "entitlements": [
            {
                "type": "wallet_credit",
                "amount_usd": 1.0,
                "balance_bucket": "gift"
            }
        ]
    });
    sqlx::query(
        r#"
INSERT INTO billing_plans (
  id, title, price_amount, price_currency, duration_unit, duration_value,
  max_active_per_user, purchase_limit_scope, entitlements_json, created_at, updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
    )
    .bind("unlimited-plan")
    .bind("不限购余额包")
    .bind(7.2_f64)
    .bind("CNY")
    .bind("month")
    .bind(1_i64)
    .bind(1_i64)
    .bind("unlimited")
    .bind(unlimited_snapshot["entitlements"].to_string())
    .bind(1_i64)
    .bind(1_i64)
    .execute(repository.pool())
    .await
    .expect("unlimited billing plan should seed");

    for index in 1..=2 {
        let order_no = format!("order-plan-unlimited-{index}");
        let gateway_order_id = format!("gateway-plan-unlimited-{index}");
        let order = match repository
            .create_plan_purchase_order(CreatePlanPurchaseOrderInput {
                preferred_wallet_id: None,
                user_id: "user-lifetime-1".to_string(),
                amount_usd: 1.0,
                pay_amount: 7.2,
                pay_currency: "CNY".to_string(),
                exchange_rate: 7.2,
                payment_method: "alipay".to_string(),
                payment_provider: Some("epay".to_string()),
                payment_channel: Some("alipay".to_string()),
                gateway_order_id: gateway_order_id.clone(),
                gateway_response: json!({ "checkout": true }),
                order_no: order_no.clone(),
                product_id: "unlimited-plan".to_string(),
                product_snapshot: unlimited_snapshot.clone(),
                expires_at_unix_secs: 4_102_444_800,
            })
            .await
            .expect("unlimited plan order should create")
        {
            CreatePlanPurchaseOrderOutcome::Created(order) => order,
            other => panic!("unlimited plan order should be created, got {other:?}"),
        };
        assert_eq!(order.status, "pending");

        let callback = repository
            .process_payment_callback(ProcessPaymentCallbackInput {
                payment_method: "alipay".to_string(),
                payment_provider: Some("epay".to_string()),
                payment_channel: Some("alipay".to_string()),
                callback_key: format!("callback-plan-unlimited-{index}"),
                order_no: Some(order_no),
                gateway_order_id: Some(gateway_order_id),
                amount_usd: 1.0,
                pay_amount: Some(7.2),
                pay_currency: Some("CNY".to_string()),
                exchange_rate: Some(7.2),
                payload_hash: format!("payload-plan-unlimited-{index}"),
                payload: json!({ "trade_status": "TRADE_SUCCESS" }),
                signature_valid: true,
            })
            .await
            .expect("unlimited plan payment callback should process");
        assert!(matches!(
            callback,
            ProcessPaymentCallbackOutcome::Applied { .. }
        ));
    }

    let unlimited_entitlement_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM user_plan_entitlements WHERE user_id = ? AND plan_id = ?",
    )
    .bind("user-lifetime-1")
    .bind("unlimited-plan")
    .fetch_one(repository.pool())
    .await
    .expect("unlimited entitlement count should query");
    assert_eq!(unlimited_entitlement_count, 2);
}

impl SqliteWalletReadRepository {
    fn pool(&self) -> &sqlx::SqlitePool {
        &self.pool
    }
}

async fn seed_rows(pool: &sqlx::SqlitePool) {
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
