use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_contracts::ExecutionPlan;
use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
};
use aether_data::repository::auth_modules::{
    AuthModuleReadRepository, InMemoryAuthModuleReadRepository, StoredOAuthProviderModuleConfig,
};
use aether_data::repository::global_models::InMemoryGlobalModelReadRepository;
use aether_data::repository::oauth_providers::{
    InMemoryOAuthProviderRepository, OAuthProviderReadRepository, StoredOAuthProviderConfig,
};
use aether_data::repository::pool_scores::InMemoryPoolMemberScoreRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::users::{StoredUserAuthRecord, UserReadRepository};
use aether_data::repository::wallet::{StoredWalletSnapshot, WalletLookupKey};
use aether_data_contracts::repository::global_models::{
    AdminGlobalModelListQuery, AdminProviderModelListQuery, GlobalModelReadRepository,
    StoredPublicGlobalModel,
};
use aether_data_contracts::repository::pool_scores::{
    GetPoolMemberScoresByIdsQuery, PoolMemberHardState, PoolMemberIdentity, PoolMemberProbeStatus,
    PoolScoreReadRepository,
};
use aether_data_contracts::repository::provider_catalog::ProviderCatalogReadRepository;
use axum::body::{Body, Bytes};
use axum::http::HeaderMap;
use axum::routing::{any, post};
use axum::{extract::Request, Json, Router};
use http::StatusCode;
use serde_json::{json, Value};

use super::super::helpers::{hash_api_key, sample_endpoint, sample_key, sample_provider};
use super::super::{
    build_router_with_state, build_state_with_execution_runtime_override, start_server, AppState,
};
use crate::ai_serving::{
    build_provider_key_pool_score_upsert, provider_key_pool_score_id, provider_key_pool_score_scope,
};
use crate::backup::executor::{encrypt_backup_bytes, BackupDecryptionKey, BackupRestoreLimits};
use crate::backup::{apply_restored_backup, BackupRestoreScope};
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;
use crate::handlers::shared::{
    decrypt_or_migrate_identity_oauth_provider_client_secret,
    decrypt_or_migrate_ldap_bind_password, open_auth_api_key_secret,
    open_provider_catalog_credential, ProviderCatalogCredentialField,
};
use crate::restore_backup_json;

fn decrypt_test_provider_catalog_credential(
    key: &aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey,
    field: ProviderCatalogCredentialField,
) -> String {
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::disabled().with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        );
    let ciphertext = match field {
        ProviderCatalogCredentialField::ApiKey => key.encrypted_api_key.as_deref(),
        ProviderCatalogCredentialField::AuthConfig => key.encrypted_auth_config.as_deref(),
    }
    .expect("provider credential should be present");
    open_provider_catalog_credential(&state, &key.provider_id, &key.id, field, ciphertext)
        .expect("provider credential should decrypt with its record binding")
        .plaintext
}

fn build_admin_system_data_state_with_repositories(
    provider_catalog_repository: Arc<InMemoryProviderCatalogReadRepository>,
    global_model_repository: Arc<InMemoryGlobalModelReadRepository>,
) -> GatewayDataState {
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

fn build_empty_admin_system_data_state() -> GatewayDataState {
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(Vec::<
        StoredPublicGlobalModel,
    >::new()));
    build_admin_system_data_state_with_repositories(
        provider_catalog_repository,
        global_model_repository,
    )
}

