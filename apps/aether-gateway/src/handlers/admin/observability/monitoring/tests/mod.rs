use super::maybe_build_local_admin_monitoring_response;
use super::test_support::*;
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::AppState;
use aether_data_contracts::repository::{
    candidates::{RequestCandidateStatus, StoredRequestCandidate},
    provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    },
    usage::StoredRequestUsageAudit,
};
use axum::body::to_bytes;
use serde_json::json;
use std::sync::Arc;

use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeyExportRecord,
};
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_data::repository::users::{
    InMemoryUserReadRepository, StoredUserAuthRecord, StoredUserExportRow,
};

mod basics;

pub(super) async fn local_monitoring_response(
    state: &AppState,
    context: &GatewayPublicRequestContext,
) -> crate::handlers::admin::request::AdminRouteResult {
    maybe_build_local_admin_monitoring_response(
        &AdminAppState::new(state),
        &AdminRequestContext::new(context),
    )
    .await
}

#[tokio::test]
async fn admin_monitoring_cache_affinities_returns_empty_payload_without_runtime_or_test_entries() {
    let state = AppState::new().expect("state should build");
    let context = request_context(http::Method::GET, "/api/admin/monitoring/cache/affinities");

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["data"]["items"], json!([]));
    assert_eq!(payload["data"]["meta"]["total"], json!(0));
    assert_eq!(payload["data"]["meta"]["count"], json!(0));
    assert_eq!(payload["data"]["matched_user_id"], serde_json::Value::Null);
}

#[tokio::test]
async fn admin_monitoring_cache_affinity_returns_not_found_without_runtime_or_test_entries() {
    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![sample_monitoring_auth_user("user-1")])
            .with_export_users(vec![sample_monitoring_export_user("user-1")]),
    );
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::default().with_export_records(vec![
            sample_monitoring_export_api_key("user-1", "user-key-1"),
        ]),
    );
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_user_reader_for_tests(user_repository)
                .with_auth_api_key_reader(auth_repository),
        );
    let context = request_context(
        http::Method::GET,
        "/api/admin/monitoring/cache/affinity/alice",
    );

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], json!("not_found"));
    assert_eq!(payload["user_info"]["user_id"], json!("user-1"));
    assert_eq!(payload["affinities"], json!([]));
    assert_eq!(
        payload["message"],
        json!("用户 alice (alice@example.com) 没有缓存亲和性")
    );
}

