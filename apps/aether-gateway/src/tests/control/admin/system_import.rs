use std::sync::{Arc, Mutex};

use aether_crypto::{
    decrypt_python_fernet_ciphertext, encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY,
};
use aether_data::repository::auth::InMemoryAuthApiKeySnapshotRepository;
use aether_data::repository::auth_modules::{
    AuthModuleReadRepository, InMemoryAuthModuleReadRepository, StoredOAuthProviderModuleConfig,
};
use aether_data::repository::global_models::InMemoryGlobalModelReadRepository;
use aether_data::repository::oauth_providers::{
    InMemoryOAuthProviderRepository, OAuthProviderReadRepository, StoredOAuthProviderConfig,
};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::users::StoredUserAuthRecord;
use aether_data::repository::wallet::{StoredWalletSnapshot, WalletLookupKey};
use aether_data_contracts::repository::global_models::{
    AdminGlobalModelListQuery, AdminProviderModelListQuery, GlobalModelReadRepository,
    StoredPublicGlobalModel,
};
use aether_data_contracts::repository::provider_catalog::ProviderCatalogReadRepository;
use axum::body::{Body, Bytes};
use axum::http::HeaderMap;
use axum::routing::{any, post};
use axum::{extract::Request, Router};
use http::StatusCode;
use serde_json::{json, Value};

use super::super::helpers::{sample_endpoint, sample_key, sample_provider};
use super::super::{build_router_with_state, start_server, AppState};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

fn build_empty_admin_system_data_state() -> GatewayDataState {
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(Vec::<
        StoredPublicGlobalModel,
    >::new()));
    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::seed(
        Vec::<StoredOAuthProviderModuleConfig>::new(),
        None,
    ));
    let oauth_provider_repository = Arc::new(InMemoryOAuthProviderRepository::seed(Vec::<
        StoredOAuthProviderConfig,
    >::new()));

    GatewayDataState::with_provider_catalog_repository_for_tests(provider_catalog_repository)
        .with_global_model_repository_for_tests(global_model_repository)
        .attach_auth_module_repository_for_tests(auth_module_repository)
        .attach_oauth_provider_repository_for_tests(oauth_provider_repository)
        .with_system_config_values_for_tests(Vec::<(String, Value)>::new())
        .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY)
}

fn sample_system_import_payload() -> Value {
    json!({
        "version": "2.2",
        "merge_mode": "overwrite",
        "global_models": [{
            "name": "gpt-5",
            "display_name": "GPT 5",
            "default_price_per_request": 0.03,
            "default_tiered_pricing": {
                "tiers": [{
                    "up_to": null,
                    "input_price_per_1m": 4.0,
                    "output_price_per_1m": 20.0,
                }]
            },
            "supported_capabilities": ["streaming", "vision"],
            "config": { "quality": "high" },
            "is_active": true
        }],
        "providers": [{
            "name": "import-openai",
            "provider_type": "custom",
            "website": "https://example.com",
            "billing_type": "pay_as_you_go",
            "provider_priority": 10,
            "keep_priority_on_conversion": false,
            "enable_format_conversion": true,
            "is_active": true,
            "max_retries": 2,
            "request_timeout": 30.0,
            "stream_first_byte_timeout": 15.0,
            "config": {
                "provider_ops": {
                    "connector": {
                        "credentials": {
                            "api_key": "ops-secret"
                        }
                    }
                }
            },
            "endpoints": [{
                "api_format": "openai:chat",
                "base_url": "https://api.example.com",
                "max_retries": 2,
                "is_active": true
            }],
            "api_keys": [{
                "name": "primary",
                "api_formats": ["openai:chat"],
                "auth_type": "api_key",
                "api_key": "sk-import-123",
                "internal_priority": 5,
                "is_active": true
            }],
            "models": [{
                "global_model_name": "gpt-5",
                "provider_model_name": "gpt-5",
                "price_per_request": 0.03,
                "tiered_pricing": {
                    "tiers": [{
                        "up_to": null,
                        "input_price_per_1m": 4.0,
                        "output_price_per_1m": 20.0,
                    }]
                },
                "supports_vision": true,
                "supports_function_calling": true,
                "supports_streaming": true,
                "supports_extended_thinking": false,
                "supports_image_generation": false,
                "is_active": true,
                "config": {
                    "kind": "chat"
                }
            }]
        }],
        "ldap_config": {
            "server_url": "ldaps://ldap.example.com",
            "bind_dn": "cn=admin,dc=example,dc=com",
            "bind_password": "bind-secret",
            "base_dn": "dc=example,dc=com",
            "user_search_filter": "(uid={username})",
            "username_attr": "uid",
            "email_attr": "mail",
            "display_name_attr": "displayName",
            "is_enabled": false,
            "is_exclusive": false,
            "use_starttls": true,
            "connect_timeout": 10
        },
        "oauth_providers": [{
            "provider_type": "linuxdo",
            "display_name": "Linux Do",
            "client_id": "linuxdo-client",
            "client_secret": "linuxdo-secret",
            "authorization_url_override": "https://connect.linux.do/oauth2/authorize",
            "token_url_override": "https://connect.linux.do/oauth2/token",
            "userinfo_url_override": "https://connect.linux.do/api/user",
            "scopes": ["openid", "profile"],
            "redirect_uri": "https://backend.example.com/oauth/callback",
            "frontend_callback_url": "https://frontend.example.com/auth/callback",
            "attribute_mapping": { "email": "email" },
            "extra_config": { "team": true },
            "is_enabled": true
        }],
        "system_configs": [
            {
                "key": "site_name",
                "value": "Imported Aether",
                "description": "Site name"
            },
            {
                "key": "smtp_password",
                "value": "smtp-secret",
                "description": "SMTP secret"
            }
        ]
    })
}