fn sample_system_import_payload() -> Value {
    json!({
        "version": "2.2",
        "merge_mode": "overwrite",
        "global_models": [{
            "name": "gpt-5",
            "display_name": "GPT 5",
            "usage_count": 123,
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
                "auth_type_by_format": {
                    "openai:chat": "api_key",
                    "openai:video": "bearer"
                },
                "allow_auth_channel_mismatch_formats": [
                    "openai:chat",
                    "openai:video"
                ],
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
                "key": "smtp_host",
                "value": "smtp.example.com",
                "description": "SMTP host"
            },
            {
                "key": "smtp_user",
                "value": "smtp-user",
                "description": "SMTP user"
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
                "rpm_limit": null,
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

fn sample_import_user_session(
    user_id: &str,
    session_id: &str,
) -> crate::data::state::StoredUserSessionRecord {
    let now = chrono::Utc::now();
    crate::data::state::StoredUserSessionRecord::new(
        session_id.to_string(),
        user_id.to_string(),
        format!("device-{session_id}"),
        None,
        format!("refresh-{session_id}"),
        None,
        None,
        Some(now),
        Some(now + chrono::Duration::days(7)),
        None,
        None,
        Some("127.0.0.1".to_string()),
        Some("system-import-test".to_string()),
        Some(now),
        Some(now),
    )
    .expect("session should build")
}

const ADMIN_SYSTEM_IMPORT_TEST_STACK_BYTES: usize = 16 * 1024 * 1024;

fn run_admin_system_import_test<F, Fut>(test_name: &'static str, make_future: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let handle = std::thread::Builder::new()
        .name(test_name.to_string())
        .stack_size(ADMIN_SYSTEM_IMPORT_TEST_STACK_BYTES)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            runtime.block_on(make_future());
        })
        .expect("admin system import test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

#[test]
fn gateway_imports_admin_system_config_locally_and_persists_data() {
    run_admin_system_import_test(
        "gateway_imports_admin_system_config_locally_and_persists_data",
        gateway_imports_admin_system_config_locally_and_persists_data_impl,
    );
}

#[test]
fn gateway_rejects_ldap_filter_and_attribute_injection_before_system_import() {
    run_admin_system_import_test(
        "gateway_rejects_ldap_filter_and_attribute_injection_before_system_import",
        gateway_rejects_ldap_filter_and_attribute_injection_before_system_import_impl,
    );
}

async fn gateway_rejects_ldap_filter_and_attribute_injection_before_system_import_impl() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(build_empty_admin_system_data_state()),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    for (field, value, expected_detail) in [
        (
            "user_search_filter",
            "(uid={username})(objectClass=*)",
            "LDAP 搜索过滤器格式无效",
        ),
        (
            "username_attr",
            "uid)(|(objectClass=*)",
            "LDAP 用户名、邮箱或显示名称属性格式无效",
        ),
    ] {
        let mut import_payload = sample_system_import_payload();
        import_payload["ldap_config"][field] = json!(value);
        let response = client
            .post(format!("{gateway_url}/api/admin/system/config/import"))
            .header(GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .json(&import_payload)
            .send()
            .await
            .expect("invalid LDAP import should complete locally");

        let status = response.status();
        let payload: Value = response.json().await.expect("json body should parse");
        assert_eq!(status, StatusCode::BAD_REQUEST, "payload={payload}");
        assert!(
            payload["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains(expected_detail)),
            "payload={payload}"
        );
    }

    gateway_handle.abort();
}

async fn gateway_imports_admin_system_config_locally_and_persists_data_impl() {
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
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(data_state);
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let mut import_payload = sample_system_import_payload();
    import_payload["system_configs"]
        .as_array_mut()
        .expect("system configs should be an array")
        .push(json!({
            "key": "external_models_proxy_node_id",
            "value": null,
            "description": "External models proxy"
        }));
    let response = client
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&import_payload)
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
    assert_eq!(payload["stats"]["system_configs"]["created"], json!(5));
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
    assert_eq!(global_models.items[0].usage_count, 123);

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
        decrypt_test_provider_catalog_credential(&keys[0], ProviderCatalogCredentialField::ApiKey,),
        "sk-import-123"
    );
    assert!(open_provider_catalog_credential(
        &state,
        &keys[0].provider_id,
        "different-destination-key-id",
        ProviderCatalogCredentialField::ApiKey,
        keys[0]
            .encrypted_api_key
            .as_deref()
            .expect("api key should be present"),
    )
    .is_err());
    assert_eq!(keys[0].api_formats, Some(json!(["openai:chat"])));
    assert_eq!(
        keys[0].auth_type_by_format,
        Some(json!({ "openai:chat": "api_key" }))
    );
    assert_eq!(
        keys[0].allow_auth_channel_mismatch_formats,
        Some(json!(["openai:chat"]))
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
        decrypt_or_migrate_ldap_bind_password(&state, &ldap_config)
            .await
            .expect("ldap password should decrypt")
            .expect("ldap password should exist"),
        "bind-secret"
    );

    let oauth_provider = oauth_provider_repository
        .get_oauth_provider_config("linuxdo")
        .await
        .expect("oauth config should load")
        .expect("oauth config should exist");
    assert_eq!(oauth_provider.client_id, "linuxdo-client");
    assert_eq!(
        decrypt_or_migrate_identity_oauth_provider_client_secret(&state, &oauth_provider)
            .await
            .expect("oauth secret should decrypt")
            .expect("oauth secret should exist"),
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
        exported_provider["config"]["provider_ops"]["connector"]["credentials"],
        "***"
    );

    let exported_ldap = export_payload["ldap_config"]
        .as_object()
        .expect("ldap export should exist");
    assert!(exported_ldap.get("bind_password").is_none());
    assert_eq!(exported_ldap["is_enabled"], false);

    let exported_oauth = export_payload["oauth_providers"]
        .as_array()
        .and_then(|items| items.first())
        .expect("oauth export should exist");
    assert!(exported_oauth.get("client_secret").is_none());
    assert_eq!(exported_oauth["is_enabled"], false);

    let exported_system_configs = export_payload["system_configs"]
        .as_array()
        .expect("system configs export should exist");
    let exported_site_name = exported_system_configs
        .iter()
        .find(|entry| entry["key"] == "site_name")
        .expect("site_name should exist");
    let exported_smtp_password = exported_system_configs
        .iter()
        .find(|entry| entry["key"] == "smtp_password");
    let exported_external_models_proxy = exported_system_configs
        .iter()
        .find(|entry| entry["key"] == "external_models_proxy_node_id")
        .expect("external models proxy should exist");
    assert_eq!(exported_site_name["value"], "Imported Aether");
    assert!(exported_smtp_password.is_none());
    assert_eq!(
        exported_external_models_proxy["value"],
        serde_json::Value::Null
    );
    let serialized = export_payload.to_string();
    for secret in ["ops-secret", "bind-secret", "linuxdo-secret", "smtp-secret"] {
        assert!(!serialized.contains(secret), "leaked secret: {secret}");
    }

    gateway_handle.abort();
    upstream_handle.abort();
    let _ = upstream_url;
}

#[test]
fn gateway_imports_admin_system_config_openai_image_aliases() {
    run_admin_system_import_test(
        "gateway_imports_admin_system_config_openai_image_aliases",
        gateway_imports_admin_system_config_openai_image_aliases_impl,
    );
}

async fn gateway_imports_admin_system_config_openai_image_aliases_impl() {
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(Vec::<
        StoredPublicGlobalModel,
    >::new()));
    let data_state = build_admin_system_data_state_with_repositories(
        Arc::clone(&provider_catalog_repository),
        Arc::clone(&global_model_repository),
    );
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let mut payload = sample_system_import_payload();
    payload["providers"][0]["endpoints"][0]["api_format"] = json!("openai_image");
    payload["providers"][0]["api_keys"][0]["api_formats"] = json!(["images"]);
    payload["providers"][0]["api_keys"][0]["supported_endpoints"] = json!(["openai:image"]);
    payload["providers"][0]["models"][0]["supports_image_generation"] = json!(true);

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

    let status = response.status();
    let body: Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={body}");
    assert_eq!(body["stats"]["endpoints"]["created"], json!(1));
    assert_eq!(body["stats"]["keys"]["created"], json!(1));

    let providers = provider_catalog_repository
        .list_providers(false)
        .await
        .expect("providers should load");
    let provider_ids = providers
        .iter()
        .map(|provider| provider.id.clone())
        .collect::<Vec<_>>();
    let endpoints = provider_catalog_repository
        .list_endpoints_by_provider_ids(&provider_ids)
        .await
        .expect("endpoints should load");
    assert_eq!(endpoints[0].api_format, "openai:image");

    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(&provider_ids)
        .await
        .expect("keys should load");
    assert_eq!(keys[0].api_formats, Some(json!(["openai:image"])));

    gateway_handle.abort();
}

#[test]
fn gateway_returns_503_for_admin_system_config_import_when_local_data_is_unavailable() {
    run_admin_system_import_test(
        "gateway_returns_503_for_admin_system_config_import_when_local_data_is_unavailable",
        gateway_returns_503_for_admin_system_config_import_when_local_data_is_unavailable_impl,
    );
}

async fn gateway_returns_503_for_admin_system_config_import_when_local_data_is_unavailable_impl() {
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

#[test]
fn gateway_imports_legacy_admin_system_config_versions_and_model_test_succeeds() {
    run_admin_system_import_test(
        "gateway_imports_legacy_admin_system_config_versions_and_model_test_succeeds",
        gateway_imports_legacy_admin_system_config_versions_and_model_test_succeeds_impl,
    );
}

async fn gateway_imports_legacy_admin_system_config_versions_and_model_test_succeeds_impl() {
    for (
        fixture_name,
        expected_provider_name,
        expected_model_name,
        expected_api_key,
        expected_base_url,
    ) in [
        (
            "v20",
            "legacy-provider-v20",
            "legacy-gpt-5-v20",
            "sk-legacy-v20",
            "https://legacy-v20.example.com/v1",
        ),
        (
            "v21",
            "legacy-provider-v21",
            "legacy-gpt-5-v21",
            "sk-legacy-v21",
            "https://legacy-v21.example.com/v1",
        ),
    ] {
        assert_legacy_admin_system_config_import_model_test_succeeds(
            fixture_name,
            expected_provider_name,
            expected_model_name,
            expected_api_key,
            expected_base_url,
        )
        .await;
    }
}

async fn assert_legacy_admin_system_config_import_model_test_succeeds(
    fixture_name: &str,
    expected_provider_name: &str,
    expected_model_name: &str,
    expected_api_key: &str,
    expected_base_url: &str,
) {
    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let expected_base_url_for_runtime = expected_base_url.to_string();
    let expected_model_for_runtime = expected_model_name.to_string();
    let expected_bearer_for_runtime = format!("Bearer {expected_api_key}");
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            let expected_base_url = expected_base_url_for_runtime.clone();
            let expected_model = expected_model_for_runtime.clone();
            let expected_bearer = expected_bearer_for_runtime.clone();
            async move {
                *execution_runtime_hits_inner
                    .lock()
                    .expect("mutex should lock") += 1;
                assert_eq!(plan.provider_api_format, "openai:chat");
                assert!(
                    plan.url.starts_with(expected_base_url.as_str()),
                    "unexpected execution url: {}",
                    plan.url
                );
                assert_eq!(plan.model_name.as_deref(), Some(expected_model.as_str()));
                assert_eq!(
                    plan.headers.get("authorization").map(String::as_str),
                    Some(expected_bearer.as_str())
                );
                assert_eq!(
                    plan.body
                        .json_body
                        .as_ref()
                        .and_then(|body| body.get("model"))
                        .and_then(Value::as_str),
                    Some(expected_model.as_str())
                );
                Json(json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "id": "chatcmpl-legacy-import",
                            "object": "chat.completion",
                            "choices": [{
                                "message": {
                                    "role": "assistant",
                                    "content": "Hello from imported provider"
                                }
                            }]
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 17
                    }
                }))
            }
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(Vec::<
        StoredPublicGlobalModel,
    >::new()));
    let data_state = build_admin_system_data_state_with_repositories(
        Arc::clone(&provider_catalog_repository),
        Arc::clone(&global_model_repository),
    );
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let import_response = client
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&fixture_system_import_payload(fixture_name))
        .send()
        .await
        .expect("request should succeed");

    let import_status = import_response.status();
    let import_payload: Value = import_response
        .json()
        .await
        .expect("json body should parse");
    assert_eq!(import_status, StatusCode::OK, "payload={import_payload}");
    assert_eq!(import_payload["stats"]["providers"]["created"], json!(1));
    assert_eq!(import_payload["stats"]["endpoints"]["created"], json!(1));
    assert_eq!(import_payload["stats"]["keys"]["created"], json!(1));
    assert_eq!(import_payload["stats"]["models"]["created"], json!(1));

    let providers = provider_catalog_repository
        .list_providers(false)
        .await
        .expect("providers should load");
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].name, expected_provider_name);
    let provider_id = providers[0].id.clone();
    let provider_ids = vec![provider_id.clone()];
    let endpoints = provider_catalog_repository
        .list_endpoints_by_provider_ids(&provider_ids)
        .await
        .expect("endpoints should load");
    assert_eq!(endpoints.len(), 1);
    assert_eq!(endpoints[0].base_url, expected_base_url);
    let endpoint_id = endpoints[0].id.clone();

    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(&provider_ids)
        .await
        .expect("keys should load");
    assert_eq!(keys.len(), 1);
    assert_eq!(
        decrypt_test_provider_catalog_credential(&keys[0], ProviderCatalogCredentialField::ApiKey,),
        expected_api_key
    );

    let provider_models = global_model_repository
        .list_admin_provider_models(&AdminProviderModelListQuery {
            provider_id: provider_id.clone(),
            is_active: None,
            offset: 0,
            limit: 10_000,
        })
        .await
        .expect("provider models should load");
    assert_eq!(provider_models.len(), 1);
    assert_eq!(provider_models[0].provider_model_name, expected_model_name);

    let test_response = client
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": provider_id,
            "endpoint_id": endpoint_id,
            "model": expected_model_name,
            "api_format": "openai:chat"
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(test_response.status(), StatusCode::OK);
    let test_payload: Value = test_response.json().await.expect("json body should parse");
    assert_eq!(test_payload["success"], json!(true));
    assert_eq!(test_payload["model"], json!(expected_model_name));
    assert_eq!(test_payload["error"], Value::Null);
    assert_eq!(
        test_payload["data"]["response"]["choices"][0]["message"]["content"],
        json!("Hello from imported provider")
    );
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        1
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_rejects_unknown_admin_system_config_import_versions() {
    run_admin_system_import_test(
        "gateway_rejects_unknown_admin_system_config_import_versions",
        gateway_rejects_unknown_admin_system_config_import_versions_impl,
    );
}

async fn gateway_rejects_unknown_admin_system_config_import_versions_impl() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(build_empty_admin_system_data_state()),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    for version in ["1.9", "2.4"] {
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
        assert!(detail.contains("支持的版本: 2.0, 2.1, 2.2, 2.3"));
    }

    gateway_handle.abort();
}

#[test]
fn gateway_rejects_unsafe_oauth_targets_before_system_import_mutates_data() {
    run_admin_system_import_test(
        "gateway_rejects_unsafe_oauth_targets_before_system_import_mutates_data",
        gateway_rejects_unsafe_oauth_targets_before_system_import_mutates_data_impl,
    );
}

async fn gateway_rejects_unsafe_oauth_targets_before_system_import_mutates_data_impl() {
    let oauth_repository = Arc::new(InMemoryOAuthProviderRepository::default());
    let data = build_empty_admin_system_data_state()
        .attach_oauth_provider_repository_for_tests(Arc::clone(&oauth_repository));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    for (field, value) in [
        (
            "frontend_callback_url",
            "http://attacker.example/auth/callback",
        ),
        ("token_url_override", "https://attacker.example/oauth/token"),
    ] {
        let mut payload = sample_system_import_payload();
        payload["oauth_providers"][0][field] = json!(value);
        let response = reqwest::Client::new()
            .post(format!("{gateway_url}/api/admin/system/config/import"))
            .header(GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .json(&payload)
            .send()
            .await
            .expect("unsafe OAuth import should complete locally");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "field={field}");
        assert!(oauth_repository
            .list_oauth_provider_configs()
            .await
            .expect("OAuth provider list should load")
            .is_empty());
    }

    gateway_handle.abort();
}

#[test]
fn gateway_prevalidates_aggregate_user_data_before_config_mutation() {
    run_admin_system_import_test(
        "gateway_prevalidates_aggregate_user_data_before_config_mutation",
        gateway_prevalidates_aggregate_user_data_before_config_mutation_impl,
    );
}

async fn gateway_prevalidates_aggregate_user_data_before_config_mutation_impl() {
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::default());
    let user_repository =
        Arc::new(aether_data::repository::users::InMemoryUserReadRepository::default());
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(Vec::<
        StoredPublicGlobalModel,
    >::new()));
    let data =
        GatewayDataState::with_auth_api_key_repository_for_tests(Arc::clone(&auth_repository))
            .with_user_reader(user_repository)
            .attach_provider_catalog_repository_for_tests(provider_catalog_repository)
            .with_global_model_repository_for_tests(global_model_repository)
            .with_system_config_values_for_tests(Vec::<(String, Value)>::new())
            .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(data)
        .with_auth_users_for_tests([sample_import_admin_user("admin-user-123")])
        .with_auth_wallets_for_tests(Vec::<StoredWalletSnapshot>::new());
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/data/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "version": "1.0",
            "merge_mode": "overwrite",
            "config_data": {
                "version": "2.3",
                "global_models": [],
                "providers": [],
                "system_configs": [{
                    "key": "site_name",
                    "value": "must-not-be-written"
                }]
            },
            "user_data": {
                "version": "9.9",
                "users": [],
                "standalone_keys": []
            }
        }))
        .send()
        .await
        .expect("aggregate import should complete locally");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        state
            .read_system_config_json_value_strong("site_name")
            .await
            .expect("system config lookup should succeed"),
        None
    );

    gateway_handle.abort();
}