#[tokio::test]
async fn admin_monitoring_cache_affinities_and_affinity_return_local_payload_from_test_store() {
    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![sample_monitoring_auth_user("user-1")])
            .with_export_users(vec![sample_monitoring_export_user("user-1")]),
    );
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::default().with_export_records(vec![
            sample_monitoring_export_api_key("user-1", "user-key-1"),
        ]),
    );
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_monitoring_catalog_endpoint()],
        vec![sample_monitoring_catalog_key()],
    ));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_provider_catalog_reader_for_tests(provider_catalog)
                .with_user_reader(user_repository)
                .with_auth_api_key_reader(auth_repository),
        )
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-1:openai:model-alpha",
            json!({
                "provider_id": "provider-1",
                "endpoint_id": "endpoint-1",
                "key_id": "provider-key-1",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 7,
            }),
        )
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-2:openai:model-beta",
            json!({
                "provider_id": "provider-2",
                "endpoint_id": "endpoint-2",
                "key_id": "provider-key-2",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 4,
            }),
        );

    let list_context = request_context(
        http::Method::GET,
        "/api/admin/monitoring/cache/affinities?keyword=alice&limit=20&offset=0",
    );
    let list_response = local_monitoring_response(&state, &list_context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");
    assert_eq!(list_response.status(), http::StatusCode::OK);
    let list_body = to_bytes(list_response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let list_payload: serde_json::Value =
        serde_json::from_slice(&list_body).expect("json body should parse");
    assert_eq!(list_payload["status"], json!("ok"));
    assert_eq!(list_payload["data"]["meta"]["total"], json!(1));
    assert_eq!(list_payload["data"]["matched_user_id"], json!("user-1"));
    assert_eq!(
        list_payload["data"]["items"][0]["affinity_key"],
        json!("user-key-1")
    );
    assert_eq!(list_payload["data"]["items"][0]["username"], json!("alice"));
    assert_eq!(
        list_payload["data"]["items"][0]["provider_name"],
        json!("OpenAI")
    );
    assert_eq!(
        list_payload["data"]["items"][0]["endpoint_url"],
        json!("https://api.openai.example/v1")
    );
    assert_eq!(
        list_payload["data"]["items"][0]["key_name"],
        json!("prod-key")
    );
    assert_eq!(list_payload["data"]["items"][0]["request_count"], json!(7));

    let detail_context = request_context(
        http::Method::GET,
        "/api/admin/monitoring/cache/affinity/alice",
    );
    let detail_response = local_monitoring_response(&state, &detail_context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");
    assert_eq!(detail_response.status(), http::StatusCode::OK);
    let detail_body = to_bytes(detail_response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let detail_payload: serde_json::Value =
        serde_json::from_slice(&detail_body).expect("json body should parse");
    assert_eq!(detail_payload["status"], json!("ok"));
    assert_eq!(detail_payload["user_info"]["user_id"], json!("user-1"));
    assert_eq!(
        detail_payload["affinities"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(
        detail_payload["affinities"][0]["api_format"],
        json!("openai")
    );
    assert_eq!(detail_payload["total_endpoints"], json!(1));
}

#[tokio::test]
async fn admin_monitoring_cache_users_delete_returns_local_payload_from_test_store() {
    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![sample_monitoring_auth_user("user-1")])
            .with_export_users(vec![sample_monitoring_export_user("user-1")]),
    );
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::default().with_export_records(vec![
            sample_monitoring_export_api_key("user-1", "user-key-1"),
        ]),
    );
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_user_reader_for_tests(user_repository)
                .with_auth_api_key_reader(auth_repository),
        )
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-1:openai:model-alpha",
            json!({
                "provider_id": "provider-1",
                "endpoint_id": "endpoint-1",
                "key_id": "provider-key-1",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 7,
            }),
        )
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-2:openai:model-beta",
            json!({
                "provider_id": "provider-2",
                "endpoint_id": "endpoint-2",
                "key_id": "provider-key-2",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 4,
            }),
        );

    let response = local_monitoring_response(
        &state,
        &request_context(
            http::Method::DELETE,
            "/api/admin/monitoring/cache/users/alice",
        ),
    )
    .await
    .expect("handler should not error")
    .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(
        payload["message"],
        json!("已清除用户 alice 的所有缓存亲和性")
    );
    assert_eq!(payload["user_info"]["user_id"], json!("user-1"));
    let remaining = state.list_admin_monitoring_cache_affinity_entries_for_tests();
    assert_eq!(remaining.len(), 1);
    assert!(remaining
        .iter()
        .any(|(key, _)| key == "cache_affinity:user-key-2:openai:model-beta"));
}

#[tokio::test]
async fn admin_monitoring_cache_users_delete_returns_not_found_for_unknown_identifier() {
    let state = AppState::new()
        .expect("state should build")
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-1:openai:model-alpha",
            json!({
                "provider_id": "provider-1",
                "endpoint_id": "endpoint-1",
                "key_id": "provider-key-1",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 7,
            }),
        );

    let response = local_monitoring_response(
        &state,
        &request_context(
            http::Method::DELETE,
            "/api/admin/monitoring/cache/users/unknown",
        ),
    )
    .await
    .expect("handler should not error")
    .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(
        payload["detail"],
        json!("无法识别的标识符: unknown。支持用户名、邮箱、User ID或API Key ID")
    );
}

#[tokio::test]
async fn admin_monitoring_cache_flush_returns_local_payload_from_test_store() {
    let state = AppState::new()
        .expect("state should build")
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-1:openai:model-alpha",
            json!({
                "provider_id": "provider-1",
                "endpoint_id": "endpoint-1",
                "key_id": "provider-key-1",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 7,
            }),
        )
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-2:openai:model-beta",
            json!({
                "provider_id": "provider-2",
                "endpoint_id": "endpoint-2",
                "key_id": "provider-key-2",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 4,
            }),
        );

    let response = local_monitoring_response(
        &state,
        &request_context(http::Method::DELETE, "/api/admin/monitoring/cache"),
    )
    .await
    .expect("handler should not error")
    .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["message"], json!("已清除全部缓存亲和性"));
    assert_eq!(payload["deleted_affinities"], json!(2));
    assert!(state
        .list_admin_monitoring_cache_affinity_entries_for_tests()
        .is_empty());
}

