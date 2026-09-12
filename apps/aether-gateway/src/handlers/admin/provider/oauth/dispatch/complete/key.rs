use super::super::super::duplicates::{
    acquire_codex_oauth_account_locks, find_duplicate_provider_oauth_key,
    release_codex_oauth_account_locks,
};
use super::super::super::errors::build_internal_control_error_response;
use super::super::super::provisioning::{
    provider_oauth_token_payload_expires_at_unix_secs, seed_provider_oauth_pool_score,
};
use super::super::super::runtime::{
    resolve_provider_oauth_runtime_endpoints,
    spawn_provider_oauth_account_state_refresh_after_update,
};
use super::super::super::state::{
    admin_provider_oauth_template, enrich_admin_provider_oauth_auth_config,
    is_fixed_provider_type_for_provider_oauth, json_non_empty_string,
};
use super::shared::{
    admin_provider_oauth_state_matches_principal, parse_admin_provider_oauth_complete_callback,
    parse_admin_provider_oauth_complete_request_body,
};
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_complete_key_id;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::shared::sync_provider_key_oauth_status_snapshot;
use crate::provider_key_auth::provider_key_is_oauth_managed;
use crate::GatewayError;
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogKeyOAuthCredentialFence, ProviderCatalogKeyOAuthRuntimeStateCasUpdate,
    ProviderCatalogUpstreamMetadataNamespaceExpectation,
};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

const CODEX_OAUTH_COMPLETE_NAMESPACE_CAS_MAX_RETRIES: usize = 3;
const CODEX_CREDENTIAL_GENERATION_KEY: &str = "credential_generation";

#[derive(Debug, PartialEq)]
enum CodexOAuthCompleteCasMissAction {
    AlreadyCompleted,
    RetryNamespace(Option<Value>),
    Conflict,
}

fn codex_oauth_complete_cas_miss_action(
    latest_encrypted_auth_config: Option<&str>,
    latest_upstream_metadata: Option<&Value>,
    latest_status_snapshot: Option<&Value>,
    expected_encrypted_auth_config: Option<&str>,
    persisted_encrypted_auth_config: &str,
    expected_codex_metadata_value: Option<&Value>,
    replacement_codex_metadata_value: &Value,
) -> CodexOAuthCompleteCasMissAction {
    let latest_codex_metadata_value = latest_upstream_metadata
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("codex"))
        .cloned();
    let quota_is_cleared = latest_status_snapshot
        .and_then(Value::as_object)
        .and_then(|snapshot| snapshot.get("quota"))
        == Some(&Value::Null);
    if latest_encrypted_auth_config == Some(persisted_encrypted_auth_config)
        && latest_codex_metadata_value.as_ref() == Some(replacement_codex_metadata_value)
        && quota_is_cleared
    {
        return CodexOAuthCompleteCasMissAction::AlreadyCompleted;
    }
    if latest_encrypted_auth_config != expected_encrypted_auth_config
        || latest_codex_metadata_value.as_ref() == expected_codex_metadata_value
    {
        return CodexOAuthCompleteCasMissAction::Conflict;
    }
    CodexOAuthCompleteCasMissAction::RetryNamespace(latest_codex_metadata_value)
}

