use super::actions::{
    admin_provider_ops_is_valid_action_type, admin_provider_ops_local_action_response,
};
use super::config::{
    admin_provider_ops_config_object, admin_provider_ops_connector_object,
    admin_provider_ops_decrypted_credentials, admin_provider_ops_merge_credentials,
    build_admin_provider_ops_config_payload, build_admin_provider_ops_saved_config_value,
    build_admin_provider_ops_status_payload, resolve_admin_provider_ops_base_url,
};
use super::support::{
    AdminProviderOpsConnectRequest, AdminProviderOpsExecuteActionRequest,
    AdminProviderOpsSaveConfigRequest, ADMIN_PROVIDER_OPS_CONNECT_RUST_ONLY_MESSAGE,
};
use super::verify::{
    admin_provider_ops_local_verify_response, admin_provider_ops_normalized_verify_architecture_id,
    admin_provider_ops_verify_failure,
};
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::shared::paths::{
    admin_provider_id_for_provider_ops_balance, admin_provider_id_for_provider_ops_checkin,
    admin_provider_id_for_provider_ops_config, admin_provider_id_for_provider_ops_connect,
    admin_provider_id_for_provider_ops_disconnect, admin_provider_id_for_provider_ops_status,
    admin_provider_id_for_provider_ops_verify, admin_provider_ops_action_route_parts,
};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) async fn maybe_build_local_admin_provider_ops_providers_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("provider_ops_manage") {
        return Ok(None);
    }

    let route_kind = decision.route_kind.as_deref().unwrap_or_default();
    if !state.has_provider_catalog_data_reader() && route_kind != "disconnect_provider" {
        return Ok(None);
    }
    if route_kind == "batch_balance" {
        let requested_provider_ids = match request_body {
            Some(body) if !body.is_empty() => {
                let raw_value = match serde_json::from_slice::<serde_json::Value>(body) {
                    Ok(value) => value,
                    Err(_) => {
                        return Ok(Some(
                            (
                                http::StatusCode::BAD_REQUEST,
                                Json(json!({ "detail": "请求体必须是 provider_id 数组" })),
                            )
                                .into_response(),
                        ));
                    }
                };
                let ids = if let Some(items) = raw_value.as_array() {
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                        .collect::<Vec<_>>()
                } else if let Some(items) = raw_value
                    .get("provider_ids")
                    .and_then(serde_json::Value::as_array)
                {
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                        .collect::<Vec<_>>()
                } else {
                    return Ok(Some(
                        (
                            http::StatusCode::BAD_REQUEST,
                            Json(json!({ "detail": "请求体必须是 provider_id 数组" })),
                        )
                            .into_response(),
                    ));
                };
                Some(ids)
            }
            _ => None,
        };

        let provider_ids = if let Some(provider_ids) = requested_provider_ids {
            provider_ids
        } else {
            state
                .list_provider_catalog_providers(true)
                .await?
                .into_iter()
                .filter(|provider| {
                    provider
                        .config
                        .as_ref()
                        .and_then(serde_json::Value::as_object)
                        .is_some_and(|config| config.contains_key("provider_ops"))
                })
                .map(|provider| provider.id)
                .collect::<Vec<_>>()
        };

        if provider_ids.is_empty() {
            return Ok(Some(Json(json!({})).into_response()));
        }

        let providers = state
            .read_provider_catalog_providers_by_ids(&provider_ids)
            .await?;
        let endpoints = state
            .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
            .await?;
        let mut payload = serde_json::Map::new();
        for provider_id in &provider_ids {
            let provider = providers
                .iter()
                .find(|provider| provider.id == *provider_id);
            let provider_endpoints = endpoints
                .iter()
                .filter(|endpoint| endpoint.provider_id == *provider_id)
                .cloned()
                .collect::<Vec<_>>();
            let result = admin_provider_ops_local_action_response(
                state,
                provider_id,
                provider,
                &provider_endpoints,
                "query_balance",
                None,
            )
            .await;
            payload.insert(provider_id.clone(), result);
        }
        return Ok(Some(
            Json(serde_json::Value::Object(payload)).into_response(),
        ));
    }

    let action_route = if route_kind == "execute_provider_action" {
        admin_provider_ops_action_route_parts(&request_context.request_path)
    } else {
        None
    };
    let provider_id = if matches!(
        route_kind,
        "get_provider_status"
            | "get_provider_config"
            | "save_provider_config"
            | "delete_provider_config"
            | "verify_provider"
            | "connect_provider"
            | "disconnect_provider"
            | "get_provider_balance"
            | "refresh_provider_balance"
            | "provider_checkin"
            | "execute_provider_action"
    ) {
        admin_provider_id_for_provider_ops_config(&request_context.request_path)
            .or_else(|| admin_provider_id_for_provider_ops_status(&request_context.request_path))
            .or_else(|| admin_provider_id_for_provider_ops_verify(&request_context.request_path))
            .or_else(|| admin_provider_id_for_provider_ops_connect(&request_context.request_path))
            .or_else(|| admin_provider_id_for_provider_ops_balance(&request_context.request_path))
            .or_else(|| admin_provider_id_for_provider_ops_checkin(&request_context.request_path))
            .or_else(|| {
                action_route
                    .as_ref()
                    .map(|(provider_id, _)| provider_id.clone())
            })
            .or_else(|| {
                admin_provider_id_for_provider_ops_disconnect(&request_context.request_path)
            })
    } else {
        None
    };
    let Some(provider_id) = provider_id else {
        return Ok(None);
    };

    if decision.route_kind.as_deref() != Some(route_kind) {
        return Ok(None);
    }

    if route_kind == "save_provider_config" {
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体不能为空" })),
                )
                    .into_response(),
            ));
        };
        let raw_value = match serde_json::from_slice::<serde_json::Value>(request_body) {
            Ok(value) => value,
            Err(_) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                    )
                        .into_response(),
                ));
            }
        };
        if !raw_value.is_object() {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                )
                    .into_response(),
            ));
        }
        let payload = match serde_json::from_value::<AdminProviderOpsSaveConfigRequest>(raw_value) {
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
        };
        let Some(existing_provider) = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        let provider_ops_config =
            match build_admin_provider_ops_saved_config_value(state, &existing_provider, payload) {
                Ok(config) => config,
                Err(detail) => {
                    return Ok(Some(
                        (
                            http::StatusCode::BAD_REQUEST,
                            Json(json!({ "detail": detail })),
                        )
                            .into_response(),
                    ));
                }
            };
        let mut updated_provider = existing_provider.clone();
        let mut provider_config = updated_provider
            .config
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        provider_config.insert("provider_ops".to_string(), provider_ops_config);
        updated_provider.config = Some(serde_json::Value::Object(provider_config));
        updated_provider.updated_at_unix_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs());
        let Some(_updated) = state
            .update_provider_catalog_provider(&updated_provider)
            .await?
        else {
            return Ok(None);
        };
        return Ok(Some(
            Json(json!({
                "success": true,
                "message": "配置保存成功",
            }))
            .into_response(),
        ));
    }

    if route_kind == "verify_provider" {
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体不能为空" })),
                )
                    .into_response(),
            ));
        };
        let raw_value = match serde_json::from_slice::<serde_json::Value>(request_body) {
            Ok(value) => value,
            Err(_) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                    )
                        .into_response(),
                ));
            }
        };
        if !raw_value.is_object() {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                )
                    .into_response(),
            ));
        }
        let payload = match serde_json::from_value::<AdminProviderOpsSaveConfigRequest>(raw_value) {
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
        };

        let existing_provider = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
            .await?
            .into_iter()
            .next();
        let endpoints = if existing_provider.is_some() {
            state
                .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
                .await?
        } else {
            Vec::new()
        };
        let base_url = payload
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                existing_provider.as_ref().and_then(|provider| {
                    resolve_admin_provider_ops_base_url(
                        provider,
                        &endpoints,
                        admin_provider_ops_config_object(provider),
                    )
                })
            });
        let Some(base_url) = base_url else {
            return Ok(Some(
                Json(admin_provider_ops_verify_failure("请提供 API 地址")).into_response(),
            ));
        };

        let architecture_id =
            admin_provider_ops_normalized_verify_architecture_id(&payload.architecture_id);

        let credentials = existing_provider.as_ref().map_or_else(
            || payload.connector.credentials.clone(),
            |provider| {
                admin_provider_ops_merge_credentials(
                    state,
                    provider,
                    payload.connector.credentials.clone(),
                )
            },
        );
        let payload = admin_provider_ops_local_verify_response(
            state,
            &base_url,
            architecture_id,
            &payload.connector.config,
            &credentials,
        )
        .await;
        return Ok(Some(attach_admin_audit_response(
            Json(payload).into_response(),
            "admin_provider_ops_config_verified",
            "verify_provider_ops_config",
            "provider",
            &provider_id,
        )));
    }

    if route_kind == "connect_provider" {
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体不能为空" })),
                )
                    .into_response(),
            ));
        };
        let raw_value = match serde_json::from_slice::<serde_json::Value>(request_body) {
            Ok(value) => value,
            Err(_) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                    )
                        .into_response(),
                ));
            }
        };
        if !raw_value.is_object() {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                )
                    .into_response(),
            ));
        }
        let payload = match serde_json::from_value::<AdminProviderOpsConnectRequest>(raw_value) {
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
        };

        let Some(existing_provider) = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(provider_ops_config) = admin_provider_ops_config_object(&existing_provider) else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "未配置操作设置" })),
                )
                    .into_response(),
            ));
        };
        let endpoints = state
            .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
            .await?;
        if resolve_admin_provider_ops_base_url(
            &existing_provider,
            &endpoints,
            Some(provider_ops_config),
        )
        .is_none()
        {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "Provider 未配置 base_url" })),
                )
                    .into_response(),
            ));
        }

        let actual_credentials = payload
            .credentials
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                admin_provider_ops_decrypted_credentials(
                    state,
                    admin_provider_ops_config_object(&existing_provider)
                        .and_then(admin_provider_ops_connector_object)
                        .and_then(|connector| connector.get("credentials")),
                )
            });
        if actual_credentials.is_empty() {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "未提供凭据" })),
                )
                    .into_response(),
            ));
        }

        return Ok(Some(
            (
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": ADMIN_PROVIDER_OPS_CONNECT_RUST_ONLY_MESSAGE })),
            )
                .into_response(),
        ));
    }

    if matches!(
        route_kind,
        "get_provider_balance"
            | "refresh_provider_balance"
            | "provider_checkin"
            | "execute_provider_action"
    ) {
        let action_type = if route_kind == "provider_checkin" {
            "checkin".to_string()
        } else if matches!(
            route_kind,
            "get_provider_balance" | "refresh_provider_balance"
        ) {
            "query_balance".to_string()
        } else {
            let Some((_, action_type)) = action_route else {
                return Ok(None);
            };
            if !admin_provider_ops_is_valid_action_type(&action_type) {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": format!("无效的操作类型: {action_type}") })),
                    )
                        .into_response(),
                ));
            }
            action_type
        };

        let request_config = if route_kind == "execute_provider_action" {
            match request_body {
                Some(body) if !body.is_empty() => {
                    let raw_value = match serde_json::from_slice::<serde_json::Value>(body) {
                        Ok(value) => value,
                        Err(_) => {
                            return Ok(Some(
                                (
                                    http::StatusCode::BAD_REQUEST,
                                    Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                                )
                                    .into_response(),
                            ));
                        }
                    };
                    let payload = match serde_json::from_value::<AdminProviderOpsExecuteActionRequest>(
                        raw_value,
                    ) {
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
                    };
                    payload.config
                }
                _ => None,
            }
        } else {
            None
        };

        let providers = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
            .await?;
        let provider = providers.first();
        let endpoints = if provider.is_some() {
            state
                .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
                .await?
        } else {
            Vec::new()
        };
        let payload = admin_provider_ops_local_action_response(
            state,
            &provider_id,
            provider,
            &endpoints,
            &action_type,
            request_config.as_ref(),
        )
        .await;
        return Ok(Some(Json(payload).into_response()));
    }

    if route_kind == "delete_provider_config" {
        let Some(existing_provider) = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        let mut updated_provider = existing_provider.clone();
        let mut provider_config = updated_provider
            .config
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        if provider_config.remove("provider_ops").is_some() {
            updated_provider.config = Some(serde_json::Value::Object(provider_config));
            updated_provider.updated_at_unix_secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|duration| duration.as_secs());
            let Some(_updated) = state
                .update_provider_catalog_provider(&updated_provider)
                .await?
            else {
                return Ok(None);
            };
        }
        return Ok(Some(
            Json(json!({
                "success": true,
                "message": "配置已删除",
            }))
            .into_response(),
        ));
    }

    if route_kind == "disconnect_provider" {
        return Ok(Some(
            Json(json!({
                "success": true,
                "message": "已断开连接",
            }))
            .into_response(),
        ));
    }

    let providers = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?;
    let provider = providers.first();
    let endpoints = if route_kind == "get_provider_config" && provider.is_some() {
        state
            .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
            .await?
    } else {
        Vec::new()
    };

    let payload = if route_kind == "get_provider_status" {
        build_admin_provider_ops_status_payload(&provider_id, provider)
    } else {
        build_admin_provider_ops_config_payload(state, &provider_id, provider, &endpoints)
    };

    let response = Json(payload).into_response();
    let response = if route_kind == "get_provider_config" {
        attach_admin_audit_response(
            response,
            "admin_provider_ops_config_viewed",
            "view_provider_ops_config",
            "provider",
            &provider_id,
        )
    } else if route_kind == "get_provider_status" {
        attach_admin_audit_response(
            response,
            "admin_provider_ops_status_viewed",
            "view_provider_ops_status",
            "provider",
            &provider_id,
        )
    } else {
        response
    };

    Ok(Some(response))
}
