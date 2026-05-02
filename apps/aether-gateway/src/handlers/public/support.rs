use super::{
    build_api_format_health_monitor_payload, build_public_auth_modules_status_payload,
    build_public_catalog_models_payload, build_public_catalog_search_models_payload,
    build_public_providers_payload, capability_detail_by_name, ldap_module_config_is_valid,
    serialize_public_capability, supported_capability_names, ApiFormatHealthMonitorOptions,
    PUBLIC_CAPABILITY_DEFINITIONS,
};
use crate::control::GatewayPublicRequestContext;
use crate::handlers::shared::{
    decrypt_catalog_secret_with_fallbacks, encrypt_catalog_secret_with_fallbacks,
    escape_admin_email_template_html, module_available_from_env, query_param_bool,
    query_param_optional_bool, query_param_value, read_admin_email_template_payload,
    render_admin_email_template_html, system_config_bool, system_config_string,
    unix_secs_to_rfc3339,
};
use crate::{AppState, GatewayError};
use aether_data_contracts::repository::global_models::PublicGlobalModelQuery;
use axum::body::{Body, Bytes};
use axum::http::{self, Response};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "support/announcements.rs"]
mod support_announcements;
#[path = "support/auth.rs"]
mod support_auth;
#[path = "support/dashboard.rs"]
mod support_dashboard;
#[path = "support/models.rs"]
mod support_models;
#[path = "support/monitoring.rs"]
mod support_monitoring;
#[path = "support/oauth.rs"]
mod support_oauth;
#[path = "support/payment.rs"]
mod support_payment;
#[path = "support/test_connection.rs"]
mod support_test_connection;
#[path = "support/user_me.rs"]
mod support_user_me;
#[path = "support/wallet.rs"]
mod support_wallet;

pub(crate) use self::support_announcements::maybe_build_local_admin_announcements_response;
pub(crate) use self::support_models::matches_model_mapping_for_models;

use self::support_announcements::{
    maybe_build_local_announcement_user_response, maybe_build_local_public_announcements_response,
};
use self::support_auth::auth_registration::{
    auth_password_policy_level, validate_auth_register_password,
};
use self::support_auth::auth_session::{
    build_auth_wallet_summary_payload, handle_auth_me, resolve_authenticated_local_user,
    AuthenticatedLocalUserContext,
};
use self::support_auth::{
    build_auth_error_response, build_auth_json_response, build_auth_registration_settings_payload,
    build_auth_settings_payload, extract_client_device_id, maybe_build_local_auth_response,
};
use self::support_dashboard::maybe_build_local_dashboard_response;
use self::support_models::{
    build_models_auth_error_response, maybe_build_local_models_response, models_api_format,
};
use self::support_monitoring::maybe_build_local_user_monitoring_response;
use self::support_oauth::maybe_build_local_oauth_response;
use self::support_payment::maybe_build_local_payment_callback_response;
use self::support_test_connection::maybe_build_local_test_connection_response;
use self::support_user_me::maybe_build_local_users_me_response;
use self::support_wallet::{
    maybe_build_local_wallet_response, sanitize_wallet_gateway_response,
    wallet_normalize_optional_string_field, wallet_payment_order_payload_from_row,
};

pub(crate) fn build_unhandled_public_support_response(
    request_context: &GatewayPublicRequestContext,
) -> Response<Body> {
    let decision = request_context
        .control_decision
        .as_ref()
        .expect("public support response requires control decision");
    (
        http::StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "detail": "public support route not implemented in rust frontdoor",
            "route_family": decision.route_family,
            "route_kind": decision.route_kind,
            "request_path": request_context.request_path,
        })),
    )
        .into_response()
}

