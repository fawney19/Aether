use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::gemini_file_mappings::{
    GeminiFileMappingReadRepository, InMemoryGeminiFileMappingRepository,
};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use axum::body::{to_bytes, Body};
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use http::StatusCode;
use serde_json::json;
use std::sync::{Arc, Mutex};

use super::super::{
    build_router_with_state, build_state_with_execution_runtime_override, sample_provider,
    start_server,
};
use crate::constants::{
    GATEWAY_HEADER, TRACE_ID_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER,
    TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::data::GatewayDataState;

#[derive(Debug, Clone)]
struct SeenAdminGeminiUploadExecution {
    key_id: String,
    method: String,
    url: String,
    auth_header_value: String,
    content_type: String,
    body_bytes_b64: String,
}

fn sample_admin_gemini_provider() -> StoredProviderCatalogProvider {
    sample_provider("provider-gemini-admin-1", "gemini", 10).with_transport_fields(
        true,
        false,
        false,
        None,
        Some(2),
        None,
        Some(20.0),
        None,
        None,
    )
}

fn sample_admin_gemini_endpoint() -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        "endpoint-gemini-admin-1".to_string(),
        "provider-gemini-admin-1".to_string(),
        "gemini:generate_content".to_string(),
        Some("gemini".to_string()),
        Some("chat".to_string()),
        true,
    )
    .expect("endpoint should build")
    .with_transport_fields(
        "https://generativelanguage.googleapis.com".to_string(),
        None,
        None,
        Some(2),
        None,
        None,
        None,
        None,
    )
    .expect("endpoint transport should build")
}

fn sample_admin_gemini_key(id: &str, name: &str, secret: &str) -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        id.to_string(),
        "provider-gemini-admin-1".to_string(),
        name.to_string(),
        "api_key".to_string(),
        Some(json!({"gemini_files": true})),
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(json!(["gemini:generate_content"])),
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, secret)
            .expect("api key should encrypt"),
        None,
        None,
        Some(json!({"gemini:generate_content": 1})),
        None,
        None,
        None,
        None,
    )
    .expect("key transport should build")
}

fn build_admin_gemini_upload_multipart(
    boundary: &str,
    file_name: &str,
    mime_type: &str,
    body: &[u8],
) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: {mime_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    payload.extend_from_slice(body);
    payload.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    payload
}

