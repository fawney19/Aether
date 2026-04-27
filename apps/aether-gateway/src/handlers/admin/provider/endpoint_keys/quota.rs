use crate::handlers::admin::provider::shared::paths::admin_provider_id_for_refresh_quota;
use crate::handlers::admin::provider::shared::payloads::{
    AdminProviderQuotaRefreshRequest, OAUTH_ACCOUNT_BLOCK_PREFIX,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::collections::BTreeSet;

use super::super::oauth::quota::antigravity::refresh_antigravity_provider_quota_locally;
use super::super::oauth::quota::codex::refresh_codex_provider_quota_locally;
use super::super::oauth::quota::kiro::refresh_kiro_provider_quota_locally;
use super::super::oauth::quota::shared::normalize_string_id_list;

pub(super) async fn maybe_handle(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("endpoints_manage")
        || decision.route_kind.as_deref() != Some("refresh_quota")
        || request_context.method() != http::Method::POST
        || !request_context
            .path()
            .starts_with("/api/admin/endpoints/providers/")
        || !request_context.path().ends_with("/refresh-quota")
    {
        return Ok(None);
    }

    let Some(provider_id) = admin_provider_id_for_refresh_quota(request_context.path()) else {
        return Ok(Some(
            (
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": "Provider 不存在" })),
            )
                .into_response(),
        ));
    };
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(
            (
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
            )
                .into_response(),
        ));
    };

    let normalized_provider_type = provider.provider_type.trim().to_ascii_lowercase();

    let payload = if let Some(request_body) = request_body {
        match serde_json::from_slice::<AdminProviderQuotaRefreshRequest>(request_body) {
            Ok(payload) => payload,
            Err(_) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                    )
                        .into_response(),
                ));
            }
        }
    } else {
        AdminProviderQuotaRefreshRequest { key_ids: None }
    };

    let raw_key_ids = payload.key_ids;
    let selected_key_ids = normalize_string_id_list(raw_key_ids.clone());
    let explicit_key_ids_requested = raw_key_ids.is_some();
    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
        .await?;
    let endpoint = match normalized_provider_type.as_str() {
        "codex" => endpoints.into_iter().find(|endpoint| {
            endpoint.is_active
                && crate::ai_pipeline::is_openai_responses_format(&endpoint.api_format)
        }),
        "antigravity" => endpoints.into_iter().find(|endpoint| {
            endpoint.is_active
                && (endpoint
                    .api_format
                    .trim()
                    .eq_ignore_ascii_case("gemini:chat")
                    || endpoint
                        .api_format
                        .trim()
                        .eq_ignore_ascii_case("gemini:cli"))
        }),
        "kiro" => endpoints
            .iter()
            .find(|endpoint| {
                endpoint.is_active
                    && endpoint
                        .api_format
                        .trim()
                        .eq_ignore_ascii_case("claude:cli")
            })
            .cloned()
            .or_else(|| endpoints.into_iter().find(|endpoint| endpoint.is_active)),
        _ => return Ok(None),
    };

    let Some(endpoint) = endpoint else {
        let detail = match normalized_provider_type.as_str() {
            "codex" => "找不到有效的 openai:responses 端点",
            "antigravity" => "找不到有效的 gemini:chat/gemini:cli 端点",
            "kiro" => "找不到有效的 Kiro 端点",
            _ => "找不到有效端点",
        };
        return Ok(Some(
            (
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": detail })),
            )
                .into_response(),
        ));
    };

    let mut keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider_id))
        .await?;
    keys = if let Some(selected_key_ids) = selected_key_ids.as_ref() {
        if selected_key_ids.is_empty() {
            Vec::new()
        } else {
            let selected = selected_key_ids.iter().cloned().collect::<BTreeSet<_>>();
            keys.into_iter()
                .filter(|key| selected.contains(&key.id))
                .collect()
        }
    } else {
        keys.into_iter()
            .filter(|key| {
                key.is_active
                    || key
                        .oauth_invalid_reason
                        .as_deref()
                        .map(str::trim)
                        .is_some_and(|value| value.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX))
            })
            .collect()
    };

    if explicit_key_ids_requested && selected_key_ids.is_none() {
        return Ok(Some(
            Json(json!({
                "success": 0,
                "failed": 0,
                "total": 0,
                "results": [],
                "message": "未提供可刷新的 Key",
                "auto_removed": 0,
            }))
            .into_response(),
        ));
    }

    if keys.is_empty() {
        let message = if explicit_key_ids_requested {
            "未提供可刷新的 Key"
        } else {
            "没有可刷新的 Key"
        };
        return Ok(Some(
            Json(json!({
                "success": 0,
                "failed": 0,
                "total": 0,
                "results": [],
                "message": message,
                "auto_removed": 0,
            }))
            .into_response(),
        ));
    }

    let Some(payload) = (match normalized_provider_type.as_str() {
        "codex" => {
            refresh_codex_provider_quota_locally(state, &provider, &endpoint, keys, None).await?
        }
        "kiro" => {
            refresh_kiro_provider_quota_locally(state, &provider, &endpoint, keys, None).await?
        }
        "antigravity" => {
            refresh_antigravity_provider_quota_locally(state, &provider, &endpoint, keys, None)
                .await?
        }
        _ => None,
    }) else {
        return Ok(None);
    };
    Ok(Some(Json(payload).into_response()))
}