fn sample_oauth_system_import_payload(access_token: &str, refresh_token: &str) -> Value {
    json!({
        "version": "2.2",
        "merge_mode": "overwrite",
        "global_models": [],
        "providers": [{
            "name": "oauth-import-provider",
            "provider_type": "codex",
            "website": "https://example.com",
            "is_active": true,
            "endpoints": [{
                "api_format": "openai:responses",
                "base_url": "https://chatgpt.com",
                "is_active": true
            }],
            "api_keys": [{
                "name": "oauth-primary",
                "auth_type": "oauth",
                "api_key": access_token,
                "auth_config": format!(
                    "{{\"provider_type\":\"codex\",\"refresh_token\":\"{}\",\"email\":\"alice@example.com\",\"account_id\":\"acct-codex-123\",\"plan_type\":\"plus\"}}",
                    refresh_token
                ),
                "api_formats": ["openai:responses"],
                "is_active": true
            }],
            "models": []
        }]
    })
}

fn fixture_system_import_payload(name: &str) -> Value {
    let raw = match name {
        "v20" => include_str!("../../fixtures/admin_system/config_export_v20.json"),
        "v21" => include_str!("../../fixtures/admin_system/config_export_v21.json"),
        "v22" => include_str!("../../fixtures/admin_system/config_export_v22.json"),
        _ => panic!("unknown fixture: {name}"),
    };
    serde_json::from_str(raw).expect("fixture json should parse")
}

fn sample_import_admin_user(user_id: &str) -> StoredUserAuthRecord {
    StoredUserAuthRecord::new(
        user_id.to_string(),
        Some("admin@example.com".to_string()),
        true,
        "admin".to_string(),
        Some("admin-hash".to_string()),
        "admin".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        Some(chrono::Utc::now()),
        Some(chrono::Utc::now()),
    )
    .expect("admin user should build")
}