#[test]
fn gateway_prevalidates_provider_key_duplicates_before_config_mutation() {
    run_admin_system_import_test(
        "gateway_prevalidates_provider_key_duplicates_before_config_mutation",
        gateway_prevalidates_provider_key_duplicates_before_config_mutation_impl,
    );
}

async fn gateway_prevalidates_provider_key_duplicates_before_config_mutation_impl() {
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(Vec::<
        StoredPublicGlobalModel,
    >::new()));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(build_admin_system_data_state_with_repositories(
                Arc::clone(&provider_catalog_repository),
                Arc::clone(&global_model_repository),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "version": "2.3",
            "merge_mode": "overwrite",
            "global_models": [{
                "name": "must-not-exist",
                "display_name": "Must Not Exist",
                "is_active": true
            }],
            "providers": [{
                "name": "must-not-exist-provider",
                "provider_type": "custom",
                "is_active": true,
                "endpoints": [{
                    "api_format": "openai:chat",
                    "base_url": "https://api.example.com",
                    "is_active": true
                }],
                "api_keys": [{
                    "name": "first",
                    "auth_type": "api_key",
                    "api_key": "duplicate-secret",
                    "api_formats": ["openai:chat"],
                    "is_active": true
                }, {
                    "name": "second",
                    "auth_type": "bearer",
                    "api_key": "duplicate-secret",
                    "api_formats": ["openai:chat"],
                    "is_active": true
                }],
                "models": []
            }]
        }))
        .send()
        .await
        .expect("config import should complete locally");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(provider_catalog_repository
        .list_providers(false)
        .await
        .expect("providers should load")
        .is_empty());
    assert!(global_model_repository
        .list_admin_global_models(&AdminGlobalModelListQuery {
            offset: 0,
            limit: 10_000,
            is_active: None,
            search: None,
        })
        .await
        .expect("global models should load")
        .items
        .is_empty());

    gateway_handle.abort();
}

#[test]
fn gateway_prevalidates_nested_user_key_before_user_mutation() {
    run_admin_system_import_test(
        "gateway_prevalidates_nested_user_key_before_user_mutation",
        gateway_prevalidates_nested_user_key_before_user_mutation_impl,
    );
}

async fn gateway_prevalidates_nested_user_key_before_user_mutation_impl() {
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::default());
    let user_repository =
        Arc::new(aether_data::repository::users::InMemoryUserReadRepository::default());
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_auth_api_key_repository_for_tests(auth_repository)
                .with_user_reader(user_repository)
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
            "version": "1.5",
            "merge_mode": "overwrite",
            "users": [{
                "email": "must-not-exist@example.com",
                "username": "must-not-exist",
                "role": "user",
                "is_active": true,
                "api_keys": [{
                    "key": "sk-invalid-concurrency",
                    "concurrent_limit": -1
                }]
            }],
            "standalone_keys": []
        }))
        .send()
        .await
        .expect("user import should complete locally");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(state
        .find_user_auth_by_identifier("must-not-exist@example.com")
        .await
        .expect("user lookup should succeed")
        .is_none());

    gateway_handle.abort();
}

#[test]
fn gateway_rejects_credentials_in_v16_user_import_before_mutation() {
    run_admin_system_import_test(
        "gateway_rejects_credentials_in_v16_user_import_before_mutation",
        gateway_rejects_credentials_in_v16_user_import_before_mutation_impl,
    );
}

async fn gateway_rejects_credentials_in_v16_user_import_before_mutation_impl() {
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::default());
    let user_repository =
        Arc::new(aether_data::repository::users::InMemoryUserReadRepository::default());
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_auth_api_key_repository_for_tests(auth_repository)
                .with_user_reader(user_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_auth_users_for_tests([sample_import_admin_user("admin-user-123")])
        .with_auth_wallets_for_tests(Vec::<StoredWalletSnapshot>::new());
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    let cases = [
        ("password-null", json!({ "password_hash": null })),
        ("plaintext-key", json!({ "key": "sk-must-not-import" })),
        ("key-hash", json!({ "key_hash": "legacy-hash" })),
        ("encrypted-null", json!({ "key_encrypted": null })),
    ];
    for (case_name, forbidden_fields) in cases {
        let email = format!("{case_name}@example.com");
        let mut api_key = serde_json::Map::from_iter([
            (
                "api_key_id".to_string(),
                json!(format!("source-{case_name}")),
            ),
            ("credential_state".to_string(), json!("not_exported")),
        ]);
        if case_name != "password-null" {
            api_key.extend(
                forbidden_fields
                    .as_object()
                    .expect("forbidden API key fields should be an object")
                    .clone(),
            );
        }
        let mut user = serde_json::Map::from_iter([
            ("id".to_string(), json!(format!("source-user-{case_name}"))),
            ("email".to_string(), json!(email.clone())),
            ("username".to_string(), json!(case_name)),
            ("role".to_string(), json!("user")),
            ("is_active".to_string(), json!(true)),
            (
                "api_keys".to_string(),
                Value::Array(vec![Value::Object(api_key)]),
            ),
        ]);
        if case_name == "password-null" {
            user.extend(
                forbidden_fields
                    .as_object()
                    .expect("forbidden user fields should be an object")
                    .clone(),
            );
        }

        let response = client
            .post(format!("{gateway_url}/api/admin/system/users/import"))
            .header(GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .json(&json!({
                "version": "1.6",
                "merge_mode": "overwrite",
                "users": [Value::Object(user)],
                "standalone_keys": []
            }))
            .send()
            .await
            .expect("user import should complete locally");

        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "case={case_name}"
        );
        assert!(state
            .find_user_auth_by_identifier(&email)
            .await
            .expect("user lookup should succeed")
            .is_none());
    }

    gateway_handle.abort();
}

#[test]
fn gateway_prevalidates_usage_integer_storage_before_user_mutation() {
    run_admin_system_import_test(
        "gateway_prevalidates_usage_integer_storage_before_user_mutation",
        gateway_prevalidates_usage_integer_storage_before_user_mutation_impl,
    );
}

async fn gateway_prevalidates_usage_integer_storage_before_user_mutation_impl() {
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::default());
    let user_repository =
        Arc::new(aether_data::repository::users::InMemoryUserReadRepository::default());
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_auth_api_key_repository_for_tests(auth_repository)
                .with_user_reader(user_repository)
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
            "version": "1.5",
            "merge_mode": "overwrite",
            "users": [{
                "id": "source-overflow-user",
                "email": "must-not-exist@example.com",
                "username": "must-not-exist",
                "role": "user",
                "is_active": true,
                "api_keys": []
            }],
            "standalone_keys": [],
            "usage_aggregates": {
                "stats_daily": [{
                    "date_unix_secs": 86400,
                    "total_requests": 1,
                    "success_requests": 1,
                    "error_requests": 0,
                    "input_tokens": 9223372036854775808_u64,
                    "output_tokens": 0,
                    "cache_creation_tokens": 0,
                    "cache_read_tokens": 0,
                    "total_cost": 0.0,
                    "actual_total_cost": 0.0,
                    "is_complete": true,
                    "aggregated_at_unix_secs": 86400
                }],
                "stats_user_daily": [],
                "stats_daily_api_key": []
            }
        }))
        .send()
        .await
        .expect("user import should complete locally");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(state
        .find_user_auth_by_identifier("must-not-exist@example.com")
        .await
        .expect("user lookup should succeed")
        .is_none());

    gateway_handle.abort();
}

#[test]
fn gateway_prevalidates_duplicate_usage_dimensions_before_user_mutation() {
    run_admin_system_import_test(
        "gateway_prevalidates_duplicate_usage_dimensions_before_user_mutation",
        gateway_prevalidates_duplicate_usage_dimensions_before_user_mutation_impl,
    );
}

async fn gateway_prevalidates_duplicate_usage_dimensions_before_user_mutation_impl() {
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::default());
    let user_repository =
        Arc::new(aether_data::repository::users::InMemoryUserReadRepository::default());
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_auth_api_key_repository_for_tests(auth_repository)
                .with_user_reader(user_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_auth_users_for_tests([sample_import_admin_user("admin-user-123")])
        .with_auth_wallets_for_tests(Vec::<StoredWalletSnapshot>::new());
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let daily = json!({
        "date_unix_secs": 86400,
        "total_requests": 1,
        "success_requests": 1,
        "error_requests": 0,
        "input_tokens": 1,
        "output_tokens": 0,
        "cache_creation_tokens": 0,
        "cache_read_tokens": 0,
        "total_cost": 0.0,
        "actual_total_cost": 0.0,
        "is_complete": true,
        "aggregated_at_unix_secs": 86400
    });
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/users/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "version": "1.5",
            "merge_mode": "error",
            "users": [{
                "id": "source-duplicate-user",
                "email": "must-not-exist@example.com",
                "username": "must-not-exist",
                "role": "user",
                "is_active": true,
                "api_keys": []
            }],
            "standalone_keys": [],
            "usage_aggregates": {
                "stats_daily": [daily.clone(), daily],
                "stats_user_daily": [],
                "stats_daily_api_key": []
            }
        }))
        .send()
        .await
        .expect("user import should complete locally");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(state
        .find_user_auth_by_identifier("must-not-exist@example.com")
        .await
        .expect("user lookup should succeed")
        .is_none());

    gateway_handle.abort();
}

#[test]
fn gateway_user_import_allows_chained_username_release() {
    run_admin_system_import_test(
        "gateway_user_import_allows_chained_username_release",
        gateway_user_import_allows_chained_username_release_impl,
    );
}

