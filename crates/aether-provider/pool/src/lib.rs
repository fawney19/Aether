mod capability;
mod plan;
mod presets;
mod provider;
mod quota;
mod quota_refresh;
mod service;

pub mod providers;

pub use capability::{ProviderPoolCapabilities, ProviderPoolCapability};
pub use plan::{derive_oauth_plan_type, derive_plan_tier, normalize_provider_plan_tier};
pub use presets::{
    build_admin_pool_scheduling_presets_payload, normalize_provider_scheduling_presets,
};
pub use provider::{ProviderPoolAdapter, ProviderPoolMemberInput};
pub use providers::{
    build_antigravity_pool_quota_request, build_antigravity_pool_quota_summary_request,
    build_chatgpt_web_pool_quota_request, build_claude_code_pool_quota_request,
    build_codex_pool_quota_request, build_codex_pool_reset_credit_consume_request,
    build_codex_pool_reset_credits_request, build_gemini_cli_pool_quota_request,
    build_kiro_pool_quota_request, build_windsurf_pool_model_configs_request,
    build_windsurf_pool_model_configs_request_with_base_url, build_windsurf_pool_quota_request,
    build_windsurf_pool_quota_request_with_base_url, build_windsurf_pool_rate_limit_request,
    build_windsurf_pool_rate_limit_request_with_base_url, build_xai_pool_billing_request,
    build_xai_pool_user_request, enrich_chatgpt_web_quota_metadata, grok_mode_id_for_model,
    grok_pool_tier_from_quota_bucket, grok_quota_window_key_for_model,
    grok_supported_quota_windows_for_tier, normalize_chatgpt_web_image_quota_limit,
    AntigravityProviderPoolAdapter, ChatGptWebProviderPoolAdapter, ClaudeCodeProviderPoolAdapter,
    CodexProviderPoolAdapter, DefaultProviderPoolAdapter, GeminiCliProviderPoolAdapter,
    GrokProviderPoolAdapter, KiroPoolQuotaAuthInput, KiroProviderPoolAdapter,
    UnsupportedQuotaProviderPoolAdapter, XaiProviderPoolAdapter,
    ANTIGRAVITY_FETCH_AVAILABLE_MODELS_PATH, ANTIGRAVITY_RETRIEVE_USER_QUOTA_SUMMARY_PATH,
    CHATGPT_WEB_CONVERSATION_INIT_PATH, CHATGPT_WEB_DEFAULT_BASE_URL,
    CODEX_WHAM_RESET_CREDITS_CONSUME_URL, CODEX_WHAM_RESET_CREDITS_URL, CODEX_WHAM_USAGE_URL,
    GEMINI_CLI_RETRIEVE_USER_QUOTA_PATH, GEMINI_CLI_USER_AGENT, KIRO_USAGE_LIMITS_PATH,
    KIRO_USAGE_SDK_VERSION, WINDSURF_MODEL_CONFIGS_PATH, WINDSURF_RATE_LIMIT_PATH,
    WINDSURF_USER_STATUS_PATH, XAI_BILLING_PATH, XAI_USER_PATH,
};
pub use quota::{
    provider_pool_codex_metadata_has_account_quota, provider_pool_key_account_quota_exhausted,
    provider_pool_key_minimum_quota_reached, provider_pool_key_model_quota_exhausted,
    provider_pool_key_model_quota_hard_blocked, provider_pool_key_quota_hard_blocked,
    provider_pool_key_scheduling_label, provider_pool_member_quota_snapshot,
    provider_pool_quota_metadata_provider_type, provider_pool_quota_metadata_updated_at,
    provider_pool_quota_snapshot_updated_at, provider_pool_reset_deadline_elapsed,
};
pub use quota_refresh::ProviderPoolQuotaRequestSpec;
pub use service::ProviderPoolService;

#[cfg(test)]
mod tests {
    use super::*;
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
    use aether_pool_core::PoolSchedulingPreset;
    use serde_json::{json, Value};

    fn sample_key(upstream_metadata: Option<Value>) -> StoredProviderCatalogKey {
        let mut key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key-1".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.upstream_metadata = upstream_metadata;
        key
    }

    #[test]
    fn builtin_service_registers_provider_pool_adapters() {
        let service = ProviderPoolService::with_builtin_adapters();

        assert_eq!(
            service.provider_types().collect::<Vec<_>>(),
            [
                "antigravity",
                "chatgpt_web",
                "claude_code",
                "codex",
                "gemini_cli",
                "grok",
                "kiro",
                "vertex_ai",
                "windsurf",
                "xai"
            ]
        );
        assert!(service
            .adapter("codex")
            .capabilities()
            .supports(ProviderPoolCapability::PlanTier));
        assert_eq!(service.adapter("unknown").provider_type(), "default");
    }

    #[test]
    fn builtin_service_owns_quota_refresh_support_and_endpoint_selection() {
        let service = ProviderPoolService::with_builtin_adapters();

        assert_eq!(
            service.provider_types_for_capability(ProviderPoolCapability::QuotaRefresh),
            [
                "antigravity",
                "chatgpt_web",
                "claude_code",
                "codex",
                "gemini_cli",
                "grok",
                "kiro",
                "windsurf",
                "xai"
            ]
        );
        assert!(service.supports_quota_refresh("claude_code"));
        assert!(service.supports_quota_refresh("codex"));
        assert!(service.supports_quota_refresh("antigravity"));
        assert!(service.supports_quota_refresh("grok"));
        assert!(service.supports_quota_refresh("gemini_cli"));
        assert!(service.supports_quota_refresh("windsurf"));
        assert!(service.supports_quota_refresh("xai"));
        let claude_spec = build_claude_code_pool_quota_request(
            "key-1",
            ("authorization".to_string(), "Bearer access".to_string()),
        );
        assert_eq!(claude_spec.method, "GET");
        assert_eq!(
            claude_spec.url,
            "https://api.anthropic.com/api/oauth/usage?cedar_ember=1&skip_spend=1"
        );
        assert_eq!(
            claude_spec
                .headers
                .get("anthropic-beta")
                .map(String::as_str),
            Some("oauth-2025-04-20")
        );
        assert_eq!(
            service.quota_refresh_unsupported_message("vertex_ai"),
            "Vertex AI 暂不支持自动刷新额度：额度属于 Google Cloud 项目/区域配额"
        );
    }

    #[test]
    fn codex_quota_request_adds_account_header_for_paid_accounts() {
        let spec = build_codex_pool_quota_request(
            "key-1",
            Some(("authorization".to_string(), "Bearer access".to_string())),
            None,
            Some(&json!({
                "plan_type": "plus",
                "account_id": "acct-1"
            })),
        )
        .expect("spec should build");

        assert_eq!(
            spec.headers.get("chatgpt-account-id").map(String::as_str),
            Some("acct-1")
        );
    }

    #[test]
    fn codex_quota_request_prefers_imported_authorization_header() {
        let spec = build_codex_pool_quota_request(
            "key-1",
            Some((
                "authorization".to_string(),
                "Bearer jwt-access-token".to_string(),
            )),
            None,
            Some(&json!({
                "headers": {
                    "authorization": "Bearer imported-session"
                }
            })),
        )
        .expect("spec should build");

        assert_eq!(
            spec.headers.get("authorization").map(String::as_str),
            Some("Bearer imported-session")
        );
    }