#[tokio::test]
async fn gateway_imports_admin_system_config_locally_and_persists_data() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(Vec::<
        StoredPublicGlobalModel,
    >::new()));
    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::seed(
        Vec::<StoredOAuthProviderModuleConfig>::new(),
        None,
    ));
    let oauth_provider_repository = Arc::new(InMemoryOAuthProviderRepository::seed(Vec::<
        StoredOAuthProviderConfig,
    >::new()));

    let data_state = GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
        &provider_catalog_repository,
    ))
    .with_global_model_repository_for_tests(Arc::clone(&global_model_repository))
    .attach_auth_module_repository_for_tests(Arc::clone(&auth_module_repository))
    .attach_oauth_provider_repository_for_tests(Arc::clone(&oauth_provider_repository))
    .with_system_config_values_for_tests(Vec::<(String, Value)>::new())
    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let response = client
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&sample_system_import_payload())
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["message"], "配置导入成功");
    assert_eq!(payload["stats"]["global_models"]["created"], json!(1));
    assert_eq!(payload["stats"]["providers"]["created"], json!(1));
    assert_eq!(payload["stats"]["endpoints"]["created"], json!(1));
    assert_eq!(payload["stats"]["keys"]["created"], json!(1));
    assert_eq!(payload["stats"]["models"]["created"], json!(1));
    assert_eq!(payload["stats"]["ldap"]["created"], json!(1));
    assert_eq!(payload["stats"]["oauth"]["created"], json!(1));
    assert_eq!(payload["stats"]["system_configs"]["created"], json!(2));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let global_models = global_model_repository
        .list_admin_global_models(&AdminGlobalModelListQuery {
            offset: 0,
            limit: 10_000,
            is_active: None,
            search: None,
        })
        .await
        .expect("global models should load");
    assert_eq!(global_models.items.len(), 1);
    assert_eq!(global_models.items[0].name, "gpt-5");

    let providers = provider_catalog_repository
        .list_providers(false)
        .await
        .expect("providers should load");
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].name, "import-openai");
    assert!(providers[0].enable_format_conversion);

    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    let endpoints = provider_catalog_repository
        .list_endpoints_by_provider_ids(&provider_ids)
        .await
        .expect("endpoints should load");
    assert_eq!(endpoints.len(), 1);
    assert_eq!(endpoints[0].api_format, "openai:chat");

    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(&provider_ids)
        .await
        .expect("keys should load");
    assert_eq!(keys.len(), 1);
    assert_eq!(
        decrypt_python_fernet_ciphertext(DEVELOPMENT_ENCRYPTION_KEY, &keys[0].encrypted_api_key)
            .expect("api key should decrypt"),
        "sk-import-123"
    );

    let provider_models = global_model_repository
        .list_admin_provider_models(&AdminProviderModelListQuery {
            provider_id: providers[0].id.clone(),
            is_active: None,
            offset: 0,
            limit: 10_000,
        })
        .await
        .expect("provider models should load");
    assert_eq!(provider_models.len(), 1);
    assert_eq!(provider_models[0].provider_model_name, "gpt-5");
    assert_eq!(
        provider_models[0].global_model_id,
        global_models.items[0].id
    );

    let ldap_config = auth_module_repository
        .get_ldap_config()
        .await
        .expect("ldap config should load")
        .expect("ldap config should exist");
    assert_eq!(ldap_config.server_url, "ldaps://ldap.example.com");
    assert_eq!(
        decrypt_python_fernet_ciphertext(
            DEVELOPMENT_ENCRYPTION_KEY,
            ldap_config
                .bind_password_encrypted
                .as_deref()
                .expect("bind password should exist"),
        )
        .expect("ldap password should decrypt"),
        "bind-secret"
    );

    let oauth_provider = oauth_provider_repository
        .get_oauth_provider_config("linuxdo")
        .await
        .expect("oauth config should load")
        .expect("oauth config should exist");
    assert_eq!(oauth_provider.client_id, "linuxdo-client");
    assert_eq!(
        decrypt_python_fernet_ciphertext(
            DEVELOPMENT_ENCRYPTION_KEY,
            oauth_provider
                .client_secret_encrypted
                .as_deref()
                .expect("oauth secret should exist"),
        )
        .expect("oauth secret should decrypt"),
        "linuxdo-secret"
    );

    let export_response = client
        .get(format!("{gateway_url}/api/admin/system/config/export"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("export request should succeed");
    assert_eq!(export_response.status(), StatusCode::OK);
    let export_payload: Value = export_response
        .json()
        .await
        .expect("export json should parse");

    let exported_provider = export_payload["providers"]
        .as_array()
        .and_then(|items| items.first())
        .expect("provider export should exist");
    assert_eq!(
        exported_provider["config"]["provider_ops"]["connector"]["credentials"]["api_key"],
        "ops-secret"
    );

    let exported_ldap = export_payload["ldap_config"]
        .as_object()
        .expect("ldap export should exist");
    assert_eq!(exported_ldap["bind_password"], "bind-secret");

    let exported_oauth = export_payload["oauth_providers"]
        .as_array()
        .and_then(|items| items.first())
        .expect("oauth export should exist");
    assert_eq!(exported_oauth["client_secret"], "linuxdo-secret");

    let exported_system_configs = export_payload["system_configs"]
        .as_array()
        .expect("system configs export should exist");
    let exported_site_name = exported_system_configs
        .iter()
        .find(|entry| entry["key"] == "site_name")
        .expect("site_name should exist");
    let exported_smtp_password = exported_system_configs
        .iter()
        .find(|entry| entry["key"] == "smtp_password")
        .expect("smtp_password should exist");
    assert_eq!(exported_site_name["value"], "Imported Aether");
    assert_eq!(exported_smtp_password["value"], "smtp-secret");

    gateway_handle.abort();
    upstream_handle.abort();
    let _ = upstream_url;
}

#[tokio::test]
async fn gateway_returns_503_for_admin_system_config_import_when_local_data_is_unavailable() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({ "version": "2.2" }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "Admin system data unavailable");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
    let _ = upstream_url;
}