#[tokio::test]
async fn admin_monitoring_cache_provider_delete_returns_local_payload_from_test_store() {
    let state = AppState::new()
        .expect("state should build")
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-1:openai:model-alpha",
            json!({
                "provider_id": "provider-1",
                "endpoint_id": "endpoint-1",
                "key_id": "provider-key-1",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 7,
            }),
        )
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-2:openai:model-beta",
            json!({
                "provider_id": "provider-2",
                "endpoint_id": "endpoint-2",
                "key_id": "provider-key-2",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 4,
            }),
        );

    let response = local_monitoring_response(
        &state,
        &request_context(
            http::Method::DELETE,
            "/api/admin/monitoring/cache/providers/provider-1",
        ),
    )
    .await
    .expect("handler should not error")
    .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(
        payload["message"],
        json!("已清除 provider provider-1 的缓存亲和性")
    );
    assert_eq!(payload["provider_id"], json!("provider-1"));
    assert_eq!(payload["deleted_affinities"], json!(1));
    assert_eq!(
        state
            .list_admin_monitoring_cache_affinity_entries_for_tests()
            .len(),
        1
    );
}

#[tokio::test]
async fn admin_monitoring_model_mapping_delete_returns_local_payload_from_test_store() {
    let state = AppState::new()
        .expect("state should build")
        .with_admin_monitoring_redis_key_for_tests("model:id:model-1", json!({"id": "model-1"}))
        .with_admin_monitoring_redis_key_for_tests(
            "model:provider_global:provider-1:model-alpha",
            json!({"provider_id": "provider-1", "global_model_id": "model-alpha"}),
        )
        .with_admin_monitoring_redis_key_for_tests(
            "global_model:name:model-alpha",
            json!({"name": "model-alpha"}),
        )
        .with_admin_monitoring_redis_key_for_tests(
            "global_model:resolve:model-alpha",
            json!({"id": "model-alpha"}),
        );

    let response = local_monitoring_response(
        &state,
        &request_context(
            http::Method::DELETE,
            "/api/admin/monitoring/cache/model-mapping",
        ),
    )
    .await
    .expect("handler should not error")
    .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["message"], json!("已清除所有模型映射缓存"));
    assert_eq!(payload["deleted_count"], json!(4));
    assert!(state
        .list_admin_monitoring_redis_keys_for_tests()
        .is_empty());
}

#[tokio::test]
async fn admin_monitoring_model_mapping_delete_model_returns_local_payload_from_test_store() {
    let state = AppState::new()
        .expect("state should build")
        .with_admin_monitoring_redis_key_for_tests(
            "global_model:name:model-alpha",
            json!({"name": "model-alpha"}),
        )
        .with_admin_monitoring_redis_key_for_tests(
            "global_model:resolve:model-alpha",
            json!({"id": "model-alpha"}),
        )
        .with_admin_monitoring_redis_key_for_tests(
            "global_model:name:model-beta",
            json!({"name": "model-beta"}),
        );

    let response = local_monitoring_response(
        &state,
        &request_context(
            http::Method::DELETE,
            "/api/admin/monitoring/cache/model-mapping/model-alpha",
        ),
    )
    .await
    .expect("handler should not error")
    .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["model_name"], json!("model-alpha"));
    assert_eq!(
        payload["deleted_keys"],
        json!([
            "global_model:name:model-alpha",
            "global_model:resolve:model-alpha"
        ])
    );
    assert_eq!(
        state.list_admin_monitoring_redis_keys_for_tests(),
        vec!["global_model:name:model-beta".to_string()]
    );
}