    #[test]
    fn codex_agent_identity_quota_request_prefers_dynamic_assertion() {
        let spec = build_codex_pool_quota_request(
            "key-1",
            Some((
                "authorization".to_string(),
                "AgentAssertion signed-at-request-time".to_string(),
            )),
            None,
            Some(&json!({
                "auth_mode": "agentIdentity",
                "headers": {
                    "authorization": "Bearer stale-imported-session"
                }
            })),
        )
        .expect("spec should build");

        assert_eq!(
            spec.headers.get("authorization").map(String::as_str),
            Some("AgentAssertion signed-at-request-time")
        );
    }

    #[test]
    fn codex_nested_agent_identity_quota_request_prefers_dynamic_assertion() {
        let spec = build_codex_pool_quota_request(
            "key-1",
            Some((
                "authorization".to_string(),
                "AgentAssertion signed-at-request-time".to_string(),
            )),
            None,
            Some(&json!({
                "agent_identity": {
                    "agent_runtime_id": "runtime-1",
                    "agent_private_key": "private-key"
                },
                "headers": {
                    "authorization": "Bearer stale-imported-session"
                }
            })),
        )
        .expect("spec should build");

        assert_eq!(
            spec.headers.get("authorization").map(String::as_str),
            Some("AgentAssertion signed-at-request-time")
        );
    }

    #[test]
    fn gemini_cli_quota_request_uses_v1internal_retrieve_user_quota() {
        let spec = build_gemini_cli_pool_quota_request(
            "key-1",
            "https://cloudcode-pa.googleapis.com/",
            ("authorization".to_string(), "Bearer access".to_string()),
            "project-1",
        );

        assert_eq!(spec.method, "POST");
        assert_eq!(
            spec.url,
            "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota"
        );
        assert_eq!(
            spec.headers.get("authorization").map(String::as_str),
            Some("Bearer access")
        );
        assert_eq!(
            spec.json_body.as_ref().and_then(|body| body.get("project")),
            Some(&json!("project-1"))
        );
        assert_eq!(
            spec.headers.get("user-agent").map(String::as_str),
            Some(GEMINI_CLI_USER_AGENT)
        );
        assert!(spec
            .json_body
            .as_ref()
            .is_some_and(|body| body.get("userAgent").is_none()));
        assert_eq!(spec.client_api_format, "gemini:generate_content");
        assert_eq!(spec.provider_api_format, "gemini_cli:retrieve_user_quota");
    }