#[tokio::test]
async fn gateway_rejects_legacy_admin_system_config_import_versions() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(build_empty_admin_system_data_state()),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    for version in ["2.0", "2.1"] {
        let response = client
            .post(format!("{gateway_url}/api/admin/system/config/import"))
            .header(GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .json(&json!({
                "version": version,
                "merge_mode": "skip",
                "global_models": [],
                "providers": []
            }))
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let payload: Value = response.json().await.expect("json body should parse");
        let detail = payload["detail"]
            .as_str()
            .expect("detail should be a string");
        assert!(detail.contains(&format!("不支持的配置版本: {version}")));
        assert!(detail.contains("支持的版本: 2.2"));
    }

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_imports_admin_system_users_locally_and_persists_data() {
    let user_wallet_updated_at = "2024-05-06T07:08:09Z";
    let standalone_wallet_updated_at = "2024-06-07T08:09:10Z";
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().fallback(any(move |_request: Request| {
        let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
        async move {
            *upstream_hits_inner.lock().expect("mutex should lock") += 1;
            (StatusCode::OK, Body::from("unexpected upstream hit"))
        }
    }));

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::default());
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_auth_api_key_repository_for_tests(Arc::clone(&auth_repository))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_auth_users_for_tests([sample_import_admin_user("admin-user-123")])
        .with_auth_wallets_for_tests(Vec::<StoredWalletSnapshot>::new());
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/users/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "version": "2.2",
            "merge_mode": "overwrite",
            "users": [{
                "email": "alice@example.com",
                "email_verified": true,
                "username": "alice",
                "password_hash": "argon2:imported-user-hash",
                "role": "user",
                "allowed_providers": ["openai"],
                "allowed_api_formats": ["openai:chat"],
                "allowed_models": ["gpt-5"],
                "rate_limit": 77,
                "is_active": true,
                "wallet": {
                    "balance": 20.0,
                    "recharge_balance": 15.0,
                    "gift_balance": 5.0,
                    "limit_mode": "finite",
                    "currency": "CNY",
                    "status": "locked",
                    "total_recharged": 48.5,
                    "total_consumed": 31.25,
                    "total_refunded": 2.5,
                    "total_adjusted": 7.75,
                    "updated_at": user_wallet_updated_at
                },
                "api_keys": [{
                    "key": "sk-user-import-1",
                    "name": "Alice CLI",
                    "allowed_providers": ["openai"],
                    "allowed_api_formats": ["openai:chat"],
                    "allowed_models": ["gpt-5"],
                    "rate_limit": 60,
                    "concurrent_limit": 3,
                    "is_active": true,
                    "expires_at": "2099-01-01T00:00:00Z",
                    "auto_delete_on_expiry": false,
                    "total_requests": 12,
                    "total_tokens": 3456,
                    "total_cost_usd": 1.25
                }]
            }],
            "standalone_keys": [{
                "key": "sk-standalone-import-1",
                "name": "Imported Standalone",
                "allowed_providers": ["openai"],
                "allowed_api_formats": ["openai:chat"],
                "allowed_models": ["gpt-5"],
                "rate_limit": 90,
                "concurrent_limit": 4,
                "is_active": true,
                "expires_at": "2099-02-01T00:00:00Z",
                "auto_delete_on_expiry": false,
                "total_requests": 3,
                "total_tokens": 789,
                "total_cost_usd": 0.75,
                "wallet": {
                    "balance": 30.0,
                    "recharge_balance": 20.0,
                    "gift_balance": 10.0,
                    "limit_mode": "finite",
                    "currency": "EUR",
                    "status": "disabled",
                    "total_recharged": 91.0,
                    "total_consumed": 63.25,
                    "total_refunded": 4.5,
                    "total_adjusted": 13.0,
                    "updated_at": standalone_wallet_updated_at
                }
            }]
        }))
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["message"], "用户数据导入成功");
    assert_eq!(payload["stats"]["users"]["created"], json!(1));
    assert_eq!(payload["stats"]["api_keys"]["created"], json!(1));
    assert_eq!(payload["stats"]["standalone_keys"]["created"], json!(1));
    assert_eq!(payload["stats"]["errors"], json!([]));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let imported_user = state
        .find_user_auth_by_identifier("alice@example.com")
        .await
        .expect("user lookup should succeed")
        .expect("imported user should exist");
    assert_eq!(imported_user.username, "alice");
    assert_eq!(
        imported_user.password_hash.as_deref(),
        Some("argon2:imported-user-hash")
    );
    assert_eq!(imported_user.role, "user");
    assert_eq!(
        imported_user.allowed_providers,
        Some(vec!["openai".to_string()])
    );
    assert_eq!(
        imported_user.allowed_api_formats,
        Some(vec!["openai:chat".to_string()])
    );
    assert_eq!(
        imported_user.allowed_models,
        Some(vec!["gpt-5".to_string()])
    );
    assert!(imported_user.is_active);

    let user_wallet = state
        .find_wallet(WalletLookupKey::UserId(&imported_user.id))
        .await
        .expect("user wallet lookup should succeed")
        .expect("user wallet should exist");
    assert_eq!(user_wallet.balance, 15.0);
    assert_eq!(user_wallet.gift_balance, 5.0);
    assert_eq!(user_wallet.limit_mode, "finite");
    assert_eq!(user_wallet.currency, "CNY");
    assert_eq!(user_wallet.status, "locked");
    assert_eq!(user_wallet.total_recharged, 48.5);
    assert_eq!(user_wallet.total_consumed, 31.25);
    assert_eq!(user_wallet.total_refunded, 2.5);
    assert_eq!(user_wallet.total_adjusted, 7.75);
    assert_eq!(
        user_wallet.updated_at_unix_secs,
        chrono::DateTime::parse_from_rfc3339(user_wallet_updated_at)
            .expect("user wallet updated_at should parse")
            .timestamp() as u64
    );

    let user_api_keys = state
        .list_auth_api_key_export_records_by_user_ids(std::slice::from_ref(&imported_user.id))
        .await
        .expect("user api keys should load");
    assert_eq!(user_api_keys.len(), 1);
    assert_eq!(user_api_keys[0].name.as_deref(), Some("Alice CLI"));
    assert_eq!(user_api_keys[0].total_tokens, 3456);
    assert_eq!(
        user_api_keys[0].allowed_api_formats,
        Some(vec!["openai:chat".to_string()])
    );
    assert_eq!(
        decrypt_python_fernet_ciphertext(
            DEVELOPMENT_ENCRYPTION_KEY,
            user_api_keys[0]
                .key_encrypted
                .as_deref()
                .expect("encrypted user api key should exist"),
        )
        .expect("user api key should decrypt"),
        "sk-user-import-1"
    );

    let standalone_keys = state
        .list_auth_api_key_export_standalone_records()
        .await
        .expect("standalone api keys should load");
    assert_eq!(standalone_keys.len(), 1);
    assert_eq!(
        standalone_keys[0].name.as_deref(),
        Some("Imported Standalone")
    );
    assert_eq!(standalone_keys[0].total_tokens, 789);
    assert_eq!(
        decrypt_python_fernet_ciphertext(
            DEVELOPMENT_ENCRYPTION_KEY,
            standalone_keys[0]
                .key_encrypted
                .as_deref()
                .expect("encrypted standalone api key should exist"),
        )
        .expect("standalone api key should decrypt"),
        "sk-standalone-import-1"
    );

    let standalone_wallet = state
        .find_wallet(WalletLookupKey::ApiKeyId(&standalone_keys[0].api_key_id))
        .await
        .expect("standalone wallet lookup should succeed")
        .expect("standalone wallet should exist");
    assert_eq!(standalone_wallet.balance, 20.0);
    assert_eq!(standalone_wallet.gift_balance, 10.0);
    assert_eq!(standalone_wallet.limit_mode, "finite");
    assert_eq!(standalone_wallet.currency, "EUR");
    assert_eq!(standalone_wallet.status, "disabled");
    assert_eq!(standalone_wallet.total_recharged, 91.0);
    assert_eq!(standalone_wallet.total_consumed, 63.25);
    assert_eq!(standalone_wallet.total_refunded, 4.5);
    assert_eq!(standalone_wallet.total_adjusted, 13.0);
    assert_eq!(
        standalone_wallet.updated_at_unix_secs,
        chrono::DateTime::parse_from_rfc3339(standalone_wallet_updated_at)
            .expect("standalone wallet updated_at should parse")
            .timestamp() as u64
    );

    gateway_handle.abort();
    upstream_handle.abort();
    let _ = upstream_url;
}