pub(crate) async fn maybe_build_local_public_support_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
    request_body: Option<&Bytes>,
) -> Option<Response<Body>> {
    let decision = request_context.control_decision.as_ref()?;
    if decision.route_class.as_deref() != Some("public_support") {
        return None;
    }

    if decision.route_family.as_deref() == Some("auth") {
        return maybe_build_local_auth_response(state, request_context, headers, request_body)
            .await;
    }

    if decision.route_family.as_deref() == Some("oauth") {
        return maybe_build_local_oauth_response(state, request_context, headers, request_body)
            .await;
    }

    if decision.route_family.as_deref() == Some("dashboard") {
        return Some(maybe_build_local_dashboard_response(state, request_context, headers).await);
    }

    if decision.route_family.as_deref() == Some("monitoring_user") {
        return maybe_build_local_user_monitoring_response(state, request_context, headers).await;
    }

    if decision.route_family.as_deref() == Some("announcement_user") {
        return maybe_build_local_announcement_user_response(
            state,
            request_context,
            headers,
            request_body,
        )
        .await;
    }

    if decision.route_family.as_deref() == Some("wallet") {
        if let Some(response) =
            maybe_build_local_wallet_response(state, request_context, headers, request_body).await
        {
            return Some(response);
        }
        return Some(build_unhandled_public_support_response(request_context));
    }

    if decision.route_family.as_deref() == Some("users_me") {
        return maybe_build_local_users_me_response(state, request_context, headers, request_body)
            .await;
    }

    if decision.route_family.as_deref() == Some("payment_callback") {
        return maybe_build_local_payment_callback_response(
            state,
            request_context,
            headers,
            request_body,
        )
        .await;
    }

    if decision.route_family.as_deref() == Some("models") {
        if decision.auth_context.is_none() {
            return Some(build_models_auth_error_response(
                models_api_format(request_context).unwrap_or("openai:chat"),
            ));
        }
        if let Some(response) = maybe_build_local_models_response(state, request_context).await {
            return Some(response);
        }
    }

    if decision.route_family.as_deref() == Some("auth_public") {
        if decision.route_kind.as_deref() == Some("registration_settings")
            && request_context.request_path == "/api/auth/registration-settings"
        {
            let payload = build_auth_registration_settings_payload(state).await.ok()?;
            return Some(Json(payload).into_response());
        }

        if decision.route_kind.as_deref() == Some("settings")
            && request_context.request_path == "/api/auth/settings"
        {
            let payload = build_auth_settings_payload(state).await.ok()?;
            return Some(Json(payload).into_response());
        }
    }

    if decision.route_family.as_deref() == Some("announcements") {
        return maybe_build_local_public_announcements_response(state, request_context, headers)
            .await;
    }

    if decision.route_family.as_deref() == Some("public_catalog") {
        if decision.route_kind.as_deref() == Some("site_info")
            && request_context.request_path == "/api/public/site-info"
        {
            let site_name = state
                .read_system_config_json_value("site_name")
                .await
                .ok()
                .flatten()
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| "Aether".to_string());
            let site_subtitle = state
                .read_system_config_json_value("site_subtitle")
                .await
                .ok()
                .flatten()
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| "AI Gateway".to_string());
            return Some(
                Json(json!({
                    "site_name": site_name,
                    "site_subtitle": site_subtitle,
                }))
                .into_response(),
            );
        }

        if decision.route_kind.as_deref() == Some("providers")
            && request_context.request_path == "/api/public/providers"
        {
            let payload = build_public_providers_payload(
                state,
                request_context.request_query_string.as_deref(),
            )
            .await?;
            return Some(Json(payload).into_response());
        }

        if decision.route_kind.as_deref() == Some("models")
            && request_context.request_path == "/api/public/models"
        {
            let payload = build_public_catalog_models_payload(
                state,
                request_context.request_query_string.as_deref(),
            )
            .await?;
            return Some(Json(payload).into_response());
        }

        if decision.route_kind.as_deref() == Some("search_models")
            && request_context.request_path == "/api/public/search/models"
        {
            let payload = build_public_catalog_search_models_payload(
                state,
                request_context.request_query_string.as_deref(),
            )
            .await?;
            return Some(Json(payload).into_response());
        }

        if decision.route_kind.as_deref() == Some("stats")
            && request_context.request_path == "/api/public/stats"
        {
            let providers = state
                .list_provider_catalog_providers(true)
                .await
                .ok()
                .unwrap_or_default();
            let active_providers = providers.len();
            let provider_ids = providers
                .iter()
                .map(|provider| provider.id.clone())
                .collect::<Vec<_>>();
            let endpoints = if provider_ids.is_empty() {
                Vec::new()
            } else {
                state
                    .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
                    .await
                    .ok()
                    .unwrap_or_default()
            };
            let mut supported_formats = endpoints
                .iter()
                .filter(|endpoint| endpoint.is_active)
                .map(|endpoint| endpoint.api_format.clone())
                .collect::<Vec<_>>();
            supported_formats.sort();
            supported_formats.dedup();

            let mut active_model_ids = std::collections::BTreeSet::new();
            if state.has_minimal_candidate_selection_reader() {
                for api_format in &supported_formats {
                    let rows = state
                        .list_minimal_candidate_selection_rows_for_api_format(api_format)
                        .await
                        .ok()
                        .unwrap_or_default();
                    for row in rows {
                        active_model_ids.insert(row.model_id);
                    }
                }
            }
            let active_models = active_model_ids.len();

            return Some(
                Json(json!({
                    "total_providers": active_providers,
                    "active_providers": active_providers,
                    "total_models": active_models,
                    "active_models": active_models,
                    "supported_formats": supported_formats,
                }))
                .into_response(),
            );
        }

        if decision.route_kind.as_deref() == Some("global_models")
            && request_context.request_path == "/api/public/global-models"
        {
            if !state.has_global_model_data_reader() {
                return None;
            }

            let skip = query_param_value(request_context.request_query_string.as_deref(), "skip")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            let limit = query_param_value(request_context.request_query_string.as_deref(), "limit")
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0 && *value <= 1000)
                .unwrap_or(100);
            let is_active = query_param_optional_bool(
                request_context.request_query_string.as_deref(),
                "is_active",
            );
            let search =
                query_param_value(request_context.request_query_string.as_deref(), "search");

            let page = state
                .list_public_global_models(&PublicGlobalModelQuery {
                    offset: skip,
                    limit,
                    is_active,
                    search,
                })
                .await
                .ok()?;

            let models = page
                .items
                .into_iter()
                .map(|model| {
                    json!({
                        "id": model.id,
                        "name": model.name,
                        "display_name": model.display_name,
                        "is_active": model.is_active,
                        "default_price_per_request": model.default_price_per_request,
                        "default_tiered_pricing": model.default_tiered_pricing,
                        "supported_capabilities": model.supported_capabilities,
                        "config": model.config,
                        "usage_count": model.usage_count,
                    })
                })
                .collect::<Vec<_>>();

            return Some(
                Json(json!({
                    "models": models,
                    "total": page.total,
                }))
                .into_response(),
            );
        }

        if decision.route_kind.as_deref() == Some("health_api_formats")
            && request_context.request_path == "/api/public/health/api-formats"
        {
            let lookback_hours = query_param_value(
                request_context.request_query_string.as_deref(),
                "lookback_hours",
            )
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| (1..=168).contains(value))
            .unwrap_or(6);
            let per_format_limit = query_param_value(
                request_context.request_query_string.as_deref(),
                "per_format_limit",
            )
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| (10..=500).contains(value))
            .unwrap_or(100);
            let payload = build_api_format_health_monitor_payload(
                state,
                lookback_hours,
                per_format_limit,
                ApiFormatHealthMonitorOptions {
                    include_api_path: true,
                    include_provider_count: false,
                    include_key_count: false,
                },
            )
            .await?;
            return Some(Json(payload).into_response());
        }
    }

    if decision.route_family.as_deref() == Some("capabilities") {
        if decision.route_kind.as_deref() == Some("list")
            && request_context.request_path == "/api/capabilities"
        {
            return Some(
                Json(json!({
                    "capabilities": PUBLIC_CAPABILITY_DEFINITIONS
                        .iter()
                        .copied()
                        .map(serialize_public_capability)
                        .collect::<Vec<_>>(),
                }))
                .into_response(),
            );
        }

        if decision.route_kind.as_deref() == Some("user_configurable")
            && request_context.request_path == "/api/capabilities/user-configurable"
        {
            let capabilities = PUBLIC_CAPABILITY_DEFINITIONS
                .iter()
                .copied()
                .filter(|capability| capability.config_mode == "user_configurable")
                .map(serialize_public_capability)
                .collect::<Vec<_>>();
            return Some(Json(json!({ "capabilities": capabilities })).into_response());
        }

        if decision.route_kind.as_deref() == Some("model")
            && request_context
                .request_path
                .starts_with("/api/capabilities/model/")
        {
            let model_name = request_context
                .request_path
                .trim_start_matches("/api/capabilities/model/")
                .trim();
            if model_name.is_empty() {
                return Some(
                    Json(json!({
                        "model": "",
                        "supported_capabilities": [],
                        "capability_details": [],
                        "error": "模型不存在",
                    }))
                    .into_response(),
                );
            }

            let model = state
                .get_public_global_model_by_name(model_name)
                .await
                .ok()
                .flatten();
            let Some(model) = model else {
                return Some(
                    Json(json!({
                        "model": model_name,
                        "supported_capabilities": [],
                        "capability_details": [],
                        "error": "模型不存在",
                    }))
                    .into_response(),
                );
            };

            let supported_capabilities =
                supported_capability_names(model.supported_capabilities.as_ref());
            let capability_details = supported_capabilities
                .iter()
                .filter_map(|capability| capability_detail_by_name(capability))
                .collect::<Vec<_>>();
            return Some(
                Json(json!({
                    "model": model_name,
                    "global_model_id": model.id,
                    "global_model_name": model.name,
                    "supported_capabilities": supported_capabilities,
                    "capability_details": capability_details,
                }))
                .into_response(),
            );
        }
    }

    if decision.route_family.as_deref() == Some("modules") {
        if decision.route_kind.as_deref() == Some("auth_status")
            && request_context.request_path == "/api/modules/auth-status"
        {
            let payload = build_public_auth_modules_status_payload(state).await.ok()?;
            return Some(Json(payload).into_response());
        }
    }

    if decision.route_family.as_deref() == Some("system_catalog") {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_string());

        if decision.route_kind.as_deref() == Some("health")
            && request_context.request_path == "/health"
        {
            return Some(
                Json(json!({
                    "status": "healthy",
                    "timestamp": timestamp,
                    "database_pool": {
                        "checked_out": 0,
                        "pool_size": 0,
                        "overflow": 0,
                        "max_capacity": 0,
                        "usage_rate": "0.0%",
                        "source": "rust_frontdoor",
                    },
                }))
                .into_response(),
            );
        }

        if decision.route_kind.as_deref() == Some("health")
            && request_context.request_path == "/v1/health"
        {
            let active_providers = state
                .list_provider_catalog_providers(true)
                .await
                .map(|providers| providers.len())
                .unwrap_or(0);
            return Some(
                Json(json!({
                    "status": "ok",
                    "timestamp": timestamp,
                    "stats": {
                        "active_providers": active_providers,
                        "active_models": 0,
                    },
                    "dependencies": {
                        "database": {
                            "status": if state.has_data_backends() { "ok" } else { "degraded" },
                        },
                        "redis": {
                            "status": if state.has_redis_data_backend() { "ok" } else { "degraded" },
                        },
                    },
                }))
                .into_response(),
            );
        }

        if decision.route_kind.as_deref() == Some("root") && request_context.request_path == "/" {
            let providers = state
                .list_provider_catalog_providers(true)
                .await
                .ok()
                .unwrap_or_default();
            let current_provider = providers
                .first()
                .map(|provider| provider.name.clone())
                .unwrap_or_else(|| "None".to_string());
            return Some(
                Json(json!({
                    "message": "AI Proxy with Modular Architecture v4.0.0",
                    "status": "running",
                    "current_provider": current_provider,
                    "available_providers": providers.len(),
                    "config": {},
                    "endpoints": {
                        "messages": "/v1/messages",
                        "count_tokens": "/v1/messages/count_tokens",
                        "health": "/v1/health",
                        "providers": "/v1/providers",
                        "test_connection": "/v1/test-connection",
                    },
                }))
                .into_response(),
            );
        }

        if decision.route_kind.as_deref() == Some("providers")
            && request_context.request_path == "/v1/providers"
        {
            if !state.has_provider_catalog_data_reader() {
                return None;
            }
            let include_models = query_param_bool(
                request_context.request_query_string.as_deref(),
                "include_models",
                false,
            );
            let include_endpoints = query_param_bool(
                request_context.request_query_string.as_deref(),
                "include_endpoints",
                false,
            );
            let active_only = query_param_bool(
                request_context.request_query_string.as_deref(),
                "active_only",
                true,
            );
            if include_models {
                return None;
            }
            let providers = state
                .list_provider_catalog_providers(active_only)
                .await
                .ok()
                .unwrap_or_default();
            let provider_ids = providers
                .iter()
                .map(|provider| provider.id.clone())
                .collect::<Vec<_>>();
            let endpoints = if include_endpoints {
                state
                    .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
                    .await
                    .ok()
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            return Some(
                Json(json!({
                    "providers": providers
                        .into_iter()
                        .map(|provider| {
                            let provider_id = provider.id.clone();
                            let provider_name = provider.name.clone();
                            let mut payload = json!({
                                "id": provider_id.clone(),
                                "name": provider_name,
                                "is_active": provider.is_active,
                                "provider_priority": provider.provider_priority,
                            });
                            if include_endpoints {
                                payload["endpoints"] = serde_json::Value::Array(
                                    endpoints
                                        .iter()
                                        .filter(|endpoint| endpoint.provider_id == provider_id)
                                        .map(|endpoint| json!({
                                            "id": endpoint.id,
                                            "base_url": endpoint.base_url,
                                            "api_format": endpoint.api_format,
                                            "is_active": endpoint.is_active,
                                        }))
                                        .collect(),
                                );
                            }
                            payload
                        })
                        .collect::<Vec<_>>(),
                }))
                .into_response(),
            );
        }

        if decision.route_kind.as_deref() == Some("provider_detail")
            && request_context.request_path.starts_with("/v1/providers/")
        {
            let include_models = query_param_bool(
                request_context.request_query_string.as_deref(),
                "include_models",
                false,
            );
            let include_endpoints = query_param_bool(
                request_context.request_query_string.as_deref(),
                "include_endpoints",
                false,
            );
            if include_models {
                return None;
            }
            let provider_identifier = request_context
                .request_path
                .trim_start_matches("/v1/providers/")
                .trim();
            if provider_identifier.is_empty() {
                return None;
            }
            let provider = if let Ok(providers) = state
                .read_provider_catalog_providers_by_ids(&[provider_identifier.to_string()])
                .await
            {
                providers.into_iter().next()
            } else {
                None
            };
            let provider = match provider {
                Some(provider) => provider,
                None => state
                    .list_provider_catalog_providers(false)
                    .await
                    .ok()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|provider| provider.name == provider_identifier)?,
            };
            let provider_id = provider.id.clone();
            let provider_name = provider.name.clone();
            let mut payload = json!({
                "id": provider_id.clone(),
                "name": provider_name,
                "is_active": provider.is_active,
                "provider_priority": provider.provider_priority,
            });
            if include_endpoints {
                let endpoints = state
                    .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(
                        &provider_id,
                    ))
                    .await
                    .ok()
                    .unwrap_or_default();
                payload["endpoints"] = serde_json::Value::Array(
                    endpoints
                        .into_iter()
                        .map(|endpoint| {
                            json!({
                                "id": endpoint.id,
                                "base_url": endpoint.base_url,
                                "api_format": endpoint.api_format,
                                "is_active": endpoint.is_active,
                            })
                        })
                        .collect(),
                );
            }
            return Some(Json(payload).into_response());
        }

        if decision.route_kind.as_deref() == Some("test_connection")
            && request_context.request_path == "/v1/test-connection"
        {
            return maybe_build_local_test_connection_response(state, request_context).await;
        }

        if decision.route_kind.as_deref() == Some("test_connection")
            && request_context.request_path == "/test-connection"
        {
            return Some(
                (
                    http::StatusCode::GONE,
                    Json(json!({
                        "detail": "Deprecated endpoint. Please use /v1/test-connection.",
                    })),
                )
                    .into_response(),
            );
        }
    }

    None
}