async fn gateway_user_import_allows_chained_username_release_impl() {
    let first = StoredUserAuthRecord::new(
        "user-a".to_string(),
        Some("a@example.com".to_string()),
        true,
        "a".to_string(),
        Some("hash-a".to_string()),
        "user".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        Some(chrono::Utc::now()),
        None,
    )
    .expect("first user should build");
    let second = StoredUserAuthRecord::new(
        "user-b".to_string(),
        Some("b@example.com".to_string()),
        true,
        "b".to_string(),
        Some("hash-b".to_string()),
        "user".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        Some(chrono::Utc::now()),
        None,
    )
    .expect("second user should build");
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::default());
    let user_repository =
        Arc::new(aether_data::repository::users::InMemoryUserReadRepository::default());
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_auth_api_key_repository_for_tests(auth_repository)
                .with_user_reader(user_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_auth_users_for_tests([sample_import_admin_user("admin-user-123"), first, second])
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
            "version": "1.5",
            "merge_mode": "overwrite",
            "users": [{
                "email": "b@example.com",
                "username": "c",
                "role": "user",
                "is_active": true,
                "api_keys": []
            }, {
                "email": "a@example.com",
                "username": "b",
                "role": "user",
                "is_active": true,
                "api_keys": []
            }],
            "standalone_keys": []
        }))
        .send()
        .await
        .expect("user import should complete locally");

    let status = response.status();
    let payload: Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(
        state
            .find_user_auth_by_identifier("b")
            .await
            .expect("user lookup should succeed")
            .expect("released username should be reassigned")
            .id,
        "user-a"
    );
    assert_eq!(
        state
            .find_user_auth_by_identifier("c")
            .await
            .expect("user lookup should succeed")
            .expect("second user should be renamed")
            .id,
        "user-b"
    );

    gateway_handle.abort();
}

#[test]
fn gateway_user_import_preserves_omitted_email_in_simulated_state() {
    run_admin_system_import_test(
        "gateway_user_import_preserves_omitted_email_in_simulated_state",
        gateway_user_import_preserves_omitted_email_in_simulated_state_impl,
    );
}

async fn gateway_user_import_preserves_omitted_email_in_simulated_state_impl() {
    let existing = StoredUserAuthRecord::new(
        "user-existing".to_string(),
        Some("existing@example.com".to_string()),
        true,
        "existing".to_string(),
        Some("existing-hash".to_string()),
        "user".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        Some(chrono::Utc::now()),
        None,
    )
    .expect("existing user should build");
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::default());
    let user_repository =
        Arc::new(aether_data::repository::users::InMemoryUserReadRepository::default());
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_auth_api_key_repository_for_tests(auth_repository)
                .with_user_reader(user_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_auth_users_for_tests([sample_import_admin_user("admin-user-123"), existing])
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
            "version": "1.5",
            "merge_mode": "overwrite",
            "users": [{
                "username": "existing",
                "role": "user",
                "is_active": true,
                "api_keys": []
            }, {
                "email": "existing@example.com",
                "username": "renamed",
                "role": "user",
                "is_active": true,
                "api_keys": []
            }],
            "standalone_keys": []
        }))
        .send()
        .await
        .expect("user import should complete locally");

    let status = response.status();
    let payload: Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    let updated = state
        .find_user_auth_by_identifier("existing@example.com")
        .await
        .expect("user lookup should succeed")
        .expect("existing email should remain attached");
    assert_eq!(updated.id, "user-existing");
    assert_eq!(updated.username, "renamed");

    gateway_handle.abort();
}

#[test]
fn gateway_user_import_password_hash_overwrite_revokes_existing_sessions() {
    run_admin_system_import_test(
        "gateway_user_import_password_hash_overwrite_revokes_existing_sessions",
        gateway_user_import_password_hash_overwrite_revokes_existing_sessions_impl,
    );
}

#[test]
fn authenticated_recovery_restores_password_and_api_key_login_material() {
    run_admin_system_import_test(
        "authenticated_recovery_restores_password_and_api_key_login_material",
        authenticated_recovery_restores_password_and_api_key_login_material_impl,
    );
}

async fn authenticated_recovery_restores_password_and_api_key_login_material_impl() {
    let password = "recovered-password-123";
    let password_hash = bcrypt::hash(password, 4).expect("password should hash");
    let plaintext_key = "sk-recovered-user-key";
    let key_hash = hash_api_key(plaintext_key);
    let payload = json!({
        "version": "1.5",
        "exported_at": "2026-08-22T12:00:00Z",
        "merge_mode": "overwrite",
        "user_groups": [],
        "users": [{
            "id": "source-recovered-user",
            "email": "recovered@example.com",
            "email_verified": true,
            "username": "recovered",
            "password_hash": password_hash,
            "role": "user",
            "is_active": true,
            "api_keys": [{
                "api_key_id": "source-recovered-key",
                "key": plaintext_key,
                "key_hash": key_hash,
                "name": "Recovered Key",
                "is_active": true
            }]
        }],
        "standalone_keys": [],
        "usage_aggregates": {}
    });
    let object_key = "prod/aether-users-backup-20260822-120000.json.zst.aes256gcm";
    let compressed = zstd::stream::encode_all(
        serde_json::to_vec(&payload)
            .expect("payload should serialize")
            .as_slice(),
        0,
    )
    .expect("payload should compress");
    let (envelope, _) = encrypt_backup_bytes(DEVELOPMENT_ENCRYPTION_KEY, object_key, &compressed)
        .expect("backup should encrypt");
    let restored = restore_backup_json(
        object_key,
        &envelope,
        &[BackupDecryptionKey::current(DEVELOPMENT_ENCRYPTION_KEY)
            .expect("restore key should build")],
        BackupRestoreLimits::default(),
    )
    .expect("backup should authenticate");
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::default());
    let user_repository =
        Arc::new(aether_data::repository::users::InMemoryUserReadRepository::default());
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_auth_api_key_repository_for_tests(Arc::clone(&auth_repository))
                .with_user_reader(user_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_auth_users_for_tests([sample_import_admin_user("admin-user-123")])
        .with_auth_wallets_for_tests(Vec::<StoredWalletSnapshot>::new());
    let result = apply_restored_backup(
        &state,
        restored,
        BackupRestoreScope::Users,
        Some("admin-user-123"),
    )
    .await
    .expect("restore should execute")
    .expect("restore payload should be valid");
    assert_eq!(result["stats"]["users"]["created"], json!(1));
    assert_eq!(result["stats"]["api_keys"]["created"], json!(1));

    let user = state
        .find_user_auth_by_identifier("recovered@example.com")
        .await
        .expect("user lookup should succeed")
        .expect("recovered user should exist");
    assert!(bcrypt::verify(
        password,
        user.password_hash
            .as_deref()
            .expect("password hash should be restored")
    )
    .expect("bcrypt hash should verify"));
    let keys = state
        .list_auth_api_key_export_records_by_user_ids(std::slice::from_ref(&user.id))
        .await
        .expect("keys should load");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].key_hash, key_hash);
    assert!(keys[0].is_active);
    assert!(keys[0]
        .key_encrypted
        .as_deref()
        .is_some_and(|value| value.starts_with("aether-auth-api-key-secret-v2:")));
    let decrypted = open_auth_api_key_secret(&state, &keys[0])
        .ok()
        .map(|projection| projection.plaintext);
    assert_eq!(decrypted.as_deref(), Some(plaintext_key));
    let mut copied_record = keys[0].clone();
    copied_record.api_key_id = "different-destination-record".to_string();
    assert!(open_auth_api_key_secret(&state, &copied_record).is_err());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_secs();
    let authenticated = state
        .data
        .read_auth_api_key_snapshot_by_key_hash_strong(&hash_api_key(plaintext_key), now)
        .await
        .expect("API Key lookup should succeed")
        .expect("restored API Key should authenticate");
    assert!(authenticated.api_key_is_active);
    assert_eq!(authenticated.user_id, user.id);
}

#[test]
fn authenticated_recovery_reencrypts_existing_api_key() {
    run_admin_system_import_test(
        "authenticated_recovery_reencrypts_existing_api_key",
        authenticated_recovery_reencrypts_existing_api_key_impl,
    );
}