#[tokio::test]
async fn admin_monitoring_model_mapping_delete_provider_returns_local_payload_from_test_store() {
    let state = AppState::new()
        .expect("state should build")
        .with_admin_monitoring_redis_key_for_tests(
            "model:provider_global:provider-1:model-alpha",
            json!({"provider_id": "provider-1"}),
        )
        .with_admin_monitoring_redis_key_for_tests(
            "model:provider_global:hits:provider-1:model-alpha",
            json!(12),
        )
        .with_admin_monitoring_redis_key_for_tests(
            "model:provider_global:provider-2:model-alpha",
            json!({"provider_id": "provider-2"}),
        );

    let response = local_monitoring_response(
        &state,
        &request_context(
            http::Method::DELETE,
            "/api/admin/monitoring/cache/model-mapping/provider/provider-1/model-alpha",
        ),
    )
    .await
    .expect("handler should not error")
    .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["provider_id"], json!("provider-1"));
    assert_eq!(payload["global_model_id"], json!("model-alpha"));
    assert_eq!(
        payload["deleted_keys"],
        json!([
            "model:provider_global:hits:provider-1:model-alpha",
            "model:provider_global:provider-1:model-alpha"
        ])
    );
    assert_eq!(
        state.list_admin_monitoring_redis_keys_for_tests(),
        vec!["model:provider_global:provider-2:model-alpha".to_string()]
    );
}

#[tokio::test]
async fn admin_monitoring_redis_keys_delete_returns_local_payload_from_test_store() {
    let state = AppState::new()
        .expect("state should build")
        .with_admin_monitoring_redis_key_for_tests("dashboard:summary:user-1", json!({"ok": true}))
        .with_admin_monitoring_redis_key_for_tests("dashboard:stats:user-1", json!({"ok": true}))
        .with_admin_monitoring_redis_key_for_tests("user:user-1", json!({"ok": true}));

    let response = local_monitoring_response(
        &state,
        &request_context(
            http::Method::DELETE,
            "/api/admin/monitoring/cache/redis-keys/dashboard",
        ),
    )
    .await
    .expect("handler should not error")
    .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["category"], json!("dashboard"));
    assert_eq!(payload["deleted_count"], json!(2));
    assert_eq!(payload["message"], json!("已清除 仪表盘 缓存"));
    assert_eq!(
        state.list_admin_monitoring_redis_keys_for_tests(),
        vec!["user:user-1".to_string()]
    );
}

#[tokio::test]
async fn admin_monitoring_cache_affinity_delete_returns_local_payload_from_test_store() {
    let user_repository = Arc::new(
        InMemoryUserReadRepository::seed_auth_users(vec![sample_monitoring_auth_user("user-1")])
            .with_export_users(vec![sample_monitoring_export_user("user-1")]),
    );
    let auth_repository = Arc::new(
        InMemoryAuthApiKeySnapshotRepository::default().with_export_records(vec![
            sample_monitoring_export_api_key("user-1", "user-key-1"),
        ]),
    );
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_user_reader_for_tests(user_repository)
                .with_auth_api_key_reader(auth_repository),
        )
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-1:openai:model-alpha",
            json!({
                "provider_id": "provider-1",
                "endpoint_id": "endpoint-1",
                "key_id": "provider-key-1",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 7,
            }),
        )
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-2:openai:model-beta",
            json!({
                "provider_id": "provider-2",
                "endpoint_id": "endpoint-2",
                "key_id": "provider-key-2",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 4,
            }),
        );

    let response = local_monitoring_response(
        &state,
        &request_context(
            http::Method::DELETE,
            "/api/admin/monitoring/cache/affinity/user-key-1/endpoint-1/model-alpha/openai",
        ),
    )
    .await
    .expect("handler should not error")
    .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["message"], json!("已清除缓存亲和性: Alice Key"));
    assert_eq!(payload["affinity_key"], json!("user-key-1"));
    assert_eq!(payload["endpoint_id"], json!("endpoint-1"));
    assert_eq!(payload["model_id"], json!("model-alpha"));
    let remaining = state.list_admin_monitoring_cache_affinity_entries_for_tests();
    assert_eq!(remaining.len(), 1);
    assert!(remaining
        .iter()
        .any(|(key, _)| key == "cache_affinity:user-key-2:openai:model-beta"));
}

