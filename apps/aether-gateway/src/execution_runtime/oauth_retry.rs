use aether_contracts::ExecutionPlan;
use tracing::warn;

use crate::orchestration::{
    local_failover_error_message, oauth_status_may_be_invalid as status_may_be_oauth_invalid,
    oauth_status_proves_access_token_invalid as status_proves_access_token_invalid,
};
use crate::state::{AgentIdentityAuthConfigFence, CodexRuntimeOAuthObservation};
use crate::{provider_transport::LocalOAuthRefreshError, AppState};

pub(crate) async fn refresh_oauth_plan_auth_for_retry(
    state: &AppState,
    plan: &mut ExecutionPlan,
    status_code: u16,
    response_text: Option<&str>,
    trace_id: &str,
    report_context: Option<&serde_json::Value>,
    request_started_at_unix_ms: Option<u64>,
    request_order_id: Option<&str>,
) -> bool {
    if !status_may_be_oauth_invalid(status_code, response_text) {
        return false;
    }
    let request_authorization = execution_plan_authorization(plan);
    let request_uses_agent_identity = request_authorization
        .is_some_and(aether_provider_transport::is_codex_agent_identity_authorization);
    let access_token_invalid_proven = !request_uses_agent_identity
        && status_proves_access_token_invalid(status_code, response_text);

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

    let current_uses_agent_identity =
        aether_provider_transport::is_codex_agent_identity_transport(&transport);
    if request_uses_agent_identity {
        if !current_uses_agent_identity
            || !aether_provider_transport::is_codex_agent_identity_invalid_task_response(
                status_code,
                response_text,
            )
            || !request_authorization.is_some_and(|authorization| {
                aether_provider_transport::codex_agent_identity_authorization_matches_transport(
                    &transport,
                    authorization,
                )
            })
        {
            return false;
        }
        if !matches!(
            state
                .capture_agent_identity_auth_config_fence(&transport)
                .await,
            Ok(AgentIdentityAuthConfigFence::Current(_))
        ) {
            return false;
        }
    } else if current_uses_agent_identity {
        // A bearer-token response cannot authorize refreshing an Agent Identity
        // installed under the same key id while the request was in flight.
        return false;
    } else if aether_provider_transport::supports_local_generic_oauth_request_auth_resolution(
        &transport,
    ) {
        if let Some(current_authorization) = generic_oauth_transport_authorization(&transport) {
            if !request_authorization.is_some_and(|authorization| {
                authorizations_use_same_access_token(authorization, &current_authorization)
            }) {
                replace_execution_plan_authorization(plan, current_authorization);
                return true;
            }
        }
    }

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
            let observed_credential_generation =
                report_context_string(report_context, "codex_credential_generation");
            let runtime_invalid_message = local_failover_error_message(response_text);
            let runtime_invalid_reason =
                aether_admin::provider::quota::codex_runtime_invalid_reason(
                    status_code,
                    runtime_invalid_message.as_deref(),
                );
            let persist_result = match (request_started_at_unix_ms, request_order_id) {
                (Some(request_started_at_unix_ms), Some(request_order_id))
                    if transport
                        .provider
                        .provider_type
                        .trim()
                        .eq_ignore_ascii_case("codex") =>
                {
                    state
                        .persist_local_oauth_refresh_failure_state_observed(
                            &transport,
                            refresh_status_code,
                            body_excerpt.as_str(),
                            access_token_invalid_proven,
                            CodexRuntimeOAuthObservation {
                                request_started_at_unix_ms,
                                request_order_id,
                                observed_credential_generation,
                                runtime_invalid_reason: runtime_invalid_reason.as_deref(),
                            },
                        )
                        .await
                }
                _ => {
                    state
                        .persist_local_oauth_refresh_failure_state(
                            &transport,
                            refresh_status_code,
                            body_excerpt.as_str(),
                            access_token_invalid_proven,
                        )
                        .await
                }
            };
            if let Err(err) = persist_result {
                warn!(
                    event_name = "local_oauth_retry_refresh_failure_persist_failed",
                    log_type = "ops",
                    trace_id = %trace_id,
                    provider_id = %plan.provider_id,
                    endpoint_id = %plan.endpoint_id,
                    key_id = %plan.key_id,
                    status_code,
                    refresh_status_code,
                    error = %crate::error::redact_error_debug(&err),
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
                // `LocalOAuthRefreshError` carries provider-generated details
                // in a few variants. Its `Debug` implementation deliberately
                // redacts those details; using `%` here would invoke
                // `Display` and could expose a raw transport URL/query or an
                // upstream error body in the ops log.
                error = ?err,
                "gateway oauth retry refresh failed"
            );
            false
        }
    }
}