async fn authenticated_recovery_reencrypts_existing_api_key_impl() {
    let plaintext_key = "sk-recovered-existing-key";
    let key_hash = hash_api_key(plaintext_key);
    let password_hash =
        bcrypt::hash("recovered-existing-password", 4).expect("password should hash");
    let existing_user = StoredUserAuthRecord::new(
        "user-recovered-existing".to_string(),
        Some("recovered-existing@example.com".to_string()),
        true,
        "recovered-existing".to_string(),
        Some(bcrypt::hash("old-password", 4).expect("old password should hash")),
        "user".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        Some(chrono::Utc::now()),
        None,
    )
    .expect("existing user should build");
    let existing_snapshot = StoredAuthApiKeySnapshot::new(
        existing_user.id.clone(),
        existing_user.username.clone(),
        existing_user.email.clone(),
        existing_user.role.clone(),
        existing_user.auth_source.clone(),
        true,
        false,
        None,
        None,
        None,
        "key-recovered-existing".to_string(),
        Some("Old Key".to_string()),
        false,
        false,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("existing key snapshot should build");
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed([(Some(key_hash.clone()), existing_snapshot)])
            .with_export_records([StoredAuthApiKeyExportRecord::new(
                existing_user.id.clone(),
                "key-recovered-existing".to_string(),
                key_hash.clone(),
                Some("old-unusable-ciphertext".to_string()),
                Some("Old Key".to_string()),
                None,
                None,
                None,
                None,
                None,
                None,
                false,
                None,
                false,
                0,
                0,
                0.0,
                false,
            )
            .expect("existing key export should build")]),
    );
    let user_repository =
        Arc::new(aether_data::repository::users::InMemoryUserReadRepository::default());
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_auth_api_key_repository_for_tests(Arc::clone(&auth_repository))
                .with_user_reader(user_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_auth_users_for_tests([
            sample_import_admin_user("admin-user-123"),
            existing_user.clone(),
        ])
        .with_auth_wallets_for_tests(Vec::<StoredWalletSnapshot>::new());
    let payload = json!({
        "version": "1.5",
        "exported_at": "2026-08-22T12:00:00Z",
        "merge_mode": "overwrite",
        "user_groups": [],
        "users": [{
            "id": "source-recovered-existing",
            "email": existing_user.email,
            "email_verified": true,
            "username": existing_user.username,
            "password_hash": password_hash,
            "role": "user",
            "is_active": true,
            "api_keys": [{
                "api_key_id": "source-recovered-existing-key",
                "key": plaintext_key,
                "key_hash": key_hash,
                "name": "Recovered Existing Key",
                "is_active": true
            }]
        }],
        "standalone_keys": [],
        "usage_aggregates": {}
    });
    let object_key = "prod/aether-users-backup-20260822-120001.json.zst.aes256gcm";
    let compressed = zstd::stream::encode_all(
        serde_json::to_vec(&payload)
            .expect("payload should serialize")
            .as_slice(),
        0,
    )
    .expect("payload should compress");
    let (envelope, _) = encrypt_backup_bytes(DEVELOPMENT_ENCRYPTION_KEY, object_key, &compressed)
        .expect("backup should encrypt");
    let restored = restore_backup_json(
        object_key,
        &envelope,
        &[BackupDecryptionKey::current(DEVELOPMENT_ENCRYPTION_KEY)
            .expect("restore key should build")],
        BackupRestoreLimits::default(),
    )
    .expect("backup should authenticate");

    let result = apply_restored_backup(
        &state,
        restored,
        BackupRestoreScope::Users,
        Some("admin-user-123"),
    )
    .await
    .expect("restore should execute")
    .expect("restore payload should be valid");
    assert_eq!(result["stats"]["users"]["updated"], json!(1));
    assert_eq!(result["stats"]["api_keys"]["updated"], json!(1));

    let restored_key = state
        .list_auth_api_key_export_records_by_ids(&["key-recovered-existing".to_string()])
        .await
        .expect("existing key should reload")
        .into_iter()
        .next()
        .expect("existing key should remain");
    assert!(restored_key.is_active);
    assert_ne!(
        restored_key.key_encrypted.as_deref(),
        Some("old-unusable-ciphertext")
    );
    let decrypted = open_auth_api_key_secret(&state, &restored_key)
        .ok()
        .map(|projection| projection.plaintext);
    assert_eq!(decrypted.as_deref(), Some(plaintext_key));
}

#[test]
fn authenticated_recovery_rejects_proxy_nodes_before_config_mutation() {
    run_admin_system_import_test(
        "authenticated_recovery_rejects_proxy_nodes_before_config_mutation",
        authenticated_recovery_rejects_proxy_nodes_before_config_mutation_impl,
    );
}

async fn authenticated_recovery_rejects_proxy_nodes_before_config_mutation_impl() {
    let payload = json!({
        "version": "2.3",
        "exported_at": "2026-08-22T12:00:00Z",
        "merge_mode": "overwrite",
        "global_models": [],
        "providers": [],
        "proxy_nodes": [{
            "id": "deployment-local-node",
            "name": "Local Node",
            "ip": "127.0.0.1",
            "port": 8080,
            "proxy_username": "proxy-user",
            "proxy_password": "proxy-password"
        }],
        "oauth_providers": [],
        "system_configs": [{
            "key": "site_name",
            "value": "must-not-be-written"
        }]
    });
    let object_key = "prod/aether-config-backup-20260822-120000.json.zst.aes256gcm";
    let compressed = zstd::stream::encode_all(
        serde_json::to_vec(&payload)
            .expect("payload should serialize")
            .as_slice(),
        0,
    )
    .expect("payload should compress");
    let (envelope, _) = encrypt_backup_bytes(DEVELOPMENT_ENCRYPTION_KEY, object_key, &compressed)
        .expect("backup should encrypt");
    let restored = restore_backup_json(
        object_key,
        &envelope,
        &[BackupDecryptionKey::current(DEVELOPMENT_ENCRYPTION_KEY)
            .expect("restore key should build")],
        BackupRestoreLimits::default(),
    )
    .expect("backup should authenticate");
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(build_empty_admin_system_data_state());
    let error = apply_restored_backup(&state, restored, BackupRestoreScope::Config, None)
        .await
        .expect("restore should execute")
        .expect_err("proxy nodes must fail closed");
    assert_eq!(error.0, StatusCode::BAD_REQUEST);
    assert!(error.1["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("不支持安全恢复代理节点")));
    assert!(state
        .read_system_config_json_value_strong("site_name")
        .await
        .expect("config lookup should succeed")
        .is_none());
}

#[test]
fn authenticated_recovery_rejects_ldap_filter_injection_before_config_mutation() {
    run_admin_system_import_test(
        "authenticated_recovery_rejects_ldap_filter_injection_before_config_mutation",
        authenticated_recovery_rejects_ldap_filter_injection_before_config_mutation_impl,
    );
}

async fn authenticated_recovery_rejects_ldap_filter_injection_before_config_mutation_impl() {
    let payload = json!({
        "version": "2.3",
        "exported_at": "2026-08-31T12:00:00Z",
        "merge_mode": "overwrite",
        "global_models": [],
        "providers": [],
        "proxy_nodes": [],
        "ldap_config": {
            "server_url": "ldaps://ldap.example.com",
            "bind_dn": "cn=admin,dc=example,dc=com",
            "bind_password": "bind-secret",
            "base_dn": "dc=example,dc=com",
            "user_search_filter": "(uid={username})(objectClass=*)",
            "username_attr": "uid",
            "email_attr": "mail",
            "display_name_attr": "displayName",
            "is_enabled": true,
            "is_exclusive": false,
            "use_starttls": false,
            "connect_timeout": 10
        },
        "oauth_providers": [],
        "system_configs": [{
            "key": "site_name",
            "value": "must-not-be-written"
        }]
    });
    let object_key = "prod/aether-config-backup-20260831-120000.json.zst.aes256gcm";
    let compressed = zstd::stream::encode_all(
        serde_json::to_vec(&payload)
            .expect("payload should serialize")
            .as_slice(),
        0,
    )
    .expect("payload should compress");
    let (envelope, _) = encrypt_backup_bytes(DEVELOPMENT_ENCRYPTION_KEY, object_key, &compressed)
        .expect("backup should encrypt");
    let restored = restore_backup_json(
        object_key,
        &envelope,
        &[BackupDecryptionKey::current(DEVELOPMENT_ENCRYPTION_KEY)
            .expect("restore key should build")],
        BackupRestoreLimits::default(),
    )
    .expect("backup should authenticate");
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(build_empty_admin_system_data_state());

    let error = apply_restored_backup(&state, restored, BackupRestoreScope::Config, None)
        .await
        .expect("restore should execute")
        .expect_err("LDAP filter injection must fail closed");
    assert_eq!(error.0, StatusCode::BAD_REQUEST);
    assert!(error.1["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("LDAP 搜索过滤器格式无效")));
    assert!(state
        .read_system_config_json_value_strong("site_name")
        .await
        .expect("config lookup should succeed")
        .is_none());
}

async fn gateway_user_import_password_hash_overwrite_revokes_existing_sessions_impl() {
    let existing_user = StoredUserAuthRecord::new(
        "user-existing".to_string(),
        Some("existing@example.com".to_string()),
        true,
        "existing".to_string(),
        Some("old-password-hash".to_string()),
        "user".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        Some(chrono::Utc::now()),
        None,
    )
    .expect("existing user should build");
    let old_session = sample_import_user_session("user-existing", "old-session");
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::default());
    let user_repository =
        Arc::new(aether_data::repository::users::InMemoryUserReadRepository::default());
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_auth_api_key_repository_for_tests(auth_repository)
                .with_user_reader(user_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_auth_users_for_tests([sample_import_admin_user("admin-user-123"), existing_user])
        .with_auth_session_for_tests(old_session)
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
            "version": "1.5",
            "merge_mode": "overwrite",
            "users": [{
                "email": "existing@example.com",
                "username": "existing",
                "password_hash": "new-password-hash",
                "role": "user",
                "is_active": true,
                "api_keys": []
            }],
            "standalone_keys": []
        }))
        .send()
        .await
        .expect("user import should complete locally");

    let status = response.status();
    let payload: Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    let updated = state
        .find_user_auth_by_identifier("existing@example.com")
        .await
        .expect("user lookup should succeed")
        .expect("user should exist");
    assert!(updated
        .password_hash
        .as_deref()
        .is_some_and(|hash| hash.starts_with("$aether-import-revoked$")));
    assert_ne!(updated.password_hash.as_deref(), Some("new-password-hash"));
    let session = state
        .find_user_session("user-existing", "old-session")
        .await
        .expect("session lookup should succeed")
        .expect("session should remain auditable");
    assert!(session.is_revoked());
    assert_eq!(
        session.revoke_reason.as_deref(),
        Some("admin_password_reset")
    );

    gateway_handle.abort();
}

#[test]
fn gateway_imports_admin_system_users_locally_and_persists_data() {
    run_admin_system_import_test(
        "gateway_imports_admin_system_users_locally_and_persists_data",
        gateway_imports_admin_system_users_locally_and_persists_data_impl,
    );
}

async fn gateway_imports_admin_system_users_locally_and_persists_data_impl() {
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
    let user_repository =
        Arc::new(aether_data::repository::users::InMemoryUserReadRepository::default());
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_auth_api_key_repository_for_tests(Arc::clone(&auth_repository))
                .with_user_reader(user_repository)
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
            "version": "1.4",
            "merge_mode": "overwrite",
            "user_groups": [{
                "id": "source-group-1",
                "name": "GPT Import",
                "description": "Imported group",
                "allowed_providers": ["openai"],
                "allowed_providers_mode": "specific",
                "allowed_api_formats": ["openai:chat"],
                "allowed_api_formats_mode": "specific",
                "allowed_models": ["gpt-5"],
                "allowed_models_mode": "specific",
                "rate_limit": 44,
                "rate_limit_mode": "custom"
            }],
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
                "allowed_models_mode": "specific",
                "rate_limit_mode": "custom",
                "group_ids": ["source-group-1"],
                "group_names": ["GPT Import"],
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
                    "total_cost_usd": "1.25000000"
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
                "total_cost_usd": "0.75000000",
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
    assert_eq!(payload["stats"]["user_groups"]["created"], json!(1));
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
    assert!(imported_user
        .password_hash
        .as_deref()
        .is_some_and(|hash| hash.starts_with("$aether-import-revoked$")));
    assert_ne!(
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
    assert_eq!(imported_user.allowed_models_mode, "specific");
    assert!(imported_user.is_active);

    let imported_groups = state
        .list_user_groups_for_user(&imported_user.id)
        .await
        .expect("user groups should load");
    assert_eq!(imported_groups.len(), 1);
    assert_eq!(imported_groups[0].name, "GPT Import");
    assert_eq!(imported_groups[0].allowed_models_mode, "specific");
    assert_eq!(
        imported_groups[0].allowed_models,
        Some(vec!["gpt-5".to_string()])
    );
    assert_eq!(imported_groups[0].rate_limit, Some(44));

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
    assert_eq!(user_api_keys[0].total_requests, 12);
    assert_eq!(user_api_keys[0].total_tokens, 3456);
    assert_eq!(user_api_keys[0].total_cost_usd, 1.25);
    assert_eq!(
        user_api_keys[0].allowed_api_formats,
        Some(vec!["openai:chat".to_string()])
    );
    assert!(!user_api_keys[0].is_active);
    assert!(user_api_keys[0].key_encrypted.is_none());
    assert!(user_api_keys[0]
        .key_hash
        .starts_with("$aether-import-revoked$"));
    assert_ne!(user_api_keys[0].key_hash, hash_api_key("sk-user-import-1"));

    let standalone_keys = state
        .list_auth_api_key_export_standalone_records()
        .await
        .expect("standalone api keys should load");
    assert_eq!(standalone_keys.len(), 1);
    assert_eq!(
        standalone_keys[0].name.as_deref(),
        Some("Imported Standalone")
    );
    assert_eq!(standalone_keys[0].total_requests, 3);
    assert_eq!(standalone_keys[0].total_tokens, 789);
    assert_eq!(standalone_keys[0].total_cost_usd, 0.75);
    assert!(!standalone_keys[0].is_active);
    assert!(standalone_keys[0].key_encrypted.is_none());
    assert!(standalone_keys[0]
        .key_hash
        .starts_with("$aether-import-revoked$"));
    assert_ne!(
        standalone_keys[0].key_hash,
        hash_api_key("sk-standalone-import-1")
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

#[test]
fn gateway_legacy_user_import_does_not_mutate_live_keys_by_exported_hash() {
    run_admin_system_import_test(
        "gateway_legacy_user_import_does_not_mutate_live_keys_by_exported_hash",
        gateway_legacy_user_import_does_not_mutate_live_keys_by_exported_hash_impl,
    );
}

async fn gateway_legacy_user_import_does_not_mutate_live_keys_by_exported_hash_impl() {
    let user_key_hash = hash_api_key("sk-existing-user-key");
    let standalone_key_hash = hash_api_key("sk-existing-standalone-key");
    let existing_user = StoredUserAuthRecord::new(
        "user-existing".to_string(),
        Some("existing@example.com".to_string()),
        true,
        "existing".to_string(),
        Some("existing-hash".to_string()),
        "user".to_string(),
        "local".to_string(),
        None,
        None,
        None,
        true,
        false,
        Some(chrono::Utc::now()),
        Some(chrono::Utc::now()),
    )
    .expect("existing user should build");
    let user_key_snapshot = StoredAuthApiKeySnapshot::new(
        "user-existing".to_string(),
        "existing".to_string(),
        Some("existing@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        None,
        None,
        None,
        "key-user-existing".to_string(),
        Some("Existing User Key".to_string()),
        true,
        false,
        false,
        Some(10),
        None,
        None,
        None,
        None,
        None,
    )
    .expect("user key snapshot should build");
    let standalone_key_snapshot = StoredAuthApiKeySnapshot::new(
        "admin-user-123".to_string(),
        "admin".to_string(),
        Some("admin@example.com".to_string()),
        "admin".to_string(),
        "local".to_string(),
        true,
        false,
        None,
        None,
        None,
        "key-standalone-existing".to_string(),
        Some("Existing Standalone Key".to_string()),
        true,
        false,
        true,
        Some(20),
        None,
        None,
        None,
        None,
        None,
    )
    .expect("standalone key snapshot should build");
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::seed(vec![
            (Some(user_key_hash.clone()), user_key_snapshot),
            (Some(standalone_key_hash.clone()), standalone_key_snapshot),
        ])
        .with_export_records(vec![
            StoredAuthApiKeyExportRecord::new(
                "user-existing".to_string(),
                "key-user-existing".to_string(),
                user_key_hash.clone(),
                None,
                Some("Existing User Key".to_string()),
                None,
                None,
                None,
                Some(10),
                None,
                None,
                true,
                None,
                false,
                1,
                2,
                0.03,
                false,
            )
            .expect("existing user key export should build"),
            StoredAuthApiKeyExportRecord::new(
                "admin-user-123".to_string(),
                "key-standalone-existing".to_string(),
                standalone_key_hash.clone(),
                None,
                Some("Existing Standalone Key".to_string()),
                None,
                None,
                None,
                Some(20),
                None,
                None,
                true,
                None,
                false,
                4,
                5,
                0.06,
                true,
            )
            .expect("existing standalone key export should build"),
        ]),
    );
    let user_repository =
        Arc::new(aether_data::repository::users::InMemoryUserReadRepository::default());
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(
            GatewayDataState::with_auth_api_key_repository_for_tests(Arc::clone(&auth_repository))
                .with_user_reader(user_repository)
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
        )
        .with_auth_users_for_tests([sample_import_admin_user("admin-user-123"), existing_user])
        .with_auth_wallets_for_tests(Vec::<StoredWalletSnapshot>::new());
    let gateway = build_router_with_state(state.clone());
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let import_payload = json!({
        "version": "1.4",
        "merge_mode": "overwrite",
        "users": [{
            "id": "source-user-existing",
            "email": "existing@example.com",
            "username": "existing",
            "password_hash": "existing-hash",
            "role": "user",
            "is_active": true,
            "api_keys": [{
                "api_key_id": "source-user-key",
                "key_hash": user_key_hash,
                "name": "Imported User Key",
                "is_active": true,
                "total_requests": 222,
                "total_tokens": 3333,
                "total_cost_usd": 4.56
            }]
        }],
        "standalone_keys": [{
            "api_key_id": "source-standalone-key",
            "key_hash": standalone_key_hash,
            "name": "Imported Standalone Key",
            "is_active": true,
            "total_requests": 444,
            "total_tokens": 5555,
            "total_cost_usd": 6.78
        }]
    });
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{gateway_url}/api/admin/system/users/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&import_payload)
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["stats"]["users"]["updated"], json!(1));
    assert_eq!(payload["stats"]["api_keys"]["created"], json!(1));
    assert_eq!(payload["stats"]["standalone_keys"]["created"], json!(1));

    let response = client
        .post(format!("{gateway_url}/api/admin/system/users/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&import_payload)
        .send()
        .await
        .expect("repeated request should succeed");
    let status = response.status();
    let repeated_payload: Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={repeated_payload}");
    assert_eq!(repeated_payload["stats"]["api_keys"]["updated"], json!(1));
    assert_eq!(
        repeated_payload["stats"]["standalone_keys"]["updated"],
        json!(1)
    );

    let updated_records = state
        .list_auth_api_key_export_records_by_ids(&[
            "key-user-existing".to_string(),
            "key-standalone-existing".to_string(),
        ])
        .await
        .expect("api key export records should load");
    let user_key = updated_records
        .iter()
        .find(|record| record.api_key_id == "key-user-existing")
        .expect("updated user key should exist");
    assert!(user_key.is_active);
    assert_eq!(user_key.total_requests, 1);
    assert_eq!(user_key.total_tokens, 2);
    assert_eq!(user_key.total_cost_usd, 0.03);
    let standalone_key = updated_records
        .iter()
        .find(|record| record.api_key_id == "key-standalone-existing")
        .expect("updated standalone key should exist");
    assert!(standalone_key.is_active);
    assert_eq!(standalone_key.total_requests, 4);
    assert_eq!(standalone_key.total_tokens, 5);
    assert_eq!(standalone_key.total_cost_usd, 0.06);

    let user_keys = state
        .list_auth_api_key_export_records_by_user_ids(&["user-existing".to_string()])
        .await
        .expect("user key export records should load");
    assert_eq!(user_keys.len(), 2);
    let imported_user_key = user_keys
        .iter()
        .find(|record| record.api_key_id != "key-user-existing")
        .expect("disabled imported user key should exist");
    assert!(!imported_user_key.is_active);
    assert!(imported_user_key.key_encrypted.is_none());
    assert!(imported_user_key
        .key_hash
        .starts_with("$aether-import-revoked$"));
    assert_eq!(imported_user_key.total_requests, 222);
    assert_eq!(imported_user_key.total_tokens, 3333);
    assert_eq!(imported_user_key.total_cost_usd, 4.56);

    let standalone_keys = state
        .list_auth_api_key_export_standalone_records()
        .await
        .expect("standalone key export records should load");
    assert_eq!(standalone_keys.len(), 2);
    let imported_standalone_key = standalone_keys
        .iter()
        .find(|record| record.api_key_id != "key-standalone-existing")
        .expect("disabled imported standalone key should exist");
    assert!(!imported_standalone_key.is_active);
    assert!(imported_standalone_key.key_encrypted.is_none());
    assert!(imported_standalone_key
        .key_hash
        .starts_with("$aether-import-revoked$"));
    assert_eq!(imported_standalone_key.total_requests, 444);
    assert_eq!(imported_standalone_key.total_tokens, 5555);
    assert_eq!(imported_standalone_key.total_cost_usd, 6.78);

    gateway_handle.abort();
}

#[test]
fn gateway_imports_admin_system_config_fixture_v22() {
    run_admin_system_import_test(
        "gateway_imports_admin_system_config_fixture_v22",
        gateway_imports_admin_system_config_fixture_v22_impl,
    );
}

async fn gateway_imports_admin_system_config_fixture_v22_impl() {
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

#[test]
fn gateway_imports_admin_system_config_fixtures_from_legacy_exports() {
    run_admin_system_import_test(
        "gateway_imports_admin_system_config_fixtures_from_legacy_exports",
        gateway_imports_admin_system_config_fixtures_from_legacy_exports_impl,
    );
}

async fn gateway_imports_admin_system_config_fixtures_from_legacy_exports_impl() {
    for fixture in ["v20", "v21"] {
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
            StatusCode::OK,
            "fixture {fixture} should be accepted for Python migration compatibility"
        );
        let payload: Value = response.json().await.expect("json body should parse");
        assert_eq!(payload["message"], "配置导入成功");
        assert_eq!(payload["stats"]["global_models"]["created"], json!(1));
        assert_eq!(payload["stats"]["providers"]["created"], json!(1));

        gateway_handle.abort();
    }
}

#[test]
fn gateway_imports_python_cli_alias_export_and_model_test_smoke() {
    run_admin_system_import_test(
        "gateway_imports_python_cli_alias_export_and_model_test_smoke",
        gateway_imports_python_cli_alias_export_and_model_test_smoke_impl,
    );
}

async fn gateway_imports_python_cli_alias_export_and_model_test_smoke_impl() {
    let seen_plan = Arc::new(Mutex::new(None::<ExecutionPlan>));
    let seen_plan_clone = Arc::clone(&seen_plan);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |Json(plan): Json<ExecutionPlan>| {
            let seen_plan_inner = Arc::clone(&seen_plan_clone);
            async move {
                assert_eq!(plan.provider_api_format, "claude:messages");
                assert_eq!(plan.model_name.as_deref(), Some("claude-sonnet-python"));
                assert_eq!(
                    plan.body
                        .json_body
                        .as_ref()
                        .and_then(|body| body.get("model")),
                    Some(&json!("claude-sonnet-python"))
                );
                *seen_plan_inner.lock().expect("mutex should lock") = Some(plan.clone());
                Json(json!({
                    "request_id": plan.request_id,
                    "candidate_id": plan.candidate_id,
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "id": "msg_python_alias_smoke",
                            "type": "message",
                            "model": "claude-sonnet-python",
                            "content": [{
                                "type": "text",
                                "text": "ok"
                            }]
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 17
                    }
                }))
            }
        }),
    );
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(Vec::<
        StoredPublicGlobalModel,
    >::new()));
    let data_state = build_admin_system_data_state_with_repositories(
        Arc::clone(&provider_catalog_repository),
        Arc::clone(&global_model_repository),
    );
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let mut import_payload = sample_system_import_payload();
    import_payload["global_models"][0]["name"] = json!("claude-sonnet-python");
    import_payload["global_models"][0]["display_name"] = json!("Claude Sonnet Python");
    import_payload["providers"][0]["name"] = json!("python-export-claude");
    import_payload["providers"][0]["provider_type"] = json!("custom");
    import_payload["providers"][0]["endpoints"][0]["api_format"] = json!("claude:cli");
    import_payload["providers"][0]["endpoints"][0]["base_url"] =
        json!("https://python-export-claude.example.com");
    import_payload["providers"][0]["api_keys"][0]["name"] = json!("python-alias-key");
    import_payload["providers"][0]["api_keys"][0]["api_formats"] = json!(["claude:cli"]);
    import_payload["providers"][0]["api_keys"][0]["api_key"] = json!("sk-python-alias");
    import_payload["providers"][0]["models"][0]["global_model_name"] =
        json!("claude-sonnet-python");
    import_payload["providers"][0]["models"][0]["provider_model_name"] =
        json!("claude-sonnet-python");

    let client = reqwest::Client::new();
    let response = client
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&import_payload)
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");

    let providers = provider_catalog_repository
        .list_providers(false)
        .await
        .expect("providers should load");
    assert_eq!(providers.len(), 1);
    let provider_id = providers[0].id.clone();
    let endpoints = provider_catalog_repository
        .list_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
        .await
        .expect("endpoints should load");
    assert_eq!(endpoints.len(), 1);
    assert_eq!(endpoints[0].api_format, "claude:messages");
    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(std::slice::from_ref(&provider_id))
        .await
        .expect("keys should load");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].api_formats, Some(json!(["claude:messages"])));

    let response = client
        .post(format!("{gateway_url}/api/admin/provider-query/test-model"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&json!({
            "provider_id": provider_id,
            "model": "claude-sonnet-python",
            "endpoint_id": endpoints[0].id,
            "api_format": "claude:messages"
        }))
        .send()
        .await
        .expect("model test request should succeed");

    let status = response.status();
    let payload: Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["success"], json!(true));
    assert_eq!(
        payload["attempts"][0]["endpoint_api_format"],
        json!("claude:messages")
    );
    assert_eq!(
        payload["attempts"][0]["request_body"]["model"],
        json!("claude-sonnet-python")
    );
    assert!(
        seen_plan.lock().expect("mutex should lock").is_some(),
        "post-import model test should execute through runtime"
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[test]
fn gateway_rejects_legacy_user_import_string_bool_field() {
    run_admin_system_import_test(
        "gateway_rejects_legacy_user_import_string_bool_field",
        gateway_rejects_legacy_user_import_string_bool_field_impl,
    );
}

async fn gateway_rejects_legacy_user_import_string_bool_field_impl() {
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
            "version": "1.3",
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

#[test]
fn gateway_reports_field_path_for_invalid_admin_system_config_import_shape() {
    run_admin_system_import_test(
        "gateway_reports_field_path_for_invalid_admin_system_config_import_shape",
        gateway_reports_field_path_for_invalid_admin_system_config_import_shape_impl,
    );
}

async fn gateway_reports_field_path_for_invalid_admin_system_config_import_shape_impl() {
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

#[test]
fn gateway_rejects_invalid_anthropic_profiles_during_admin_system_config_import() {
    run_admin_system_import_test(
        "gateway_rejects_invalid_anthropic_profiles_during_admin_system_config_import",
        gateway_rejects_invalid_anthropic_profiles_during_admin_system_config_import_impl,
    );
}

async fn gateway_rejects_invalid_anthropic_profiles_during_admin_system_config_import_impl() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(build_empty_admin_system_data_state()),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    for config_scope in ["provider", "endpoint"] {
        let mut payload = sample_system_import_payload();
        let invalid_config = json!({
            "anthropic": {"compatibility_profile": "claude_cod_typo"}
        });
        if config_scope == "provider" {
            payload["providers"][0]["config"] = invalid_config;
        } else {
            payload["providers"][0]["endpoints"][0]["config"] = invalid_config;
        }

        let response = client
            .post(format!("{gateway_url}/api/admin/system/config/import"))
            .header(GATEWAY_HEADER, "rust-phase3b")
            .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
            .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
            .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
            .json(&payload)
            .send()
            .await
            .expect("invalid Anthropic profile import should complete locally");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = response.json().await.expect("json body should parse");
        assert_eq!(
            body["detail"], "无效的 Anthropic compatibility profile",
            "unexpected {config_scope} validation response: {body}"
        );
    }

    gateway_handle.abort();
}

#[test]
fn gateway_does_not_restore_retired_vertex_claude_endpoint_from_system_import() {
    run_admin_system_import_test(
        "gateway_does_not_restore_retired_vertex_claude_endpoint_from_system_import",
        gateway_does_not_restore_retired_vertex_claude_endpoint_from_system_import_impl,
    );
}

async fn gateway_does_not_restore_retired_vertex_claude_endpoint_from_system_import_impl() {
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(Vec::<
        StoredPublicGlobalModel,
    >::new()));
    let data_state = build_admin_system_data_state_with_repositories(
        Arc::clone(&provider_catalog_repository),
        Arc::clone(&global_model_repository),
    );
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let mut payload = sample_system_import_payload();
    payload["providers"][0]["name"] = json!("legacy-vertex-backup");
    payload["providers"][0]["provider_type"] = json!("vertex_ai");
    payload["providers"][0]["endpoints"] = json!([
        {
            "api_format": "gemini:generate_content",
            "base_url": "https://aiplatform.googleapis.com",
            "max_retries": 2,
            "is_active": true
        },
        {
            "api_format": "claude:messages",
            "base_url": "https://aiplatform.googleapis.com",
            "max_retries": 2,
            "is_active": true
        }
    ]);
    payload["providers"][0]["api_keys"] = json!([]);
    payload["providers"][0]["models"] = json!([]);

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&payload)
        .send()
        .await
        .expect("legacy Vertex import should complete");

    let status = response.status();
    let response_body: Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={response_body}");
    assert_eq!(response_body["stats"]["endpoints"]["created"], json!(1));
    assert_eq!(response_body["stats"]["endpoints"]["skipped"], json!(1));
    assert!(response_body["stats"]["errors"]
        .as_array()
        .is_some_and(|errors| errors.iter().any(|error| {
            error
                .as_str()
                .is_some_and(|error| error.contains("claude:messages"))
        })));

    let providers = provider_catalog_repository
        .list_providers(false)
        .await
        .expect("providers should load");
    assert_eq!(providers.len(), 1);
    let endpoints = provider_catalog_repository
        .list_endpoints_by_provider_ids(std::slice::from_ref(&providers[0].id))
        .await
        .expect("endpoints should load");
    assert_eq!(endpoints.len(), 1, "unexpected endpoints: {endpoints:?}");
    assert_eq!(endpoints[0].api_format, "gemini:generate_content");
    assert!(endpoints[0].is_active);

    gateway_handle.abort();
}

