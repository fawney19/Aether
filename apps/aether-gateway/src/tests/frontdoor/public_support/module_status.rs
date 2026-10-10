use super::*;

#[tokio::test]
async fn gateway_exposes_only_user_module_status_with_canonical_referral_switch() {
    for (enabled, legacy_enabled) in [(Some(true), false), (Some(false), true), (None, true)] {
        let mut configs = vec![
            ("module.referral.enabled".to_string(), json!(legacy_enabled)),
            ("module.management_tokens.enabled".to_string(), json!(true)),
            ("smtp_password".to_string(), json!("private-config")),
        ];
        if let Some(enabled) = enabled {
            configs.push(("referral_enabled".to_string(), json!(enabled)));
        }
        let data = GatewayDataState::disabled().with_system_config_values_for_tests(configs);
        let (gateway_url, handle) = start_server(build_router_with_state(
            AppState::new()
                .expect("gateway should build")
                .with_data_state_for_tests(data),
        ))
        .await;

        // 普通用户入口必须能够读取状态，且旧开关不能覆盖显式关闭。
        let response = reqwest::Client::new()
            .get(format!("{gateway_url}/api/modules/user-status"))
            .send()
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let payload: serde_json::Value = response.json().await.expect("json body should parse");
        let referral_available =
            crate::handlers::shared::module_available_from_env("REFERRAL_AVAILABLE", true);
        let tokens_available =
            crate::handlers::shared::module_available_from_env("MANAGEMENT_TOKENS_AVAILABLE", true);
        let enabled = enabled.unwrap_or(false);
        assert_eq!(
            payload,
            json!({
                "referral": {
                    "name": "referral", "available": referral_available,
                    "enabled": referral_available && enabled, "active": referral_available && enabled,
                },
                "management_tokens": {
                    "name": "management_tokens", "available": tokens_available,
                    "enabled": tokens_available, "active": tokens_available,
                },
            })
        );
        handle.abort();
    }
}

#[tokio::test]
async fn gateway_referral_module_toggle_controls_user_status_and_actual_rewards() {
    let data = GatewayDataState::with_auth_module_reader_for_tests(Arc::new(
        InMemoryAuthModuleReadRepository::default(),
    ))
    .with_system_config_values_for_tests([
        ("referral_enabled".to_string(), json!(false)),
        ("module.referral.enabled".to_string(), json!(true)),
    ]);
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(data);
    let (gateway_url, handle) = start_server(build_router_with_state(state.clone())).await;
    let client = reqwest::Client::new();

    if !crate::handlers::shared::module_available_from_env("REFERRAL_AVAILABLE", true) {
        let response = client
            .put(format!(
                "{gateway_url}/api/admin/modules/status/referral/enabled"
            ))
            .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .json(&json!({ "enabled": true }))
            .send()
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        handle.abort();
        return;
    }

    for enabled in [true, false] {
        let response = client
            .put(format!(
                "{gateway_url}/api/admin/modules/status/referral/enabled"
            ))
            .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .json(&json!({ "enabled": enabled }))
            .send()
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let payload: serde_json::Value = response.json().await.expect("json body should parse");
        assert_eq!(payload["active"], json!(enabled));
        assert_eq!(
            state
                .read_system_config_json_value("referral_enabled")
                .await
                .expect("config should load"),
            Some(json!(enabled))
        );
        assert_eq!(
            state
                .referral_reward_config()
                .await
                .expect("reward config should load")
                .is_some(),
            enabled
        );

        let response = client
            .get(format!("{gateway_url}/api/modules/user-status"))
            .send()
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let payload: serde_json::Value = response.json().await.expect("json body should parse");
        assert_eq!(payload["referral"]["enabled"], json!(enabled));
        assert_eq!(payload["referral"]["active"], json!(enabled));

        let response = client
            .get(format!("{gateway_url}/api/admin/modules/status/referral"))
            .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .send()
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let payload: serde_json::Value = response.json().await.expect("json body should parse");
        assert_eq!(payload["active"], json!(enabled));
    }
    handle.abort();
}