fn report_context_string<'a>(
    report_context: Option<&'a serde_json::Value>,
    field: &str,
) -> Option<&'a str> {
    report_context
        .and_then(|context| context.get(field))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn execution_plan_authorization(plan: &ExecutionPlan) -> Option<&str> {
    plan.headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.as_str())
}

fn generic_oauth_transport_authorization(
    transport: &aether_provider_transport::GatewayProviderTransportSnapshot,
) -> Option<String> {
    aether_provider_transport::resolve_local_generic_oauth_transport_authorization(transport)
}

fn authorizations_use_same_access_token(left: &str, right: &str) -> bool {
    match (bearer_access_token(left), bearer_access_token(right)) {
        (Some(left), Some(right)) => left == right,
        _ => left.trim() == right.trim(),
    }
}

fn bearer_access_token(authorization: &str) -> Option<&str> {
    let mut parts = authorization.split_ascii_whitespace();
    let scheme = parts.next()?;
    let token = parts.next()?;
    (scheme.eq_ignore_ascii_case("bearer") && parts.next().is_none()).then_some(token)
}

fn replace_execution_plan_authorization(plan: &mut ExecutionPlan, authorization: String) {
    plan.headers
        .retain(|name, _| !name.eq_ignore_ascii_case("authorization"));
    plan.headers
        .insert("authorization".to_string(), authorization);
}

#[cfg(test)]
mod tests {
    use super::{
        refresh_oauth_plan_auth_for_retry, status_may_be_oauth_invalid,
        status_proves_access_token_invalid,
    };
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use aether_contracts::{ExecutionPlan, RequestBody};
    use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogKeyAdminCasUpdate, ProviderCatalogKeyOAuthCredentialFence,
        ProviderCatalogReadRepository, ProviderCatalogWriteRepository,
        StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
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
        assert!(status_may_be_oauth_invalid(
            403,
            Some("oauth_token_invalid")
        ));
        assert!(!status_may_be_oauth_invalid(403, None));
        assert!(!status_may_be_oauth_invalid(
            403,
            Some(
                r#"{"type":"error","error":{"type":"permission_error","message":"this token is not authorized for the workspace"}}"#
            )
        ));
        assert!(!status_may_be_oauth_invalid(
            403,
            Some(
                r#"{"error":{"type":"permission_error","message":"the authentication token has been invalidated for this workspace"}}"#
            )
        ));
        assert!(status_may_be_oauth_invalid(
            403,
            Some(
                r#"{"type":"error","error":{"type":"authentication_error","message":"credential expired"}}"#
            )
        ));
        assert!(status_may_be_oauth_invalid(
            403,
            Some(r#"{"error":{"type":"oauth_token_invalid","message":"sign in again"}}"#)
        ));
        assert!(status_may_be_oauth_invalid(
            403,
            Some(
                r#"{"error":{"code":"biscuit_baker_service_auth_credential_error_status","message":"Personal access token owner is inactive."}}"#
            )
        ));
        assert!(!status_may_be_oauth_invalid(
            403,
            Some(
                r#"{"error":{"type":"invalid_request_error","message":"Your authentication token has been invalidated. Please try signing in again."}}"#
            )
        ));
        assert!(!status_may_be_oauth_invalid(
            403,
            Some(
                r#"{"error":{"type":"invalid_request_error","message":"invalid request: token budget is invalid"}}"#
            )
        ));
        assert!(!status_may_be_oauth_invalid(403, Some("quota exceeded")));
        assert!(!status_may_be_oauth_invalid(
            403,
            Some("invalid request: max token budget is invalid")
        ));
        assert!(!status_may_be_oauth_invalid(
            403,
            Some("invalid_token_budget")
        ));
        assert!(!status_may_be_oauth_invalid(403, Some("not authorized")));
        assert!(!status_may_be_oauth_invalid(
            403,
            Some("authorization denied")
        ));
        assert!(!status_may_be_oauth_invalid(429, Some("token bucket")));
    }

