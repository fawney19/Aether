use super::*;
use crate::repository::{users::StoredUserAuthRecord, wallet::StoredWalletSnapshot};

#[cfg(feature = "postgres")]
async fn bind_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    invitee: &str,
    code: Option<&str>,
    source: Option<serde_json::Value>,
    config: Option<ReferralRewardConfig>,
    email_verified: bool,
    email_verification_required: bool,
) -> Result<Option<ReferralRelationshipRecord>, DataLayerError> {
    let Some(raw) = code.filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    let Some(code) = normalize_referral_code(raw) else {
        return Err(DataLayerError::InvalidInput("邀请码无效".into()));
    };
    let inviter=sqlx::query_scalar::<_,String>("SELECT c.user_id FROM user_invite_codes c JOIN users u ON u.id=c.user_id WHERE c.invite_code=$1 AND c.active=TRUE AND u.is_active=TRUE AND u.is_deleted=FALSE FOR SHARE OF c,u")
        .bind(&code).fetch_optional(&mut **tx).await.map_err(DataLayerError::postgres)?.ok_or_else(||DataLayerError::InvalidInput("邀请码无效".into()))?;
    if inviter == invitee {
        return Err(DataLayerError::InvalidInput(
            "不能使用自己的邀请码注册".into(),
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let mut source = match source {
        Some(serde_json::Value::Object(v)) => v,
        Some(v) => serde_json::Map::from_iter([("registration_source".to_string(), v)]),
        None => serde_json::Map::new(),
    };
    source.insert(REFERRAL_SNAPSHOT_KEY.into(),serde_json::json!({"config":config,"email_verified":email_verified,"email_verification_required":email_verification_required}));
    let source = serde_json::Value::Object(source);
    let row=sqlx::query("INSERT INTO user_referrals(id,inviter_user_id,invitee_user_id,invite_code_snapshot,source_json,created_at,updated_at) VALUES($1,$2,$3,$4,$5,NOW(),NOW()) ON CONFLICT(invitee_user_id) DO NOTHING RETURNING EXTRACT(EPOCH FROM created_at)::BIGINT AS created_at_unix_secs")
        .bind(&id).bind(&inviter).bind(invitee).bind(&code).bind(&source).fetch_optional(&mut **tx).await.map_err(DataLayerError::postgres)?;
    let Some(row) = row else {
        return Ok(None);
    };
    if let Some(config) = config {
        if config.headcount_enabled
            && (config.headcount_trigger == "registration"
                || (config.headcount_trigger == "email_verified"
                    && email_verified
                    && email_verification_required))
        {
            insert_registration_obligation(tx, &id, &inviter, invitee, &config).await?;
        }
    }
    Ok(Some(ReferralRelationshipRecord {
        id,
        inviter_user_id: inviter,
        inviter_username: None,
        invitee_user_id: invitee.to_string(),
        invitee_username: None,
        invite_code_snapshot: code,
        first_paid_order_id: None,
        first_paid_at_unix_secs: None,
        source: Some(source),
        created_at_unix_secs: row_unix_secs(&row, "created_at_unix_secs")?,
    }))
}

#[cfg(all(test, feature = "postgres"))]
mod postgres_tests {
    use super::*;
    async fn isolated_backend() -> DataBackends {
        let backend = crate::backend::PostgresBackend::from_config(
            crate::driver::postgres::PostgresPoolConfig {
                database_url: std::env::var("AETHER_TEST_DATABASE_URL")
                    .expect("explicit test database required"),
                min_connections: 0,
                max_connections: 1,
                idle_timeout_ms: 86_400_000,
                max_lifetime_ms: 86_400_000,
                ..Default::default()
            },
        )
        .unwrap();
        let pool = backend.pool_clone();
        sqlx::query("SET search_path=pg_temp,public")
            .execute(&pool)
            .await
            .unwrap();
        for table in [
            "users",
            "wallets",
            "wallet_transactions",
            "system_configs",
            "user_invite_codes",
            "user_referrals",
            "referral_rewards",
            "payment_orders",
            "refund_requests",
            "user_group_members",
        ] {
            sqlx::query(&format!(
                "CREATE TEMP TABLE {table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .execute(&pool)
            .await
            .unwrap();
        }
        for (key, value) in [
            ("referral_enabled", serde_json::json!(true)),
            ("referral_reward_mode", serde_json::json!("headcount")),
            (
                "referral_headcount_trigger",
                serde_json::json!("registration"),
            ),
            ("referral_headcount_amount_usd", serde_json::json!(3)),
        ] {
            sqlx::query("INSERT INTO system_configs(id,key,value) VALUES($1,$2,$3)")
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(key)
                .bind(value)
                .execute(&pool)
                .await
                .unwrap();
        }
        let mut backends = DataBackends::default();
        backends.postgres = Some(backend);
        backends
    }

    #[tokio::test]
    #[ignore = "requires explicit AETHER_TEST_DATABASE_URL; only session-local temp tables are mutated"]
    async fn admin_referral_reads_paginate_debt_and_preserve_historical_facts() {
        let backends = isolated_backend().await;
        let pool = backends.postgres().unwrap().pool_clone();
        let state = ReferralDataState::new(Some(&backends));
        let inviter = uuid::Uuid::new_v4().to_string();
        let invitee = uuid::Uuid::new_v4().to_string();
        let wallet = uuid::Uuid::new_v4().to_string();
        let order = uuid::Uuid::new_v4().to_string();
        let relationship = uuid::Uuid::new_v4().to_string();
        for (id, name) in [(&inviter, "Inviter%Literal"), (&invitee, "Invitee")] {
            sqlx::query("INSERT INTO users(id,username) VALUES($1,$2)")
                .bind(id)
                .bind(name)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO wallets(id,user_id) VALUES($1,$2)")
            .bind(&wallet)
            .bind(&inviter)
            .execute(&pool)
            .await
            .unwrap();
        let config = ReferralRewardConfig {
            percent_enabled: true,
            percent_rate: 5.0,
            headcount_enabled: false,
            headcount_amount_usd: 0.0,
            headcount_trigger: "registration".into(),
        };
        let snapshot =
            serde_json::json!({(REFERRAL_SNAPSHOT_KEY):{"config":config},"secret":"gateway-key"});
        sqlx::query("INSERT INTO payment_orders(id,order_no,wallet_id,user_id,amount_usd,payment_method,status,gateway_response,refunded_amount_usd,refundable_amount_usd,created_at,credited_at) VALUES($1,'READ-ORDER',$2,$3,100,'stripe','credited',$4,40,60,NOW(),NOW())").bind(&order).bind(&wallet).bind(&invitee).bind(snapshot).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO user_referrals(id,inviter_user_id,invitee_user_id,invite_code_snapshot,source_json) VALUES($1,$2,$3,'AE-READ',$4)").bind(&relationship).bind(&inviter).bind(&invitee).bind("{\"secret\":\"private-source\"}").execute(&pool).await.unwrap();
        let mut ids = Vec::new();
        for index in 0..24 {
            let id = uuid::Uuid::new_v4().to_string();
            let status = if index == 22 {
                "applied"
            } else if index == 23 {
                "reversed"
            } else {
                "failed"
            };
            sqlx::query("INSERT INTO referral_rewards(id,referral_id,inviter_user_id,invitee_user_id,reward_type,source_order_id,trigger_point,amount_usd,status,idempotency_key,pending_reversal_amount_usd,reversed_amount_usd) VALUES($1,$2,$3,$4,'percent',$5,'paid_order',10,$6,$1,$7,$8)").bind(&id).bind(&relationship).bind(&inviter).bind(&invitee).bind(&order).bind(status).bind(if index<22 {2.0} else {0.0}).bind(if index==23 {10.0} else {0.0}).execute(&pool).await.unwrap();
            if index >= 22 {
                let tx = uuid::Uuid::new_v4().to_string();
                sqlx::query("INSERT INTO wallet_transactions(id,wallet_id,category,reason_code,amount,balance_before,balance_after,recharge_balance_before,recharge_balance_after,gift_balance_before,gift_balance_after,link_type,link_id,created_at) VALUES($1,$2,'adjust','referral_reward',10,0,10,0,0,0,10,'referral_reward',$3,NOW())").bind(&tx).bind(&wallet).bind(&id).execute(&pool).await.unwrap();
                sqlx::query("UPDATE referral_rewards SET wallet_transaction_id=$2 WHERE id=$1")
                    .bind(&id)
                    .bind(tx)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            ids.push(id);
        }
        let query = ReferralRewardListQuery {
            pending_reversal: Some(true),
            inviter: Some("Inviter%Literal".into()),
            order_no: Some("READ-ORDER".into()),
            limit: 20,
            offset: 20,
            ..Default::default()
        };
        let (page, total, _) = state
            .list_admin_referral_rewards(query)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(total, 22);
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].inviter_username.as_deref(), Some("Inviter%Literal"));
        let (_, total, _) = state
            .list_admin_referral_rewards(ReferralRewardListQuery {
                order_id: Some("READ-ORDER".into()),
                limit: 20,
                ..Default::default()
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            total, 0,
            "order_id must not silently search human order number"
        );
        let summary = state
            .referral_admin_overview_stats()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summary.cumulative_reward_usd, 20.0);
        assert_eq!(summary.legacy.paid_reward_usd, 10.0);
        assert_eq!(summary.failed_reward_count, 22);
        assert_eq!(summary.pending_reversal_reward_usd, 44.0);
        assert_eq!(summary.pending_reversal_reward_count, 22);
        let detail = state
            .referral_reward_detail(&ids[23])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.rule_snapshot.as_ref().unwrap().percent_rate, 5.0);
        assert_eq!(
            detail.source_order.as_ref().unwrap().refunded_amount_usd,
            40.0
        );
        let payload = serde_json::to_string(&detail).unwrap();
        assert!(!payload.contains("gateway-key"));
        assert!(!payload.contains("private-source"));
        assert!(!payload.contains("idempotency_key"));
        assert_eq!(detail.ledger_entries.len(), 1);
        assert!(state
            .referral_reward_detail("missing")
            .await
            .unwrap()
            .is_none());
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires explicit AETHER_TEST_DATABASE_URL; only session-local temp tables are mutated"]
    async fn referral_reversal_queue_includes_one_numeric_unit_targets_and_debts() {
        let backends = isolated_backend().await;
        let pool = backends.postgres().unwrap().pool_clone();
        let state = ReferralDataState::new(Some(&backends));
        let inviter = uuid::Uuid::new_v4().to_string();
        let invitee = uuid::Uuid::new_v4().to_string();
        let wallet = uuid::Uuid::new_v4().to_string();
        let order = uuid::Uuid::new_v4().to_string();
        let relationship = uuid::Uuid::new_v4().to_string();
        let reward = uuid::Uuid::new_v4().to_string();
        for user in [&inviter, &invitee] {
            sqlx::query("INSERT INTO users(id,username) VALUES($1,$1)")
                .bind(user)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO wallets(id,user_id,gift_balance) VALUES($1,$2,0.00000002)")
            .bind(&wallet)
            .bind(&inviter)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO payment_orders(id,order_no,wallet_id,user_id,amount_usd,payment_method,status,refunded_amount_usd,refundable_amount_usd,created_at,credited_at) VALUES($1,$1,$2,$3,1,'stripe','credited',0.5,0.5,NOW(),NOW())").bind(&order).bind(&wallet).bind(&invitee).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO user_referrals(id,inviter_user_id,invitee_user_id,invite_code_snapshot) VALUES($1,$2,$3,'AE-TINY')").bind(&relationship).bind(&inviter).bind(&invitee).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO referral_rewards(id,referral_id,inviter_user_id,invitee_user_id,reward_type,source_order_id,trigger_point,amount_usd,status,idempotency_key) VALUES($1,$2,$3,$4,'headcount',$5,'first_paid_order',0.00000002,'applied',$1)").bind(&reward).bind(&relationship).bind(&inviter).bind(&invitee).bind(&order).execute(&pool).await.unwrap();
        let candidates = state.list_referral_reversal_candidates().await.unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, reward);
        state
            .apply_referral_reward_reversal(&candidates[0])
            .await
            .unwrap();
        let updated = state.find_referral_reward(&reward).await.unwrap().unwrap();
        assert_eq!(updated.reversed_amount_usd, 0.00000001);
        assert!(state
            .list_referral_reversal_candidates()
            .await
            .unwrap()
            .is_empty());
        // Outstanding one-unit debt remains eligible even if the wallet is empty.
        sqlx::query("UPDATE referral_rewards SET reversed_amount_usd=0,pending_reversal_amount_usd=0.00000001 WHERE id=$1").bind(&reward).execute(&pool).await.unwrap();
        assert_eq!(
            state
                .list_referral_reversal_candidates()
                .await
                .unwrap()
                .len(),
            1
        );
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires explicit AETHER_TEST_DATABASE_URL; only session-local temp tables are mutated"]
    async fn pending_paid_referral_credit_commits_net_gift_and_both_ledgers() {
        let backends = isolated_backend().await;
        let pool = backends.postgres().unwrap().pool_clone();
        let state = ReferralDataState::new(Some(&backends));
        for (succeeded, processing, expected_reversed) in
            [(0.0, 100.0, 0.0), (40.0, 20.0, 4.0), (100.0, 0.0, 10.0)]
        {
            let inviter = uuid::Uuid::new_v4().to_string();
            let invitee = uuid::Uuid::new_v4().to_string();
            let inviter_wallet = uuid::Uuid::new_v4().to_string();
            let invitee_wallet = uuid::Uuid::new_v4().to_string();
            let order = uuid::Uuid::new_v4().to_string();
            let relationship = uuid::Uuid::new_v4().to_string();
            let reward = uuid::Uuid::new_v4().to_string();
            for user in [&inviter, &invitee] {
                sqlx::query("INSERT INTO users(id,username) VALUES($1,$1)")
                    .bind(user)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            for (wallet, user, gift) in [
                (&inviter_wallet, &inviter, 1.0),
                (&invitee_wallet, &invitee, 0.0),
            ] {
                sqlx::query("INSERT INTO wallets(id,user_id,gift_balance,total_adjusted) VALUES($1,$2,$3,$3)").bind(wallet).bind(user).bind(gift).execute(&pool).await.unwrap();
            }
            sqlx::query("INSERT INTO payment_orders(id,order_no,wallet_id,user_id,amount_usd,payment_method,status,refunded_amount_usd,refundable_amount_usd,created_at,credited_at) VALUES($1,$1,$2,$3,100,'stripe','credited',$4,100-$4,NOW(),NOW())")
                .bind(&order).bind(&invitee_wallet).bind(&invitee).bind(succeeded+processing).execute(&pool).await.unwrap();
            for (amount, status) in [(succeeded, "succeeded"), (processing, "processing")] {
                if amount <= 0.0 {
                    continue;
                }
                let refund = uuid::Uuid::new_v4().to_string();
                sqlx::query("INSERT INTO refund_requests(id,refund_no,wallet_id,user_id,payment_order_id,source_type,refund_mode,amount_usd,status,created_at,updated_at) VALUES($1,$1,$2,$3,$4,'payment_order','original',$5,$6,NOW(),NOW())")
                    .bind(refund).bind(&invitee_wallet).bind(&invitee).bind(&order).bind(amount).bind(status).execute(&pool).await.unwrap();
            }
            sqlx::query("INSERT INTO user_referrals(id,inviter_user_id,invitee_user_id,invite_code_snapshot) VALUES($1,$2,$3,'AE-NET')")
                .bind(&relationship).bind(&inviter).bind(&invitee).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO referral_rewards(id,referral_id,inviter_user_id,invitee_user_id,reward_type,source_order_id,trigger_point,amount_usd,status,idempotency_key) VALUES($1,$2,$3,$4,'headcount',$5,'first_paid_order',10,'pending',$1)")
                .bind(&reward).bind(&relationship).bind(&inviter).bind(&invitee).bind(&order).execute(&pool).await.unwrap();
            if expected_reversed == 10.0 {
                let summary = state.reconcile_referral_rewards_once(None).await.unwrap();
                assert_eq!(summary.reward_applied, 1);
                assert_eq!(summary.deferred, 0);
            } else {
                state
                    .settle_paid_order_referral_rewards(&order)
                    .await
                    .unwrap();
            }
            let updated = state.find_referral_reward(&reward).await.unwrap().unwrap();
            assert_eq!(updated.reversed_amount_usd, expected_reversed);
            assert_eq!(updated.pending_reversal_amount_usd, 0.0);
            assert_eq!(
                updated.status,
                if expected_reversed == 10.0 {
                    "reversed"
                } else {
                    "applied"
                }
            );
            let gift: f64 = sqlx::query_scalar(
                "SELECT CAST(gift_balance AS DOUBLE PRECISION) FROM wallets WHERE id=$1",
            )
            .bind(&inviter_wallet)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(gift, 11.0 - expected_reversed);
            let ledgers=sqlx::query("SELECT reason_code, CAST(amount AS DOUBLE PRECISION) AS amount, CAST(recharge_balance_before AS DOUBLE PRECISION) AS recharge, CAST(gift_balance_before AS DOUBLE PRECISION) AS gift_before, CAST(gift_balance_after AS DOUBLE PRECISION) AS gift_after FROM wallet_transactions WHERE link_type='referral_reward' AND link_id=$1 ORDER BY amount DESC")
                .bind(&reward).fetch_all(&pool).await.unwrap();
            assert_eq!(ledgers.len(), if expected_reversed > 0.0 { 2 } else { 1 });
            for row in &ledgers {
                assert!(referral_signed_gift_fact_valid(
                    row.try_get("amount").unwrap(),
                    row.try_get("recharge").unwrap(),
                    row.try_get("gift_before").unwrap(),
                    row.try_get("gift_after").unwrap()
                ));
            }
            assert_eq!(ledgers[0].try_get::<f64, _>("amount").unwrap(), 10.0);
            state
                .settle_paid_order_referral_rewards(&order)
                .await
                .unwrap();
            let count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM wallet_transactions WHERE link_id=$1")
                    .bind(&reward)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(count, ledgers.len() as i64);
        }
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires explicit AETHER_TEST_DATABASE_URL; only session-local temp tables are mutated"]
    async fn registration_and_reward_obligation_commit_together_without_crediting_inviter() {
        let backends = isolated_backend().await;
        let pool = backends.postgres().unwrap().pool_clone();
        let inviter = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO users(id,username,email_verified) VALUES($1,$1,FALSE)")
            .bind(&inviter)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO wallets(id,user_id,gift_balance) VALUES($1,$2,0)")
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(&inviter)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO user_invite_codes(user_id,invite_code) VALUES($1,'AE-REG')")
            .bind(&inviter)
            .execute(&pool)
            .await
            .unwrap();
        let state = ReferralDataState::new(Some(&backends));
        let (user, wallet, _) = state
            .register_local_auth_user_with_referral(
                None,
                false,
                "atomic-new-user".into(),
                "hash".into(),
                2.0,
                false,
                Some("AE-REG"),
                None,
                None,
                Some("v1"),
                None,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(wallet.gift_balance, 2.0);
        let row=sqlx::query("SELECT u.privacy_policy_accepted_version,rw.status,CAST(w.gift_balance AS DOUBLE PRECISION) AS inviter_gift FROM users u JOIN user_referrals r ON r.invitee_user_id=u.id JOIN referral_rewards rw ON rw.referral_id=r.id JOIN wallets w ON w.user_id=rw.inviter_user_id WHERE u.id=$1").bind(&user.id).fetch_one(&pool).await.unwrap();
        assert_eq!(
            row.try_get::<String, _>("privacy_policy_accepted_version")
                .unwrap(),
            "v1"
        );
        assert_eq!(row.try_get::<String, _>("status").unwrap(), "pending");
        assert_eq!(row.try_get::<f64, _>("inviter_gift").unwrap(), 0.0);
        state
            .settle_registration_referral_rewards(&user.id)
            .await
            .unwrap();
        let gift: f64 = sqlx::query_scalar(
            "SELECT CAST(gift_balance AS DOUBLE PRECISION) FROM wallets WHERE user_id=$1",
        )
        .bind(&inviter)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(gift, 3.0);
        state
            .settle_registration_referral_rewards(&user.id)
            .await
            .unwrap();
        let gift: f64 = sqlx::query_scalar(
            "SELECT CAST(gift_balance AS DOUBLE PRECISION) FROM wallets WHERE user_id=$1",
        )
        .bind(&inviter)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(gift, 3.0);
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires explicit AETHER_TEST_DATABASE_URL; only session-local temp tables are mutated"]
    async fn invalid_invite_rolls_back_new_user_wallet_and_initial_gift() {
        let backends = isolated_backend().await;
        let pool = backends.postgres().unwrap().pool_clone();
        let state = ReferralDataState::new(Some(&backends));
        assert!(state
            .register_local_auth_user_with_referral(
                None,
                false,
                "should-rollback".into(),
                "hash".into(),
                2.0,
                false,
                Some("INVALID"),
                None,
                None,
                Some("v1"),
                None
            )
            .await
            .is_err());
        for table in [
            "users",
            "wallets",
            "wallet_transactions",
            "referral_rewards",
            "user_referrals",
        ] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
        pool.close().await;
    }
}

#[cfg(feature = "postgres")]
async fn insert_registration_obligation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: &str,
    inviter: &str,
    invitee: &str,
    config: &ReferralRewardConfig,
) -> Result<(), DataLayerError> {
    if !config.headcount_amount_usd.is_finite() || config.headcount_amount_usd < 0.000000005 {
        return Ok(());
    }
    let key = format!("referral:{id}:headcount:{}", config.headcount_trigger);
    // All callers hold the relationship row lock (including its initial insert).
    // Any previous headcount trigger remains a durable one-time event.
    sqlx::query("INSERT INTO referral_rewards(id,referral_id,inviter_user_id,invitee_user_id,reward_type,trigger_point,amount_usd,status,idempotency_key,created_at,updated_at) SELECT $1,$2,$3,$4,'headcount',$5,$6,'pending',$7,NOW(),NOW() WHERE NOT EXISTS(SELECT 1 FROM referral_rewards WHERE referral_id=$2 AND reward_type='headcount') ON CONFLICT(idempotency_key) DO NOTHING")
        .bind(uuid::Uuid::new_v4().to_string()).bind(id).bind(inviter).bind(invitee).bind(&config.headcount_trigger).bind(config.headcount_amount_usd).bind(key).execute(&mut **tx).await.map_err(DataLayerError::postgres)?;
    Ok(())
}

impl ReferralDataState<'_> {
    pub async fn validate_referral_invite_code(
        &self,
        code: Option<&str>,
    ) -> Result<(), DataLayerError> {
        let Some(code) = code.filter(|v| !v.trim().is_empty()) else {
            return Ok(());
        };
        let code = normalize_referral_code(code)
            .ok_or_else(|| DataLayerError::InvalidInput("邀请码无效".into()))?;
        #[cfg(feature = "postgres")]
        if let Some(backend) = self.backends.and_then(DataBackends::postgres) {
            let exists=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM user_invite_codes c JOIN users u ON u.id=c.user_id WHERE c.invite_code=$1 AND c.active=TRUE AND u.is_active=TRUE AND u.is_deleted=FALSE)").bind(code).fetch_one(&backend.pool_clone()).await.map_err(DataLayerError::postgres)?;
            if !exists {
                return Err(DataLayerError::InvalidInput("邀请码无效".into()));
            }
        }
        Ok(())
    }

    pub async fn bind_referral_invite_code_with_reward(
        &self,
        invitee: &str,
        code: Option<&str>,
        source: Option<serde_json::Value>,
        _config: Option<ReferralRewardConfig>,
        email_verified: bool,
    ) -> Result<Option<ReferralRelationshipRecord>, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(backend) = self.backends.and_then(DataBackends::postgres) {
            let mut tx = backend
                .pool_clone()
                .begin()
                .await
                .map_err(DataLayerError::postgres)?;
            let (config, required) = super::settings::capture_registration_config(&mut tx).await?;
            let record = if config.is_some() {
                bind_in_transaction(
                    &mut tx,
                    invitee,
                    code,
                    source,
                    config,
                    email_verified,
                    required,
                )
                .await?
            } else {
                None
            };
            tx.commit().await.map_err(DataLayerError::postgres)?;
            return Ok(record);
        }
        Ok(None)
    }

    pub async fn settle_registration_referral_rewards(
        &self,
        invitee: &str,
    ) -> Result<Vec<ReferralRewardRecord>, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(backend) = self.backends.and_then(DataBackends::postgres) {
            let keys=sqlx::query_scalar::<_,String>("SELECT idempotency_key FROM referral_rewards WHERE invitee_user_id=$1 AND source_order_id IS NULL AND status IN ('pending','failed') AND amount_usd>0 ORDER BY created_at,id").bind(invitee).fetch_all(&backend.pool_clone()).await.map_err(DataLayerError::postgres)?;
            return self
                .credit_pending_referral_rewards(&keys, None, None)
                .await;
        }
        Ok(Vec::new())
    }

    pub async fn activate_email_verified_referral_reward(
        &self,
        invitee: &str,
    ) -> Result<(), DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(backend) = self.backends.and_then(DataBackends::postgres) {
            let mut tx = backend
                .pool_clone()
                .begin()
                .await
                .map_err(DataLayerError::postgres)?;
            let row=sqlx::query("SELECT r.id,r.inviter_user_id,r.source_json FROM user_referrals r JOIN users u ON u.id=r.invitee_user_id WHERE r.invitee_user_id=$1 AND u.email_verified=TRUE FOR UPDATE OF r")
                .bind(invitee).fetch_optional(&mut *tx).await.map_err(DataLayerError::postgres)?;
            if let Some(row) = row {
                let source: Option<serde_json::Value> = row
                    .try_get("source_json")
                    .map_err(DataLayerError::postgres)?;
                let config = source
                    .as_ref()
                    .and_then(|v| v.get(REFERRAL_SNAPSHOT_KEY))
                    .and_then(|v| v.get("config"))
                    .filter(|v| !v.is_null())
                    .map(|v| serde_json::from_value::<ReferralRewardConfig>(v.clone()))
                    .transpose()
                    .map_err(DataLayerError::sql)?;
                let required = source
                    .as_ref()
                    .and_then(|v| v.get(REFERRAL_SNAPSHOT_KEY))
                    .and_then(|v| v.get("email_verification_required"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if let Some(config) = config {
                    if required
                        && config.headcount_enabled
                        && config.headcount_trigger == "email_verified"
                    {
                        insert_registration_obligation(
                            &mut tx,
                            &row_string!(row, "id"),
                            &row_string!(row, "inviter_user_id"),
                            invitee,
                            &config,
                        )
                        .await?;
                    }
                }
            }
            tx.commit().await.map_err(DataLayerError::postgres)?;
        }
        Ok(())
    }

    /// Account, wallet and obligations become visible in one commit. DTOs are
    /// constructed before writing, so no post-commit read can trigger deletion.
    #[allow(clippy::too_many_arguments)]
    pub async fn register_local_auth_user_with_referral(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
        initial_gift_usd: f64,
        unlimited: bool,
        invite_code: Option<&str>,
        source: Option<serde_json::Value>,
        _config: Option<ReferralRewardConfig>,
        privacy_version: Option<&str>,
        default_group_id: Option<&str>,
    ) -> Result<Option<(StoredUserAuthRecord, StoredWalletSnapshot, bool)>, DataLayerError> {
        if !initial_gift_usd.is_finite() {
            return Err(DataLayerError::InvalidInput(
                "initial gift amount must be finite".into(),
            ));
        }
        let now = chrono::Utc::now();
        let user_id = uuid::Uuid::new_v4().to_string();
        let wallet_id = uuid::Uuid::new_v4().to_string();
        let gift = if unlimited {
            0.0
        } else {
            initial_gift_usd.max(0.0)
        };
        let user = StoredUserAuthRecord::new(
            user_id.clone(),
            email.clone(),
            email_verified,
            username.clone(),
            Some(password_hash.clone()),
            "user".into(),
            "local".into(),
            None,
            None,
            None,
            true,
            false,
            Some(now),
            None,
        )?
        .with_policy_modes("inherit".into(), "inherit".into(), "inherit".into())?;
        let wallet = StoredWalletSnapshot::new(
            wallet_id.clone(),
            Some(user_id.clone()),
            None,
            0.0,
            gift,
            if unlimited {
                "unlimited".into()
            } else {
                "finite".into()
            },
            "USD".into(),
            "active".into(),
            0.0,
            0.0,
            0.0,
            gift,
            now.timestamp(),
        )?;
        #[cfg(feature = "postgres")]
        if let Some(backend) = self.backends.and_then(DataBackends::postgres) {
            let mut tx = backend
                .pool_clone()
                .begin()
                .await
                .map_err(DataLayerError::postgres)?;
            sqlx::query("INSERT INTO users(id,email,email_verified,username,password_hash,role,auth_source,allowed_providers_mode,allowed_api_formats_mode,allowed_models_mode,rate_limit_mode,is_active,is_deleted,privacy_policy_accepted_version,privacy_policy_accepted_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,'user'::userrole,'local'::authsource,'inherit','inherit','inherit','inherit',TRUE,FALSE,$6,CASE WHEN $6::TEXT IS NULL THEN NULL ELSE NOW() END,NOW(),NOW())")
                .bind(&user_id).bind(email).bind(email_verified).bind(username).bind(password_hash).bind(privacy_version).execute(&mut *tx).await.map_err(DataLayerError::postgres)?;
            sqlx::query("INSERT INTO wallets(id,user_id,balance,gift_balance,limit_mode,currency,status,total_recharged,total_consumed,total_refunded,total_adjusted,created_at,updated_at) VALUES($1,$2,0,$3,$4,'USD','active',0,0,0,$3,NOW(),NOW())")
                .bind(&wallet_id).bind(&user_id).bind(gift).bind(&wallet.limit_mode).execute(&mut *tx).await.map_err(DataLayerError::postgres)?;
            if gift > 0.0 {
                sqlx::query("INSERT INTO wallet_transactions(id,wallet_id,category,reason_code,amount,balance_before,balance_after,recharge_balance_before,recharge_balance_after,gift_balance_before,gift_balance_after,link_type,link_id,description,created_at) VALUES($1,$2,'gift','gift_initial',$3,0,$3,0,0,0,$3,'system_task',$4,'用户初始赠款',NOW())")
                .bind(uuid::Uuid::new_v4().to_string()).bind(&wallet_id).bind(gift).bind(&user_id).execute(&mut *tx).await.map_err(DataLayerError::postgres)?;
            }
            if let Some(group) = default_group_id {
                sqlx::query("INSERT INTO user_group_members(group_id,user_id) VALUES($1,$2)")
                    .bind(group)
                    .bind(&user_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(DataLayerError::postgres)?;
            }
            let (config, required) = super::settings::capture_registration_config(&mut tx).await?;
            if config.is_some() {
                bind_in_transaction(
                    &mut tx,
                    &user_id,
                    invite_code,
                    source,
                    config,
                    email_verified,
                    required,
                )
                .await?;
            }
            tx.commit().await.map_err(DataLayerError::postgres)?;
            return Ok(Some((user, wallet, true)));
        }
        Ok(None)
    }
}