pub(super) async fn handle_admin_provider_oauth_complete_key(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(key_id) = admin_provider_oauth_complete_key_id(request_context.path()) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Key 不存在",
        ));
    };
    let payload = match parse_admin_provider_oauth_complete_request_body(request_body) {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };
    let callback = match parse_admin_provider_oauth_complete_callback(&payload.callback_url) {
        Ok(callback) => callback,
        Err(response) => return Ok(response),
    };

    let preview = match state.load_provider_oauth_state(&callback.state_nonce).await {
        Ok(Some(state_data)) => state_data,
        Ok(None) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "state 无效或已过期",
            ));
        }
        Err(GatewayError::Client {
            status: http::StatusCode::BAD_REQUEST,
            ..
        }) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "state 无效或已过期",
            ));
        }
        Err(_) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "provider oauth redis unavailable",
            ));
        }
    };
    if preview.key_id != key_id
        || !admin_provider_oauth_state_matches_principal(&preview, request_context)
    {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "state 无效或已过期",
        ));
    }
    let state_data = match state
        .consume_provider_oauth_state(&callback.state_nonce)
        .await
    {
        Ok(Some(state_data)) if state_data == preview => state_data,
        Ok(Some(_)) | Ok(None) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "state 无效或已过期",
            ));
        }
        Err(GatewayError::Client {
            status: http::StatusCode::BAD_REQUEST,
            ..
        }) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "state 无效或已过期",
            ));
        }
        Err(_) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "provider oauth redis unavailable",
            ));
        }
    };

    let key = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
        .await?
        .into_iter()
        .next();
    let Some(key) = key else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Key 不存在",
        ));
    };
    if !state_data.provider_id.trim().is_empty() && state_data.provider_id != key.provider_id {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "state 无效或已过期",
        ));
    }
    if state_data.expected_encrypted_auth_config != key.encrypted_auth_config {
        return Ok(build_internal_control_error_response(
            http::StatusCode::CONFLICT,
            "授权期间 Key 认证信息已变更，请重新获取授权",
        ));
    }

    let provider_id = key.provider_id.clone();
    let provider = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next();
    let Some(provider) = provider else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if !provider_key_is_oauth_managed(&key, provider_type.as_str()) {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Key 不是 OAuth 管理账号",
        ));
    }
    if !is_fixed_provider_type_for_provider_oauth(&provider_type) {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Provider 不是固定类型，无法使用 provider-oauth",
        ));
    }
    if !state_data.provider_type.trim().is_empty()
        && !state_data
            .provider_type
            .eq_ignore_ascii_case(&provider_type)
    {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "state 无效或已过期",
        ));
    }
    let Some(template) = admin_provider_oauth_template(&provider_type) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Provider 不支持 OAuth 授权",
        ));
    };
    let endpoint_resolution =
        resolve_provider_oauth_runtime_endpoints(state, &provider, &provider_type).await?;
    let runtime_endpoint = endpoint_resolution.runtime_endpoint;
    let request_proxy = state
        .resolve_admin_provider_oauth_operation_proxy_snapshot(
            payload.proxy_node_id.as_deref(),
            &[
                key.proxy.as_ref(),
                runtime_endpoint
                    .as_ref()
                    .and_then(|endpoint| endpoint.proxy.as_ref()),
                provider.proxy.as_ref(),
            ],
        )
        .await;

    let token_payload = match state
        .exchange_admin_provider_oauth_code(
            template,
            &callback.code,
            &callback.state_nonce,
            state_data.pkce_verifier.as_deref(),
            request_proxy.clone(),
        )
        .await
    {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };

    let Some(access_token) = json_non_empty_string(token_payload.get("access_token")) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "token exchange 返回缺少 access_token",
        ));
    };
    let refresh_token = json_non_empty_string(token_payload.get("refresh_token"));
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let expires_at =
        provider_oauth_token_payload_expires_at_unix_secs(&token_payload, now_unix_secs);

    let mut auth_config = serde_json::Map::new();
    auth_config.insert("provider_type".to_string(), json!(provider_type.clone()));
    auth_config.insert("updated_at".to_string(), json!(now_unix_secs));
    if let Some(token_type) = token_payload.get("token_type").cloned() {
        auth_config.insert("token_type".to_string(), token_type);
    }
    if let Some(refresh_token) = refresh_token.as_ref() {
        auth_config.insert("refresh_token".to_string(), json!(refresh_token));
    }
    if let Some(expires_at) = expires_at {
        auth_config.insert("expires_at".to_string(), json!(expires_at));
    }
    if let Some(scope) = token_payload.get("scope").cloned() {
        auth_config.insert("scope".to_string(), scope);
    }
    enrich_admin_provider_oauth_auth_config(&provider_type, &mut auth_config, &token_payload);

    let Ok(encrypted_api_key) =
        state
            .app()
            .seal_provider_catalog_key_api_key(&provider_id, &key_id, &access_token)
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "provider oauth encryption unavailable",
        ));
    };
    let auth_config_json = serde_json::to_string(&serde_json::Value::Object(auth_config.clone()))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let Ok(encrypted_auth_config) =
        state
            .app()
            .seal_provider_catalog_key_auth_config(&provider_id, &key_id, &auth_config_json)
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "provider oauth encryption unavailable",
        ));
    };
    let codex_oauth_account_leases = if provider_type == "codex" {
        match acquire_codex_oauth_account_locks(state, &provider_id, &auth_config, "key-complete")
            .await
        {
            Ok(leases) => leases,
            Err(error) => {
                return Ok(build_internal_control_error_response(
                    error.status_code(),
                    error.detail(),
                ));
            }
        }
    } else {
        Vec::new()
    };
    if provider_type == "codex" {
        let duplicate = match state
            .find_duplicate_provider_oauth_key(&provider_id, &auth_config, Some(&key_id))
            .await
        {
            Ok(duplicate) => duplicate,
            Err(detail) => {
                release_codex_oauth_account_locks(state, codex_oauth_account_leases).await;
                return Ok(build_internal_control_error_response(
                    http::StatusCode::CONFLICT,
                    detail,
                ));
            }
        };
        if let Some(duplicate) = duplicate {
            release_codex_oauth_account_locks(state, codex_oauth_account_leases).await;
            return Ok(build_internal_control_error_response(
                http::StatusCode::CONFLICT,
                format!(
                    "该 ChatGPT 账号已存在于其他 Key（名称: {}）",
                    duplicate.name
                ),
            ));
        }
    }
    let mut recovered_key = key.clone();
    recovered_key.encrypted_api_key = Some(encrypted_api_key.clone());
    recovered_key.encrypted_auth_config = Some(encrypted_auth_config.clone());
    recovered_key.expires_at_unix_secs = expires_at;
    recovered_key.oauth_invalid_at_unix_secs = None;
    recovered_key.oauth_invalid_reason = None;
    recovered_key.updated_at_unix_secs = Some(now_unix_secs);
    recovered_key.status_snapshot = sync_provider_key_oauth_status_snapshot(
        recovered_key.status_snapshot.as_ref(),
        &recovered_key,
    );
    let oauth_status = recovered_key
        .status_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.get("oauth"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let mut expected_codex_metadata_value = key
        .upstream_metadata
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|metadata| metadata.get("codex"))
        .cloned();
    let mut status_snapshot_patch =
        serde_json::Map::from_iter([("oauth".to_string(), oauth_status)]);
    if provider_type == "codex" {
        status_snapshot_patch.insert("quota".to_string(), serde_json::Value::Null);
    }
    let persisted_encrypted_auth_config = recovered_key
        .encrypted_auth_config
        .clone()
        .expect("recovered auth config should be present");
    let replacement_codex_metadata_value = json!({
        CODEX_CREDENTIAL_GENERATION_KEY: uuid::Uuid::now_v7().to_string()
    });
    let expected_encrypted_auth_config = state_data.expected_encrypted_auth_config.clone();
    let expected_credential = ProviderCatalogKeyOAuthCredentialFence {
        encrypted_api_key: key.encrypted_api_key.clone(),
        auth_type: key.auth_type.clone(),
        provider_id: key.provider_id.clone(),
        provider_type: provider.provider_type.clone(),
    };
    let updated_result: Result<bool, GatewayError> = async {
        let max_namespace_retries = if provider_type == "codex" {
            CODEX_OAUTH_COMPLETE_NAMESPACE_CAS_MAX_RETRIES
        } else {
            0
        };
        for retry in 0..=max_namespace_retries {
            let updated = state
                .app()
                .compare_and_update_provider_catalog_key_oauth_runtime_state(
                    &ProviderCatalogKeyOAuthRuntimeStateCasUpdate {
                        key_id: key_id.clone(),
                        expected_encrypted_auth_config: expected_encrypted_auth_config.clone(),
                        expected_credential: Some(expected_credential.clone()),
                        expected_upstream_metadata_namespace: (provider_type == "codex").then(
                            || ProviderCatalogUpstreamMetadataNamespaceExpectation {
                                namespace: "codex".to_string(),
                                expected_value: expected_codex_metadata_value.clone(),
                            },
                        ),
                        encrypted_auth_config: persisted_encrypted_auth_config.clone(),
                        encrypted_api_key_update: Some(encrypted_api_key.clone()),
                        expires_at_unix_secs_update: Some(expires_at),
                        oauth_invalid_at_unix_secs: None,
                        oauth_invalid_reason: None,
                        reset_error_count: true,
                        upstream_metadata_patch: (provider_type == "codex")
                            .then(|| json!({"codex": replacement_codex_metadata_value.clone()})),
                        upstream_metadata_namespace_to_remove: None,
                        status_snapshot_patch: serde_json::Value::Object(
                            status_snapshot_patch.clone(),
                        ),
                        updated_at_unix_secs: Some(now_unix_secs),
                    },
                )
                .await?;
            if updated {
                return Ok(true);
            }
            if provider_type != "codex" {
                return Ok(false);
            }

            let Some(latest_key) = state
                .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
                .await?
                .into_iter()
                .next()
            else {
                return Ok(false);
            };
            match codex_oauth_complete_cas_miss_action(
                latest_key.encrypted_auth_config.as_deref(),
                latest_key.upstream_metadata.as_ref(),
                latest_key.status_snapshot.as_ref(),
                expected_encrypted_auth_config.as_deref(),
                &persisted_encrypted_auth_config,
                expected_codex_metadata_value.as_ref(),
                &replacement_codex_metadata_value,
            ) {
                CodexOAuthCompleteCasMissAction::AlreadyCompleted => return Ok(true),
                CodexOAuthCompleteCasMissAction::Conflict => return Ok(false),
                CodexOAuthCompleteCasMissAction::RetryNamespace(latest_codex_metadata_value) => {
                    if retry == max_namespace_retries {
                        return Ok(false);
                    }
                    expected_codex_metadata_value = latest_codex_metadata_value;
                }
            }
        }
        Ok(false)
    }
    .await;
    let _ = state
        .app()
        .invalidate_local_oauth_refresh_entry(&key_id)
        .await;
    let updated = match updated_result {
        Ok(updated) => updated,
        Err(error) => {
            release_codex_oauth_account_locks(state, codex_oauth_account_leases).await;
            return Err(error);
        }
    };
    if !updated {
        release_codex_oauth_account_locks(state, codex_oauth_account_leases).await;
        return Ok(build_internal_control_error_response(
            http::StatusCode::CONFLICT,
            "授权期间 Key 认证信息已变更，请重新获取授权",
        ));
    }
    let current_after_cas = match state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
        .await
    {
        Ok(keys) => keys.into_iter().next(),
        Err(error) => {
            release_codex_oauth_account_locks(state, codex_oauth_account_leases).await;
            return Err(error);
        }
    };
    let Some(current_after_cas) = current_after_cas else {
        release_codex_oauth_account_locks(state, codex_oauth_account_leases).await;
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Key 不存在",
        ));
    };
    if current_after_cas.encrypted_auth_config.as_deref()
        != Some(persisted_encrypted_auth_config.as_str())
    {
        release_codex_oauth_account_locks(state, codex_oauth_account_leases).await;
        return Ok(build_internal_control_error_response(
            http::StatusCode::CONFLICT,
            "授权期间 Key 认证信息已变更，请重新获取授权",
        ));
    }
    let recovered_key = match state
        .reset_provider_catalog_key_recovery_state_fenced(&key_id, &persisted_encrypted_auth_config)
        .await
    {
        Ok(recovered_key) => recovered_key,
        Err(error) => {
            release_codex_oauth_account_locks(state, codex_oauth_account_leases).await;
            return Err(error);
        }
    };
    let Some(recovered_key) = recovered_key else {
        let key_exists = match state
            .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
            .await
        {
            Ok(keys) => !keys.is_empty(),
            Err(error) => {
                release_codex_oauth_account_locks(state, codex_oauth_account_leases).await;
                return Err(error);
            }
        };
        release_codex_oauth_account_locks(state, codex_oauth_account_leases).await;
        if !key_exists {
            return Ok(build_internal_control_error_response(
                http::StatusCode::NOT_FOUND,
                "Key 不存在",
            ));
        }
        return Ok(build_internal_control_error_response(
            http::StatusCode::CONFLICT,
            "授权期间 Key 认证信息已变更，请重新获取授权",
        ));
    };
    seed_provider_oauth_pool_score(state, &provider.id, &recovered_key, now_unix_secs).await;
    release_codex_oauth_account_locks(state, codex_oauth_account_leases).await;

    spawn_provider_oauth_account_state_refresh_after_update(
        state.cloned_app(),
        provider.clone(),
        key_id.clone(),
        request_proxy.clone(),
    );

    Ok(Json(json!({
        "provider_type": provider_type,
        "expires_at": expires_at,
        "has_refresh_token": refresh_token.is_some(),
        "email": auth_config.get("email").cloned().unwrap_or(serde_json::Value::Null),
        "account_state_recheck_attempted": false,
        "account_state_recheck_error": serde_json::Value::Null,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::{codex_oauth_complete_cas_miss_action, CodexOAuthCompleteCasMissAction};
    use serde_json::json;

    #[test]
    fn codex_oauth_complete_retries_only_when_namespace_changed() {
        let expected_codex = json!({"request_id": "old"});
        let replacement_codex = json!({"credential_generation": "generation-new"});
        let latest_metadata = json!({
            "codex": {"request_id": "new"},
            "unrelated": {"preserved": true}
        });

        assert_eq!(
            codex_oauth_complete_cas_miss_action(
                Some("old-auth"),
                Some(&latest_metadata),
                None,
                Some("old-auth"),
                "new-auth",
                Some(&expected_codex),
                &replacement_codex,
            ),
            CodexOAuthCompleteCasMissAction::RetryNamespace(Some(json!({
                "request_id": "new"
            })))
        );
        assert_eq!(
            codex_oauth_complete_cas_miss_action(
                Some("old-auth"),
                Some(&json!({"codex": expected_codex.clone()})),
                None,
                Some("old-auth"),
                "new-auth",
                Some(&expected_codex),
                &replacement_codex,
            ),
            CodexOAuthCompleteCasMissAction::Conflict
        );
    }

    #[test]
    fn codex_oauth_complete_accepts_an_ambiguous_success_but_rejects_auth_rotation() {
        let replacement_codex = json!({"credential_generation": "generation-new"});
        assert_eq!(
            codex_oauth_complete_cas_miss_action(
                Some("new-auth"),
                Some(&json!({
                    "codex": replacement_codex.clone(),
                    "unrelated": {"preserved": true}
                })),
                Some(&json!({"quota": null})),
                Some("old-auth"),
                "new-auth",
                Some(&json!({"request_id": "old"})),
                &replacement_codex,
            ),
            CodexOAuthCompleteCasMissAction::AlreadyCompleted
        );
        assert_eq!(
            codex_oauth_complete_cas_miss_action(
                Some("other-auth"),
                Some(&json!({"codex": {"request_id": "new"}})),
                Some(&json!({"quota": null})),
                Some("old-auth"),
                "new-auth",
                Some(&json!({"request_id": "old"})),
                &replacement_codex,
            ),
            CodexOAuthCompleteCasMissAction::Conflict
        );
    }
}