    #[test]
    fn codex_quota_request_uses_wham_usage_endpoint() {
        let spec = build_codex_pool_quota_request(
            "key-1",
            Some(("authorization".to_string(), "Bearer access".to_string())),
            None,
            None,
        )
        .expect("spec should build");

        assert_eq!(spec.method, "GET");
        assert_eq!(spec.url, "https://chatgpt.com/backend-api/wham/usage");
        assert_eq!(
            spec.headers.get("authorization").map(String::as_str),
            Some("Bearer access")
        );
        assert_eq!(
            spec.headers.get("accept").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(spec.model_name.as_deref(), Some("codex-wham-usage"));
    }

    #[test]
    fn codex_reset_credits_request_uses_wham_detail_endpoint() {
        let spec = build_codex_pool_reset_credits_request(
            "key-1",
            Some(("authorization".to_string(), "Bearer access".to_string())),
            None,
            Some(&json!({
                "plan_type": "plus",
                "account_id": "acct-1"
            })),
        )
        .expect("spec should build");

        assert_eq!(spec.method, "GET");
        assert_eq!(spec.url, CODEX_WHAM_RESET_CREDITS_URL);
        assert_eq!(
            spec.headers.get("authorization").map(String::as_str),
            Some("Bearer access")
        );
        assert_eq!(
            spec.headers.get("chatgpt-account-id").map(String::as_str),
            Some("acct-1")
        );
        assert_eq!(spec.model_name.as_deref(), Some("codex-wham-reset-credits"));
    }

    #[test]
    fn codex_reset_credit_consume_request_posts_redeem_request_id() {
        let spec = build_codex_pool_reset_credit_consume_request(
            "key-1",
            Some(("authorization".to_string(), "Bearer access".to_string())),
            None,
            None,
            "8ae6f1c7-7e9e-4f5d-9b8a-000000000000",
        )
        .expect("spec should build");

        assert_eq!(spec.method, "POST");
        assert_eq!(spec.url, CODEX_WHAM_RESET_CREDITS_CONSUME_URL);
        assert_eq!(spec.content_type.as_deref(), Some("application/json"));
        assert_eq!(
            spec.json_body
                .as_ref()
                .and_then(|body| body.get("redeem_request_id")),
            Some(&json!("8ae6f1c7-7e9e-4f5d-9b8a-000000000000"))
        );
        assert_eq!(
            spec.json_body
                .as_ref()
                .and_then(|body| body.as_object())
                .map(|body| body.len()),
            Some(1)
        );
        assert_eq!(
            spec.model_name.as_deref(),
            Some("codex-wham-reset-credit-consume")
        );
    }

    #[test]
    fn codex_quota_request_skips_account_header_for_free_accounts() {
        let spec = build_codex_pool_quota_request(
            "key-1",
            Some(("authorization".to_string(), "Bearer access".to_string())),
            None,
            Some(&json!({
                "plan_type": "codex:free",
                "account_id": "acct-1"
            })),
        )
        .expect("spec should build");

        assert!(!spec.headers.contains_key("chatgpt-account-id"));
    }

    #[test]
    fn kiro_quota_request_includes_profile_arn_when_present() {
        let spec = build_kiro_pool_quota_request(
            "key-1",
            &KiroPoolQuotaAuthInput {
                authorization_value: "Bearer access".to_string(),
                api_region: "us-west-2".to_string(),
                kiro_version: "0.3.210".to_string(),
                machine_id: "machine".to_string(),
                profile_arn: Some("arn:aws:sso:::profile/p-1".to_string()),
            },
        );

        assert!(spec.url.contains("q.us-west-2.amazonaws.com"));
        assert!(spec
            .url
            .contains("profileArn=arn%3Aaws%3Asso%3A%3A%3Aprofile%2Fp-1"));
    }

    #[test]
    fn kiro_quota_request_rejects_region_url_injection() {
        let spec = build_kiro_pool_quota_request(
            "key-1",
            &KiroPoolQuotaAuthInput {
                authorization_value: "Bearer sensitive-access-token".to_string(),
                api_region: "attacker.example/evil".to_string(),
                kiro_version: "0.3.210".to_string(),
                machine_id: "machine".to_string(),
                profile_arn: None,
            },
        );

        assert_eq!(
            spec.url,
            "https://q.us-east-1.amazonaws.com/getUsageLimits?origin=AI_EDITOR&resourceType=AGENTIC_REQUEST&isEmailRequired=true"
        );
        assert_eq!(
            spec.headers.get("host").map(String::as_str),
            Some("q.us-east-1.amazonaws.com")
        );
        assert!(!spec.url.contains("attacker.example"));
        assert!(!spec
            .headers
            .get("host")
            .is_some_and(|value| value.contains("attacker.example")));
    }

    #[test]
    fn chatgpt_web_quota_request_uses_default_base_url_when_empty() {
        let spec = build_chatgpt_web_pool_quota_request(
            "key-1",
            "",
            ("authorization".to_string(), "Bearer access".to_string()),
        );

        assert_eq!(
            spec.url,
            "https://chatgpt.com/backend-api/conversation/init"
        );
        assert_eq!(
            spec.headers.get("origin").map(String::as_str),
            Some("https://chatgpt.com")
        );
    }

    #[test]
    fn chatgpt_web_quota_metadata_enriches_auth_and_uses_first_remaining_as_limit() {
        let mut metadata = json!({
            "image_quota_remaining": 12,
        });
        enrich_chatgpt_web_quota_metadata(
            &mut metadata,
            Some(&json!({
                "plan": "free",
                "email": "user@example.com",
                "accountId": "acct-1"
            })),
        );
        normalize_chatgpt_web_image_quota_limit(&mut metadata, None);

        assert_eq!(metadata["plan_type"], json!("free"));
        assert_eq!(metadata["email"], json!("user@example.com"));
        assert_eq!(metadata["account_id"], json!("acct-1"));
        assert_eq!(metadata["image_quota_total"], json!(12.0));
        assert_eq!(metadata["image_quota_used"], json!(0.0));
    }

    #[test]
    fn chatgpt_web_quota_metadata_preserves_existing_paid_limit() {
        let mut metadata = json!({
            "plan_type": "plus",
            "image_quota_remaining": 7,
        });
        normalize_chatgpt_web_image_quota_limit(
            &mut metadata,
            Some(&json!({
                "chatgpt_web": {
                    "image_quota_total": 40
                }
            })),
        );

        assert_eq!(metadata["image_quota_total"], json!(40.0));
        assert_eq!(metadata["image_quota_used"], json!(33.0));
    }

    #[test]
    fn chatgpt_web_quota_metadata_does_not_preserve_legacy_free_25_limit() {
        let mut metadata = json!({
            "plan_type": "free",
            "image_quota_remaining": 19,
        });
        normalize_chatgpt_web_image_quota_limit(
            &mut metadata,
            Some(&json!({
                "chatgpt_web": {
                    "plan_type": "free",
                    "image_quota_total": 25
                }
            })),
        );

        assert_eq!(metadata["image_quota_total"], json!(19.0));
        assert_eq!(metadata["image_quota_used"], json!(0.0));
        assert_eq!(
            metadata["image_quota_limit_source"],
            json!("first_remaining")
        );
    }

    #[test]
    fn chatgpt_web_quota_metadata_ignores_upstream_free_25_default() {
        let mut metadata = json!({
            "plan_type": "free",
            "image_quota_remaining": 19,
            "image_quota_total": 25,
        });
        normalize_chatgpt_web_image_quota_limit(&mut metadata, None);

        assert_eq!(metadata["image_quota_total"], json!(19.0));
        assert_eq!(metadata["image_quota_used"], json!(0.0));
        assert_eq!(
            metadata["image_quota_limit_source"],
            json!("first_remaining")
        );
    }

    #[test]
    fn chatgpt_web_quota_metadata_preserves_marked_free_first_limit() {
        let mut metadata = json!({
            "plan_type": "free",
            "image_quota_remaining": 18,
        });
        normalize_chatgpt_web_image_quota_limit(
            &mut metadata,
            Some(&json!({
                "chatgpt_web": {
                    "plan_type": "free",
                    "image_quota_total": 19,
                    "image_quota_limit_source": "first_remaining"
                }
            })),
        );

        assert_eq!(metadata["image_quota_total"], json!(19.0));
        assert_eq!(metadata["image_quota_used"], json!(1.0));
        assert_eq!(
            metadata["image_quota_limit_source"],
            json!("first_remaining")
        );
    }

    #[test]
    fn windsurf_quota_request_uses_user_status_connect_rpc() {
        let spec = build_windsurf_pool_quota_request("key-ws", "session-token-123");

        assert_eq!(spec.request_id, "windsurf-quota:key-ws");
        assert_eq!(spec.method, "POST");
        assert_eq!(
            spec.url,
            format!("https://server.codeium.com{WINDSURF_USER_STATUS_PATH}")
        );
        assert_eq!(spec.content_type.as_deref(), Some("application/json"));
        assert_eq!(
            spec.headers
                .get("connect-protocol-version")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            spec.json_body
                .as_ref()
                .and_then(|body| body.pointer("/metadata/apiKey"))
                .and_then(Value::as_str),
            Some("session-token-123")
        );
        assert_eq!(spec.provider_api_format, "windsurf:user_status");
    }

    #[test]
    fn windsurf_model_and_rate_limit_requests_use_connect_rpc_metadata() {
        let models = build_windsurf_pool_model_configs_request("key-ws", "api-key-123");
        let rate_limit = build_windsurf_pool_rate_limit_request("key-ws", "api-key-123");

        assert_eq!(
            models.url,
            format!("https://server.codeium.com{WINDSURF_MODEL_CONFIGS_PATH}")
        );
        assert_eq!(
            rate_limit.url,
            format!("https://server.codeium.com{WINDSURF_RATE_LIMIT_PATH}")
        );
        for spec in [models, rate_limit] {
            assert_eq!(spec.method, "POST");
            assert_eq!(
                spec.headers
                    .get("connect-protocol-version")
                    .map(String::as_str),
                Some("1")
            );
            assert_eq!(
                spec.json_body
                    .as_ref()
                    .and_then(|body| body.pointer("/metadata/apiKey"))
                    .and_then(Value::as_str),
                Some("api-key-123")
            );
            assert_eq!(spec.client_api_format, "openai:chat");
        }
    }

    #[test]
    fn windsurf_rate_limit_metadata_keeps_member_schedulable() {
        let service = ProviderPoolService::with_builtin_adapters();
        let key = sample_key(Some(json!({
            "windsurf": {
                "updated_at": 1_700_000_000u64,
                "rate_limit": {
                    "limited": true,
                    "retry_after_ms": 60_000
                }
            }
        })));

        let signals = service.member_signals("windsurf", &key, None, None);

        assert!(!signals.quota_exhausted);
    }

    #[test]
    fn windsurf_status_snapshot_ban_marks_member_exhausted() {
        let service = ProviderPoolService::with_builtin_adapters();
        let mut key = sample_key(Some(json!({
            "windsurf": {
                "updated_at": 1_700_000_000u64,
                "daily_remaining_percent": 100.0
            }
        })));
        key.status_snapshot = Some(json!({
            "quota": {
                "provider_type": "windsurf",
                "code": "banned",
                "exhausted": false,
                "windows": [{
                    "code": "daily",
                    "used_ratio": 0.0,
                    "remaining_ratio": 1.0
                }]
            }
        }));

        let signals = service.member_signals("windsurf", &key, None, None);

        assert!(signals.quota_exhausted);
    }

    #[test]
    fn preset_payload_derives_provider_support_from_capabilities() {
        let payload = build_admin_pool_scheduling_presets_payload();
        let items = payload.as_array().expect("payload should be array");
        let free_first = items
            .iter()
            .find(|item| item["name"] == "free_first")
            .expect("free_first should exist");
        let recent_refresh = items
            .iter()
            .find(|item| item["name"] == "recent_refresh")
            .expect("recent_refresh should exist");
        let legacy_free_team = items
            .iter()
            .find(|item| item["name"] == "free_team_first")
            .expect("legacy free_team_first should remain configurable");

        assert_eq!(
            free_first["providers"],
            json!(["codex", "grok", "kiro", "windsurf", "xai"])
        );
        assert_eq!(
            recent_refresh["providers"],
            json!(["codex", "grok", "kiro", "windsurf", "xai"])
        );
        assert_eq!(free_first["default_enabled"], json!(false));
        assert_eq!(recent_refresh["default_enabled"], json!(false));
        assert_eq!(
            recent_refresh["default_enabled_providers"],
            json!(["codex", "windsurf"])
        );
        assert_eq!(legacy_free_team["default_mode"], json!("both"));
        assert_eq!(legacy_free_team["modes"].as_array().map(Vec::len), Some(3));
    }

    #[test]
    fn quota_metadata_provider_type_comes_from_pool_registry() {
        assert_eq!(
            provider_pool_quota_metadata_provider_type(&json!({
                "gemini_cli": {
                    "updated_at": 1_700_000_000u64
                }
            }))
            .as_deref(),
            Some("gemini_cli")
        );
        assert_eq!(
            provider_pool_quota_metadata_provider_type(&json!({
                "custom_provider": {
                    "updated_at": 1_700_000_000u64
                }
            }))
            .as_deref(),
            Some("custom_provider")
        );
    }

    #[test]
    fn codex_adapter_injects_recent_refresh_and_filters_by_capability() {
        let service = ProviderPoolService::with_builtin_adapters();
        let normalized = service.normalize_scheduling_presets(
            "codex",
            &[PoolSchedulingPreset {
                preset: "cache_affinity".to_string(),
                enabled: true,
                mode: None,
            }],
        );

        assert_eq!(
            normalized
                .iter()
                .map(|preset| preset.preset.as_str())
                .collect::<Vec<_>>(),
            ["cache_affinity", "recent_refresh"]
        );

        let legacy_free_team = service.normalize_scheduling_presets(
            "codex",
            &[PoolSchedulingPreset {
                preset: "free_team_first".to_string(),
                enabled: true,
                mode: Some("team_only".to_string()),
            }],
        );
        assert_eq!(legacy_free_team[0].preset, "free_team_first");
        assert_eq!(legacy_free_team[0].mode.as_deref(), Some("team_only"));

        let unsupported = service.normalize_scheduling_presets(
            "chatgpt_web",
            &[PoolSchedulingPreset {
                preset: "plus_first".to_string(),
                enabled: true,
                mode: None,
            }],
        );
        assert!(unsupported.is_empty());
    }

    #[test]
    fn codex_minimum_quota_respects_boundary_and_provider() {
        for (used_percent, expected) in [
            (98.9, false),
            (98.99999, false),
            (99.0, true),
            (99.5, true),
            (100.0, true),
        ] {
            let key = sample_key(Some(json!({
                "codex": { "primary_used_percent": used_percent }
            })));
            assert_eq!(
                provider_pool_key_minimum_quota_reached(&key, "codex", None),
                expected,
                "used_percent={used_percent}"
            );
            assert!(!provider_pool_key_minimum_quota_reached(&key, "kiro", None));
        }
        assert!(!provider_pool_key_minimum_quota_reached(
            &sample_key(None),
            "codex",
            None
        ));
    }

    #[test]
    fn codex_minimum_quota_short_model_names_still_use_account_windows() {
        for (used_percent, expected) in [(98.0, false), (99.0, true)] {
            let key = sample_key(Some(json!({
                "codex": { "primary_used_percent": used_percent }
            })));
            for model in ["o1", "o3", "", "   "] {
                assert_eq!(
                    provider_pool_key_minimum_quota_reached(&key, "codex", Some(model)),
                    expected,
                    "model={model:?}, used_percent={used_percent}"
                );
            }
        }
    }

    #[test]
    fn codex_minimum_quota_respects_windows_and_reset() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_secs();
        for (reset_at, expected) in [(now + 3600, true), (now - 60, false)] {
            let mut key = sample_key(None);
            key.status_snapshot = Some(json!({
                "quota": {
                    "provider_type": "codex",
                    "updated_at": now - 600,
                    "windows": [
                        { "code": "5h", "used_ratio": 0.2 },
                        { "code": "weekly", "used_ratio": 0.99, "reset_at": reset_at }
                    ]
                }
            }));
            for model in [None, Some("gpt-5.4")] {
                assert_eq!(
                    provider_pool_key_minimum_quota_reached(&key, "codex", model),
                    expected
                );
            }
        }
    }

