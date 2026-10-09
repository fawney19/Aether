use super::SqlxUsageReadRepository;
use aether_data_contracts::repository::usage::*;
use chrono::{TimeZone, Utc};
use sqlx::Row;

#[tokio::test]
#[ignore = "requires local AETHER_TEST_DATABASE_URL with temporary database creation"]
async fn live_overview_user_finance_uses_credited_period_and_full_roster() {
    use futures_util::FutureExt;
    use std::panic::AssertUnwindSafe;
    let options = std::env::var("AETHER_TEST_DATABASE_URL")
        .unwrap()
        .parse::<sqlx::postgres::PgConnectOptions>()
        .unwrap();
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .unwrap();
    let database = format!("overview_finance_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {database}"))
        .execute(&admin)
        .await
        .unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.database(&database))
        .await
        .unwrap();
    let outcome = AssertUnwindSafe(async {
        crate::POSTGRES_MIGRATOR.run(&pool).await.unwrap();
        let at = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        for (id, active, deleted, balance, gift) in [
            ("alice", true, false, 12.0, 3.0),
            ("bob", false, false, 5.0, 1.0),
            ("deleted", false, true, 999.0, 999.0),
        ] {
            sqlx::query("INSERT INTO users(id,username,email_verified,is_active,is_deleted) VALUES($1,$1,false,$2,$3)")
                .bind(id).bind(active).bind(deleted).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO wallets(id,user_id,balance,gift_balance,currency,status,limit_mode,created_at,updated_at) VALUES($1,$1,$2,$3,'USD','active','finite',$4,$4)")
                .bind(id).bind(balance).bind(gift).bind(at).execute(&pool).await.unwrap();
        }
        for (id, user, cost) in [("a1", "alice", 1.0), ("a2", "alice", 2.0), ("b", "bob", 2.0), ("d", "deleted", 999.0)] {
            sqlx::query("INSERT INTO usage(id,request_id,user_id,model,provider_name,status,billing_status,total_tokens,total_cost_usd,actual_total_cost_usd,created_at,request_metadata) VALUES($1,$1,$2,'model','provider','completed','settled',100,$3,$3,$4,$5)")
                .bind(id).bind(user).bind(cost).bind(at)
                .bind(serde_json::json!({"analytics_attribution":{"is_standalone":false}}))
                .execute(&pool).await.unwrap();
        }
        // Old creation timestamps must not hide credits received in this period.
        // Gift orders, plans, pending orders and boundary credits stay distinct.
        for (id, user, kind, method, status, credit_minutes, amount) in [
            ("credit-start", "alice", "wallet_recharge", "stripe", "credited", Some(0), 100.0),
            ("credit-later", "alice", "wallet_recharge", "admin_manual", "credited", Some(2), 11.0),
            ("plan", "alice", "plan_purchase", "stripe", "credited", Some(3), 25.0),
            ("gift", "alice", "wallet_recharge", "gift_code", "credited", Some(4), 7.0),
            ("grant", "alice", "plan_purchase", "admin_grant", "credited", Some(5), 4.0),
            ("pending", "alice", "wallet_recharge", "stripe", "pending", None, 999.0),
            ("old", "alice", "wallet_recharge", "stripe", "credited", Some(-1), 999.0),
            ("end", "alice", "wallet_recharge", "stripe", "credited", Some(60), 999.0),
            ("bob-credit", "bob", "wallet_recharge", "stripe", "credited", Some(1), 20.0),
            ("deleted-credit", "deleted", "wallet_recharge", "stripe", "credited", Some(1), 999.0),
        ] {
            sqlx::query("INSERT INTO payment_orders(id,order_no,wallet_id,user_id,order_kind,payment_method,status,amount_usd,refunded_amount_usd,created_at,paid_at,credited_at) VALUES($1,$1,$2,$2,$3,$4,$5,$6,0,$7,$8,$8)")
                .bind(id).bind(user).bind(kind).bind(method).bind(status).bind(amount)
                .bind(at - chrono::Duration::days(1))
                .bind(credit_minutes.map(|minutes| at + chrono::Duration::minutes(minutes)))
                .execute(&pool).await.unwrap();
        }
        // A later cumulative refund must not rewrite the original gross credit.
        sqlx::query("UPDATE payment_orders SET refunded_amount_usd=10 WHERE id='credit-start'")
            .execute(&pool).await.unwrap();
        let repo = SqlxUsageReadRepository::new(pool.clone());
        let query = UsageAnalyticsQuery {
            from_unix_ms: at.timestamp_millis() as u64,
            to_unix_ms: (at + chrono::Duration::hours(1)).timestamp_millis() as u64,
            timezone: "UTC".into(), view: UsageAnalyticsView::Users,
            descending: true, limit: 1, ..Default::default()
        };
        let first = repo.query_usage_analytics(&query).await.unwrap();
        assert_eq!(first.users.len(), 1);
        assert_eq!(first.users[0].user_id, "alice");
        assert_eq!(first.user_summary.as_ref().unwrap().user_count, 2);
        assert_eq!(first.user_summary.as_ref().unwrap().active_user_count, 2);
        assert_eq!(first.summary.enabled_users, 1);
        assert_eq!(first.summary.request_count, 3);
        assert_eq!(first.summary.billable_amount.as_deref(), Some("5.00000000"));
        let finance = first.user_finance_summary.as_ref().unwrap();
        assert_eq!(finance.wallet_balance.as_deref(), Some("21.00000000"));
        assert_eq!(finance.recharge_amount.as_deref(), Some("131.00000000"));
        assert_eq!(finance.recharge_count, 3);
        assert_eq!(finance.plan_purchase_amount.as_deref(), Some("25.00000000"));
        assert_eq!(finance.plan_purchase_count, 1);
        assert_eq!(finance.gift_credit_amount.as_deref(), Some("11.00000000"));
        assert_eq!(finance.gift_credit_count, 2);
        assert_eq!(first.users[0].finance.as_ref().unwrap().recharge_amount.as_deref(), Some("111.00000000"));
        assert!(first.user_payments.is_none());
        let next = repo.query_usage_analytics(&UsageAnalyticsQuery { offset: 1, ..query.clone() }).await.unwrap();
        assert_eq!(next.users[0].user_id, "bob");
        assert_eq!(next.user_summary, first.user_summary);
        assert_eq!(next.user_finance_summary, first.user_finance_summary);
        let search = repo.query_usage_analytics(&UsageAnalyticsQuery { search: Some("bob".into()), ..query.clone() }).await.unwrap();
        assert_eq!(search.total, 1);
        assert_eq!(search.summary.request_count, 1);
        assert_eq!(search.user_finance_summary.unwrap().recharge_amount.as_deref(), Some("20.00000000"));
        let detail = repo.query_usage_analytics(&UsageAnalyticsQuery {
            credential_owner_id: Some("alice".into()), payment_limit: Some(2), payment_offset: Some(1), ..query.clone()
        }).await.unwrap();
        let payments = detail.user_payments.unwrap();
        assert_eq!(payments.total, 5);
        assert_eq!(payments.limit, 2);
        assert_eq!(payments.offset, 1);
        assert_eq!(payments.items[0].id, "gift");
        assert_eq!(payments.items[0].kind, "gift_credit");
        assert_eq!(payments.items[1].id, "plan");
        let empty = repo.query_usage_analytics(&UsageAnalyticsQuery { search: Some("missing".into()), ..query.clone() }).await.unwrap();
        assert_eq!(empty.total, 0);
        assert_eq!(empty.summary.request_count, 0);
        assert_eq!(empty.user_finance_summary.unwrap().wallet_balance.as_deref(), Some("0.00000000"));
        sqlx::query("UPDATE wallets SET currency='EUR' WHERE user_id='alice'").execute(&pool).await.unwrap();
        let mixed = repo.query_usage_analytics(&query).await.unwrap();
        assert!(mixed.user_finance_summary.unwrap().wallet_balance.is_none());
        assert!(mixed.users[0].finance.as_ref().unwrap().wallet_balance.is_none());
    }).catch_unwind().await;
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {database} WITH (FORCE)"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    if let Err(error) = outcome {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test]
#[ignore = "requires local AETHER_TEST_DATABASE_URL with temporary database creation"]
async fn live_overview_dashboard_all_history_and_charts_share_canonical_metrics() {
    use futures_util::FutureExt;
    use std::panic::AssertUnwindSafe;
    let connection = std::env::var("AETHER_TEST_DATABASE_URL")
        .unwrap()
        .parse::<sqlx::postgres::PgConnectOptions>()
        .unwrap();
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(connection.clone())
        .await
        .unwrap();
    let database = format!("overview_dashboard_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {database}"))
        .execute(&admin)
        .await
        .unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(connection.database(&database))
        .await
        .unwrap();
    let outcome=AssertUnwindSafe(async {
        crate::POSTGRES_MIGRATOR.run(&pool).await.unwrap();
        let repo=SqlxUsageReadRepository::new(pool.clone());
        let query=UsageDashboardAnalyticsQuery{timezone:"Asia/Shanghai".into()};
        let empty=repo.query_dashboard_analytics(&query).await.unwrap();
        assert_eq!(empty.total.summary.request_count,0); assert_eq!(empty.total_from,None);
        assert_eq!(empty.today.read_revision,empty.total.read_revision);
        assert_eq!(empty.total.coverage.missing_bucket_count,0);
        assert_eq!(empty.total.summary.billable_amount.as_deref(),Some("0.00000000"));
        let coverage_start=Utc.with_ymd_and_hms(2020,1,1,0,0,0).unwrap();
        for index in 0..4 {
            sqlx::query("INSERT INTO stats_bucket_state(projection_version,granularity,bucket_start,source_revision,built_revision,coverage_status) VALUES('overview-v2','hour',$1,1,$2,'complete')")
                .bind(coverage_start+chrono::Duration::hours(index)).bind(if index==1 || index==3 {0_i64}else{1_i64}).execute(&pool).await.unwrap();
        }
        let coverage_query=UsageAnalyticsQuery{from_unix_ms:coverage_start.timestamp_millis()as u64,to_unix_ms:(coverage_start+chrono::Duration::hours(5)).timestamp_millis()as u64,timezone:"Asia/Kathmandu".into(),limit:1,..Default::default()};
        let mut coverage_tx=pool.begin().await.unwrap();
        let coverage=super::projection_reader::read_projection_coverage(&mut coverage_tx,&coverage_query,true).await.unwrap();
        assert_eq!(coverage.projection_through,Some((coverage_start+chrono::Duration::hours(1)).to_rfc3339()));
        assert_eq!(coverage.dirty_bucket_count,2); assert_eq!(coverage.missing_bucket_count,1);
        let partial_query=UsageAnalyticsQuery{from_unix_ms:coverage_query.from_unix_ms+30*60*1000,to_unix_ms:coverage_query.from_unix_ms+3*60*60*1000+30*60*1000,..coverage_query.clone()};
        let partial=super::projection_reader::read_projection_coverage(&mut coverage_tx,&partial_query,true).await.unwrap();
        assert_eq!(partial.projection_from,Some((coverage_start+chrono::Duration::hours(1)).to_rfc3339()));
        assert_eq!(partial.projection_from,partial.projection_through); assert_eq!(partial.missing_bucket_count,0); assert_eq!(partial.dirty_bucket_count,1);
        coverage_tx.rollback().await.unwrap();
        sqlx::query("DELETE FROM stats_bucket_state").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO stats_bucket_state(projection_version,granularity,bucket_start,source_revision,built_revision,coverage_status) VALUES('overview-v1','hour',$1,1,1,'complete')")
            .bind(coverage_start).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO stats_overview_hourly(projection_version,bucket_start,dimensions,metrics) VALUES('overview-v1',$1,'{}',$2)")
            .bind(coverage_start)
            .bind(serde_json::json!({"request_count":999,"trusted_attribution_count":999,"billable_amount":"999.00000000"}))
            .execute(&pool).await.unwrap();
        let legacy_query = UsageAnalyticsQuery {
            from_unix_ms: coverage_start.timestamp_millis() as u64,
            to_unix_ms: (coverage_start + chrono::Duration::hours(1)).timestamp_millis() as u64,
            timezone: "UTC".into(),
            limit: 1,
            ..Default::default()
        };
        let legacy = repo.query_usage_analytics(&legacy_query).await.unwrap();
        assert_eq!(legacy.summary.request_count, 0);
        assert_eq!(legacy.summary.trusted_attribution_count, 0);
        assert_eq!(legacy.coverage.missing_bucket_count, 1);
        assert_eq!(legacy.coverage.projection_through, Some(coverage_start.to_rfc3339()));
        let legacy_dashboard = repo.query_dashboard_analytics(&query).await.unwrap();
        assert_eq!(legacy_dashboard.total.summary.request_count, 0);
        assert_eq!(legacy_dashboard.total.summary.billable_amount.as_deref(), Some("0.00000000"));
        sqlx::query("UPDATE stats_bucket_state SET coverage_status='unrecoverable' WHERE projection_version='overview-v1'")
            .execute(&pool).await.unwrap();
        assert_eq!(repo.query_usage_analytics(&legacy_query).await.unwrap().unrecoverable_bucket_count, 1);
        let legacy_lost = repo.query_dashboard_analytics(&query).await.unwrap();
        assert_eq!(legacy_lost.history_complete, Some(false));
        assert_eq!(legacy_lost.total.unrecoverable_bucket_count, 1);
        assert_eq!(legacy_lost.total.summary.billable_amount, None);
        assert_eq!(legacy_lost.total_from, Some(coverage_start.to_rfc3339()));
        sqlx::query("DELETE FROM stats_overview_hourly WHERE projection_version='overview-v1'").execute(&pool).await.unwrap();
        sqlx::query("DELETE FROM stats_bucket_state WHERE projection_version='overview-v1'").execute(&pool).await.unwrap();
        let now=Utc::now();
        let day=query.today_start(now).unwrap();
        let old=(day-chrono::Duration::days(800)).with_timezone(&Utc);
        sqlx::query("INSERT INTO users(id,username,email_verified,is_active) VALUES('user','user',false,true)").execute(&pool).await.unwrap();
        for (id,at,model,provider,status,money) in [
            ("old",old,"old-model","old-provider","completed",Some(2.0)),
            ("before-today",day-chrono::Duration::milliseconds(1),"current-model","new-provider","completed",Some(0.2)),
            ("today",day,"current-model","new-provider","completed",Some(0.5)),
            ("pending",day,"current-model","new-provider","pending",None),
            ("parent",day,"current-model","new-provider","completed",Some(99.0)),
            ("future",now+chrono::Duration::days(1),"future-model","future-provider","completed",Some(999.0)),
        ] {
            let metadata=serde_json::json!({"analytics_attribution":{"is_standalone":false,"record_kind":if id=="parent" {"session"}else{"request"}},"usage_pricing_available":money.is_some(),"analytics_measurement":{"source":"reported"}});
            sqlx::query("INSERT INTO usage(id,request_id,user_id,model,provider_id,provider_name,status,billing_status,input_tokens,output_tokens,total_tokens,actual_total_cost_usd,total_cost_usd,created_at,request_metadata) VALUES($1,$1,'user',$2,$3,$3,$4,$5,100,20,120,$6,$6,$7,$8)")
                .bind(id).bind(model).bind(provider).bind(status).bind(if money.is_some(){"settled"}else{"pending"}).bind(money.unwrap_or(0.0)).bind(at).bind(metadata).execute(&pool).await.unwrap();
        }
        let snapshot=repo.query_dashboard_analytics(&query).await.unwrap();
        assert_eq!(snapshot.today.summary.request_count,2);
        assert_eq!(snapshot.today.summary.successful_request_count,1);
        assert_eq!(snapshot.today.summary.in_flight_request_count,1);
        assert_eq!(snapshot.today.summary.billable_amount.as_deref(),Some("0.50000000"));
        assert_eq!(snapshot.today.summary.pricing_available_count,1);
        assert_eq!(snapshot.total.summary.request_count,4);
        assert_eq!(snapshot.total.summary.total_tokens,480);
        assert_eq!(snapshot.total.summary.billable_amount.as_deref(),Some("2.70000000"));
        assert_eq!(snapshot.total.summary.enabled_users,1);
        assert_eq!(snapshot.total_from,Some(old.to_rfc3339()));
        assert_eq!(snapshot.history_complete,None);
        assert_eq!(snapshot.today.read_revision,snapshot.total.read_revision);
        assert_eq!(snapshot.today.generated_at,snapshot.total.generated_at);
        let old_hour=chrono::DateTime::from_timestamp(old.timestamp()/3600*3600,0).unwrap();
        assert!(repo.rebuild_overview_bucket("hour",old_hour).await.unwrap());
        let projected=repo.query_dashboard_analytics(&query).await.unwrap();
        let raw=SqlxUsageReadRepository::new(pool.clone()).with_overview_projection_reads(false).query_dashboard_analytics(&query).await.unwrap();
        assert_eq!(projected.total.summary,raw.total.summary);
        assert_eq!(projected.total.coverage.projection_through,Some((old_hour+chrono::Duration::hours(1)).to_rfc3339()));
        let chart_query=UsageAnalyticsQuery{from_unix_ms:(day-chrono::Duration::days(1)).timestamp_millis()as u64,to_unix_ms:now.timestamp_millis()as u64,timezone:query.timezone.clone(),view:UsageAnalyticsView::DashboardCharts,limit:1,..Default::default()};
        let charts=repo.query_usage_analytics(&chart_query).await.unwrap();
        assert_eq!(charts.summary.request_count,3); assert_eq!(charts.rows.len(),2);
        assert_eq!(charts.model_rows.len(),2); assert_eq!(charts.provider_rows.len(),1);
        assert_eq!(charts.provider_rows[0].metrics.billable_amount.as_deref(),Some("0.70000000"));
        assert_eq!(charts.model_rows.iter().map(|row|row.metrics.request_count).sum::<u64>(),3);
        // Performance model rows span the whole selected period, independently of
        // page limits, and retain the same metrics with projection reads enabled.
        let performance_query = UsageAnalyticsQuery {
            view: UsageAnalyticsView::Performance,
            offset: 1,
            ..chart_query.clone()
        };
        let raw_performance = SqlxUsageReadRepository::new(pool.clone())
            .with_overview_projection_reads(false)
            .query_usage_analytics(&performance_query).await.unwrap();
        assert!(repo.rebuild_overview_bucket("hour", day).await.unwrap());
        let projected_performance = repo.query_usage_analytics(&performance_query).await.unwrap();
        assert_eq!(projected_performance.model_rows, raw_performance.model_rows);
        assert_eq!(projected_performance.model_rows.len(), 1);
        let model = &projected_performance.model_rows[0];
        assert_eq!(model.id.as_deref(), Some("current-model"));
        assert_eq!(model.bucket_start, None);
        assert_eq!(model.metrics.request_count, 3);
        assert_eq!(model.metrics.successful_request_count, 2);
        assert_eq!(model.metrics.in_flight_request_count, 1);
        assert_eq!(model.metrics.billable_amount.as_deref(), Some("0.70000000"));
        sqlx::query("DELETE FROM usage WHERE request_id='old'").execute(&pool).await.unwrap();
        let deleted=repo.query_dashboard_analytics(&query).await.unwrap();
        assert_eq!(deleted.total.summary.request_count,3);
        assert_eq!(deleted.history_complete,Some(false));
        assert_eq!(deleted.total.unrecoverable_bucket_count,1);
        sqlx::query("ALTER TABLE stats_daily ALTER COLUMN date TYPE bigint USING EXTRACT(EPOCH FROM date)::bigint").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO stats_daily(id,date,total_requests) VALUES('legacy-epoch',$1,1)").bind((old-chrono::Duration::days(1)).timestamp()).execute(&pool).await.unwrap();
        assert_eq!(repo.query_dashboard_analytics(&query).await.unwrap().history_complete,Some(false));
        for at in ["2026-09-05T12:00:00Z","2026-09-06T12:00:00Z","2026-09-07T12:00:00Z"] {
            sqlx::query("INSERT INTO usage(id,request_id,model,provider_name,status,billing_status,total_cost_usd,actual_total_cost_usd,created_at) VALUES($1,$1,'santiago-model','santiago','completed','settled',0.1,0.1,$2)").bind(at).bind(chrono::DateTime::parse_from_rfc3339(at).unwrap()).execute(&pool).await.unwrap();
        }
        let santiago=repo.query_usage_analytics(&UsageAnalyticsQuery{from_unix_ms:chrono::DateTime::parse_from_rfc3339("2026-09-05T04:00:00Z").unwrap().timestamp_millis()as u64,to_unix_ms:chrono::DateTime::parse_from_rfc3339("2026-09-08T03:00:00Z").unwrap().timestamp_millis()as u64,timezone:"America/Santiago".into(),view:UsageAnalyticsView::DashboardCharts,model:Some("santiago-model".into()),limit:1,..Default::default()}).await.unwrap();
        assert_eq!(santiago.summary.request_count,3); assert_eq!(santiago.rows.len(),3); assert_eq!(santiago.model_rows.len(),3);
        assert_eq!(santiago.summary.billable_amount.as_deref(),Some("0.30000000"));
        for (series,model) in santiago.rows.iter().zip(&santiago.model_rows) {
            assert_eq!(series.metrics.request_count,1); assert_eq!(series.bucket_start,model.bucket_start); assert_eq!(series.metrics.billable_amount,model.metrics.billable_amount);
        }
        sqlx::query("INSERT INTO usage(id,request_id,model,provider_name,status,created_at) SELECT 'limit-'||n,'limit-'||n,'model-'||n,'provider','completed',$1 FROM generate_series(1,10001)n").bind(day).execute(&pool).await.unwrap();
        assert!(repo.query_usage_analytics(&chart_query).await.unwrap_err().to_string().contains("10000 groups"));
        sqlx::query("DELETE FROM usage").execute(&pool).await.unwrap();
        let lost=repo.query_dashboard_analytics(&query).await.unwrap();
        assert_eq!(lost.today.summary.request_count,0);
        assert_eq!(lost.today.summary.billable_amount,None);
        assert_eq!(lost.total.summary.billable_amount,None);
        assert_eq!(lost.history_complete,Some(false));
    }).catch_unwind().await;
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {database}"))
        .execute(&admin)
        .await
        .unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "requires migrated isolated AETHER_TEST_DATABASE_URL"]
