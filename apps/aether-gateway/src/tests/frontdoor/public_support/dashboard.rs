use super::{
    build_test_auth_token, json, sample_auth_session, sample_auth_user, sample_auth_wallet,
    sample_provider, sample_user_usage_audit, start_auth_dashboard_gateway_with_state,
    start_auth_gateway_with_builder, start_auth_gateway_with_usage_state, AppState, Arc,
    GatewayDataState, InMemoryAuthApiKeySnapshotRepository, InMemoryProviderCatalogReadRepository,
    InMemoryUsageReadRepository, InMemoryUserReadRepository, InMemoryWalletRepository, StatusCode,
    StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot, StoredUserAuthRecord,
    StoredUserExportRow, Utc,
};

fn stable_dashboard_now() -> chrono::DateTime<Utc> {
    Utc::now()
        .date_naive()
        .and_hms_opt(12, 0, 0)
        .expect("stable dashboard test time should build")
        .and_utc()
}

#[tokio::test]
async fn gateway_handles_dashboard_stats_locally_without_proxying_upstream() {
    let now = stable_dashboard_now();
    let user = sample_auth_user(now);
    let access_token = build_test_auth_token(
        "access",
        serde_json::Map::from_iter([
            ("user_id".to_string(), json!(user.id)),
            ("role".to_string(), json!(user.role)),
            (
                "created_at".to_string(),
                json!(user.created_at.map(|value| value.to_rfc3339())),
            ),
            ("session_id".to_string(), json!("session-dashboard-stats")),
        ]),
        chrono::Utc::now() + chrono::Duration::hours(1),
    );
    let session = sample_auth_session(
        "user-auth-1",
        "session-dashboard-stats",
        "device-dashboard-stats",
        "refresh-dashboard-stats",
        now,
    );
    let current_usage = sample_user_usage_audit(
        "usage-dashboard-stats-1",
        "req-dashboard-stats-1",
        "user-auth-1",
        "gpt-5",
        "openai",
        "completed",
        now - chrono::Duration::minutes(10),
    );
    let mut prior_usage = sample_user_usage_audit(
        "usage-dashboard-stats-2",
        "req-dashboard-stats-2",
        "user-auth-1",
        "gpt-4.1",
        "openai",
        "completed",
        now - chrono::Duration::days(2),
    );
    prior_usage.actual_total_cost_usd = 0.75;
    let other_usage = sample_user_usage_audit(
        "usage-dashboard-stats-3",
        "req-dashboard-stats-3",
        "user-auth-2",
        "claude-3-7",
        "claude",
        "completed",
        now - chrono::Duration::minutes(3),
    );
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        current_usage,
        prior_usage,
        other_usage,
    ]));
    let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
        user.clone()
    ]));
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![sample_auth_wallet(
        "user-auth-1",
        now,
    )]));
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(
            Vec::<(Option<String>, StoredAuthApiKeySnapshot)>::new(),
        )
        .with_export_records(vec![
            StoredAuthApiKeyExportRecord::new(
                "user-auth-1".to_string(),
                "user-key-1".to_string(),
                "hash-user-key-1".to_string(),
                None,
                Some("primary".to_string()),
                Some(json!(["openai"])),
                Some(json!(["openai:chat"])),
                Some(json!(["gpt-5"])),
                Some(60),
                Some(5),
                None,
                true,
                None,
                false,
                5,
                0,
                1.5,
                false,
            )
            .expect("api key export should build"),
            StoredAuthApiKeyExportRecord::new(
                "user-auth-1".to_string(),
                "user-key-2".to_string(),
                "hash-user-key-2".to_string(),
                None,
                Some("secondary".to_string()),
                Some(json!(["openai"])),
                Some(json!(["openai:chat"])),
                Some(json!(["gpt-4.1"])),
                Some(60),
                Some(5),
                None,
                false,
                None,
                false,
                1,
                0,
                0.5,
                false,
            )
            .expect("api key export should build"),
        ]),
    );

    let (gateway_url, upstream_hits, gateway_handle, upstream_handle) =
        start_auth_gateway_with_builder(|| {
            let data_state = GatewayDataState::with_user_wallet_and_usage_for_tests(
                user_repository,
                wallet_repository,
                usage_repository,
            )
            .with_auth_api_key_reader(auth_repository);
            AppState::new()
                .expect("gateway should build")
                .with_data_state_for_tests(data_state)
                .with_auth_sessions_for_tests([session])
        })
        .await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/dashboard/stats?days=3"))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-dashboard-stats")
        .header("user-agent", "AetherTest/1.0")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["today"]["requests"], 1);
    assert_eq!(payload["today"]["tokens"], 160);
    assert_eq!(payload["api_keys"]["total"], 2);
    assert_eq!(payload["api_keys"]["active"], 1);
    assert_eq!(payload["stats"][3]["subValue"], json!("输入 240 / 输出 60"));
    assert_eq!(payload["token_breakdown"]["input"], 240);
    assert_eq!(payload["token_breakdown"]["output"], 60);
    assert_eq!(payload["token_breakdown"]["cache_creation"], 20);
    assert_eq!(payload["token_breakdown"]["cache_read"], 30);
    assert_eq!(payload["monthly_cost"], json!(2.5));
    assert_eq!(payload["stats"].as_array().map(Vec::len), Some(4));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_dashboard_stats_include_end_of_day_boundary() {
    let now = stable_dashboard_now();
    let user = sample_auth_user(now);
    let access_token = build_test_auth_token(
        "access",
        serde_json::Map::from_iter([
            ("user_id".to_string(), json!(user.id)),
            ("role".to_string(), json!(user.role)),
            (
                "created_at".to_string(),
                json!(user.created_at.map(|value| value.to_rfc3339())),
            ),
            ("session_id".to_string(), json!("session-dashboard-day-end")),
        ]),
        chrono::Utc::now() + chrono::Duration::hours(1),
    );
    let session = sample_auth_session(
        "user-auth-1",
        "session-dashboard-day-end",
        "device-dashboard-day-end",
        "refresh-dashboard-day-end",
        now,
    );
    let day = now.date_naive();
    let end_of_day = day
        .and_hms_opt(23, 59, 59)
        .expect("end of day should build")
        .and_utc();
    let next_midnight = day
        .checked_add_signed(chrono::Duration::days(1))
        .expect("next day should build")
        .and_hms_opt(0, 0, 0)
        .expect("next midnight should build")
        .and_utc();
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_user_usage_audit(
            "usage-dashboard-day-end",
            "req-dashboard-day-end",
            "user-auth-1",
            "gpt-5",
            "openai",
            "completed",
            end_of_day,
        ),
        sample_user_usage_audit(
            "usage-dashboard-next-midnight",
            "req-dashboard-next-midnight",
            "user-auth-1",
            "gpt-5",
            "openai",
            "completed",
            next_midnight,
        ),
    ]));

    let (gateway_url, upstream_hits, gateway_handle, upstream_handle) =
        start_auth_gateway_with_usage_state(
            user,
            sample_auth_wallet("user-auth-1", now),
            [session],
            usage_repository,
        )
        .await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/dashboard/stats?start_date={day}&end_date={day}"
        ))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-dashboard-day-end")
        .header("user-agent", "AetherTest/1.0")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["today"]["requests"], 1);
    assert_eq!(payload["monthly_cost"], json!(1.25));
    assert_eq!(payload["stats"][1]["value"], json!("1"));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_dashboard_stats_locally_without_proxying_upstream() {
    let now = stable_dashboard_now();
    let admin = StoredUserAuthRecord::new(
        "admin-auth-1".to_string(),
        Some("admin@example.com".to_string()),
        true,
        "admin".to_string(),
        Some("$2y$10$.OBQfixAECpsb8V/VS3csOMf00x2E/jD/gnud20t6RG0yiQosyOZ2".to_string()),
        "admin".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        Some(now),
        Some(now),
    )
    .expect("admin auth user should build");
    let access_token = build_test_auth_token(
        "access",
        serde_json::Map::from_iter([
            ("user_id".to_string(), json!(admin.id)),
            ("role".to_string(), json!(admin.role)),
            (
                "created_at".to_string(),
                json!(admin.created_at.map(|value| value.to_rfc3339())),
            ),
            (
                "session_id".to_string(),
                json!("session-dashboard-stats-admin"),
            ),
        ]),
        chrono::Utc::now() + chrono::Duration::hours(1),
    );
    let session = sample_auth_session(
        "admin-auth-1",
        "session-dashboard-stats-admin",
        "device-dashboard-stats-admin",
        "refresh-dashboard-stats-admin",
        now,
    );
    let mut openai_usage = sample_user_usage_audit(
        "usage-dashboard-admin-1",
        "req-dashboard-admin-1",
        "user-auth-1",
        "gpt-5",
        "openai",
        "completed",
        now - chrono::Duration::minutes(10),
    );
    openai_usage.input_tokens = 12_000;
    openai_usage.output_tokens = 3_000;
    openai_usage.total_tokens = 15_000;
    openai_usage.cache_creation_input_tokens = 1_200;
    openai_usage.cache_creation_ephemeral_5m_input_tokens = 600;
    openai_usage.cache_creation_ephemeral_1h_input_tokens = 600;
    openai_usage.cache_read_input_tokens = 800;
    openai_usage.cache_read_cost_usd = 0.01;
    openai_usage.output_price_per_1m = Some(100.0);

    let mut claude_usage = sample_user_usage_audit(
        "usage-dashboard-admin-2",
        "req-dashboard-admin-2",
        "user-auth-2",
        "claude-3-7",
        "claude",
        "completed",
        now - chrono::Duration::minutes(5),
    );
    claude_usage.input_tokens = 900;
    claude_usage.output_tokens = 100;
    claude_usage.total_tokens = 1_000;
    claude_usage.api_format = Some("claude:messages".to_string());
    claude_usage.endpoint_api_format = Some("claude:messages".to_string());
    claude_usage.cache_creation_input_tokens = 50;
    claude_usage.cache_creation_ephemeral_5m_input_tokens = 20;
    claude_usage.cache_creation_ephemeral_1h_input_tokens = 30;
    claude_usage.cache_read_input_tokens = 200;
    claude_usage.cache_read_cost_usd = 0.005;
    claude_usage.output_price_per_1m = Some(100.0);

    let mut streaming_usage = sample_user_usage_audit(
        "usage-dashboard-admin-3",
        "req-dashboard-admin-3",
        "user-auth-3",
        "gpt-4.1",
        "openai",
        "streaming",
        now - chrono::Duration::minutes(1),
    );
    streaming_usage.cache_read_input_tokens = 0;
    streaming_usage.cache_read_cost_usd = 0.0;

    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        openai_usage,
        claude_usage,
        streaming_usage,
    ]));
    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![admin.clone()]).with_export_users(vec![
            StoredUserExportRow::new(
                "user-auth-1".to_string(),
                Some("alice@example.com".to_string()),
                true,
                "alice".to_string(),
                Some("hash".to_string()),
                "user".to_string(),
                "local".to_string(),
                Some(json!(["openai"])),
                Some(json!(["openai:chat"])),
                Some(json!(["gpt-5"])),
                Some(60),
                None,
                true,
            )
            .expect("user export row should build"),
            StoredUserExportRow::new(
                "user-auth-2".to_string(),
                Some("bob@example.com".to_string()),
                true,
                "bob".to_string(),
                Some("hash".to_string()),
                "user".to_string(),
                "local".to_string(),
                Some(json!(["anthropic"])),
                Some(json!(["claude:messages"])),
                Some(json!(["claude-3-7"])),
                Some(30),
                None,
                false,
            )
            .expect("user export row should build"),
        ]),
    );
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![sample_auth_wallet(
        "admin-auth-1",
        now,
    )]));
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(
            Vec::<(Option<String>, StoredAuthApiKeySnapshot)>::new(),
        )
        .with_export_records(vec![
            StoredAuthApiKeyExportRecord::new(
                "user-auth-1".to_string(),
                "user-key-1".to_string(),
                "hash-user-key-1".to_string(),
                None,
                Some("primary".to_string()),
                Some(json!(["openai"])),
                Some(json!(["openai:chat"])),
                Some(json!(["gpt-5"])),
                Some(60),
                Some(5),
                None,
                true,
                None,
                false,
                5,
                0,
                1.5,
                false,
            )
            .expect("api key export should build"),
            StoredAuthApiKeyExportRecord::new(
                "user-auth-2".to_string(),
                "user-key-2".to_string(),
                "hash-user-key-2".to_string(),
                None,
                Some("secondary".to_string()),
                Some(json!(["anthropic"])),
                Some(json!(["claude:messages"])),
                Some(json!(["claude-3-7"])),
                Some(60),
                Some(5),
                None,
                false,
                None,
                false,
                1,
                0,
                0.5,
                false,
            )
            .expect("api key export should build"),
            StoredAuthApiKeyExportRecord::new(
                "admin-auth-1".to_string(),
                "standalone-key-1".to_string(),
                "hash-standalone-key-1".to_string(),
                None,
                Some("standalone".to_string()),
                Some(json!(["openai"])),
                Some(json!(["openai:chat"])),
                Some(json!(["gpt-5"])),
                Some(60),
                Some(5),
                None,
                true,
                None,
                false,
                3,
                0,
                0.75,
                true,
            )
            .expect("standalone api key export should build"),
        ]),
    );

    let (gateway_url, upstream_hits, gateway_handle, upstream_handle) =
        start_auth_gateway_with_builder(|| {
            let data_state = GatewayDataState::with_user_wallet_and_usage_for_tests(
                user_repository,
                wallet_repository,
                usage_repository,
            )
            .with_auth_api_key_reader(auth_repository);
            AppState::new()
                .expect("gateway should build")
                .with_data_state_for_tests(data_state)
                .with_auth_sessions_for_tests([session])
        })
        .await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/dashboard/stats"))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-dashboard-stats-admin")
        .header("user-agent", "AetherTest/1.0")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["today"]["requests"], 2);
    assert_eq!(payload["today"]["tokens"], 17_450);
    assert_eq!(payload["today"]["cost"], json!(2.5));
    assert_eq!(payload["cost_stats"]["cost_savings"], json!(0.085));
    assert_eq!(payload["stats"][2]["subValue"], json!("节省 $0.09"));
    assert_eq!(payload["stats"][0]["value"], json!("2"));
    assert_eq!(payload["stats"][1]["value"], json!("17.4K"));
    assert_eq!(
        payload["stats"][1]["subValue"],
        json!("输入 12.1K / 输出 3.1K · 写缓存 1.25K / 读缓存 1K")
    );
    assert_eq!(payload["users"]["total"], 2);
    assert_eq!(payload["users"]["active"], 1);
    assert_eq!(payload["api_keys"]["total"], 3);
    assert_eq!(payload["api_keys"]["active"], 2);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_dashboard_daily_stats_locally_without_proxying_upstream() {
    let now = stable_dashboard_now();
    let admin = StoredUserAuthRecord::new(
        "admin-auth-1".to_string(),
        Some("admin@example.com".to_string()),
        true,
        "admin".to_string(),
        Some("$2y$10$.OBQfixAECpsb8V/VS3csOMf00x2E/jD/gnud20t6RG0yiQosyOZ2".to_string()),
        "admin".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        Some(now),
        Some(now),
    )
    .expect("admin auth user should build");
    let access_token = build_test_auth_token(
        "access",
        serde_json::Map::from_iter([
            ("user_id".to_string(), json!(admin.id)),
            ("role".to_string(), json!(admin.role)),
            (
                "created_at".to_string(),
                json!(admin.created_at.map(|value| value.to_rfc3339())),
            ),
            (
                "session_id".to_string(),
                json!("session-dashboard-daily-stats"),
            ),
        ]),
        chrono::Utc::now() + chrono::Duration::hours(1),
    );
    let session = sample_auth_session(
        "admin-auth-1",
        "session-dashboard-daily-stats",
        "device-dashboard-daily-stats",
        "refresh-dashboard-daily-stats",
        now,
    );
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_user_usage_audit(
            "usage-dashboard-daily-1",
            "req-dashboard-daily-1",
            "user-auth-1",
            "gpt-5",
            "openai",
            "completed",
            now - chrono::Duration::hours(1),
        ),
        sample_user_usage_audit(
            "usage-dashboard-daily-2",
            "req-dashboard-daily-2",
            "user-auth-2",
            "claude-3-7",
            "claude",
            "completed",
            now - chrono::Duration::hours(2),
        ),
        sample_user_usage_audit(
            "usage-dashboard-daily-3",
            "req-dashboard-daily-3",
            "user-auth-3",
            "gpt-5",
            "openai",
            "completed",
            now - chrono::Duration::days(1) - chrono::Duration::hours(2),
        ),
    ]));

    let (gateway_url, upstream_hits, gateway_handle, upstream_handle) =
        start_auth_gateway_with_builder(|| {
            let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
                admin.clone(),
            ]));
            let wallet_repository =
                Arc::new(InMemoryWalletRepository::seed(vec![sample_auth_wallet(
                    "admin-auth-1",
                    now,
                )]));
            let data_state = GatewayDataState::with_user_wallet_and_usage_for_tests(
                user_repository,
                wallet_repository,
                usage_repository,
            );
            AppState::new()
                .expect("gateway should build")
                .with_data_state_for_tests(data_state)
                .with_auth_sessions_for_tests([session])
        })
        .await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/dashboard/daily-stats?days=2"))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-dashboard-daily-stats")
        .header("user-agent", "AetherTest/1.0")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let daily_stats = payload["daily_stats"]
        .as_array()
        .expect("daily stats should be array");
    assert_eq!(daily_stats.len(), 2);
    assert_eq!(
        daily_stats[0]["date"],
        json!((now - chrono::Duration::days(1)).date_naive().to_string())
    );
    assert_eq!(daily_stats[0]["requests"], 1);
    assert_eq!(daily_stats[0]["unique_providers"], 1);
    assert_eq!(daily_stats[1]["date"], json!(now.date_naive().to_string()));
    assert_eq!(daily_stats[1]["requests"], 2);
    assert_eq!(daily_stats[1]["unique_models"], 2);
    assert_eq!(daily_stats[1]["unique_providers"], 2);
    assert_eq!(
        daily_stats[1]["model_breakdown"].as_array().map(Vec::len),
        Some(2)
    );

    let model_summary = payload["model_summary"]
        .as_array()
        .expect("model summary should exist");
    assert_eq!(model_summary.len(), 2);
    assert_eq!(model_summary[0]["model"], "gpt-5");
    assert_eq!(model_summary[0]["requests"], 2);

    let provider_summary = payload["provider_summary"]
        .as_array()
        .expect("provider summary should exist");
    assert_eq!(provider_summary.len(), 2);
    assert_eq!(provider_summary[0]["provider"], "openai");
    assert_eq!(provider_summary[0]["requests"], 2);
    assert_eq!(provider_summary[1]["provider"], "claude");
    assert_eq!(provider_summary[1]["requests"], 1);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_user_dashboard_daily_stats_locally_without_proxying_upstream() {
    let now = stable_dashboard_now();
    let user = sample_auth_user(now);
    let access_token = build_test_auth_token(
        "access",
        serde_json::Map::from_iter([
            ("user_id".to_string(), json!(user.id)),
            ("role".to_string(), json!(user.role)),
            (
                "created_at".to_string(),
                json!(user.created_at.map(|value| value.to_rfc3339())),
            ),
            (
                "session_id".to_string(),
                json!("session-dashboard-daily-stats-user"),
            ),
        ]),
        chrono::Utc::now() + chrono::Duration::hours(1),
    );
    let session = sample_auth_session(
        "user-auth-1",
        "session-dashboard-daily-stats-user",
        "device-dashboard-daily-stats-user",
        "refresh-dashboard-daily-stats-user",
        now,
    );
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_user_usage_audit(
            "usage-dashboard-user-daily-1",
            "req-dashboard-user-daily-1",
            "user-auth-1",
            "gpt-5",
            "openai",
            "completed",
            now - chrono::Duration::hours(1),
        ),
        sample_user_usage_audit(
            "usage-dashboard-user-daily-2",
            "req-dashboard-user-daily-2",
            "user-auth-1",
            "gpt-4.1",
            "openai",
            "completed",
            now - chrono::Duration::days(1) - chrono::Duration::hours(2),
        ),
        sample_user_usage_audit(
            "usage-dashboard-user-daily-3",
            "req-dashboard-user-daily-3",
            "user-auth-2",
            "claude-3-7",
            "claude",
            "completed",
            now - chrono::Duration::hours(2),
        ),
    ]));

    let (gateway_url, upstream_hits, gateway_handle, upstream_handle) =
        start_auth_gateway_with_builder(|| {
            let user_repository = Arc::new(InMemoryUserReadRepository::seed_auth_users(vec![
                user.clone()
            ]));
            let wallet_repository =
                Arc::new(InMemoryWalletRepository::seed(vec![sample_auth_wallet(
                    "user-auth-1",
                    now,
                )]));
            let data_state = GatewayDataState::with_user_wallet_and_usage_for_tests(
                user_repository,
                wallet_repository,
                usage_repository,
            );
            AppState::new()
                .expect("gateway should build")
                .with_data_state_for_tests(data_state)
                .with_auth_sessions_for_tests([session])
        })
        .await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/dashboard/daily-stats?days=2"))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-dashboard-daily-stats-user")
        .header("user-agent", "AetherTest/1.0")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let daily_stats = payload["daily_stats"]
        .as_array()
        .expect("daily stats should be array");
    assert_eq!(daily_stats.len(), 2);
    assert_eq!(daily_stats[0]["requests"], 1);
    assert_eq!(daily_stats[1]["requests"], 1);
    assert_eq!(
        daily_stats[1]["model_breakdown"].as_array().map(Vec::len),
        Some(1)
    );

    let model_summary = payload["model_summary"]
        .as_array()
        .expect("model summary should exist");
    assert_eq!(model_summary.len(), 2);
    assert_eq!(payload.get("provider_summary"), None);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_dashboard_recent_requests_locally_without_proxying_upstream() {
    let now = Utc::now();
    let user = sample_auth_user(now);
    let access_token = build_test_auth_token(
        "access",
        serde_json::Map::from_iter([
            ("user_id".to_string(), json!(user.id)),
            ("role".to_string(), json!(user.role)),
            (
                "created_at".to_string(),
                json!(user.created_at.map(|value| value.to_rfc3339())),
            ),
            ("session_id".to_string(), json!("session-dashboard-recent")),
        ]),
        now + chrono::Duration::hours(1),
    );
    let session = sample_auth_session(
        "user-auth-1",
        "session-dashboard-recent",
        "device-dashboard-recent",
        "refresh-dashboard-recent",
        now,
    );
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_user_usage_audit(
            "usage-dashboard-1",
            "req-dashboard-1",
            "user-auth-1",
            "gpt-5",
            "OpenAI",
            "completed",
            now - chrono::Duration::minutes(5),
        ),
        sample_user_usage_audit(
            "usage-dashboard-2",
            "req-dashboard-2",
            "user-auth-2",
            "claude-3-7",
            "Anthropic",
            "completed",
            now - chrono::Duration::minutes(2),
        ),
    ]));
    let (gateway_url, upstream_hits, gateway_handle, upstream_handle) =
        start_auth_gateway_with_usage_state(
            user,
            sample_auth_wallet("user-auth-1", now),
            [session],
            usage_repository,
        )
        .await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/dashboard/recent-requests?limit=5"
        ))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-dashboard-recent")
        .header("user-agent", "AetherTest/1.0")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let requests = payload["requests"].as_array().expect("array");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["id"], "usage-dashboard-1");
    assert_eq!(requests[0]["user"], "alice");
    assert_eq!(requests[0]["model"], "gpt-5");
    assert_eq!(requests[0]["tokens"], 150);
    assert_eq!(requests[0]["is_stream"], false);
    assert!(requests[0]["time"].as_str().is_some());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_dashboard_recent_requests_with_auth_user_fallback() {
    let now = stable_dashboard_now();
    let user = sample_auth_user(now);
    let access_token = build_test_auth_token(
        "access",
        serde_json::Map::from_iter([
            ("user_id".to_string(), json!(user.id)),
            ("role".to_string(), json!(user.role)),
            (
                "created_at".to_string(),
                json!(user.created_at.map(|value| value.to_rfc3339())),
            ),
            (
                "session_id".to_string(),
                json!("session-dashboard-recent-auth-fallback"),
            ),
        ]),
        chrono::Utc::now() + chrono::Duration::hours(1),
    );
    let session = sample_auth_session(
        "user-auth-1",
        "session-dashboard-recent-auth-fallback",
        "device-dashboard-recent-auth-fallback",
        "refresh-dashboard-recent-auth-fallback",
        now,
    );
    let mut usage = sample_user_usage_audit(
        "usage-dashboard-auth-fallback-1",
        "req-dashboard-auth-fallback-1",
        "user-auth-1",
        "gpt-5",
        "OpenAI",
        "completed",
        now - chrono::Duration::minutes(5),
    );
    usage.username = Some("stale-alice".to_string());
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![usage]));

    let (gateway_url, upstream_hits, gateway_handle, upstream_handle) =
        start_auth_gateway_with_builder(|| {
            let wallet_repository =
                Arc::new(InMemoryWalletRepository::seed(vec![sample_auth_wallet(
                    "user-auth-1",
                    now,
                )]));
            let data_state = GatewayDataState::with_user_wallet_and_usage_for_tests(
                Arc::new(InMemoryUserReadRepository::seed_auth_users(Vec::<
                    StoredUserAuthRecord,
                >::new(
                ))),
                wallet_repository,
                usage_repository,
            );
            AppState::new()
                .expect("gateway should build")
                .with_data_state_for_tests(data_state)
                .with_auth_users_for_tests([user.clone()])
                .with_auth_sessions_for_tests([session])
        })
        .await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/dashboard/recent-requests?limit=5"
        ))
        .header("authorization", format!("Bearer {access_token}"))
        .header(
            "x-client-device-id",
            "device-dashboard-recent-auth-fallback",
        )
        .header("user-agent", "AetherTest/1.0")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let requests = payload["requests"].as_array().expect("array");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["id"], "usage-dashboard-auth-fallback-1");
    assert_eq!(requests[0]["user"], "alice");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_dashboard_provider_status_locally_without_proxying_upstream() {
    let now = Utc::now();
    let user = sample_auth_user(now);
    let access_token = build_test_auth_token(
        "access",
        serde_json::Map::from_iter([
            ("user_id".to_string(), json!(user.id)),
            ("role".to_string(), json!(user.role)),
            (
                "created_at".to_string(),
                json!(user.created_at.map(|value| value.to_rfc3339())),
            ),
            (
                "session_id".to_string(),
                json!("session-dashboard-provider-status"),
            ),
        ]),
        now + chrono::Duration::hours(1),
    );
    let session = sample_auth_session(
        "user-auth-1",
        "session-dashboard-provider-status",
        "device-dashboard-provider-status",
        "refresh-dashboard-provider-status",
        now,
    );
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_user_usage_audit(
            "usage-provider-1",
            "req-provider-1",
            "user-auth-1",
            "gpt-5",
            "openai",
            "completed",
            now - chrono::Duration::hours(1),
        ),
        sample_user_usage_audit(
            "usage-provider-2",
            "req-provider-2",
            "user-auth-2",
            "gpt-5",
            "openai",
            "completed",
            now - chrono::Duration::hours(2),
        ),
        sample_user_usage_audit(
            "usage-provider-3",
            "req-provider-3",
            "user-auth-1",
            "claude-3-7",
            "claude",
            "completed",
            now - chrono::Duration::hours(3),
        ),
        sample_user_usage_audit(
            "usage-provider-4",
            "req-provider-4",
            "user-auth-1",
            "claude-3-7",
            "claude",
            "completed",
            now - chrono::Duration::hours(30),
        ),
    ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider("provider-openai", "openai", 10),
            sample_provider("provider-claude", "claude", 20),
            sample_provider("provider-gemini", "gemini", 30),
        ],
        vec![],
        vec![],
    ));
    let (gateway_url, upstream_hits, gateway_handle, upstream_handle) =
        start_auth_dashboard_gateway_with_state(
            user,
            sample_auth_wallet("user-auth-1", now),
            [session],
            usage_repository,
            provider_catalog_repository,
        )
        .await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/dashboard/provider-status"))
        .header("authorization", format!("Bearer {access_token}"))
        .header("x-client-device-id", "device-dashboard-provider-status")
        .header("user-agent", "AetherTest/1.0")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    let providers = payload["providers"].as_array().expect("array");
    assert_eq!(providers.len(), 3);
    assert_eq!(providers[0]["name"], "openai");
    assert_eq!(providers[0]["requests"], 2);
    assert_eq!(providers[1]["name"], "claude");
    assert_eq!(providers[1]["requests"], 1);
    assert_eq!(providers[2]["name"], "gemini");
    assert_eq!(providers[2]["requests"], 0);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