#[test]
fn gateway_imports_admin_system_config_with_numeric_string_prices() {
    run_admin_system_import_test(
        "gateway_imports_admin_system_config_with_numeric_string_prices",
        gateway_imports_admin_system_config_with_numeric_string_prices_impl,
    );
}

async fn gateway_imports_admin_system_config_with_numeric_string_prices_impl() {
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ));
    let global_model_repository = Arc::new(InMemoryGlobalModelReadRepository::seed(Vec::<
        StoredPublicGlobalModel,
    >::new()));
    let data_state = build_admin_system_data_state_with_repositories(
        Arc::clone(&provider_catalog_repository),
        Arc::clone(&global_model_repository),
    );
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
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

    let status = response.status();
    let body: Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={body}");

    let global_models = global_model_repository
        .list_admin_global_models(&AdminGlobalModelListQuery {
            offset: 0,
            limit: 10_000,
            is_active: None,
            search: None,
        })
        .await
        .expect("global models should load");
    assert_eq!(global_models.items[0].default_price_per_request, Some(1.8));

    let providers = provider_catalog_repository
        .list_providers(false)
        .await
        .expect("providers should load");
    let provider_models = global_model_repository
        .list_admin_provider_models(&AdminProviderModelListQuery {
            provider_id: providers[0].id.clone(),
            is_active: None,
            offset: 0,
            limit: 10_000,
        })
        .await
        .expect("provider models should load");
    assert_eq!(provider_models[0].price_per_request, Some(0.7));
    assert_eq!(providers[0].request_timeout_secs, Some(30.0));
    assert_eq!(providers[0].stream_first_byte_timeout_secs, Some(15.0));

    gateway_handle.abort();
}