#[tokio::test]
async fn gateway_imports_admin_system_config_fixture_v22() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(build_empty_admin_system_data_state()),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&fixture_system_import_payload("v22"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["message"], "配置导入成功");

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_admin_system_config_fixtures_from_removed_legacy_exports() {
    for fixture in ["v20", "v21"] {
        let version = match fixture {
            "v20" => "2.0",
            "v21" => "2.1",
            _ => unreachable!("unexpected fixture"),
        };
        let gateway = build_router_with_state(
            AppState::new()
                .expect("gateway should build")
                .with_data_state_for_tests(build_empty_admin_system_data_state()),
        );
        let (gateway_url, gateway_handle) = start_server(gateway).await;

        let response = reqwest::Client::new()
            .post(format!("{gateway_url}/api/admin/system/config/import"))
            .header(GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .json(&fixture_system_import_payload(fixture))
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "fixture {fixture} should be rejected"
        );
        let payload: Value = response.json().await.expect("json body should parse");
        let detail = payload["detail"]
            .as_str()
            .expect("detail should be a string");
        assert!(detail.contains(&format!("不支持的配置版本: {version}")));
        assert!(detail.contains("支持的版本: 2.2"));

        gateway_handle.abort();
    }
}

#[tokio::test]
async fn gateway_rejects_legacy_user_import_string_bool_field() {
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::default());
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_auth_api_key_repository_for_tests(Arc::clone(&auth_repository))
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_auth_users_for_tests([sample_import_admin_user("admin-user-123")])
        .with_auth_wallets_for_tests(Vec::<StoredWalletSnapshot>::new());
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/users/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "version": "2.2",
            "merge_mode": "overwrite",
            "users": [{
                "email": "legacy@example.com",
                "email_verified": "true"
            }]
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["detail"], "字段必须是布尔值");

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_reports_field_path_for_invalid_admin_system_config_import_shape() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(build_empty_admin_system_data_state()),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "version": "2.2",
            "providers": [{
                "name": "import-openai",
                "endpoints": [{
                    "api_format": "openai:chat",
                    "base_url": "https://api.example.com",
                    "is_active": "yes"
                }]
            }]
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: Value = response.json().await.expect("json body should parse");
    let detail = payload["detail"]
        .as_str()
        .expect("detail should be a string");
    assert!(detail.contains("配置文件格式无效"));
    assert!(detail.contains("providers[0].endpoints[0].is_active"));

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_admin_system_config_with_numeric_string_prices() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(build_empty_admin_system_data_state()),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let mut payload = sample_system_import_payload();
    payload["global_models"][0]["default_price_per_request"] = json!("1.80000000");
    payload["providers"][0]["request_timeout"] = json!("30");
    payload["providers"][0]["stream_first_byte_timeout"] = json!("15");
    payload["providers"][0]["models"][0]["price_per_request"] = json!("0.70000000");

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&payload)
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.expect("json body should parse");
    let detail = body["detail"].as_str().expect("detail should be a string");
    assert!(detail.contains("配置文件格式无效"));
    assert!(detail.contains("default_price_per_request"));

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_imports_oauth_provider_key_credentials_from_admin_system_config() {
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(Vec::<
        StoredPublicGlobalModel,
    >::new()));
    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::seed(
        Vec::<StoredOAuthProviderModuleConfig>::new(),
        None,
    ));
    let oauth_provider_repository = Arc::new(InMemoryOAuthProviderRepository::seed(Vec::<
        StoredOAuthProviderConfig,
    >::new()));

    let data_state = GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
        &provider_catalog_repository,
    ))
    .with_global_model_repository_for_tests(Arc::clone(&global_model_repository))
    .attach_auth_module_repository_for_tests(Arc::clone(&auth_module_repository))
    .attach_oauth_provider_repository_for_tests(Arc::clone(&oauth_provider_repository))
    .with_system_config_values_for_tests(Vec::<(String, Value)>::new())
    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&sample_oauth_system_import_payload(
            "oauth-access-token-1",
            "oauth-refresh-token-1",
        ))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);

    let providers = provider_catalog_repository
        .list_providers(false)
        .await
        .expect("providers should load");
    assert_eq!(providers.len(), 1);

    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(std::slice::from_ref(&providers[0].id))
        .await
        .expect("keys should load");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].auth_type, "oauth");
    assert_eq!(
        decrypt_python_fernet_ciphertext(DEVELOPMENT_ENCRYPTION_KEY, &keys[0].encrypted_api_key)
            .expect("oauth access token should decrypt"),
        "oauth-access-token-1"
    );
    let auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        keys[0]
            .encrypted_auth_config
            .as_deref()
            .expect("oauth auth config should exist"),
    )
    .expect("oauth auth config should decrypt");
    let auth_config: Value =
        serde_json::from_str(&auth_config).expect("oauth auth config json should parse");
    assert_eq!(auth_config["provider_type"], "codex");
    assert_eq!(auth_config["refresh_token"], "oauth-refresh-token-1");
    assert_eq!(auth_config["email"], "alice@example.com");

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_overwrites_oauth_provider_key_credentials_from_admin_system_import() {
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(Vec::<
        StoredPublicGlobalModel,
    >::new()));
    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::seed(
        Vec::<StoredOAuthProviderModuleConfig>::new(),
        None,
    ));
    let oauth_provider_repository = Arc::new(InMemoryOAuthProviderRepository::seed(Vec::<
        StoredOAuthProviderConfig,
    >::new()));

    let data_state = GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
        &provider_catalog_repository,
    ))
    .with_global_model_repository_for_tests(Arc::clone(&global_model_repository))
    .attach_auth_module_repository_for_tests(Arc::clone(&auth_module_repository))
    .attach_oauth_provider_repository_for_tests(Arc::clone(&oauth_provider_repository))
    .with_system_config_values_for_tests(Vec::<(String, Value)>::new())
    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    for (access_token, refresh_token) in [
        ("oauth-access-token-old", "oauth-refresh-token-old"),
        ("oauth-access-token-new", "oauth-refresh-token-new"),
    ] {
        let response = client
            .post(format!("{gateway_url}/api/admin/system/config/import"))
            .header(GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .json(&sample_oauth_system_import_payload(
                access_token,
                refresh_token,
            ))
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(response.status(), StatusCode::OK);
    }

    let providers = provider_catalog_repository
        .list_providers(false)
        .await
        .expect("providers should load");
    assert_eq!(providers.len(), 1);

    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(std::slice::from_ref(&providers[0].id))
        .await
        .expect("keys should load");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].name, "oauth-primary");
    assert_eq!(
        decrypt_python_fernet_ciphertext(DEVELOPMENT_ENCRYPTION_KEY, &keys[0].encrypted_api_key)
            .expect("oauth access token should decrypt"),
        "oauth-access-token-new"
    );
    let auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        keys[0]
            .encrypted_auth_config
            .as_deref()
            .expect("oauth auth config should exist"),
    )
    .expect("oauth auth config should decrypt");
    let auth_config: Value =
        serde_json::from_str(&auth_config).expect("oauth auth config json should parse");
    assert_eq!(auth_config["refresh_token"], "oauth-refresh-token-new");

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_overwrites_oauth_provider_key_credentials_from_admin_system_import_and_forces_refresh(
) {
    #[derive(Debug, Clone)]
    struct SeenRefreshRequest {
        content_type: String,
        body: String,
    }

    let seen_refresh = Arc::new(Mutex::new(None::<SeenRefreshRequest>));
    let seen_refresh_clone = Arc::clone(&seen_refresh);
    let refresh_hits = Arc::new(Mutex::new(0usize));
    let refresh_hits_clone = Arc::clone(&refresh_hits);
    let refresh_server = Router::new().route(
        "/oauth/token",
        post(move |headers: HeaderMap, body: Bytes| {
            let seen_refresh_inner = Arc::clone(&seen_refresh_clone);
            let refresh_hits_inner = Arc::clone(&refresh_hits_clone);
            async move {
                *refresh_hits_inner.lock().expect("mutex should lock") += 1;
                *seen_refresh_inner.lock().expect("mutex should lock") = Some(SeenRefreshRequest {
                    content_type: headers
                        .get(http::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    body: String::from_utf8(body.to_vec()).unwrap_or_default(),
                });
                axum::Json(json!({
                    "access_token": "oauth-access-token-refreshed",
                    "refresh_token": "oauth-refresh-token-refreshed",
                    "token_type": "Bearer",
                    "expires_in": 3600
                }))
            }
        }),
    );

    let mut provider = sample_provider("provider-codex-existing", "oauth-import-provider", 10);
    provider.provider_type = "codex".to_string();
    let endpoint = sample_endpoint(
        "endpoint-codex-existing",
        "provider-codex-existing",
        "openai:responses",
        "https://chatgpt.com/backend-api/codex",
    );
    let mut existing_key = sample_key(
        "key-codex-existing",
        "provider-codex-existing",
        "openai:responses",
        "oauth-access-token-old",
    );
    existing_key.name = "oauth-primary".to_string();
    existing_key.auth_type = "oauth".to_string();
    existing_key.expires_at_unix_secs = Some(1);
    existing_key.oauth_invalid_at_unix_secs = Some(1_700_000_000);
    existing_key.oauth_invalid_reason =
        Some("[REFRESH_FAILED] refresh_token 无效、已过期或已撤销，请重新登录授权".to_string());
    existing_key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"oauth-refresh-token-old","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","expires_at":1}"#,
        )
        .expect("auth config should encrypt"),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![existing_key],
    ));
    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(Vec::<
        StoredPublicGlobalModel,
    >::new()));
    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::seed(
        Vec::<StoredOAuthProviderModuleConfig>::new(),
        None,
    ));
    let oauth_provider_repository = Arc::new(InMemoryOAuthProviderRepository::seed(Vec::<
        StoredOAuthProviderConfig,
    >::new()));

    let (refresh_url, refresh_handle) = start_server(refresh_server).await;
    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::oauth_refresh::GenericOAuthRefreshAdapter::default()
                    .with_token_url_for_tests("codex", format!("{refresh_url}/oauth/token")),
            ),
        ]);
    let data_state = GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
        &provider_catalog_repository,
    ))
    .with_global_model_repository_for_tests(Arc::clone(&global_model_repository))
    .attach_auth_module_repository_for_tests(Arc::clone(&auth_module_repository))
    .attach_oauth_provider_repository_for_tests(Arc::clone(&oauth_provider_repository))
    .with_system_config_values_for_tests(Vec::<(String, Value)>::new())
    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state)
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&sample_oauth_system_import_payload(
            "oauth-access-token-new",
            "oauth-refresh-token-new",
        ))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(*refresh_hits.lock().expect("mutex should lock"), 1);

    let seen_refresh = seen_refresh
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("refresh request should be captured");
    assert_eq!(
        seen_refresh.content_type,
        "application/x-www-form-urlencoded"
    );
    assert!(seen_refresh.body.contains("grant_type=refresh_token"));
    assert!(seen_refresh
        .body
        .contains("refresh_token=oauth-refresh-token-new"));

    let providers = provider_catalog_repository
        .list_providers(false)
        .await
        .expect("providers should load");
    assert_eq!(providers.len(), 1);

    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(std::slice::from_ref(&providers[0].id))
        .await
        .expect("keys should load");
    assert_eq!(keys.len(), 1);
    let key = &keys[0];
    assert_eq!(key.name, "oauth-primary");
    assert_eq!(key.oauth_invalid_at_unix_secs, None);
    assert_eq!(key.oauth_invalid_reason, None);
    assert!(key.expires_at_unix_secs.is_some());
    assert_eq!(
        decrypt_python_fernet_ciphertext(DEVELOPMENT_ENCRYPTION_KEY, &key.encrypted_api_key)
            .expect("oauth access token should decrypt"),
        "oauth-access-token-refreshed"
    );

    let auth_config = decrypt_python_fernet_ciphertext(
        DEVELOPMENT_ENCRYPTION_KEY,
        key.encrypted_auth_config
            .as_deref()
            .expect("oauth auth config should exist"),
    )
    .expect("oauth auth config should decrypt");
    let auth_config: Value =
        serde_json::from_str(&auth_config).expect("oauth auth config json should parse");
    assert_eq!(auth_config["provider_type"], "codex");
    assert_eq!(
        auth_config["refresh_token"],
        "oauth-refresh-token-refreshed"
    );
    assert_eq!(auth_config["email"], "alice@example.com");
    assert_eq!(auth_config["account_id"], "acct-codex-123");
    assert_eq!(auth_config["plan_type"], "plus");
    assert_eq!(auth_config["token_type"], "Bearer");
    assert_eq!(auth_config["expires_at"].as_u64(), key.expires_at_unix_secs);

    gateway_handle.abort();
    refresh_handle.abort();
}

