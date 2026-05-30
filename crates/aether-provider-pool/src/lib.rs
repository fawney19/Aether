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
    build_antigravity_pool_quota_request, build_chatgpt_web_pool_quota_request,
    build_codex_pool_quota_request, build_gemini_cli_pool_quota_request,
    build_kiro_pool_quota_request, build_windsurf_pool_model_configs_request,
    build_windsurf_pool_model_configs_request_with_base_url, build_windsurf_pool_quota_request,
    build_windsurf_pool_quota_request_with_base_url, build_windsurf_pool_rate_limit_request,
    build_windsurf_pool_rate_limit_request_with_base_url, enrich_chatgpt_web_quota_metadata,
    grok_mode_id_for_model, grok_pool_tier_from_quota_bucket, grok_quota_window_key_for_model,
    grok_supported_quota_windows_for_tier, normalize_chatgpt_web_image_quota_limit,
    AntigravityProviderPoolAdapter, ChatGptWebProviderPoolAdapter, CodexProviderPoolAdapter,
    DefaultProviderPoolAdapter, GeminiCliProviderPoolAdapter, GrokProviderPoolAdapter,
    KiroPoolQuotaAuthInput, KiroProviderPoolAdapter, UnsupportedQuotaProviderPoolAdapter,
    ANTIGRAVITY_FETCH_AVAILABLE_MODELS_PATH, CHATGPT_WEB_CONVERSATION_INIT_PATH,
    CHATGPT_WEB_DEFAULT_BASE_URL, CODEX_WHAM_USAGE_URL, GEMINI_CLI_RETRIEVE_USER_QUOTA_PATH,
    GEMINI_CLI_USER_AGENT, KIRO_USAGE_LIMITS_PATH, KIRO_USAGE_SDK_VERSION,
    WINDSURF_MODEL_CONFIGS_PATH, WINDSURF_RATE_LIMIT_PATH, WINDSURF_USER_STATUS_PATH,
};
pub use quota::{
    provider_pool_key_account_quota_exhausted, provider_pool_key_scheduling_label,
    provider_pool_member_quota_snapshot, provider_pool_quota_metadata_provider_type,
    provider_pool_quota_metadata_updated_at, provider_pool_quota_snapshot_updated_at,
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
                "windsurf"
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
                "codex",
                "gemini_cli",
                "grok",
                "kiro",
                "windsurf"
            ]
        );
        assert!(service.supports_quota_refresh("codex"));
        assert!(service.supports_quota_refresh("antigravity"));
        assert!(service.supports_quota_refresh("grok"));
        assert!(service.supports_quota_refresh("gemini_cli"));
        assert!(service.supports_quota_refresh("windsurf"));
        assert_eq!(
            service.quota_refresh_unsupported_message("claude_code"),
            "Claude Code 暂不支持自动刷新额度：上游没有稳定可用的账号额度查询接口"
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
        assert!(spec.accept_invalid_certs);
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

        let signals = service.member_signals("windsurf", &key, None);

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

        let signals = service.member_signals("windsurf", &key, None);

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

        assert_eq!(
            free_first["providers"],
            json!(["codex", "grok", "kiro", "windsurf"])
        );
        assert_eq!(
            recent_refresh["providers"],
            json!(["codex", "grok", "kiro", "windsurf"])
        );
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
    fn provider_quota_exhaustion_is_adapter_owned() {
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
    fn provider_quota_exhaustion_metadata_expires_after_reset_at() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_secs();

        assert!(!provider_pool_key_account_quota_exhausted(
            &sample_key(Some(json!({
                "codex": {
                    "updated_at": now.saturating_sub(600),
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