#[test]
fn gateway_imports_oauth_provider_key_credentials_from_admin_system_config() {
    run_admin_system_import_test(
        "gateway_imports_oauth_provider_key_credentials_from_admin_system_config",
        gateway_imports_oauth_provider_key_credentials_from_admin_system_config_impl,
    );
}

async fn gateway_imports_oauth_provider_key_credentials_from_admin_system_config_impl() {
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
        decrypt_test_provider_catalog_credential(&keys[0], ProviderCatalogCredentialField::ApiKey,),
        "oauth-access-token-1"
    );
    let auth_config = decrypt_test_provider_catalog_credential(
        &keys[0],
        ProviderCatalogCredentialField::AuthConfig,
    );
    let auth_config: Value =
        serde_json::from_str(&auth_config).expect("oauth auth config json should parse");
    assert_eq!(auth_config["provider_type"], "codex");
    assert_eq!(auth_config["refresh_token"], "oauth-refresh-token-1");
    assert_eq!(auth_config["email"], "alice@example.com");

    gateway_handle.abort();
}

#[test]
fn gateway_overwrites_oauth_provider_key_credentials_from_admin_system_import() {
    run_admin_system_import_test(
        "gateway_overwrites_oauth_provider_key_credentials_from_admin_system_import",
        gateway_overwrites_oauth_provider_key_credentials_from_admin_system_import_impl,
    );
}

