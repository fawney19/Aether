use aether_contracts::ExecutionPlan;
use tracing::warn;

use crate::{provider_transport::LocalOAuthRefreshError, AppState};

pub(crate) async fn refresh_oauth_plan_auth_for_retry(
    state: &AppState,
    plan: &mut ExecutionPlan,
    status_code: u16,
    response_text: Option<&str>,
    trace_id: &str,
) -> bool {
    if !status_may_be_oauth_invalid(status_code, response_text) {
        return false;
    }
    let access_token_invalid_proven =
        status_proves_access_token_invalid(status_code, response_text);

    let transport = match state
        .read_provider_transport_snapshot(&plan.provider_id, &plan.endpoint_id, &plan.key_id)
        .await
    {
        Ok(Some(transport)) => transport,
        Ok(None) => return false,
        Err(err) => {
            warn!(
                event_name = "local_oauth_retry_transport_read_failed",
                log_type = "ops",
                trace_id = %trace_id,
                provider_id = %plan.provider_id,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                error = ?err,
                "gateway failed to read transport before oauth retry refresh"
            );
            return false;
        }
    };

    if transport.key.decrypted_auth_config.is_none()
        && !transport.key.auth_type.trim().eq_ignore_ascii_case("oauth")
    {
        return false;
    }

    match state.force_local_oauth_refresh_entry(&transport).await {
        Ok(Some(entry)) => {
            let header_name = entry.auth_header_name.trim().to_ascii_lowercase();
            let header_value = entry.auth_header_value.trim();
            if header_name.is_empty() || header_value.is_empty() {
                return false;
            }
            plan.headers.insert(header_name, header_value.to_string());
            true
        }
        Ok(None) => false,
        Err(LocalOAuthRefreshError::HttpStatus {
            status_code: refresh_status_code,
            body_excerpt,
            ..
        }) if matches!(refresh_status_code, 400 | 401 | 403) => {
            if let Err(err) = state
                .persist_local_oauth_refresh_failure_state(
                    &transport,
                    refresh_status_code,
                    body_excerpt.as_str(),
                    access_token_invalid_proven,
                )
                .await
            {
                warn!(
                    event_name = "local_oauth_retry_refresh_failure_persist_failed",
                    log_type = "ops",
                    trace_id = %trace_id,
                    provider_id = %plan.provider_id,
                    endpoint_id = %plan.endpoint_id,
                    key_id = %plan.key_id,
                    status_code,
                    refresh_status_code,
                    error = ?err,
                    "gateway failed to persist oauth retry refresh failure"
                );
            }
            warn!(
                event_name = "local_oauth_retry_refresh_failed",
                log_type = "ops",
                trace_id = %trace_id,
                provider_id = %plan.provider_id,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                status_code,
                refresh_status_code,
                "gateway oauth retry refresh failed"
            );
            false
        }
        Err(err) => {
            warn!(
                event_name = "local_oauth_retry_refresh_failed",
                log_type = "ops",
                trace_id = %trace_id,
                provider_id = %plan.provider_id,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                status_code,
                error = %err,
                "gateway oauth retry refresh failed"
            );
            false
        }
    }
}

fn status_may_be_oauth_invalid(status_code: u16, response_text: Option<&str>) -> bool {
    if status_code == 401 {
        return true;
    }
    if status_code != 403 {
        return false;
    }

    let Some(response_text) = response_text else {
        return true;
    };
    let response_text = response_text.to_ascii_lowercase();
    ["oauth", "token", "auth", "credential", "expired"]
        .iter()
        .any(|needle| response_text.contains(needle))
}