#[tokio::test]
async fn admin_monitoring_cache_affinity_delete_returns_not_found_for_mismatched_endpoint() {
    let state = AppState::new()
        .expect("state should build")
        .with_admin_monitoring_cache_affinity_entry_for_tests(
            "cache_affinity:user-key-1:openai:model-alpha",
            json!({
                "provider_id": "provider-1",
                "endpoint_id": "endpoint-1",
                "key_id": "provider-key-1",
                "created_at": 1710000000,
                "expire_at": 1710000300,
                "request_count": 7,
            }),
        );

    let response = local_monitoring_response(
        &state,
        &request_context(
            http::Method::DELETE,
            "/api/admin/monitoring/cache/affinity/user-key-1/endpoint-2/model-alpha/openai",
        ),
    )
    .await
    .expect("handler should not error")
    .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["detail"], json!("未找到指定的缓存亲和性记录"));
}

#[tokio::test]
async fn admin_monitoring_cache_metrics_returns_local_payload() {
    let now = chrono::Utc::now().timestamp();
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![
        sample_usage(
            "request-cache-hit",
            "provider-1",
            "OpenAI",
            20,
            0.20,
            "success",
            Some(200),
            now - 60,
        )
        .with_cache_input_tokens(10, 5),
        sample_usage(
            "request-cache-miss",
            "provider-1",
            "OpenAI",
            15,
            0.10,
            "success",
            Some(200),
            now - 120,
        ),
    ]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_usage_reader_for_tests(usage_repository)
                .with_system_config_values_for_tests([
                    ("scheduling_mode".to_string(), json!("cache_affinity")),
                    ("provider_priority_mode".to_string(), json!("provider")),
                ]),
        );
    let context = request_context(http::Method::GET, "/api/admin/monitoring/cache/metrics");

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(
        response.headers().get(http::header::CONTENT_TYPE),
        Some(&http::HeaderValue::from_static(
            "text/plain; version=0.0.4; charset=utf-8"
        ))
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload = String::from_utf8(body.to_vec()).expect("body should be utf8");
    assert!(
        payload.contains("# HELP cache_scheduler_cache_hits Cache hits counted during scheduling")
    );
    assert!(payload.contains("cache_scheduler_cache_hits 1"));
    assert!(payload.contains("cache_scheduler_cache_misses 1"));
    assert!(payload.contains("cache_scheduler_cache_hit_rate 0.5"));
    assert!(payload.contains("cache_affinity_total 0"));
    assert!(payload.contains("cache_scheduler_info{scheduler=\"cache_aware\"} 1"));
}

#[tokio::test]
async fn admin_monitoring_cache_config_returns_local_payload() {
    let state = AppState::new().expect("state should build");
    let context = request_context(http::Method::GET, "/api/admin/monitoring/cache/config");

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["data"]["cache_ttl_seconds"], json!(300));
    assert_eq!(payload["data"]["cache_reservation_ratio"], json!(0.1));
    assert_eq!(
        payload["data"]["dynamic_reservation"]["enabled"],
        json!(true)
    );
    assert_eq!(
        payload["data"]["dynamic_reservation"]["config"]["probe_phase_requests"],
        json!(100)
    );
    assert_eq!(
        payload["data"]["dynamic_reservation"]["config"]["stable_max_reservation"],
        json!(0.35)
    );
    assert_eq!(
        payload["data"]["description"]["dynamic_reservation"],
        json!("动态预留机制配置")
    );
}

#[tokio::test]
async fn admin_monitoring_model_mapping_stats_returns_local_payload_without_redis() {
    let state = AppState::new().expect("state should build");
    let context = request_context(
        http::Method::GET,
        "/api/admin/monitoring/cache/model-mapping/stats",
    );

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["data"]["available"], json!(false));
    assert_eq!(
        payload["data"]["message"],
        json!("Redis 未启用，模型映射缓存不可用")
    );
}