    #[test]
    fn codex_minimum_quota_checks_either_window_and_relative_resets() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_secs();
        for prefix in ["primary", "secondary"] {
            for (reset_seconds, expected) in [(3600, true), (60, false)] {
                let key = sample_key(Some(json!({
                    "codex": {
                        "updated_at": now - 600,
                        "allowed": true,
                        "limit_reached": false,
                        format!("{prefix}_used_percent"): 99.0,
                        format!("{prefix}_reset_after_seconds"): reset_seconds
                    }
                })));
                for model in [None, Some("gpt-5.4")] {
                    assert_eq!(
                        provider_pool_key_minimum_quota_reached(&key, "codex", model),
                        expected,
                        "prefix={prefix}, reset_seconds={reset_seconds}, model={model:?}"
                    );
                }
                assert!(!provider_pool_key_account_quota_exhausted(&key, "codex"));
            }
        }
    }

    #[test]
    fn codex_minimum_quota_uses_newest_source_with_or_without_model() {
        for (snapshot_at, metadata_at, metadata_wins) in [
            (Some(200), Some(100), false),
            (Some(100), Some(200), true),
            (None, None, false),
            (Some(100), None, false),
            (None, Some(100), true),
        ] {
            for snapshot_reached in [false, true] {
                let mut key = sample_key(Some(json!({
                    "codex": {
                        "updated_at": metadata_at,
                        "primary_used_percent": if snapshot_reached { 20.0 } else { 99.0 }
                    }
                })));
                key.status_snapshot = Some(json!({
                    "quota": {
                        "provider_type": "codex",
                        "observed_at": snapshot_at,
                        "windows": [{
                            "code": "5h",
                            "used_ratio": if snapshot_reached { 0.99 } else { 0.2 },
                            "is_exhausted": false
                        }]
                    }
                }));
                for model in [None, Some("gpt-5.4")] {
                    assert_eq!(
                        provider_pool_key_minimum_quota_reached(&key, "codex", model),
                        snapshot_reached != metadata_wins,
                        "snapshot_at={snapshot_at:?}, metadata_at={metadata_at:?}, model={model:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn codex_minimum_quota_isolates_model_buckets_and_checks_each_model_window() {
        for explicit_model in [false, true] {
            let mut key = sample_key(None);
            key.status_snapshot = Some(json!({
                "quota": {
                    "provider_type": "codex",
                    "windows": [
                        { "code": "5h", "used_ratio": 0.2 },
                        {
                            "code": "spark_5h", "used_ratio": 0.99,
                            "model": if explicit_model { Some("spark") } else { None },
                            "is_exhausted": false
                        },
                        {
                            "code": "spark_weekly", "used_ratio": 0.2,
                            "model": if explicit_model { Some("spark") } else { None },
                            "is_exhausted": false
                        }
                    ]
                }
            }));
            assert!(provider_pool_key_minimum_quota_reached(
                &key,
                "codex",
                Some("gpt-5.3-codex-spark")
            ));
            for model in [None, Some("gpt-5.4")] {
                assert!(!provider_pool_key_minimum_quota_reached(
                    &key, "codex", model
                ));
            }
            assert_eq!(
                provider_pool_key_model_quota_exhausted(&key, "codex", "gpt-5.3-codex-spark"),
                Some(false)
            );
        }
        for (account_percent, spark_percent) in [(99.0, 20.0), (20.0, 99.0)] {
            let key = sample_key(Some(json!({
                "codex": {
                    "primary_used_percent": account_percent,
                    "spark_primary_used_percent": spark_percent,
                    "spark_secondary_used_percent": 20.0
                }
            })));
            assert_eq!(
                provider_pool_key_minimum_quota_reached(&key, "codex", Some("gpt-5.3-codex-spark")),
                spark_percent == 99.0
            );
            assert_eq!(
                provider_pool_key_minimum_quota_reached(&key, "codex", Some("gpt-5.4")),
                account_percent == 99.0
            );
        }
    }

    #[test]
    fn codex_minimum_quota_keeps_model_bucket_when_account_metadata_is_newer() {
        for (snapshot_ratio, account_percent) in [(0.2, 99.0), (0.99, 20.0)] {
            let mut key = sample_key(Some(json!({
                "codex": { "updated_at": 200, "primary_used_percent": account_percent }
            })));
            key.status_snapshot = Some(json!({
                "quota": {
                    "provider_type": "codex",
                    "observed_at": 100,
                    "windows": [{ "code": "spark_5h", "used_ratio": snapshot_ratio }]
                }
            }));
            assert_eq!(
                provider_pool_key_minimum_quota_reached(&key, "codex", Some("gpt-5.3-codex-spark")),
                snapshot_ratio == 0.99
            );
            assert_eq!(
                provider_pool_key_minimum_quota_reached(&key, "codex", Some("gpt-5.4")),
                account_percent == 99.0
            );

            key.upstream_metadata.as_mut().unwrap()["codex"]["spark_primary_used_percent"] =
                json!(account_percent);
            assert_eq!(
                provider_pool_key_minimum_quota_reached(&key, "codex", Some("gpt-5.3-codex-spark")),
                account_percent == 99.0,
                "the newer observation for the same model bucket must win"
            );
        }
    }

    #[test]
    fn codex_minimum_quota_supports_remaining_values_and_ignores_unknown_data() {
        for (metrics, expected) in [
            (json!({ "remaining_ratio": 0.01 }), true),
            (json!({ "remaining_percent": "1" }), true),
            (json!({ "remaining": 1, "limit": 100 }), true),
            (json!({ "used_percent": "99" }), true),
            (json!({ "used_ratio": null }), false),
            (json!({ "used_ratio": "NaN" }), false),
            (json!({ "remaining": 0, "limit": 0 }), false),
            (json!({ "remaining_ratio": 0.01001 }), false),
        ] {
            let key = sample_key(Some(json!({
                "codex": { "quota_by_model": { "spark": metrics } }
            })));
            assert_eq!(
                provider_pool_key_minimum_quota_reached(&key, "codex", Some("gpt-5.3-codex-spark")),
                expected,
                "metrics={metrics}"
            );
            assert!(!provider_pool_key_minimum_quota_reached(
                &key, "codex", None
            ));
        }
        let disabled = sample_key(Some(json!({
            "codex": { "primary_used_percent": 99.0, "primary_window_minutes": 0 }
        })));
        assert!(!provider_pool_key_minimum_quota_reached(
            &disabled, "codex", None
        ));

        let mut mismatched = sample_key(None);
        mismatched.status_snapshot = Some(json!({
            "quota": {
                "provider_type": "kiro",
                "windows": [{ "code": "weekly", "used_ratio": 0.99 }]
            }
        }));
        assert!(!provider_pool_key_minimum_quota_reached(
            &mismatched,
            "codex",
            None
        ));
    }

    #[test]
    fn provider_quota_exhaustion_is_adapter_owned() {
        assert!(provider_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "codex": {
                    "allowed": false,
                    "limit_reached": true
                }
            }))),
            "codex",
        ));
        assert!(provider_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "codex": {
                    "has_credits": false,
                    "credits_unlimited": false
                }
            }))),
            "codex",
        ));
        assert!(provider_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "kiro": {
                    "remaining": 0
                }
            }))),
            "kiro",
        ));
        assert!(provider_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "chatgpt_web": {
                    "image_quota_blocked": true
                }
            }))),
            "chatgpt_web",
        ));
        assert!(provider_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "grok": {
                    "quota_by_model": {
                        "quota_fast": {
                            "is_exhausted": true,
                            "remaining": 0.0
                        }
                    }
                }
            }))),
            "grok",
        ));
        assert!(!provider_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "grok": {
                    "pool_tier": "basic",
                    "quota_by_model": {
                        "quota_fast": {
                            "is_exhausted": false,
                            "remaining": 1.0
                        },
                        "quota_heavy": {
                            "is_exhausted": true,
                            "remaining": 0.0
                        }
                    }
                }
            }))),
            "grok",
        ));
        assert!(!provider_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "codex": {
                    "has_credits": false,
                    "credits_unlimited": true
                }
            }))),
            "codex",
        ));
        assert!(!provider_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "codex": {
                    "allowed": true,
                    "primary_used_percent": 100.0
                }
            }))),
            "codex",
        ));

        let mut explicit_codex_limit = sample_key(None);
        explicit_codex_limit.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "exhausted": true,
                "allowed": false,
                "limit_reached": true,
                "usage_ratio": 0.91,
                "windows": [{
                    "code": "weekly",
                    "used_ratio": 0.91
                }]
            }
        }));
        assert!(provider_pool_key_account_quota_exhausted(
            &explicit_codex_limit,
            "codex",
        ));
    }

    #[test]
    fn provider_quota_exhaustion_snapshot_expires_after_reset_at() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_secs();

        let mut expired = sample_key(None);
        expired.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "code": "exhausted",
                "exhausted": true,
                "updated_at": now.saturating_sub(600),
                "windows": [{
                    "code": "5h",
                    "used_ratio": 1.0,
                    "reset_at": now.saturating_sub(60),
                    "is_exhausted": true
                }]
            }
        }));
        assert!(!provider_pool_key_account_quota_exhausted(
            &expired, "codex"
        ));

        let mut active = sample_key(None);
        active.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "code": "exhausted",
                "exhausted": true,
                "updated_at": now,
                "windows": [{
                    "code": "5h",
                    "used_ratio": 1.0,
                    "reset_at": now.saturating_add(3600),
                    "is_exhausted": true
                }]
            }
        }));
        assert!(provider_pool_key_account_quota_exhausted(&active, "codex"));
    }

    #[test]
    fn antigravity_model_quota_exhaustion_does_not_block_other_models() {
        let service = ProviderPoolService::with_builtin_adapters();
        let mut key = sample_key(None);
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "antigravity",
                "exhausted": false,
                "windows": [
                    {
                        "code": "model:gemini-3.1-pro-high",
                        "scope": "model",
                        "model": "gemini-3.1-pro-high",
                        "used_ratio": 1.0,
                        "is_exhausted": true
                    },
                    {
                        "code": "model:gemini-3-flash-agent",
                        "scope": "model",
                        "model": "gemini-3-flash-agent",
                        "used_ratio": 0.1,
                        "is_exhausted": false
                    }
                ]
            }
        }));

        let exhausted =
            service.member_signals("antigravity", &key, None, Some("gemini-3.1-pro-high"));
        let available =
            service.member_signals("antigravity", &key, None, Some("gemini-3-flash-agent"));

        assert!(exhausted.quota_exhausted);
        assert!(!available.quota_exhausted);
    }

    #[test]
    fn antigravity_tiered_model_quota_wins_over_display_only_group_windows() {
        let service = ProviderPoolService::with_builtin_adapters();
        let mut key = sample_key(None);
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "antigravity",
                "exhausted": true,
                "windows": [{
                    "code": "model:gemini-3.7-flash-tiered",
                    "scope": "model",
                    "model": "gemini-3.7-flash-tiered",
                    "remaining_ratio": 0.906,
                    "used_ratio": 0.094,
                    "is_exhausted": false
                }, {
                    "code": "group:0:3p-5h",
                    "scope": "quota_group",
                    "quota_group": "group:0",
                    "bucket_id": "3p-5h",
                    "used_ratio": 1.0,
                    "is_exhausted": true
                }]
            }
        }));

        let signals =
            service.member_signals("antigravity", &key, None, Some("gemini-3.7-flash-tiered"));

        assert!(!signals.quota_exhausted);
        assert!(!signals.quota_hard_blocked);
    }

    #[test]
    fn antigravity_tiered_variant_does_not_match_another_model_family() {
        let service = ProviderPoolService::with_builtin_adapters();
        let mut key = sample_key(None);
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "antigravity",
                "exhausted": false,
                "windows": [{
                    "code": "model:gemini-3.7-pro-tiered",
                    "scope": "model",
                    "model": "gemini-3.7-pro-tiered",
                    "used_ratio": 1.0,
                    "is_exhausted": true
                }]
            }
        }));

        let signals =
            service.member_signals("antigravity", &key, None, Some("gemini-3.7-flash-tiered"));

        assert!(!signals.quota_exhausted);
        assert!(!signals.quota_hard_blocked);
    }

    #[test]
    fn codex_standard_and_spark_quota_families_are_independent() {
        let service = ProviderPoolService::with_builtin_adapters();
        let mut standard_exhausted = sample_key(None);
        standard_exhausted.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "exhausted": true,
                "allowed": false,
                "limit_reached": true,
                "windows": [
                    { "code": "weekly", "used_ratio": 1.0, "is_exhausted": true },
                    { "code": "5h", "used_ratio": 0.5, "is_exhausted": false },
                    { "code": "spark_weekly", "used_ratio": 0.2, "is_exhausted": false },
                    { "code": "spark_5h", "used_ratio": 0.1, "is_exhausted": false }
                ]
            }
        }));

        let standard =
            service.member_signals("codex", &standard_exhausted, None, Some("gpt-5.3-codex"));
        let spark = service.member_signals(
            "codex",
            &standard_exhausted,
            None,
            Some("gpt-5.3-codex-spark"),
        );
        assert!(standard.quota_exhausted);
        assert!(!standard.quota_hard_blocked);
        assert!(!spark.quota_exhausted);
        assert!(!spark.quota_hard_blocked);

        let mut spark_exhausted = sample_key(None);
        spark_exhausted.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "exhausted": false,
                "windows": [
                    { "code": "weekly", "used_ratio": 0.2, "is_exhausted": false },
                    { "code": "5h", "used_ratio": 0.1, "is_exhausted": false },
                    { "code": "spark_weekly", "used_ratio": 1.0, "is_exhausted": true },
                    { "code": "spark_5h", "used_ratio": 0.4, "is_exhausted": false }
                ]
            }
        }));

        let standard =
            service.member_signals("codex", &spark_exhausted, None, Some("gpt-5.3-codex"));
        let spark =
            service.member_signals("codex", &spark_exhausted, None, Some("gpt-5.3-codex-spark"));
        assert!(!standard.quota_exhausted);
        assert!(spark.quota_exhausted);
    }

    #[test]
    fn provider_quota_exhaustion_metadata_expires_after_reset_at() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_secs();

        assert!(!provider_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "codex": {
                    "updated_at": now.saturating_sub(600),
                    "allowed": false,
                    "limit_reached": true,
                    "primary_used_percent": 100.0,
                    "primary_reset_at": now.saturating_sub(60)
                }
            }))),
            "codex",
        ));
        assert!(provider_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "codex": {
                    "updated_at": now,
                    "primary_used_percent": 100.0,
                    "primary_reset_at": now.saturating_add(3600)
                }
            }))),
            "codex",
        ));
    }

    #[test]
    fn model_quota_windows_are_isolated_without_provider_specific_names() {
        let service = ProviderPoolService::with_builtin_adapters();
        let mut key = sample_key(None);
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "exhausted": true,
                "windows": [
                    {
                        "code": "alpha_short",
                        "quota_group": "alpha",
                        "model": "vendor-alpha-model",
                        "used_ratio": 1.0,
                        "is_exhausted": true
                    },
                    {
                        "code": "alpha_long",
                        "quota_group": "alpha",
                        "model": "vendor-alpha-model",
                        "used_ratio": 0.2,
                        "is_exhausted": false
                    },
                    {
                        "code": "beta_short",
                        "quota_group": "beta",
                        "model": "vendor-beta-model",
                        "used_ratio": 1.0,
                        "is_exhausted": true
                    }
                ]
            }
        }));

        let alpha = service.member_signals("codex", &key, None, Some("vendor-alpha-model"));
        let beta = service.member_signals("codex", &key, None, Some("vendor-beta-model"));
        assert!(
            !alpha.quota_exhausted,
            "one available alpha window must keep it usable"
        );
        assert!(beta.quota_exhausted);
        assert!(!beta.quota_hard_blocked);
    }

    #[test]
    fn legacy_family_prefix_is_matched_by_model_tokens() {
        let service = ProviderPoolService::with_builtin_adapters();
        let mut key = sample_key(None);
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "exhausted": true,
                "windows": [
                    { "code": "alpha_weekly", "used_ratio": 1.0, "is_exhausted": true },
                    { "code": "default_weekly", "used_ratio": 0.1, "is_exhausted": false }
                ]
            }
        }));

        let alpha = service.member_signals("codex", &key, None, Some("vendor-alpha-v2"));
        assert!(alpha.quota_exhausted);
        // An unrelated model has no identifiable bucket and therefore falls
        // back to the account-level snapshot rather than guessing a family.
        let unknown = service.member_signals("codex", &key, None, Some("vendor-gamma-v2"));
        assert!(unknown.quota_exhausted);
    }

    #[test]
    fn compact_model_bucket_identity_matches_request_tokens() {
        let service = ProviderPoolService::with_builtin_adapters();
        let mut key = sample_key(None);
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "exhausted": true,
                "windows": [
                    {
                        "code": "additional_0_primary_window",
                        "scope": "model",
                        "model": "spark",
                        "used_ratio": 0.0,
                        "is_exhausted": false
                    }
                ]
            }
        }));

        let signals = service.member_signals("codex", &key, None, Some("gpt-5.3-codex-spark"));
        assert!(
            !signals.quota_exhausted,
            "a compact bucket name should match a token in the selected model"
        );
    }

    #[test]
    fn model_family_token_matches_versioned_alias_without_hardcoding_name() {
        let service = ProviderPoolService::with_builtin_adapters();
        let mut key = sample_key(None);
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "exhausted": true,
                "windows": [{
                    "code": "additional_0_primary_window",
                    "scope": "model",
                    "model": "gpt-5.3-codex-spark",
                    "used_ratio": 1.0,
                    "is_exhausted": true
                }]
            }
        }));

        let signals = service.member_signals("codex", &key, None, Some("gpt-5.4-codex-spark"));
        assert!(signals.quota_exhausted);
    }

    #[test]
    fn model_only_snapshot_does_not_poison_account_fallback() {
        let service = ProviderPoolService::with_builtin_adapters();
        let mut key = sample_key(None);
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "exhausted": true,
                "windows": [{
                    "code": "model:vendor-alpha",
                    "scope": "model",
                    "model": "vendor-alpha",
                    "used_ratio": 1.0,
                    "is_exhausted": true
                }]
            }
        }));

        let signals = service.member_signals("codex", &key, None, None);
        assert!(
            !signals.quota_exhausted,
            "account-level inspection must ignore model-only buckets"
        );
    }

    #[test]
    fn model_map_quota_is_resolved_without_materialized_windows() {
        let service = ProviderPoolService::with_builtin_adapters();
        let mut key = sample_key(None);
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "grok",
                "exhausted": false,
                "quota_by_model": {
                    "vendor-alpha": {
                        "remaining": 0.0,
                        "total": 10.0
                    },
                    "vendor-beta": {
                        "remaining": 5.0,
                        "total": 10.0
                    }
                }
            }
        }));

        let alpha = service.member_signals("grok", &key, None, Some("vendor-alpha"));
        let beta = service.member_signals("grok", &key, None, Some("vendor-beta"));
        assert!(alpha.quota_exhausted);
        assert!(!beta.quota_exhausted);
    }

    #[test]
    fn raw_provider_metadata_model_bucket_overrides_account_snapshot() {
        let service = ProviderPoolService::with_builtin_adapters();
        let mut key = sample_key(Some(json!({
            "codex": {
                "updated_at": 1_700_000_001u64,
                "additional_quota_windows": [{
                    "scope": "model",
                    "model": "future-spark",
                    "used_ratio": 0.0,
                    "is_exhausted": false
                }]
            }
        })));
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "exhausted": true,
                "windows": [{
                    "code": "primary",
                    "scope": "account",
                    "used_ratio": 1.0,
                    "is_exhausted": true
                }]
            }
        }));

        let signals = service.member_signals("codex", &key, None, Some("future-spark"));
        assert!(
            !signals.quota_exhausted,
            "a newer model bucket in provider metadata must not inherit an exhausted account bucket"
        );
    }

    #[test]
    fn model_quota_availability_suppresses_account_hard_block() {
        let service = ProviderPoolService::with_builtin_adapters();
        let mut key = sample_key(None);
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "allowed": false,
                "limit_reached": true,
                "exhausted": true,
                "windows": [{
                    "scope": "model",
                    "model": "future-model",
                    "used_ratio": 0.0,
                    "is_exhausted": false
                }]
            }
        }));

        let signals = service.member_signals("codex", &key, None, Some("future-model"));
        assert!(!signals.quota_exhausted);
        assert!(!signals.quota_hard_blocked);
    }

    #[test]
    fn newer_model_quota_observation_wins_over_stale_snapshot() {
        let service = ProviderPoolService::with_builtin_adapters();
        let mut key = sample_key(Some(json!({
            "codex": {
                "updated_at": 1_700_000_200u64,
                "additional_quota_windows": [{
                    "scope": "model",
                    "model": "future-model",
                    "used_ratio": 0.0,
                    "is_exhausted": false
                }]
            }
        })));
        key.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "observed_at": 1_700_000_100u64,
                "exhausted": true,
                "windows": [{
                    "scope": "model",
                    "model": "future-model",
                    "used_ratio": 1.0,
                    "is_exhausted": true
                }]
            }
        }));

        let signals = service.member_signals("codex", &key, None, Some("future-model"));
        assert!(!signals.quota_exhausted);
    }

    #[test]
    fn codex_newer_flat_quota_metadata_clears_stale_exhausted_snapshot() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_secs();
        let service = ProviderPoolService::with_builtin_adapters();
        // Headers and quota refreshes persist the flat metadata shape while the
        // derived snapshot may still describe the previous quota observation.
        for used_percent in [83.0, 99.0, 100.0] {
            let mut key = sample_key(Some(json!({
                "codex": {
                    "updated_at": now,
                    "primary_used_percent": used_percent,
                    "primary_reset_at": now + 3600
                }
            })));
            key.status_snapshot = Some(json!({
                "quota": {
                    "provider_type": "codex",
                    "observed_at": now - 60,
                    "updated_at": now - 60,
                    "code": "exhausted",
                    "exhausted": true,
                    "allowed": false,
                    "limit_reached": true,
                    "reset_at": now + 3600,
                    "windows": [{
                        "code": "weekly",
                        "scope": "account",
                        "used_ratio": 1.0,
                        "remaining_ratio": 0.0,
                        "is_exhausted": true,
                        "reset_at": now + 3600
                    }]
                }
            }));
            for model in [None, Some("gpt-5.4"), Some("o3")] {
                let signals = service.member_signals("codex", &key, None, model);
                assert_eq!(signals.quota_exhausted, used_percent >= 100.0);
                assert!(!signals.quota_hard_blocked);
                assert_eq!(
                    provider_pool_key_minimum_quota_reached(&key, "codex", model),
                    used_percent >= 99.0
                );
            }
        }
    }

    #[test]
    fn codex_account_and_model_quota_agree_on_explicit_allow_and_deny_flags() {
        let service = ProviderPoolService::with_builtin_adapters();
        for (flags, exhausted) in [
            (json!({ "allowed": true }), false),
            (json!({ "limit_reached": false }), false),
            (json!({ "allowed": true, "limit_reached": true }), true),
            (json!({ "allowed": false, "limit_reached": false }), true),
        ] {
            for use_windows in [false, true] {
                let mut metadata = flags.clone();
                metadata["updated_at"] = json!(300);
                if use_windows {
                    metadata["windows"] = json!([{ "code": "weekly", "used_ratio": 1.0 }]);
                } else {
                    metadata["primary_used_percent"] = json!(100.0);
                }
                let key = sample_key(Some(json!({ "codex": metadata })));
                for model in [None, Some("gpt-5.4"), Some("o3")] {
                    assert_eq!(
                        service
                            .member_signals("codex", &key, None, model)
                            .quota_exhausted,
                        exhausted,
                        "flags={flags}, use_windows={use_windows}, model={model:?}"
                    );
                    // The opt-in reserve still protects a numerically full
                    // window even when the upstream reports it as allowed.
                    assert!(provider_pool_key_minimum_quota_reached(
                        &key, "codex", model
                    ));
                }
            }
        }
    }

    #[test]
    fn codex_model_quota_honors_latest_account_refusal_without_blocking_spark() {
        let service = ProviderPoolService::with_builtin_adapters();
        for include_window in [false, true] {
            let mut metadata = json!({
                "updated_at": 300,
                "allowed": false,
                "limit_reached": true,
                "spark_primary_used_percent": 17.0
            });
            if include_window {
                metadata["primary_used_percent"] = json!(83.0);
            }
            let mut key = sample_key(Some(json!({ "codex": metadata })));
            for include_snapshot in [false, true] {
                if include_snapshot {
                    key.status_snapshot = Some(json!({
                        "quota": {
                            "provider_type": "codex",
                            "observed_at": 200,
                            "exhausted": false,
                            "windows": [{ "code": "weekly", "used_ratio": 0.83 }]
                        }
                    }));
                }
                assert!(
                    service
                        .member_signals("codex", &key, None, Some("gpt-5.4"))
                        .quota_exhausted
                );
                assert!(
                    !service
                        .member_signals("codex", &key, None, Some("gpt-5.3-codex-spark"))
                        .quota_exhausted
                );
            }
        }
    }

    #[test]
    fn codex_quota_source_freshness_supports_all_timestamp_formats() {
        let service = ProviderPoolService::with_builtin_adapters();
        for observed_at in [
            json!(1_700_000_200_u64),
            json!(1_700_000_200_000_u64),
            json!("1700000200000"),
            json!("2023-11-14T22:16:40Z"),
        ] {
            for use_windows in [false, true] {
                let metadata = if use_windows {
                    json!({
                        "updated_at": observed_at,
                        "windows": [{ "code": "weekly", "used_ratio": 0.83 }]
                    })
                } else {
                    json!({ "updated_at": observed_at, "primary_used_percent": 83.0 })
                };
                let mut key = sample_key(Some(json!({ "codex": metadata })));
                key.status_snapshot = Some(json!({
                    "quota": {
                        "provider_type": "codex",
                        "observed_at": "2023-11-14T22:15:00Z",
                        "exhausted": true,
                        "allowed": false,
                        "windows": [{ "code": "weekly", "used_ratio": 1.0 }]
                    }
                }));
                for model in [None, Some("gpt-5.4")] {
                    let signals = service.member_signals("codex", &key, None, model);
                    assert!(!signals.quota_exhausted, "timestamp={observed_at}");
                    assert!(!signals.quota_hard_blocked, "timestamp={observed_at}");
                    assert!(!provider_pool_key_minimum_quota_reached(
                        &key, "codex", model
                    ));
                }

                // Swap freshness while retaining the same observations. An
                // older usable bucket cannot erase a later exhausted snapshot.
                key.status_snapshot.as_mut().unwrap()["quota"]["observed_at"] =
                    json!("2023-11-14T22:18:20Z");
                assert!(provider_pool_key_account_quota_exhausted(&key, "codex"));
                assert!(provider_pool_key_quota_hard_blocked(&key, "codex"));
                assert_eq!(
                    provider_pool_key_model_quota_exhausted(&key, "codex", "gpt-5.4"),
                    Some(true)
                );
            }
        }
    }

    #[test]
    fn codex_account_quota_recovery_requires_new_account_observation() {
        for metadata in [
            json!({ "updated_at": 100, "primary_used_percent": 83.0 }),
            json!({ "primary_used_percent": 83.0 }),
            json!({ "updated_at": 300, "plan_type": "plus" }),
            json!({ "updated_at": 300, "credits_unlimited": false }),
            json!({ "updated_at": 300, "spark_primary_used_percent": 83.0 }),
            json!({ "updated_at": 300, "windows": [{ "code": "weekly" }] }),
        ] {
            let mut key = sample_key(Some(json!({ "codex": metadata })));
            key.status_snapshot = Some(json!({
                "quota": {
                    "provider_type": "codex",
                    "observed_at": 200,
                    "exhausted": true,
                    "allowed": false,
                    "limit_reached": true,
                    "windows": [{ "code": "weekly", "used_ratio": 1.0 }]
                }
            }));
            assert!(provider_pool_key_account_quota_exhausted(&key, "codex"));
            assert!(provider_pool_key_quota_hard_blocked(&key, "codex"));
            assert_eq!(
                provider_pool_key_model_quota_exhausted(&key, "codex", "gpt-5.4"),
                Some(true)
            );
        }

        let mut key = sample_key(Some(json!({
            "codex": {
                "updated_at": 300,
                "primary_used_percent": 83.0,
                "allowed": false,
                "limit_reached": true
            }
        })));
        key.status_snapshot = Some(json!({
            "quota": { "provider_type": "codex", "updated_at": 200, "exhausted": false }
        }));
        assert!(provider_pool_key_account_quota_exhausted(&key, "codex"));
        assert!(provider_pool_key_quota_hard_blocked(&key, "codex"));
    }

    #[test]
    fn codex_account_updates_do_not_override_independent_model_quota() {
        for (spark_ratio, account_percent) in [(0.83, 100.0), (1.0, 83.0)] {
            let mut key = sample_key(Some(json!({
                "codex": { "updated_at": 300, "primary_used_percent": account_percent }
            })));
            key.status_snapshot = Some(json!({
                "quota": {
                    "provider_type": "codex",
                    "updated_at": 200,
                    "windows": [{ "code": "spark_5h", "used_ratio": spark_ratio }]
                }
            }));
            assert_eq!(
                provider_pool_key_model_quota_exhausted(&key, "codex", "gpt-5.3-codex-spark"),
                Some(spark_ratio >= 1.0)
            );
        }
    }

    #[test]
    fn codex_explicit_quota_block_is_hard_until_reset() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_secs();

        assert!(provider_pool_key_quota_hard_blocked(
            &sample_key(Some(json!({
                "codex": {
                    "updated_at": now,
                    "allowed": false,
                    "limit_reached": true,
                    "primary_reset_at": now.saturating_add(3600)
                }
            }))),
            "codex",
        ));
        assert!(!provider_pool_key_quota_hard_blocked(
            &sample_key(Some(json!({
                "codex": {
                    "updated_at": now,
                    "primary_used_percent": 100.0,
                    "primary_reset_at": now.saturating_add(3600)
                }
            }))),
            "codex",
        ));
        assert!(!provider_pool_key_quota_hard_blocked(
            &sample_key(Some(json!({
                "codex": {
                    "updated_at": now.saturating_sub(600),
                    "allowed": false,
                    "limit_reached": true,
                    "primary_reset_at": now.saturating_sub(60)
                }
            }))),
            "codex",
        ));

        let mut snapshot_blocked = sample_key(None);
        snapshot_blocked.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "exhausted": true,
                "allowed": false,
                "limit_reached": true,
                "usage_ratio": 0.91,
                "reset_at": now.saturating_add(3600)
            }
        }));
        assert!(provider_pool_key_quota_hard_blocked(
            &snapshot_blocked,
            "codex",
        ));
        snapshot_blocked.status_snapshot = Some(json!({
            "quota": {
                "version": 2,
                "provider_type": "codex",
                "exhausted": true,
                "allowed": false,
                "limit_reached": true,
                "usage_ratio": 0.91,
                "reset_at": now.saturating_sub(60)
            }
        }));
        assert!(!provider_pool_key_quota_hard_blocked(
            &snapshot_blocked,
            "codex",
        ));
    }

    #[test]
    fn grok_quota_tier_boundaries_match_pool_modes() {
        assert_eq!(
            grok_supported_quota_windows_for_tier(Some("basic")),
            [("quota_fast", "fast")]
        );
        assert_eq!(
            grok_supported_quota_windows_for_tier(Some("super")),
            [
                ("quota_auto", "auto"),
                ("quota_fast", "fast"),
                ("quota_expert", "expert"),
                ("quota_grok_4_3", "grok-420-computer-use-sa")
            ]
        );
        assert_eq!(
            grok_supported_quota_windows_for_tier(Some("heavy")),
            [
                ("quota_auto", "auto"),
                ("quota_fast", "fast"),
                ("quota_expert", "expert"),
                ("quota_heavy", "heavy"),
                ("quota_grok_4_3", "grok-420-computer-use-sa")
            ]
        );
    }

    #[test]
    fn grok_pool_tier_infers_from_live_quota_totals() {
        let bucket = json!({
            "quota_by_model": {
                "quota_fast": {
                    "remaining": 20.0,
                    "total": 30.0
                },
                "quota_auto": {
                    "remaining": 7.0,
                    "total": 7.0
                }
            }
        });
        let bucket = bucket.as_object().expect("bucket should be object");

        assert_eq!(grok_pool_tier_from_quota_bucket(bucket), Some("basic"));
    }

    #[test]
    fn grok_model_name_maps_to_quota_window() {
        assert_eq!(
            grok_quota_window_key_for_model(Some("grok-4.20-fast")),
            Some("quota_fast")
        );
        assert_eq!(
            grok_quota_window_key_for_model(Some("grok-4.20-multi-agent-0309")),
            Some("quota_heavy")
        );
        assert_eq!(
            grok_quota_window_key_for_model(Some("grok-4.3-beta")),
            Some("quota_grok_4_3")
        );
    }

    #[test]
    fn plan_tier_derivation_normalizes_provider_prefix() {
        let key = sample_key(Some(json!({
            "codex": {
                "plan_type": "codex:Plus"
            }
        })));

        assert_eq!(
            derive_oauth_plan_type("codex", &key, None).as_deref(),
            Some("plus")
        );
    }

    #[test]
    fn plan_tier_derivation_reads_quota_snapshot() {
        let mut key = sample_key(None);
        key.status_snapshot = Some(json!({
            "quota": {
                "plan_type": "team"
            }
        }));

        assert_eq!(
            derive_oauth_plan_type("codex", &key, None).as_deref(),
            Some("team")
        );
    }
}