fn status_proves_access_token_invalid(status_code: u16, response_text: Option<&str>) -> bool {
    if status_code == 401 {
        return true;
    }
    if status_code != 403 {
        return false;
    }

    let Some(response_text) = response_text else {
        return false;
    };
    let response_text = response_text.to_ascii_lowercase();
    [
        "oauth_token_invalid",
        "invalid_token",
        "invalid access token",
        "access token invalid",
        "access token expired",
        "expired access token",
        "authentication token has been invalidated",
        "token has been invalidated",
        "security token included in the request is expired",
    ]
    .iter()
    .any(|needle| response_text.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::{
        refresh_oauth_plan_auth_for_retry, status_may_be_oauth_invalid,
        status_proves_access_token_invalid,
    };
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use aether_contracts::{ExecutionPlan, RequestBody};
    use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogReadRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
        StoredProviderCatalogProvider,
    };
    use axum::routing::post;
    use axum::{extract::Request, Json, Router};
    use http::StatusCode;
    use serde_json::json;
    use tokio::task::JoinHandle;

    #[test]
    fn recognizes_oauth_invalid_statuses() {
        assert!(status_may_be_oauth_invalid(401, None));
        assert!(status_may_be_oauth_invalid(
            403,
            Some("The security token included in the request is expired")
        ));
        assert!(status_may_be_oauth_invalid(403, None));
        assert!(!status_may_be_oauth_invalid(403, Some("quota exceeded")));
        assert!(!status_may_be_oauth_invalid(429, Some("token bucket")));
    }

    #[test]
    fn separates_retry_candidate_from_access_token_invalid_proof() {
        assert!(status_proves_access_token_invalid(401, None));
        assert!(status_proves_access_token_invalid(
            403,
            Some("The security token included in the request is expired")
        ));
        assert!(!status_proves_access_token_invalid(403, None));
        assert!(!status_proves_access_token_invalid(
            403,
            Some("quota exceeded")
        ));
        assert!(!status_proves_access_token_invalid(
            429,
            Some("token bucket")
        ));
    }

    #[tokio::test]
    async fn auto_removes_request_proven_oauth_failure_after_terminal_refresh_failure() {
        let token_hits = Arc::new(Mutex::new(0usize));
        let token_hits_clone = Arc::clone(&token_hits);
        let token_server = Router::new().route(
            "/oauth/token",
            post(move |_request: Request| {
                let token_hits_inner = Arc::clone(&token_hits_clone);
                async move {
                    *token_hits_inner.lock().expect("mutex should lock") += 1;
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(json!({
                            "error": {
                                "message": "Your refresh token has already been used to generate a new access token. Please try signing in again.",
                                "type": "invalid_request_error",
                                "code": "refresh_token_reused"
                            }
                        })),
                    )
                }
            }),
        );

        let mut provider = StoredProviderCatalogProvider::new(
            "provider-codex".to_string(),
            "codex".to_string(),
            Some("https://example.com".to_string()),
            "codex".to_string(),
        )
        .expect("provider should build")
        .with_routing_fields(10);
        provider.config = Some(json!({
            "pool_advanced": {
                "auto_remove_banned_keys": true
            }
        }));

        let endpoint = StoredProviderCatalogEndpoint::new(
            "endpoint-codex-cli".to_string(),
            "provider-codex".to_string(),
            "openai:responses".to_string(),
            None,
            None,
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://chatgpt.com/backend-api/codex".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build");

        let encrypted_api_key =
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "stale-codex-token")
                .expect("api key ciphertext should build");
        let mut key = StoredProviderCatalogKey::new(
            "key-codex-oauth-retry".to_string(),
            "provider-codex".to_string(),
            "default".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!(["openai:responses"])),
            encrypted_api_key,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build");
        key.expires_at_unix_secs = Some(4_102_444_800);
        key.encrypted_auth_config = Some(
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                r#"{"provider_type":"codex","refresh_token":"used-refresh-token","email":"alice@example.com","account_id":"acct-codex-123","plan_type":"plus","expires_at":4102444800}"#,
            )
            .expect("auth config ciphertext should build"),
        );

        let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![provider],
            vec![endpoint],
            vec![key],
        ));

        let (token_url, token_handle) = start_test_server(token_server).await;
        let oauth_refresh =
            crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
                Arc::new(
                    crate::provider_transport::oauth_refresh::GenericOAuthRefreshAdapter::default()
                        .with_token_url_for_tests("codex", format!("{token_url}/oauth/token")),
                ),
            ]);
        let state = crate::AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository.clone(),
                )
                .with_encryption_key_for_tests(DEVELOPMENT_ENCRYPTION_KEY),
            )
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh);

        let mut plan = ExecutionPlan {
            request_id: "req-oauth-retry".to_string(),
            candidate_id: None,
            provider_name: Some("codex".to_string()),
            provider_id: "provider-codex".to_string(),
            endpoint_id: "endpoint-codex-cli".to_string(),
            key_id: "key-codex-oauth-retry".to_string(),
            method: "POST".to_string(),
            url: "https://chatgpt.com/backend-api/codex/responses".to_string(),
            headers: BTreeMap::from([(
                "authorization".to_string(),
                "Bearer stale-codex-token".to_string(),
            )]),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-5"})),
            stream: false,
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };

        let retried = refresh_oauth_plan_auth_for_retry(
            &state,
            &mut plan,
            401,
            Some(r#"{"error":"oauth_token_invalid"}"#),
            "trace-oauth-retry",
        )
        .await;

        assert!(!retried);
        assert_eq!(*token_hits.lock().expect("mutex should lock"), 1);
        let keys = provider_catalog_repository
            .list_keys_by_ids(&["key-codex-oauth-retry".to_string()])
            .await
            .expect("keys should read");
        assert!(keys.is_empty());

        token_handle.abort();
    }

    async fn start_test_server(router: Router) -> (String, JoinHandle<()>) {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("test server should bind");
        let addr = listener
            .local_addr()
            .expect("test server address should resolve");
        let handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("test server should serve");
        });
        (format!("http://{addr}"), handle)
    }
}