#[tokio::test]
async fn gateway_uploads_admin_gemini_file_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/gemini-files/upload",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let seen_execution_runtime = Arc::new(Mutex::new(Vec::<SeenAdminGeminiUploadExecution>::new()));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            async move {
                let (_parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value = serde_json::from_slice(&raw_body)
                    .expect("execution runtime payload should parse");
                let key_id = payload
                    .get("key_id")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                seen_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(SeenAdminGeminiUploadExecution {
                        key_id: key_id.clone(),
                        method: payload
                            .get("method")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        url: payload
                            .get("url")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        auth_header_value: payload
                            .get("headers")
                            .and_then(|value| value.get("x-goog-api-key"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        content_type: payload
                            .get("headers")
                            .and_then(|value| value.get("content-type"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        body_bytes_b64: payload
                            .get("body")
                            .and_then(|value| value.get("body_bytes_b64"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    });
                match key_id.as_str() {
                    "key-gemini-admin-ok" => Json(json!({
                        "request_id": "trace-admin-gemini-upload-123:ok",
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "file": {
                                    "name": "files/admin-upload-ok"
                                }
                            }
                        }
                    })),
                    "key-gemini-admin-fail" => Json(json!({
                        "request_id": "trace-admin-gemini-upload-123:fail",
                        "status_code": 429,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "error": {
                                    "message": "quota exceeded"
                                }
                            }
                        }
                    })),
                    other => panic!("unexpected key id: {other}"),
                }
            }
        }),
    );

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_admin_gemini_provider()],
        vec![sample_admin_gemini_endpoint()],
        vec![
            sample_admin_gemini_key("key-gemini-admin-ok", "ok-key", "sk-admin-gemini-ok"),
            sample_admin_gemini_key("key-gemini-admin-fail", "fail-key", "sk-admin-gemini-fail"),
        ],
    ));
    let gemini_file_mapping_repository = Arc::new(InMemoryGeminiFileMappingRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let data_state =
        GatewayDataState::with_request_candidate_and_gemini_file_mapping_repository_for_tests(
            request_candidate_repository,
            Arc::clone(&gemini_file_mapping_repository),
        )
        .attach_provider_catalog_repository_for_tests(Arc::clone(&provider_catalog_repository))
        .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY);

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let boundary = "----AetherAdminGeminiUploadBoundary";
    let upload_body = build_admin_gemini_upload_multipart(
        boundary,
        "admin-upload.txt",
        "text/plain",
        b"admin-upload-body",
    );
    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/gemini-files/upload?key_ids=key-gemini-admin-ok,key-gemini-admin-fail"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .header(
            http::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(TRACE_ID_HEADER, "trace-admin-gemini-upload-123")
        .body(upload_body)
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(status, StatusCode::OK, "payload={payload}");
    assert_eq!(payload["display_name"], "admin-upload.txt");
    assert_eq!(payload["mime_type"], "text/plain");
    assert_eq!(payload["size_bytes"], json!(17));
    assert_eq!(payload["success_count"], json!(1));
    assert_eq!(payload["fail_count"], json!(1));
    assert_eq!(payload["results"].as_array().map(Vec::len), Some(2));
    assert_eq!(payload["results"][0]["key_id"], "key-gemini-admin-ok");
    assert_eq!(payload["results"][0]["key_name"], "ok-key");
    assert_eq!(payload["results"][0]["success"], json!(true));
    assert_eq!(payload["results"][0]["file_name"], "files/admin-upload-ok");
    assert_eq!(payload["results"][0]["error"], serde_json::Value::Null);
    assert_eq!(payload["results"][1]["key_id"], "key-gemini-admin-fail");
    assert_eq!(payload["results"][1]["key_name"], "fail-key");
    assert_eq!(payload["results"][1]["success"], json!(false));
    assert_eq!(payload["results"][1]["file_name"], serde_json::Value::Null);
    assert_eq!(payload["results"][1]["error"], "quota exceeded");

    let seen_requests = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone();
    assert_eq!(seen_requests.len(), 2);
    for request in &seen_requests {
        assert_eq!(request.method, "POST");
        assert_eq!(
            request.url,
            "https://generativelanguage.googleapis.com/upload/v1beta/files?uploadType=resumable"
        );
        assert_eq!(request.content_type, "text/plain");
        assert_eq!(
            BASE64_STANDARD
                .decode(&request.body_bytes_b64)
                .expect("execution runtime body should decode"),
            b"admin-upload-body"
        );
    }
    let seen_by_key_id = seen_requests
        .iter()
        .map(|request| (request.key_id.as_str(), request))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        seen_by_key_id
            .get("key-gemini-admin-ok")
            .expect("ok request should exist")
            .auth_header_value,
        "sk-admin-gemini-ok"
    );
    assert_eq!(
        seen_by_key_id
            .get("key-gemini-admin-fail")
            .expect("fail request should exist")
            .auth_header_value,
        "sk-admin-gemini-fail"
    );

    let stored_mapping = gemini_file_mapping_repository
        .find_by_file_name("files/admin-upload-ok")
        .await
        .expect("mapping lookup should succeed")
        .expect("mapping should exist");
    assert_eq!(stored_mapping.key_id, "key-gemini-admin-ok");
    assert_eq!(stored_mapping.user_id, None);
    assert_eq!(
        stored_mapping.display_name.as_deref(),
        Some("admin-upload.txt")
    );
    assert_eq!(stored_mapping.mime_type.as_deref(), Some("text/plain"));
    assert!(gemini_file_mapping_repository
        .find_by_file_name("files/admin-upload-fail")
        .await
        .expect("mapping lookup should succeed")
        .is_none());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}