#[tokio::test]
async fn admin_monitoring_reset_error_stats_returns_local_payload_and_clears_future_snapshot() {
    let now = chrono::Utc::now().timestamp();
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![],
        vec![sample_key().with_health_fields(
            Some(json!({
                "openai:chat": {
                    "health_score": 0.25,
                    "consecutive_failures": 3,
                    "last_failure_at": "2026-03-30T12:00:00+00:00"
                }
            })),
            Some(json!({
                "openai:chat": {
                    "open": true
                }
            })),
        )],
    ));
    let usage_repository = Arc::new(InMemoryUsageReadRepository::seed(vec![sample_usage(
        "request-recent-failed",
        "provider-1",
        "OpenAI",
        10,
        0.10,
        "failed",
        Some(502),
        now - 120,
    )]));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_provider_catalog_and_usage_reader_for_tests(
                provider_catalog,
                usage_repository,
            ),
        );

    let reset_context = request_context(
        http::Method::DELETE,
        "/api/admin/monitoring/resilience/error-stats",
    );
    let reset_response = local_monitoring_response(&state, &reset_context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(reset_response.status(), http::StatusCode::OK);
    let body = to_bytes(reset_response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["message"], json!("错误统计已重置"));
    assert_eq!(payload["previous_stats"]["total_errors"], json!(1));
    assert_eq!(payload["previous_stats"]["recent_errors"], json!(1));
    assert_eq!(
        payload["previous_stats"]["circuit_breakers"]["provider-key-1"]["state"],
        json!("open")
    );
    assert_eq!(payload["reset_by"], serde_json::Value::Null);
    assert!(payload["reset_at"].as_str().is_some());

    let status_context =
        request_context(http::Method::GET, "/api/admin/monitoring/resilience-status");
    let status_response = local_monitoring_response(&state, &status_context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(status_response.status(), http::StatusCode::OK);
    let body = to_bytes(status_response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["error_statistics"]["total_errors"], json!(0));
    assert_eq!(payload["recent_errors"], json!([]));
    assert_eq!(
        payload["error_statistics"]["open_circuit_breakers"],
        json!(1)
    );
}

#[tokio::test]
async fn admin_monitoring_redis_keys_returns_local_payload_without_redis() {
    let state = AppState::new().expect("state should build");
    let context = request_context(http::Method::GET, "/api/admin/monitoring/cache/redis-keys");

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["status"], json!("ok"));
    assert_eq!(payload["data"]["available"], json!(false));
    assert_eq!(payload["data"]["message"], json!("Redis 未启用"));
}

#[tokio::test]
async fn admin_monitoring_redis_keys_delete_returns_unavailable_without_redis() {
    let state = AppState::new().expect("state should build");
    let context = request_context(
        http::Method::DELETE,
        "/api/admin/monitoring/cache/redis-keys/upstream_models",
    );

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::SERVICE_UNAVAILABLE);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["detail"], json!("Redis 未启用"));
}

#[tokio::test]
async fn admin_monitoring_circuit_history_returns_local_payload() {
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![],
        vec![sample_key().with_health_fields(
            Some(json!({
                "openai:chat": {
                    "health_score": 0.25,
                    "consecutive_failures": 3,
                    "last_failure_at": "2026-03-30T12:00:00+00:00"
                }
            })),
            Some(json!({
                "openai:chat": {
                    "open": true,
                    "open_at": "2026-03-30T12:00:00+00:00",
                    "next_probe_at": "2026-03-30T12:05:00+00:00",
                    "reason": "错误率过高"
                }
            })),
        )],
    ));
    let state = AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_provider_catalog_reader_for_tests(provider_catalog),
        );
    let context = request_context(
        http::Method::GET,
        "/api/admin/monitoring/resilience/circuit-history?limit=10",
    );

    let response = local_monitoring_response(&state, &context)
        .await
        .expect("handler should not error")
        .expect("route should be handled locally");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value = serde_json::from_slice(&body).expect("json body should parse");
    assert_eq!(payload["count"], json!(1));
    assert_eq!(payload["items"][0]["event"], json!("opened"));
    assert_eq!(payload["items"][0]["key_id"], json!("provider-key-1"));
    assert_eq!(payload["items"][0]["provider_name"], json!("OpenAI"));
    assert_eq!(payload["items"][0]["api_format"], json!("openai:chat"));
    assert_eq!(payload["items"][0]["reason"], json!("错误率过高"));
    assert_eq!(payload["items"][0]["recovery_seconds"], json!(300));
    assert_eq!(
        payload["items"][0]["timestamp"],
        json!("2026-03-30T12:00:00+00:00")
    );
}

mod trace;