    #[test]
    fn oauth_retry_failure_log_uses_redacted_error_debug() {
        let error = crate::provider_transport::LocalOAuthRefreshError::TransportMessage {
            provider_type: "kiro",
            message: "request failed for https://user:password@example.test/token?refresh_token=oauth-retry-canary"
                .to_string(),
        };

        // The retry path logs this error with `?error` (Debug), whose contract
        // is to omit dynamic transport details. Keep the assertion here so a
        // future change back to `%error` cannot silently reintroduce leakage.
        let debug = format!("{error:?}");
        assert!(!debug.contains("oauth-retry-canary"));
        assert!(!debug.contains("password"));
        assert!(debug.contains("TransportMessage"));
    }

    #[test]
    fn separates_retry_candidate_from_access_token_invalid_proof() {
        assert!(status_proves_access_token_invalid(401, None));
        assert!(status_proves_access_token_invalid(
            403,
            Some("The security token included in the request is expired")
        ));
        assert!(status_proves_access_token_invalid(
            403,
            Some(
                r#"{"error":{"code":"biscuit_baker_service_auth_credential_error_status","message":"Personal access token owner is inactive."}}"#
            )
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
    async fn retains_codex_key_after_request_proven_terminal_refresh_failure() {
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
            None,
            Some(1_000),
            Some("01900000-0000-7000-8000-000000000010"),
        )
        .await;

        assert!(!retried);
        assert_eq!(*token_hits.lock().expect("mutex should lock"), 1);
        let stored_key = provider_catalog_repository
            .list_keys_by_ids(&["key-codex-oauth-retry".to_string()])
            .await
            .expect("keys should read")
            .into_iter()
            .next()
            .expect("request-scoped refresh failure should retain the key");
        let invalid_reason = stored_key
            .oauth_invalid_reason
            .as_deref()
            .expect("combined invalid reason should persist");
        assert!(invalid_reason.contains("[OAUTH_EXPIRED]"));
        assert!(invalid_reason.contains("[REFRESH_FAILED]"));
        assert_eq!(
            stored_key
                .upstream_metadata
                .as_ref()
                .and_then(|metadata| metadata.pointer("/codex/oauth_state_request_id")),
            Some(&json!("01900000-0000-7000-8000-000000000010"))
        );

        token_handle.abort();
    }

    #[tokio::test]
    async fn stale_claude_code_request_reuses_rotated_access_token_without_second_refresh() {
        let refresh_hits = Arc::new(AtomicUsize::new(0));
        let refresh_hits_for_server = Arc::clone(&refresh_hits);
        let token_server = Router::new().route(
            "/oauth/token",
            post(move |_request: Request| {
                let hits = Arc::clone(&refresh_hits_for_server);
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "access_token": "fresh-claude-access-token",
                        "refresh_token": "fresh-claude-refresh-token",
                        "expires_in": 3600,
                        "token_type": "Bearer"
                    }))
                }
            }),
        );

        let provider = StoredProviderCatalogProvider::new(
            "provider-claude-code".to_string(),
            "Claude Code".to_string(),
            Some("https://api.anthropic.com".to_string()),
            "claude_code".to_string(),
        )
        .expect("provider should build");
        let endpoint = StoredProviderCatalogEndpoint::new(
            "endpoint-claude-code".to_string(),
            "provider-claude-code".to_string(),
            "claude:messages".to_string(),
            None,
            None,
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.anthropic.com".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build");
        let encrypted_api_key = encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            "stale-claude-access-token",
        )
        .expect("api key ciphertext should build");
        let encrypted_auth_config = encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"claude_code","access_token":"stale-claude-access-token","refresh_token":"stale-claude-refresh-token","expires_at":4102444800}"#,
        )
        .expect("auth config ciphertext should build");
        let mut key = StoredProviderCatalogKey::new(
            "key-claude-code".to_string(),
            "provider-claude-code".to_string(),
            "Claude OAuth".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!(["claude:messages"])),
            encrypted_api_key,
            Some(encrypted_auth_config),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build");
        key.expires_at_unix_secs = Some(4_102_444_800);

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
                        .with_token_url_for_tests(
                            "claude_code",
                            format!("{token_url}/oauth/token"),
                        ),
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
        let stale_transport = state
            .read_provider_transport_snapshot(
                "provider-claude-code",
                "endpoint-claude-code",
                "key-claude-code",
            )
            .await
            .expect("stale transport should load")
            .expect("stale transport should exist");
        let stale_plan = ExecutionPlan {
            request_id: "req-claude-oauth-fence".to_string(),
            candidate_id: None,
            provider_name: Some("claude_code".to_string()),
            provider_id: "provider-claude-code".to_string(),
            endpoint_id: "endpoint-claude-code".to_string(),
            key_id: "key-claude-code".to_string(),
            method: "POST".to_string(),
            url: "https://api.anthropic.com/v1/messages".to_string(),
            headers: BTreeMap::from([(
                "authorization".to_string(),
                "Bearer stale-claude-access-token".to_string(),
            )]),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "claude-sonnet-4-5"})),
            stream: false,
            client_api_format: "claude:messages".to_string(),
            provider_api_format: "claude:messages".to_string(),
            model_name: Some("claude-sonnet-4-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };

        let mut first_plan = stale_plan.clone();
        assert!(
            refresh_oauth_plan_auth_for_retry(
                &state,
                &mut first_plan,
                401,
                Some(r#"{"error":"invalid_token"}"#),
                "trace-claude-oauth-fence-first",
                None,
                None,
                None,
            )
            .await
        );
        assert_eq!(
            first_plan.headers.get("authorization").map(String::as_str),
            Some("Bearer fresh-claude-access-token")
        );
        assert_eq!(refresh_hits.load(Ordering::SeqCst), 1);

        let stale_force_result = state
            .force_local_oauth_refresh_entry(&stale_transport)
            .await
            .expect("stale force should reuse the persisted winner")
            .expect("stale force should return the winner entry");
        assert_eq!(
            stale_force_result.auth_header_value,
            "Bearer fresh-claude-access-token"
        );
        assert_eq!(refresh_hits.load(Ordering::SeqCst), 1);

        let mut stale_in_flight_plan = stale_plan;
        assert!(
            refresh_oauth_plan_auth_for_retry(
                &state,
                &mut stale_in_flight_plan,
                401,
                Some(r#"{"error":"invalid_token"}"#),
                "trace-claude-oauth-fence-stale",
                None,
                None,
                None,
            )
            .await
        );
        assert_eq!(
            stale_in_flight_plan
                .headers
                .get("authorization")
                .map(String::as_str),
            Some("Bearer fresh-claude-access-token")
        );
        assert_eq!(refresh_hits.load(Ordering::SeqCst), 1);

        let mut admin_replacement = provider_catalog_repository
            .list_keys_by_ids(&["key-claude-code".to_string()])
            .await
            .expect("Claude key should load")
            .pop()
            .expect("Claude key should exist");
        let expected_admin_replacement = admin_replacement.clone();
        admin_replacement.encrypted_api_key = Some(
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                "admin-claude-access-token",
            )
            .expect("admin access token should encrypt"),
        );
        admin_replacement.expires_at_unix_secs = Some(4_102_444_800);
        assert!(provider_catalog_repository
            .compare_and_update_key_admin_state(&ProviderCatalogKeyAdminCasUpdate {
                expected_encrypted_auth_config: expected_admin_replacement
                    .encrypted_auth_config
                    .clone(),
                expected_credential: ProviderCatalogKeyOAuthCredentialFence {
                    encrypted_api_key: expected_admin_replacement.encrypted_api_key.clone(),
                    auth_type: expected_admin_replacement.auth_type.clone(),
                    provider_id: expected_admin_replacement.provider_id.clone(),
                    provider_type: "claude_code".to_string(),
                },
                key: admin_replacement,
                codex_rotation: None,
                reset_oauth_runtime: true,
            })
            .await
            .expect("admin replacement CAS should run"));

        let admin_result = state
            .force_local_oauth_refresh_entry(&stale_transport)
            .await
            .expect("stale force should reuse the admin replacement")
            .expect("admin replacement should resolve");
        assert_eq!(
            admin_result.auth_header_value,
            "Bearer admin-claude-access-token"
        );
        assert_eq!(refresh_hits.load(Ordering::SeqCst), 1);

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