async fn gateway_overwrites_oauth_provider_key_credentials_from_admin_system_import_impl() {
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

    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(data_state);
    let gateway = build_router_with_state(state.clone());
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
        decrypt_test_provider_catalog_credential(&keys[0], ProviderCatalogCredentialField::ApiKey,),
        "oauth-access-token-new"
    );
    let auth_config = decrypt_test_provider_catalog_credential(
        &keys[0],
        ProviderCatalogCredentialField::AuthConfig,
    );
    let auth_config: Value =
        serde_json::from_str(&auth_config).expect("oauth auth config json should parse");
    assert_eq!(auth_config["refresh_token"], "oauth-refresh-token-new");

    assert!(state
        .update_provider_catalog_key_oauth_runtime_state(
            &keys[0].id,
            Some(1_700_000_001),
            Some("[REFRESH_FAILED] imported token remains invalid"),
            Some(1_700_000_001),
        )
        .await
        .expect("invalid marker should be seeded"));
    let mut metadata_only_payload = sample_oauth_system_import_payload("unused", "unused");
    let key_payload = metadata_only_payload["providers"][0]["api_keys"][0]
        .as_object_mut()
        .expect("OAuth key payload should be an object");
    key_payload.remove("api_key");
    key_payload.remove("auth_config");
    key_payload.insert("internal_priority".to_string(), json!(71));

    let response = client
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&metadata_only_payload)
        .send()
        .await
        .expect("metadata-only import should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(std::slice::from_ref(&providers[0].id))
        .await
        .expect("keys should reload");
    assert_eq!(keys[0].internal_priority, 71);
    assert_eq!(keys[0].oauth_invalid_at_unix_secs, Some(1_700_000_001));
    assert_eq!(
        keys[0].oauth_invalid_reason.as_deref(),
        Some("[REFRESH_FAILED] imported token remains invalid")
    );

    let response = client
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
        .expect("same valid credentials should be accepted as recovery input");
    assert_eq!(response.status(), StatusCode::OK);
    let keys = provider_catalog_repository
        .list_keys_by_provider_ids(std::slice::from_ref(&providers[0].id))
        .await
        .expect("keys should reload after credential recovery");
    assert_eq!(keys[0].oauth_invalid_at_unix_secs, None);
    assert_eq!(keys[0].oauth_invalid_reason, None);

    gateway_handle.abort();
}

#[test]
fn gateway_overwrites_oauth_provider_key_credentials_from_admin_system_import_without_refresh() {
    run_admin_system_import_test("gateway_overwrites_oauth_provider_key_credentials_from_admin_system_import_without_refresh", gateway_overwrites_oauth_provider_key_credentials_from_admin_system_import_without_refresh_impl);
}

async fn gateway_overwrites_oauth_provider_key_credentials_from_admin_system_import_without_refresh_impl(
) {
    let seen_refresh = Arc::new(Mutex::new(false));
    let seen_refresh_clone = Arc::clone(&seen_refresh);
    let refresh_hits = Arc::new(Mutex::new(0usize));
    let refresh_hits_clone = Arc::clone(&refresh_hits);
    let refresh_server = Router::new().route(
        "/oauth/token",
        post(move |_headers: HeaderMap, _body: Bytes| {
            let seen_refresh_inner = Arc::clone(&seen_refresh_clone);
            let refresh_hits_inner = Arc::clone(&refresh_hits_clone);
            async move {
                *refresh_hits_inner.lock().expect("mutex should lock") += 1;
                *seen_refresh_inner.lock().expect("mutex should lock") = true;
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
    provider.config = Some(json!({"pool_advanced": {}}));
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
    existing_key.learned_rpm_limit = Some(31);
    existing_key.oauth_invalid_at_unix_secs = Some(1_700_000_000);
    existing_key.oauth_invalid_reason =
        Some("[REFRESH_FAILED] refresh_token 无效、已过期或已撤销，请重新登录授权".to_string());
    existing_key.error_count = Some(7);
    existing_key.health_by_format = Some(json!({
        "openai:responses": {"consecutive_failures": 3}
    }));
    existing_key.circuit_breaker_by_format = Some(json!({
        "openai:responses": {"state": "open"}
    }));
    existing_key.encrypted_auth_config = Some(
        encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","refresh_token":"oauth-refresh-token-old","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","expires_at":1}"#,
        )
        .expect("auth config should encrypt"),
    );
    existing_key.upstream_metadata = Some(json!({
        "codex": {
            "credential_generation": "generation-before-import",
            "primary_used_percent": 90.0,
        },
        "unrelated": {"preserved": true},
    }));
    existing_key.status_snapshot = Some(json!({
        "oauth": {"status": "invalid"},
        "quota": {"used_ratio": 0.9},
    }));

    let score_identity =
        PoolMemberIdentity::provider_api_key("provider-codex-existing", "key-codex-existing");
    let score_scope = provider_key_pool_score_scope();
    let mut invalid_score = build_provider_key_pool_score_upsert(
        &existing_key,
        "codex",
        None,
        1_700_000_000,
        aether_pool_core::PoolMemberScoreRules::default(),
    )
    .into_stored();
    invalid_score.last_failure_at = Some(1_700_000_000);
    invalid_score.failure_count = 9;
    invalid_score.last_probe_failure_at = Some(1_700_000_000);
    invalid_score.probe_failure_count = 4;
    invalid_score.probe_status = PoolMemberProbeStatus::Failed;
    assert_eq!(invalid_score.hard_state, PoolMemberHardState::AuthInvalid);
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![provider],
        vec![endpoint],
        vec![existing_key],
    ));
    let pool_score_repository =
        Arc::new(InMemoryPoolMemberScoreRepository::seed(vec![invalid_score]));
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
    .with_pool_score_repository_for_tests(Arc::clone(&pool_score_repository))
    .with_system_config_values_for_tests(Vec::<(String, Value)>::new())
    .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state)
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let mut import_payload =
        sample_oauth_system_import_payload("oauth-access-token-new", "oauth-refresh-token-new");
    import_payload["providers"][0]["config"] = json!({"pool_advanced": {}});
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/admin/system/config/import"))
        .header(GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .json(&import_payload)
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(*refresh_hits.lock().expect("mutex should lock"), 0);
    assert!(!*seen_refresh.lock().expect("mutex should lock"));

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
    assert_eq!(key.expires_at_unix_secs, None);
    assert_eq!(key.learned_rpm_limit, None);
    assert_eq!(key.error_count, Some(0));
    assert_eq!(key.health_by_format, Some(json!({})));
    assert_eq!(key.circuit_breaker_by_format, Some(json!({})));
    let codex = key
        .upstream_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("codex"))
        .and_then(Value::as_object)
        .expect("Codex metadata should exist");
    assert_eq!(codex.len(), 1);
    assert_ne!(
        codex
            .get(aether_admin::provider::quota::CODEX_CREDENTIAL_GENERATION_KEY)
            .and_then(Value::as_str),
        Some("generation-before-import")
    );
    assert_eq!(
        key.upstream_metadata
            .as_ref()
            .and_then(|metadata| metadata.pointer("/unrelated/preserved")),
        Some(&json!(true))
    );
    assert_eq!(
        key.status_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.get("quota")),
        Some(&Value::Null)
    );
    assert_eq!(
        decrypt_test_provider_catalog_credential(key, ProviderCatalogCredentialField::ApiKey,),
        "oauth-access-token-new"
    );

    let auth_config =
        decrypt_test_provider_catalog_credential(key, ProviderCatalogCredentialField::AuthConfig);
    let auth_config: Value =
        serde_json::from_str(&auth_config).expect("oauth auth config json should parse");
    assert_eq!(auth_config["provider_type"], "codex");
    assert_eq!(auth_config["refresh_token"], "oauth-refresh-token-new");
    assert_eq!(auth_config["email"], "alice@example.com");
    assert_eq!(auth_config["account_id"], "acct-codex-123");
    assert_eq!(auth_config["plan_type"], "plus");
    assert!(auth_config.get("token_type").is_none());
    assert!(auth_config.get("expires_at").is_none());

    let scores = pool_score_repository
        .get_pool_member_scores_by_ids(&GetPoolMemberScoresByIdsQuery {
            ids: vec![provider_key_pool_score_id(&score_identity, &score_scope)],
        })
        .await
        .expect("pool score should load");
    assert_eq!(scores.len(), 1);
    assert!(
        scores[0].hard_state.schedulable(),
        "OAuth credential import should reset the pool score: {:?}",
        scores[0]
    );
    assert_eq!(scores[0].last_failure_at, None);
    assert_eq!(scores[0].failure_count, 0);
    assert_eq!(scores[0].last_probe_failure_at, None);
    assert_eq!(scores[0].probe_failure_count, 0);
    assert_eq!(scores[0].probe_status, PoolMemberProbeStatus::Never);

    gateway_handle.abort();
    refresh_handle.abort();
}

#[test]
fn gateway_skips_proxy_nodes_during_admin_system_config_import() {
    run_admin_system_import_test(
        "gateway_skips_proxy_nodes_during_admin_system_config_import",
        gateway_skips_proxy_nodes_during_admin_system_config_import_impl,
    );
}

async fn gateway_skips_proxy_nodes_during_admin_system_config_import_impl() {
    let data_state = build_empty_admin_system_data_state();
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state.clone()),
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
            }],
            "system_configs": [{
                "key": "external_models_proxy_node_id",
                "value": "legacy-node-1",
                "description": "External models proxy"
            }]
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["stats"]["proxy_nodes"]["skipped"], json!(1));
    assert_eq!(payload["stats"]["system_configs"]["created"], json!(1));
    assert!(payload["stats"]["errors"]
        .as_array()
        .expect("errors should be an array")
        .iter()
        .any(|item| item
            .as_str()
            .is_some_and(|value| value.contains("暂不支持导入代理节点"))));
    assert!(payload["stats"]["errors"]
        .as_array()
        .expect("errors should be an array")
        .iter()
        .any(|item| item
            .as_str()
            .is_some_and(|value| value.contains("已切换为直连"))));
    assert_eq!(
        data_state
            .find_system_config_value("external_models_proxy_node_id")
            .await
            .expect("external models proxy config lookup should succeed"),
        Some(Value::Null)
    );

    gateway_handle.abort();
}

#[test]
fn gateway_preserves_manual_proxy_configs_while_skipping_proxy_nodes_during_import() {
    run_admin_system_import_test(
        "gateway_preserves_manual_proxy_configs_while_skipping_proxy_nodes_during_import",
        gateway_preserves_manual_proxy_configs_while_skipping_proxy_nodes_during_import_impl,
    );
}

async fn gateway_preserves_manual_proxy_configs_while_skipping_proxy_nodes_during_import_impl() {
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
            "url": "https://proxy.example/"
        }))
    );

    gateway_handle.abort();
}