#[tokio::test]
async fn gateway_skips_proxy_nodes_during_admin_system_config_import() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(build_empty_admin_system_data_state()),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "version": "2.2",
            "merge_mode": "overwrite",
            "global_models": [],
            "providers": [],
            "proxy_nodes": [{
                "id": "legacy-node-1",
                "name": "Legacy Node",
                "ip": "127.0.0.1",
                "port": 8080
            }]
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["stats"]["proxy_nodes"]["skipped"], json!(1));
    assert!(payload["stats"]["errors"]
        .as_array()
        .expect("errors should be an array")
        .iter()
        .any(|item| item
            .as_str()
            .is_some_and(|value| value.contains("暂不支持导入代理节点"))));

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_preserves_manual_proxy_configs_while_skipping_proxy_nodes_during_import() {
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(Vec::<
        StoredPublicGlobalModel,
    >::new()));
    let auth_module_repository = Arc::new(InMemoryAuthModuleReadRepository::seed(
        Vec::<StoredOAuthProviderModuleConfig>::new(),
        None,
    ));
    let oauth_provider_repository = Arc::new(InMemoryOAuthProviderRepository::seed(Vec::<
        StoredOAuthProviderConfig,
    >::new()));

    let data_state = GatewayDataState::with_provider_catalog_repository_for_tests(Arc::clone(
        &provider_catalog_repository,
    ))
    .with_global_model_repository_for_tests(Arc::clone(&global_model_repository))
    .attach_auth_module_repository_for_tests(Arc::clone(&auth_module_repository))
    .attach_oauth_provider_repository_for_tests(Arc::clone(&oauth_provider_repository))
    .with_system_config_values_for_tests(Vec::<(String, Value)>::new())
    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "version": "2.2",
            "merge_mode": "overwrite",
            "global_models": [],
            "providers": [{
                "name": "manual-proxy-provider",
                "provider_type": "custom",
                "is_active": true,
                "proxy": {
                    "enabled": true,
                    "url": "https://proxy.example"
                },
                "endpoints": [{
                    "api_format": "openai:chat",
                    "base_url": "https://api.example.com",
                    "is_active": true
                }],
                "api_keys": [],
                "models": []
            }],
            "proxy_nodes": [{
                "id": "legacy-node-1",
                "name": "Legacy Node",
                "ip": "127.0.0.1",
                "port": 8080
            }]
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value = response.json().await.expect("json body should parse");
    assert!(payload["stats"]["errors"]
        .as_array()
        .expect("errors should be an array")
        .iter()
        .any(|item| item
            .as_str()
            .is_some_and(|value| value.contains("手动 URL 代理配置会保留"))));

    let providers = provider_catalog_repository
        .list_providers(false)
        .await
        .expect("providers should load");
    assert_eq!(providers.len(), 1);
    assert_eq!(
        providers[0].proxy,
        Some(json!({
            "enabled": true,
            "url": "https://proxy.example"
        }))
    );

    gateway_handle.abort();
}