async fn live_overview_canonical_queries_and_dirty_rebuild() {
    let pool = sqlx::PgPool::connect(&std::env::var("AETHER_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let repo = SqlxUsageReadRepository::new(pool.clone());
    let user = uuid::Uuid::new_v4().to_string();
    let zero_user = uuid::Uuid::new_v4().to_string();
    let model = format!("overview-test-{}", uuid::Uuid::new_v4().simple());
    for (id, suffix) in [(&user, "used"), (&zero_user, "zero")] {
        sqlx::query("INSERT INTO users(id,username,email,email_verified,password_hash,role,is_active,is_deleted) VALUES($1,$2,$3,false,'test','user',true,false)")
            .bind(id).bind(format!("{model}-{suffix}")).bind(format!("{id}@test.invalid")).execute(&pool).await.unwrap();
    }
    let start = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()
        + chrono::Duration::hours((uuid::Uuid::new_v4().as_u128() % 8760) as i64);
    let request_ids = (0..5)
        .map(|_| uuid::Uuid::new_v4().to_string())
        .collect::<Vec<_>>();
    for (index, status) in ["completed", "failed", "failed", "cancelled", "pending"]
        .iter()
        .enumerate()
    {
        let origin = if index == 1 { "upstream" } else { "client" };
        let metadata = serde_json::json!({"analytics_attribution":{"is_standalone":false},"analytics_failure":{"origin":origin,"stage":"authentication","reason":"invalid_credentials"}});
        sqlx::query("INSERT INTO usage(id,request_id,user_id,provider_name,model,api_format,status,billing_status,input_tokens,output_tokens,response_time_ms,first_byte_time_ms,is_stream,total_cost_usd,actual_total_cost_usd,created_at,request_metadata) VALUES($1,$1,$2,'Provider',$3,'openai:chat',$4,$5,100,20,1000,100,true,1.25000001,0.75000001,$6,$7)")
            .bind(&request_ids[index]).bind(&user).bind(&model).bind(status).bind(if index == 0 {"settled"} else {"pending"}).bind(start + chrono::Duration::minutes(index as i64)).bind(metadata).execute(&pool).await.unwrap();
    }
    let mut query = UsageAnalyticsQuery {
        from_unix_ms: start.timestamp_millis() as u64,
        to_unix_ms: (start + chrono::Duration::hours(2)).timestamp_millis() as u64,
        timezone: "UTC".into(),
        model: Some(model.clone()),
        limit: 25,
        descending: true,
        ..Default::default()
    };
    let summary = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(summary.summary.request_count, 5);
    assert_eq!(summary.summary.successful_request_count, 1);
    assert_eq!(summary.summary.failed_request_count, 2);
    assert_eq!(summary.summary.cancelled_request_count, 1);
    assert_eq!(summary.summary.in_flight_request_count, 1);
    assert_eq!(
        summary.summary.billable_amount.as_deref(),
        Some("0.75000001")
    );
    assert_eq!(summary.summary.trusted_attribution_count, 5);
    assert_eq!(summary.summary.allocation_available_count, 0);
    query.view = UsageAnalyticsView::Users;
    query.search = Some(model.clone());
    query.limit = 1;
    let users = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(users.total, 2);
    assert_eq!(users.users[0].user_id, user);
    query.offset = 1;
    let users = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(users.users[0].user_id, zero_user);
    assert_eq!(users.users[0].metrics.request_count, 0);
    query.view = UsageAnalyticsView::Performance;
    query.granularity = UsageAnalyticsGranularity::Hour;
    let performance = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(performance.rows.len(), 2);
    assert_eq!(performance.rows[1].metrics.request_count, 0);
    assert_eq!(performance.provider_rows.len(), 1);
    assert_eq!(performance.provider_timeline_rows.len(), 1);
    assert_eq!(performance.summary.first_byte_sample_count, 5);
    assert_eq!(performance.summary.output_tps_sample_count, 5);
    let health = repo
        .summarize_health_observations(&HealthObservationQuery {
            from_unix_ms: query.from_unix_ms,
            to_unix_ms: query.to_unix_ms,
            object_kind: HealthObservationObjectKind::Model,
            object_values: Some(vec![model.clone()]),
            segments: 4,
        })
        .await
        .unwrap();
    assert_eq!(health.overall.service_succeeded_count, 1);
    assert_eq!(health.overall.service_failed_count, 1);
    assert_eq!(health.overall.excluded_count, 2);
    assert_eq!(health.overall.request_count, 5);
    let empty = repo
        .summarize_health_observations(&HealthObservationQuery {
            from_unix_ms: query.from_unix_ms,
            to_unix_ms: query.to_unix_ms,
            object_kind: HealthObservationObjectKind::Model,
            object_values: Some(vec![]),
            segments: 4,
        })
        .await
        .unwrap();
    assert_eq!(empty.overall.request_count, 0);

    assert!(repo.rebuild_overview_bucket("hour", start).await.unwrap());
    let projected = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            view: UsageAnalyticsView::Summary,
            ..query.clone()
        })
        .await
        .unwrap();
    assert_eq!(projected.summary, summary.summary);
    assert_eq!(
        projected.coverage.projection_through,
        Some((start + chrono::Duration::hours(1)).to_rfc3339())
    );
    let raw = SqlxUsageReadRepository::new(pool.clone())
        .with_overview_projection_reads(false)
        .query_usage_analytics(&UsageAnalyticsQuery {
            view: UsageAnalyticsView::Summary,
            ..query.clone()
        })
        .await
        .unwrap();
    assert_eq!(raw.summary, projected.summary);
    assert!(!raw.coverage.read_enabled);
    // UTC bucket membership must not depend on the database session timezone.
    // Partial edge hours still come entirely from raw facts, without duplication.
    let mut timezone_tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL TIME ZONE 'Asia/Kathmandu'")
        .execute(&mut *timezone_tx)
        .await
        .unwrap();
    for (from_offset_ms, to_offset_ms, expected_count) in
        [(0, 7_200_000, 5), (30_000, 7_200_000, 4), (0, 120_000, 2)]
    {
        let range = UsageAnalyticsQuery {
            view: UsageAnalyticsView::Summary,
            from_unix_ms: query.from_unix_ms + from_offset_ms,
            to_unix_ms: query.from_unix_ms + to_offset_ms,
            ..query.clone()
        };
        let projected = super::analytics::read_analytics_metrics(&mut timezone_tx, &range, true)
            .await
            .unwrap();
        let raw = super::analytics::read_analytics_metrics(&mut timezone_tx, &range, false)
            .await
            .unwrap();
        assert_eq!(projected.request_count, expected_count);
        assert_eq!(projected, raw);
        let projected_total =
            super::dashboard::read_dashboard_total_metrics(&mut timezone_tx, &range, true)
                .await
                .unwrap();
        let raw_total =
            super::dashboard::read_dashboard_total_metrics(&mut timezone_tx, &range, false)
                .await
                .unwrap();
        assert_eq!(projected_total, raw_total);
        assert_dashboard_total_matches_canonical(&raw_total, &raw);
    }
    timezone_tx.rollback().await.unwrap();
    let partial = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            view: UsageAnalyticsView::Summary,
            from_unix_ms: query.from_unix_ms + 30_000,
            ..query.clone()
        })
        .await
        .unwrap();
    assert_eq!(partial.summary.request_count, 4);
    let state = sqlx::query("SELECT source_revision,built_revision FROM stats_bucket_state WHERE projection_version='overview-v2' AND granularity='hour' AND bucket_start=$1").bind(start).fetch_one(&pool).await.unwrap();
    assert_eq!(
        state.get::<i64, _>("source_revision"),
        state.get::<i64, _>("built_revision")
    );
    sqlx::query("UPDATE usage SET response_time_ms=2000 WHERE request_id=$1")
        .bind(&request_ids[0])
        .execute(&pool)
        .await
        .unwrap();
    let state = sqlx::query("SELECT source_revision,built_revision FROM stats_bucket_state WHERE projection_version='overview-v2' AND granularity='hour' AND bucket_start=$1").bind(start).fetch_one(&pool).await.unwrap();
    // Bucket state stays unchanged on the foreground write; pending events
    // invalidate the projection before the background merger consumes them.
    assert_eq!(
        state.get::<i64, _>("source_revision"),
        state.get::<i64, _>("built_revision")
    );
    let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM stats_overview_dirty_events WHERE projection_version='overview-v2' AND granularity='hour' AND bucket_start=$1)")
        .bind(start).fetch_one(&pool).await.unwrap();
    assert!(pending);
    let dirty = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            view: UsageAnalyticsView::Summary,
            ..query.clone()
        })
        .await
        .unwrap();
    assert_eq!(dirty.summary.request_count, 5);
    assert_eq!(dirty.summary.latency_sum_ms, 6000.0);
    assert!(repo.rebuild_overview_bucket("hour", start).await.unwrap());
    let rebuilt = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            view: UsageAnalyticsView::Summary,
            ..query.clone()
        })
        .await
        .unwrap();
    assert_eq!(rebuilt.summary, dirty.summary);
    sqlx::query("UPDATE users SET is_deleted=true WHERE id=$1")
        .bind(&user)
        .execute(&pool)
        .await
        .unwrap();
    let actor: Option<String> = sqlx::query_scalar(
        "SELECT credential_owner_id FROM usage_analytics_facts_v1 WHERE request_id=$1",
    )
    .bind(&request_ids[0])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(actor, None);
    sqlx::query("DELETE FROM usage WHERE request_id=ANY($1)")
        .bind(&request_ids)
        .execute(&pool)
        .await
        .unwrap();
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_attribution_snapshots WHERE request_id=ANY($1)",
    )
    .bind(&request_ids)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(remaining, 0);
    assert!(!repo.rebuild_overview_bucket("hour", start).await.unwrap());
    let deleted = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            view: UsageAnalyticsView::Summary,
            ..query.clone()
        })
        .await
        .unwrap();
    assert_eq!(deleted.unrecoverable_bucket_count, 1);
    sqlx::query("DELETE FROM users WHERE id=ANY($1)")
        .bind(vec![user, zero_user])
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires migrated isolated AETHER_TEST_DATABASE_URL"]
async fn live_overview_concurrent_revision_aborts_publication() {
    let pool = sqlx::PgPool::connect(&std::env::var("AETHER_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let bucket = Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap()
        + chrono::Duration::hours((uuid::Uuid::new_v4().as_u128() % 8760) as i64);
    sqlx::query("INSERT INTO stats_bucket_state(projection_version,granularity,bucket_start,source_revision,built_revision) VALUES('overview-v2','hour',$1,1,0) ON CONFLICT DO NOTHING").bind(bucket).execute(&pool).await.unwrap();
    let mut builder = pool.begin().await.unwrap();
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *builder)
        .await
        .unwrap();
    let revision:i64=sqlx::query_scalar("SELECT source_revision FROM stats_bucket_state WHERE projection_version='overview-v2' AND granularity='hour' AND bucket_start=$1").bind(bucket).fetch_one(&mut *builder).await.unwrap();
    sqlx::query("INSERT INTO stats_overview_hourly(projection_version,bucket_start,dimensions,metrics) VALUES('overview-v2',$1,'{}','{}')").bind(bucket).execute(&mut *builder).await.unwrap();
    sqlx::query("UPDATE stats_bucket_state SET source_revision=source_revision+1 WHERE projection_version='overview-v2' AND granularity='hour' AND bucket_start=$1").bind(bucket).execute(&pool).await.unwrap();
    let publish=sqlx::query("UPDATE stats_bucket_state SET built_revision=$2 WHERE projection_version='overview-v2' AND granularity='hour' AND bucket_start=$1 AND source_revision=$2").bind(bucket).bind(revision).execute(&mut *builder).await;
    assert!(publish.is_err() || publish.unwrap().rows_affected() == 0);
    builder.rollback().await.unwrap();
    let rows:i64=sqlx::query_scalar("SELECT count(*) FROM stats_overview_hourly WHERE projection_version='overview-v2' AND bucket_start=$1").bind(bucket).fetch_one(&pool).await.unwrap();
    assert_eq!(rows, 0);
    sqlx::query("DELETE FROM stats_bucket_state WHERE projection_version='overview-v2' AND granularity='hour' AND bucket_start=$1").bind(bucket).execute(&pool).await.unwrap();
}

#[tokio::test]
#[ignore = "requires migrated isolated AETHER_TEST_DATABASE_URL"]
async fn live_overview_dirty_events_merge_without_shared_writer_bucket_lock() {
    let pool = sqlx::PgPool::connect(&std::env::var("AETHER_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let repo = SqlxUsageReadRepository::new(pool.clone());
    let bucket = Utc.with_ymd_and_hms(2022, 1, 1, 0, 0, 0).unwrap()
        + chrono::Duration::hours((uuid::Uuid::new_v4().as_u128() % 8760) as i64);
    sqlx::query("DELETE FROM stats_overview_dirty_events WHERE bucket_start=$1")
        .bind(bucket)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM stats_bucket_state WHERE projection_version='overview-v2' AND granularity='hour' AND bucket_start=$1")
        .bind(bucket)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO stats_overview_dirty_events(transaction_id,projection_version,granularity,bucket_start,unrecoverable) VALUES($1,'overview-v2','hour',$2,false),($3,'overview-v2','hour',$2,true)")
        .bind((uuid::Uuid::new_v4().as_u128() % 9_000_000_000_000_000_000u128) as i64)
        .bind(bucket)
        .bind((uuid::Uuid::new_v4().as_u128() % 9_000_000_000_000_000_000u128) as i64)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(repo.merge_overview_dirty_events().await.unwrap(), 2);
    let row: (i64, String, Option<String>) = sqlx::query_as("SELECT source_revision,coverage_status,last_error FROM stats_bucket_state WHERE projection_version='overview-v2' AND granularity='hour' AND bucket_start=$1")
        .bind(bucket)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        row,
        (
            2,
            "unrecoverable".into(),
            Some("retained usage facts were deleted".into())
        )
    );
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM stats_overview_dirty_events WHERE bucket_start=$1",
    )
    .bind(bucket)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(remaining, 0);
    sqlx::query("DELETE FROM stats_bucket_state WHERE projection_version='overview-v2' AND granularity='hour' AND bucket_start=$1")
        .bind(bucket)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires migrated isolated AETHER_TEST_DATABASE_URL"]
async fn live_overview_settlement_allocations_preserve_unlimited_and_finite_wallets() {
    use aether_data_contracts::repository::settlement::{
        SettlementWriteRepository, UsageSettlementInput,
    };
    let pool = sqlx::PgPool::connect(&std::env::var("AETHER_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let repository = crate::SqlxSettlementRepository::new(pool.clone());
    for mode in [
        "unlimited",
        "finite",
        "mixed_quota",
        "quota_only",
        "minimum",
    ] {
        let user = uuid::Uuid::new_v4().to_string();
        let wallet = uuid::Uuid::new_v4().to_string();
        let request = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO users(id,username,email_verified) VALUES($1,$1,false)")
            .bind(&user)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO wallets(id,user_id,balance,gift_balance,limit_mode,created_at,updated_at) VALUES($1,$2,1,2,$3,NOW(),NOW())").bind(&wallet).bind(&user).bind(if mode=="unlimited" {"unlimited"}else{"finite"}).execute(&pool).await.unwrap();
        let quota: f64 = match mode {
            "mixed_quota" => 2.0,
            "quota_only" => 7.0,
            _ => 0.0,
        };
        if quota > 0.0 {
            sqlx::query(
                "INSERT INTO user_preferences(id,user_id,allow_wallet_overage) VALUES($1,$1,true)",
            )
            .bind(&user)
            .execute(&pool)
            .await
            .unwrap();
            let grant = serde_json::json!([{"type":"daily_quota","daily_quota_usd":quota,"reset_timezone":"UTC","allow_wallet_overage":true}]);
            sqlx::query("INSERT INTO billing_plans(id,title,price_amount,duration_unit,duration_value,entitlements_json,created_at,updated_at) VALUES($1,'test',1,'month',1,$2,NOW(),NOW())").bind(&user).bind(&grant).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO payment_orders(id,order_no,wallet_id,user_id,amount_usd,payment_method,created_at) VALUES($1,$1,$2,$1,1,'test',NOW())").bind(&user).bind(&wallet).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO user_plan_entitlements(id,user_id,plan_id,payment_order_id,starts_at,expires_at,entitlements_snapshot,created_at,updated_at) VALUES($1,$1,$1,$1,NOW()-INTERVAL '1 hour',NOW()+INTERVAL '1 day',$2,NOW(),NOW())").bind(&user).bind(&grant).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO usage(id,request_id,user_id,provider_name,model,status,billing_status) VALUES($1,$1,$2,'test','test','completed','pending')").bind(&request).bind(&user).execute(&pool).await.unwrap();
        let cost = if mode == "minimum" { 0.00000001 } else { 5.0 };
        let input = UsageSettlementInput {
            request_id: request.clone(),
            user_id: Some(user.clone()),
            api_key_id: None,
            api_key_is_standalone: false,
            provider_id: None,
            status: "completed".into(),
            billing_status: "pending".into(),
            total_cost_usd: cost,
            actual_total_cost_usd: cost,
            billing_cost_usd: None,
            finalized_at_unix_secs: None,
        };
        assert_eq!(
            repository
                .settle_usage(input.clone())
                .await
                .unwrap()
                .unwrap()
                .billing_status,
            "settled"
        );
        let snapshot=sqlx::query("SELECT quota_covered_amount_usd::text AS quota,wallet_consumed_amount_usd::text AS consumed,wallet_debit_amount_usd::text AS debit,wallet_recharge_debit_usd::text AS recharge,wallet_gift_debit_usd::text AS gift,wallet_overdraft_usd::text AS overdraft,allocation_status FROM usage_settlement_snapshots WHERE request_id=$1").bind(&request).fetch_one(&pool).await.unwrap();
        let quota_covered = quota.min(cost);
        let consumed = cost - quota_covered;
        assert_eq!(
            snapshot.get::<String, _>("quota"),
            format!("{quota_covered:.8}")
        );
        assert_eq!(
            snapshot.get::<String, _>("consumed"),
            format!("{consumed:.8}")
        );
        assert_eq!(snapshot.get::<String, _>("allocation_status"), "complete");
        if mode == "unlimited" {
            for field in ["debit", "recharge", "gift", "overdraft"] {
                assert_eq!(snapshot.get::<String, _>(field), "0.00000000");
            }
        } else {
            assert_eq!(snapshot.get::<String, _>("debit"), format!("{consumed:.8}"));
            assert_eq!(
                snapshot.get::<String, _>("recharge"),
                format!("{:.8}", consumed.min(1.0))
            );
            assert_eq!(
                snapshot.get::<String, _>("gift"),
                format!("{:.8}", (consumed - 1.0).clamp(0.0, 2.0))
            );
            assert_eq!(
                snapshot.get::<String, _>("overdraft"),
                if mode == "finite" {
                    "2.00000000"
                } else {
                    "0.00000000"
                }
            );
        }
        repository.settle_usage(input).await.unwrap();
        let wallet_consumed: String =
            sqlx::query_scalar("SELECT total_consumed::text FROM wallets WHERE id=$1")
                .bind(&wallet)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(wallet_consumed, format!("{consumed:.8}"));
        sqlx::query("DELETE FROM usage_settlement_snapshots WHERE request_id=$1")
            .bind(&request)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM usage WHERE request_id=$1")
            .bind(&request)
            .execute(&pool)
            .await
            .unwrap();
        if quota > 0.0 {
            sqlx::query("DELETE FROM user_plan_entitlements WHERE id=$1")
                .bind(&user)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("DELETE FROM billing_plans WHERE id=$1")
                .bind(&user)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("DELETE FROM payment_orders WHERE id=$1")
                .bind(&user)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM wallets WHERE id=$1")
            .bind(&wallet)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(&user)
            .execute(&pool)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires migrated isolated AETHER_TEST_DATABASE_URL"]
async fn live_overview_worker_skips_unrecoverable_and_backoff_buckets() {
    let pool = sqlx::PgPool::connect(&std::env::var("AETHER_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let start = Utc.with_ymd_and_hms(1880, 1, 1, 0, 0, 0).unwrap()
        + chrono::Duration::hours((uuid::Uuid::new_v4().as_u128() % 10000) as i64);
    for index in 0..50 {
        sqlx::query("INSERT INTO stats_bucket_state(projection_version,granularity,bucket_start,source_revision,coverage_status,last_failed_at) VALUES('overview-v2','hour',$1,1,$2,$3)")
            .bind(start+chrono::Duration::hours(index)).bind(if index%2==0 {"unrecoverable"}else{"unbuilt"})
            .bind(if index%2==0 {None}else{Some(Utc::now())}).execute(&pool).await.unwrap();
    }
    let available = start + chrono::Duration::hours(50);
    sqlx::query("INSERT INTO stats_bucket_state(projection_version,granularity,bucket_start,source_revision) VALUES('overview-v2','hour',$1,1)").bind(available).execute(&pool).await.unwrap();
    let repo = SqlxUsageReadRepository::new(pool.clone());
    assert!(
        repo.rebuild_overview_buckets(available + chrono::Duration::hours(1), 1)
            .await
            .unwrap()
            > 0
    );
    let clean:bool=sqlx::query_scalar("SELECT source_revision=built_revision FROM stats_bucket_state WHERE projection_version='overview-v2' AND granularity='hour' AND bucket_start=$1").bind(available).fetch_one(&pool).await.unwrap();
    assert!(clean);
    assert_eq!(
        repo.rebuild_overview_buckets(available + chrono::Duration::hours(1), 1)
            .await
            .unwrap(),
        0
    );
    sqlx::query("DELETE FROM stats_bucket_state WHERE projection_version='overview-v2' AND granularity='hour' AND bucket_start BETWEEN $1 AND $2").bind(start).bind(available).execute(&pool).await.unwrap();
}

#[tokio::test]
#[ignore = "requires migrated isolated AETHER_TEST_DATABASE_URL"]
async fn live_overview_account_attribution_corrections_and_late_events() {
    let pool = sqlx::PgPool::connect(&std::env::var("AETHER_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let owner = uuid::Uuid::new_v4().to_string();
    let unrelated_user = uuid::Uuid::new_v4().to_string();
    let request = uuid::Uuid::new_v4().to_string();
    for user in [&owner, &unrelated_user] {
        sqlx::query("INSERT INTO users(id,username,email_verified) VALUES($1,$1,false)")
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();
    }
    let metadata = serde_json::json!({"analytics_attribution":{"is_standalone":false,"actor_user_id":unrelated_user},"analytics_failure":{"origin":"upstream","reason":"provider_error"}});
    sqlx::query("INSERT INTO usage(id,request_id,user_id,provider_name,model,status,request_metadata) VALUES($1,$1,$2,'test',$1,'failed',$3)").bind(&request).bind(&owner).bind(metadata.clone()).execute(&pool).await.unwrap();
    let repo = SqlxUsageReadRepository::new(pool.clone());
    let query = UsageAnalyticsQuery {
        from_unix_ms: (Utc::now() - chrono::Duration::hours(1)).timestamp_millis() as u64,
        to_unix_ms: (Utc::now() + chrono::Duration::hours(1)).timestamp_millis() as u64,
        timezone: "UTC".into(),
        view: UsageAnalyticsView::Users,
        model: Some(request.clone()),
        attribution_kind: Some("employee".into()),
        has_usage: Some(true),
        limit: 100,
        ..Default::default()
    };
    let result = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.users[0].user_id, owner);
    let identity: (String, String, String) = sqlx::query_as(
        "SELECT actor_user_id,credential_owner_id,attribution_source FROM usage_analytics_facts_v1 WHERE request_id=$1",
    )
    .bind(&request)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        identity,
        (owner.clone(), owner.clone(), "user_account".into())
    );
    sqlx::query("UPDATE usage SET request_metadata='{}',status='completed' WHERE request_id=$1")
        .bind(&request)
        .execute(&pool)
        .await
        .unwrap();
    let result = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(result.users[0].user_id, owner);
    let failure: Option<String> =
        sqlx::query_scalar("SELECT failure_origin FROM usage WHERE request_id=$1")
            .bind(&request)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(failure, None);
    let correction = UsageAttributionSnapshot {
        request_id: request.clone(),
        actor_user_id: Some(owner.clone()),
        credential_owner_id: Some(owner.clone()),
        attribution_kind: "employee".into(),
        attribution_source: "user_account".into(),
        record_kind: "session".into(),
        parent_request_id: None,
        schema_version: 1,
        attribution_revision: 3,
    };
    assert!(repo
        .correct_usage_attribution(&correction, 2)
        .await
        .unwrap());
    assert!(!repo
        .correct_usage_attribution(&correction, 2)
        .await
        .unwrap());
    sqlx::query("UPDATE usage SET request_metadata=$2 WHERE request_id=$1")
        .bind(&request)
        .bind(metadata.clone())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.query_usage_analytics(&query)
            .await
            .unwrap()
            .summary
            .request_count,
        0
    );
    sqlx::query("UPDATE users SET is_deleted=true WHERE id=$1")
        .bind(&owner)
        .execute(&pool)
        .await
        .unwrap();
    let identities:(Option<String>,Option<String>)=sqlx::query_as("SELECT actor_user_id,credential_owner_id FROM usage_analytics_facts_v1 WHERE request_id=$1").bind(&request).fetch_one(&pool).await.unwrap();
    assert_eq!(identities, (None, None));
    sqlx::query("UPDATE users SET is_deleted=false WHERE id=$1")
        .bind(&owner)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE usage SET request_metadata=$2 WHERE request_id=$1")
        .bind(&request)
        .bind(metadata)
        .execute(&pool)
        .await
        .unwrap();
    let anonymized: (Option<String>, Option<String>, i64) = sqlx::query_as(
        "SELECT actor_user_id,credential_owner_id,attribution_revision FROM usage_attribution_snapshots WHERE request_id=$1",
    )
    .bind(&request)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(anonymized, (None, None, 13));
    let identities: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT actor_user_id,credential_owner_id FROM usage_analytics_facts_v1 WHERE request_id=$1",
    )
    .bind(&request)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(identities, (None, None));
    sqlx::query("DELETE FROM usage WHERE request_id=$1")
        .bind(&request)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=ANY($1)")
        .bind(vec![owner, unrelated_user])
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires migrated isolated AETHER_TEST_DATABASE_URL"]
async fn live_overview_existing_key_types_classify_usage_without_snapshots() {
    let pool = sqlx::PgPool::connect(&std::env::var("AETHER_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let owner = uuid::Uuid::new_v4().to_string();
    let member_key = uuid::Uuid::new_v4().to_string();
    let standalone_key = uuid::Uuid::new_v4().to_string();
    let requests = [
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
    ];
    sqlx::query("INSERT INTO users(id,username,email_verified) VALUES($1,$1,false)")
        .bind(&owner)
        .execute(&pool)
        .await
        .unwrap();
    for (key, is_standalone) in [(&member_key, false), (&standalone_key, true)] {
        sqlx::query("INSERT INTO api_keys(id,user_id,key_hash,is_standalone) VALUES($1,$2,$1,$3)")
            .bind(key)
            .bind(&owner)
            .bind(is_standalone)
            .execute(&pool)
            .await
            .unwrap();
    }
    for (request, key) in requests.iter().zip([&member_key, &standalone_key]) {
        sqlx::query("INSERT INTO usage(id,request_id,user_id,api_key_id,provider_name,model,status) VALUES($1,$1,$2,$3,'test',$2,'completed')")
            .bind(request).bind(&owner).bind(key).execute(&pool).await.unwrap();
    }
    // Simulate retained usage written before attribution snapshots existed.
    sqlx::query("DELETE FROM usage_attribution_snapshots WHERE request_id=ANY($1)")
        .bind(requests.to_vec())
        .execute(&pool)
        .await
        .unwrap();
    for (request, actor, kind, source) in [
        (
            &requests[0],
            Some(owner.clone()),
            "employee",
            "user_account",
        ),
        (&requests[1], None, "standalone", "standalone_key"),
    ] {
        let identity: (Option<String>, Option<String>, String, String) = sqlx::query_as(
            "SELECT actor_user_id,credential_owner_id,attribution_kind,attribution_source FROM usage_analytics_facts_v1 WHERE request_id=$1",
        )
        .bind(request)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            identity,
            (actor, Some(owner.clone()), kind.into(), source.into())
        );
    }
    let repo = SqlxUsageReadRepository::new(pool.clone());
    let query = UsageAnalyticsQuery {
        from_unix_ms: (Utc::now() - chrono::Duration::hours(1)).timestamp_millis() as u64,
        to_unix_ms: (Utc::now() + chrono::Duration::hours(1)).timestamp_millis() as u64,
        timezone: "UTC".into(),
        model: Some(owner.clone()),
        limit: 100,
        ..Default::default()
    };
    let summary = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(summary.summary.request_count, 2);
    assert_eq!(summary.summary.trusted_attribution_count, 1);
    let users = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            view: UsageAnalyticsView::Users,
            attribution_kind: Some("employee".into()),
            has_usage: Some(true),
            ..query.clone()
        })
        .await
        .unwrap();
    assert_eq!(users.total, 1);
    assert_eq!(users.users[0].user_id, owner);
    assert_eq!(users.users[0].metrics.request_count, 1);
    let standalone = repo
        .query_usage_analytics(&UsageAnalyticsQuery {
            attribution_kind: Some("standalone".into()),
            ..query
        })
        .await
        .unwrap();
    assert_eq!(standalone.summary.request_count, 1);
    assert_eq!(standalone.summary.trusted_attribution_count, 0);
    let snapshots: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_attribution_snapshots WHERE request_id=ANY($1)",
    )
    .bind(requests.to_vec())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(snapshots, 0);
    sqlx::query("DELETE FROM usage WHERE request_id=ANY($1)")
        .bind(requests.to_vec())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM api_keys WHERE id=ANY($1)")
        .bind(vec![member_key, standalone_key])
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(&owner)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires migrated isolated AETHER_TEST_DATABASE_URL"]
async fn live_overview_cache_pricing_and_record_drilldown() {
    let pool = sqlx::PgPool::connect(&std::env::var("AETHER_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let owner = uuid::Uuid::new_v4().to_string();
    let request = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO users(id,username,email_verified) VALUES($1,$1,false)")
        .bind(&owner)
        .execute(&pool)
        .await
        .unwrap();
    let metadata = serde_json::json!({"analytics_attribution":{"is_standalone":false},"analytics_measurement":{"source":"reported"}});
    for (id, latency) in [(&request, 6000), (&other, 100)] {
        sqlx::query("INSERT INTO usage(id,request_id,user_id,api_key_id,provider_id,provider_name,model,api_format,endpoint_kind,request_type,is_stream,has_format_conversion,status,billing_status,response_time_ms,input_tokens,output_tokens,cache_read_input_tokens,request_metadata) VALUES($1,$1,$2,$2,$2,'test',$2,'openai:chat','chat','chat',true,true,'completed','settled',$3,1000,20,100,$4)")
            .bind(id).bind(&owner).bind(latency).bind(metadata.clone()).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO usage_settlement_snapshots(request_id,billing_status,input_price_per_1m,billing_cache_read_cost_usd,billing_cache_creation_cost_usd) VALUES($1,'settled',2,0.00005,0.00008)").bind(&request).execute(&pool).await.unwrap();
    let repo = SqlxUsageReadRepository::new(pool.clone());
    let query = UsageAnalyticsQuery {
        from_unix_ms: (Utc::now() - chrono::Duration::hours(1)).timestamp_millis() as u64,
        to_unix_ms: (Utc::now() + chrono::Duration::hours(1)).timestamp_millis() as u64,
        timezone: "UTC".into(),
        model: Some(owner.clone()),
        limit: 100,
        ..Default::default()
    };
    let result = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(result.summary.reported_usage_count, 2);
    assert_eq!(result.summary.cache_pricing_available_count, 1);
    assert_eq!(
        result.summary.cache_estimated_full_cost_amount.as_deref(),
        Some("0.00020000")
    );
    assert_eq!(
        result.summary.cache_read_cost_amount.as_deref(),
        Some("0.00005000")
    );
    assert_eq!(
        result.summary.cache_creation_cost_amount.as_deref(),
        Some("0.00008000")
    );
    let list = UsageAuditListQuery {
        provider_id: Some(owner.clone()),
        api_key_id: Some(owner.clone()),
        actor_user_id: Some(owner.clone()),
        attribution_kind: Some("employee".into()),
        slow_threshold_ms: Some(5000),
        endpoint_kind: Some("chat".into()),
        request_type: Some("chat".into()),
        has_format_conversion: Some(true),
        is_stream: Some(true),
        ..Default::default()
    };
    assert_eq!(repo.count_usage_audits(&list).await.unwrap(), 1);
    assert_eq!(
        repo.list_usage_audits(&list).await.unwrap()[0].request_id,
        request
    );
    let search = UsageAuditKeywordSearchQuery {
        provider_id: list.provider_id.clone(),
        api_key_id: list.api_key_id.clone(),
        request_id: Some(request.clone()),
        actor_user_id: list.actor_user_id.clone(),
        attribution_kind: list.attribution_kind.clone(),
        slow_threshold_ms: list.slow_threshold_ms,
        endpoint_kind: list.endpoint_kind.clone(),
        request_type: list.request_type.clone(),
        has_format_conversion: list.has_format_conversion,
        is_stream: list.is_stream,
        keywords: vec![owner.clone()],
        ..Default::default()
    };
    assert_eq!(
        repo.count_usage_audits_by_keyword_search(&search)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        repo.list_usage_audits_by_keyword_search(&search)
            .await
            .unwrap()[0]
            .request_id,
        request
    );
    assert_eq!(
        repo.count_usage_audits(&UsageAuditListQuery {
            provider_id: Some("no-such-provider".into()),
            ..list.clone()
        })
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        repo.count_usage_audits_by_keyword_search(&UsageAuditKeywordSearchQuery {
            request_id: Some(other.clone()),
            ..search
        })
        .await
        .unwrap(),
        0
    );
    sqlx::query("UPDATE usage SET request_metadata=request_metadata::jsonb||'{\"usage_available\":false}'::jsonb WHERE request_id=$1").bind(&request).execute(&pool).await.unwrap();
    let unavailable = repo.query_usage_analytics(&query).await.unwrap();
    assert_eq!(unavailable.summary.reported_usage_count, 1);
    assert_eq!(unavailable.summary.unknown_usage_count, 1);
    assert_eq!(unavailable.summary.cache_estimated_full_cost_amount, None);
    sqlx::query("DELETE FROM usage_settlement_snapshots WHERE request_id=$1")
        .bind(&request)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM usage WHERE request_id=ANY($1)")
        .bind(vec![request, other])
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(&owner)
        .execute(&pool)
        .await
        .unwrap();
}

fn assert_dashboard_total_matches_canonical(
    total: &UsageAnalyticsMetrics,
    canonical: &UsageAnalyticsMetrics,
) {
    assert_eq!(total.request_count, canonical.request_count);
    assert_eq!(total.total_tokens, canonical.total_tokens);
    assert_eq!(total.billable_amount, canonical.billable_amount);
    assert_eq!(total.usage_available_count, canonical.usage_available_count);
    assert_eq!(
        total.pricing_available_count,
        canonical.pricing_available_count
    );
    assert_eq!(total.settled_count, canonical.settled_count);
    assert_eq!(
        total.allocation_available_count,
        canonical.allocation_available_count
    );
}

#[tokio::test]
#[ignore = "requires migrated isolated AETHER_TEST_DATABASE_URL"]
async fn live_overview_dashboard_total_matches_canonical_settlement_and_legacy_tokens() {
    let pool = sqlx::PgPool::connect(&std::env::var("AETHER_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let user = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO users(id,username,email_verified,is_active) VALUES($1,$1,false,true)")
        .bind(&user)
        .execute(&mut *tx)
        .await
        .unwrap();
    let start = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
    for (case, api_format, stored_total, metadata, expected_tokens) in [
        (
            "openai-cache",
            "openai:chat",
            0_i64,
            serde_json::json!({}),
            120_u64,
        ),
        (
            "anthropic-cache",
            "claude:messages",
            0,
            serde_json::json!({}),
            138,
        ),
        (
            "gemini-cache",
            "gemini:generateContent",
            0,
            serde_json::json!({}),
            120,
        ),
        (
            "explicit-total",
            "claude:messages",
            777,
            serde_json::json!({}),
            777,
        ),
        (
            "snapshot-effective",
            "openai:chat",
            777,
            serde_json::json!({}),
            27,
        ),
        (
            "snapshot-context",
            "claude:messages",
            777,
            serde_json::json!({}),
            1002,
        ),
        (
            "billing-snapshot",
            "openai:chat",
            120,
            serde_json::json!({
                "billing_multiplier_snapshot": {
                    "version": 1,
                    "factors": {"routing_group": 2.0, "user_group": 0.75},
                    "multiplier": 1.5
                }
            }),
            120,
        ),
        (
            "unavailable",
            "openai:chat",
            120,
            serde_json::json!({"usage_available":false,"usage_pricing_available":false}),
            0,
        ),
        (
            "null-metadata",
            "openai:chat",
            120,
            serde_json::json!(null),
            120,
        ),
        (
            "array-metadata",
            "openai:chat",
            120,
            serde_json::json!([]),
            120,
        ),
        (
            "scalar-metadata",
            "openai:chat",
            120,
            serde_json::json!(false),
            120,
        ),
        (
            "string-false",
            "openai:chat",
            120,
            serde_json::json!({"usage_available":"false","usage_pricing_available":"false"}),
            120,
        ),
        (
            "session",
            "openai:chat",
            120,
            serde_json::json!({"analytics_attribution":{"record_kind":"session"}}),
            0,
        ),
        (
            "standalone",
            "openai:chat",
            120,
            serde_json::json!({"analytics_attribution":{"is_standalone":true},"analytics_measurement":{"source":"estimated"}}),
            120,
        ),
    ] {
        let request = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO usage(id,request_id,user_id,model,provider_name,api_format,status,billing_status,input_tokens,output_tokens,total_tokens,cache_creation_input_tokens,cache_creation_input_tokens_5m,cache_creation_input_tokens_1h,cache_read_input_tokens,total_cost_usd,actual_total_cost_usd,created_at,request_metadata) VALUES($1,$1,$2,$1,'test',$3,'completed','settled',100,20,$4,0,3,4,11,0.25,0.12500001,$5,$6)")
            .bind(&request).bind(&user).bind(api_format).bind(stored_total).bind(start).bind(metadata)
            .execute(&mut *tx).await.unwrap();
        if case.starts_with("snapshot-") {
            sqlx::query("INSERT INTO usage_settlement_snapshots(request_id,billing_status,billing_effective_input_tokens,billing_total_input_context,billing_output_tokens,billing_cache_creation_5m_tokens,billing_cache_creation_1h_tokens,billing_cache_read_tokens,billing_total_cost_usd,billing_actual_total_cost_usd,allocation_status) VALUES($1,'settled',$2,$3,2,3,5,7,1.00000001,0.90000001,'complete')")
                .bind(&request)
                .bind((case == "snapshot-effective").then_some(10_i64))
                .bind((case == "snapshot-context").then_some(1000_i64))
                .execute(&mut *tx).await.unwrap();
        }
        let query = UsageAnalyticsQuery {
            from_unix_ms: start.timestamp_millis() as u64,
            to_unix_ms: (start + chrono::Duration::hours(1)).timestamp_millis() as u64,
            model: Some(request),
            ..Default::default()
        };
        let canonical = super::analytics::read_analytics_metrics(&mut tx, &query, false)
            .await
            .unwrap();
        let total = super::dashboard::read_dashboard_total_metrics(&mut tx, &query, false)
            .await
            .unwrap();
        assert_eq!(total.total_tokens, expected_tokens, "{case}");
        if case == "billing-snapshot" {
            assert_eq!(total.billable_amount.as_deref(), Some("0.37500000"));
        }
        assert_dashboard_total_matches_canonical(&total, &canonical);
    }
    tx.rollback().await.unwrap();
}

#[tokio::test]
#[ignore = "requires migrated isolated AETHER_TEST_DATABASE_URL"]
async fn live_customer_billing_amount_matches_canonical_and_dashboard_facts() {
    let pool = sqlx::PgPool::connect(&std::env::var("AETHER_TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let start = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
    let composite = serde_json::json!({
        "billing_multiplier_snapshot": {
            "version": 1, "factors": {"routing_group": 2.0, "user_group": 0.75}, "multiplier": 1.5
        },
        "routing_group_billing_multiplier": 99.0,
        "rate_multiplier": 0.25
    });
    for (case, metadata, expected) in [
        ("legacy", serde_json::json!({}), Some("0.50000000")),
        ("composite", composite.clone(), Some("3.00000000")),
        ("settlement-base", composite, Some("6.00000000")),
        (
            "free",
            serde_json::json!({"routing_group_billing_multiplier": 0}),
            Some("0.00000000"),
        ),
        (
            "null",
            serde_json::json!({"billing_multiplier_snapshot": null, "routing_group_billing_multiplier": 1}),
            None,
        ),
        (
            "negative-factor",
            serde_json::json!({"billing_multiplier_snapshot": {"version": 1, "factors": {"routing_group": -1}, "multiplier": 1}}),
            None,
        ),
        (
            "mismatch",
            serde_json::json!({"billing_multiplier_snapshot": {"version": 1, "factors": {"routing_group": 2}, "multiplier": 1}}),
            None,
        ),
        (
            "negative-legacy",
            serde_json::json!({"routing_group_billing_multiplier": -1}),
            None,
        ),
        (
            "string-legacy",
            serde_json::json!({"routing_group_billing_multiplier": "1"}),
            None,
        ),
        (
            "zero-before-overflow",
            serde_json::json!({"billing_multiplier_snapshot": {"version": 1, "factors": {"a": 1e308, "b": 1e308, "z": 0}, "multiplier": 0}}),
            Some("0.00000000"),
        ),
        (
            "overflow",
            serde_json::json!({"billing_multiplier_snapshot": {"version": 1, "factors": {"a": 1e308, "b": 1e308}, "multiplier": 1}}),
            None,
        ),
        (
            "bad-key",
            serde_json::json!({"billing_multiplier_snapshot": {"version": 1, "factors": {"routing-group": 2}, "multiplier": 2}}),
            None,
        ),
    ] {
        let request = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO usage(id,request_id,model,provider_name,status,billing_status,total_cost_usd,actual_total_cost_usd,created_at,request_metadata) VALUES($1,$1,$1,'billing-test','completed','settled',2,0.5,$2,$3)")
            .bind(&request).bind(start).bind(metadata).execute(&mut *tx).await.unwrap();
        if case == "settlement-base" {
            sqlx::query("INSERT INTO usage_settlement_snapshots(request_id,billing_status,billing_total_cost_usd,billing_actual_total_cost_usd) VALUES($1,'settled',4,0.25)")
                .bind(&request).execute(&mut *tx).await.unwrap();
        }
        let amount: Option<String> = sqlx::query_scalar(
            "SELECT billable_amount::text FROM usage_analytics_facts_v1 WHERE request_id=$1",
        )
        .bind(&request)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(amount.as_deref(), expected, "{case}");
        let query = UsageAnalyticsQuery {
            from_unix_ms: start.timestamp_millis() as u64,
            to_unix_ms: (start + chrono::Duration::hours(1)).timestamp_millis() as u64,
            model: Some(request),
            ..Default::default()
        };
        let canonical = super::analytics::read_analytics_metrics(&mut tx, &query, false)
            .await
            .unwrap();
        let inline = super::dashboard::read_dashboard_total_metrics(&mut tx, &query, false)
            .await
            .unwrap();
        assert_dashboard_total_matches_canonical(&inline, &canonical);
        if let Some(expected) = expected {
            assert_eq!(
                canonical.billable_amount.as_deref(),
                Some(expected),
                "{case}"
            );
        }
    }
    tx.rollback().await.unwrap();
}
