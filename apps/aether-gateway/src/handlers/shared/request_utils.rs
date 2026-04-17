use crate::constants::{
    CONTROL_EXECUTE_FALLBACK_HEADER, TRUSTED_ADMIN_MANAGEMENT_TOKEN_ID_HEADER,
    TRUSTED_ADMIN_SESSION_ID_HEADER, TRUSTED_ADMIN_USER_ID_HEADER, TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::control::GatewayControlDecision;
use crate::control::GatewayPublicRequestContext;
use crate::headers::header_value_str;
use crate::tunnel::TUNNEL_ROUTE_FAMILY;
use axum::http::{self, HeaderName};
use chrono::{SecondsFormat, Utc};
use url::form_urlencoded;

use super::OFFICIAL_EXTERNAL_MODEL_PROVIDERS;

pub(crate) fn rust_auth_terminates_provider_credentials(
    decision: Option<&GatewayControlDecision>,
) -> bool {
    decision.is_some_and(|decision| {
        decision.route_class.as_deref() == Some("ai_public") && decision.auth_context.is_some()
    })
}

pub(crate) fn mark_external_models_official_providers(
    value: &serde_json::Value,
) -> Option<serde_json::Value> {
    let providers = value.as_object()?;
    Some(serde_json::Value::Object(
        providers
            .iter()
            .map(|(provider_id, provider_value)| {
                let updated = match provider_value.as_object() {
                    Some(object) => {
                        let mut cloned = object.clone();
                        cloned.insert(
                            "official".to_string(),
                            serde_json::Value::Bool(
                                OFFICIAL_EXTERNAL_MODEL_PROVIDERS
                                    .iter()
                                    .any(|value| value == provider_id),
                            ),
                        );
                        serde_json::Value::Object(cloned)
                    }
                    None => provider_value.clone(),
                };
                (provider_id.clone(), updated)
            })
            .collect(),
    ))
}

pub(crate) fn should_strip_forwarded_provider_credential_header(
    decision: Option<&GatewayControlDecision>,
    header_name: &HeaderName,
) -> bool {
    if !rust_auth_terminates_provider_credentials(decision) {
        return false;
    }

    matches!(
        header_name.as_str(),
        "authorization" | "x-api-key" | "api-key" | "x-goog-api-key"
    )
}

pub(crate) fn should_strip_forwarded_trusted_admin_header(
    decision: Option<&GatewayControlDecision>,
    header_name: &HeaderName,
) -> bool {
    let Some(decision) = decision else {
        return false;
    };
    if decision.route_class.as_deref() != Some("admin_proxy") {
        return false;
    }

    matches!(
        header_name.as_str(),
        TRUSTED_ADMIN_USER_ID_HEADER
            | TRUSTED_ADMIN_USER_ROLE_HEADER
            | TRUSTED_ADMIN_SESSION_ID_HEADER
            | TRUSTED_ADMIN_MANAGEMENT_TOKEN_ID_HEADER
    )
}

pub(crate) fn sanitize_upstream_path_and_query(
    decision: Option<&GatewayControlDecision>,
    default_path_and_query: &str,
) -> String {
    let base = decision
        .map(GatewayControlDecision::proxy_path_and_query)
        .unwrap_or_else(|| default_path_and_query.to_string());
    let Some(decision) = decision else {
        return base;
    };
    if !rust_auth_terminates_provider_credentials(Some(decision))
        || decision.route_family.as_deref() != Some("gemini")
    {
        return base;
    }

    strip_query_param(&base, "key")
}

pub(crate) fn strip_query_param(path_and_query: &str, key_to_strip: &str) -> String {
    let Some((path, query)) = path_and_query.split_once('?') else {
        return path_and_query.to_string();
    };

    let mut serializer = form_urlencoded::Serializer::new(String::new());
    let mut kept_any = false;
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        if key == key_to_strip {
            continue;
        }
        serializer.append_pair(key.as_ref(), value.as_ref());
        kept_any = true;
    }

    if !kept_any {
        return path.to_string();
    }

    format!("{path}?{}", serializer.finish())
}

pub(crate) fn query_param_bool(query: Option<&str>, key: &str, default: bool) -> bool {
    let Some(query) = query else {
        return default;
    };
    for (entry_key, value) in form_urlencoded::parse(query.as_bytes()) {
        if entry_key == key {
            let normalized = value.trim().to_ascii_lowercase();
            return matches!(normalized.as_str(), "1" | "true" | "yes" | "on");
        }
    }
    default
}

pub(crate) fn query_param_optional_bool(query: Option<&str>, key: &str) -> Option<bool> {
    let query = query?;
    for (entry_key, value) in form_urlencoded::parse(query.as_bytes()) {
        if entry_key == key {
            let normalized = value.trim().to_ascii_lowercase();
            return match normalized.as_str() {
                "1" | "true" | "yes" | "on" => Some(true),
                "0" | "false" | "no" | "off" => Some(false),
                _ => None,
            };
        }
    }
    None
}

pub(crate) fn query_param_value(query: Option<&str>, key: &str) -> Option<String> {
    let query = query?;
    for (entry_key, value) in form_urlencoded::parse(query.as_bytes()) {
        if entry_key == key {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

pub(crate) fn request_enables_control_execute(headers: &http::HeaderMap) -> bool {
    header_value_str(headers, CONTROL_EXECUTE_FALLBACK_HEADER).is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

pub(crate) fn unix_secs_to_rfc3339(unix_secs: u64) -> Option<String> {
    let timestamp = i64::try_from(unix_secs).ok()?;
    Some(
        chrono::DateTime::<Utc>::from_timestamp(timestamp, 0)?
            .to_rfc3339_opts(SecondsFormat::Secs, true),
    )
}

pub(crate) fn unix_ms_to_rfc3339(unix_ms: u64) -> Option<String> {
    let secs = i64::try_from(unix_ms / 1000).ok()?;
    let nanos = ((unix_ms % 1000) * 1_000_000) as u32;
    Some(
        chrono::DateTime::<Utc>::from_timestamp(secs, nanos)?
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    )
}

pub(crate) fn json_string_list(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

pub(crate) fn admin_proxy_local_requires_buffered_body(
    request_context: &GatewayPublicRequestContext,
) -> bool {
    request_context
        .control_decision
        .as_ref()
        .is_some_and(|decision| {
            if decision.route_class.as_deref() != Some("admin_proxy") {
                return false;
            }

            match (
                decision.route_family.as_deref(),
                request_context.request_method.clone(),
                decision.route_kind.as_deref(),
            ) {
                (Some("endpoints_manage"), http::Method::POST, Some("create_provider_key"))
                | (Some("endpoints_manage"), http::Method::POST, Some("create_endpoint"))
                | (Some("endpoints_manage"), http::Method::POST, Some("batch_delete_keys"))
                | (Some("endpoints_manage"), http::Method::PUT, Some("update_key"))
                | (Some("endpoints_manage"), http::Method::PUT, Some("update_endpoint"))
                | (Some("modules_manage"), http::Method::PUT, Some("set_enabled"))
                | (Some("oauth_manage"), http::Method::PUT, Some("upsert_provider"))
                | (Some("oauth_manage"), http::Method::POST, Some("test_provider"))
                | (Some("provider_oauth_manage"), http::Method::POST, Some("complete_key_oauth"))
                | (
                    Some("provider_oauth_manage"),
                    http::Method::POST,
                    Some("complete_provider_oauth"),
                )
                | (
                    Some("provider_oauth_manage"),
                    http::Method::POST,
                    Some("import_refresh_token"),
                )
                | (Some("provider_oauth_manage"), http::Method::POST, Some("batch_import_oauth"))
                | (
                    Some("provider_oauth_manage"),
                    http::Method::POST,
                    Some("start_batch_import_oauth_task"),
                )
                | (Some("provider_oauth_manage"), http::Method::POST, Some("device_authorize"))
                | (Some("provider_oauth_manage"), http::Method::POST, Some("device_poll"))
                | (Some("system_manage"), http::Method::POST, Some("config_import"))
                | (Some("system_manage"), http::Method::POST, Some("users_import"))
                | (Some("system_manage"), http::Method::PUT, Some("settings_set"))
                | (Some("system_manage"), http::Method::PUT, Some("config_set"))
                | (Some("system_manage"), http::Method::PUT, Some("email_template_set"))
                | (Some("system_manage"), http::Method::POST, Some("email_template_preview"))
                | (
                    Some("provider_models_manage"),
                    http::Method::POST,
                    Some("create_provider_model"),
                )
                | (
                    Some("provider_models_manage"),
                    http::Method::PATCH,
                    Some("update_provider_model"),
                )
                | (
                    Some("provider_models_manage"),
                    http::Method::POST,
                    Some("batch_create_provider_models"),
                )
                | (
                    Some("provider_models_manage"),
                    http::Method::POST,
                    Some("assign_global_models"),
                )
                | (
                    Some("provider_models_manage"),
                    http::Method::POST,
                    Some("import_from_upstream"),
                )
                | (
                    Some("provider_ops_manage"),
                    http::Method::POST,
                    Some("execute_provider_action"),
                )
                | (Some("provider_ops_manage"), http::Method::POST, Some("batch_balance"))
                | (Some("provider_ops_manage"), http::Method::POST, Some("connect_provider"))
                | (Some("provider_ops_manage"), http::Method::POST, Some("verify_provider"))
                | (Some("provider_ops_manage"), http::Method::PUT, Some("save_provider_config"))
                | (Some("announcements_manage"), http::Method::POST, Some("create_announcement"))
                | (Some("announcements_manage"), http::Method::PUT, Some("update_announcement"))
                | (
                    Some("provider_strategy_manage"),
                    http::Method::PUT,
                    Some("update_provider_billing"),
                )
                | (
                    Some("provider_query_manage"),
                    http::Method::POST,
                    Some("query_models" | "test_model" | "test_model_failover"),
                )
                | (Some("billing_manage"), http::Method::POST, Some("apply_preset"))
                | (Some("billing_manage"), http::Method::POST, Some("create_rule"))
                | (Some("billing_manage"), http::Method::PUT, Some("update_rule"))
                | (Some("billing_manage"), http::Method::POST, Some("create_collector"))
                | (Some("billing_manage"), http::Method::PUT, Some("update_collector"))
                | (Some("payments_manage"), http::Method::POST, Some("credit_order"))
                | (Some("payments_manage"), http::Method::POST, Some("create_redeem_code_batch"))
                | (Some("payments_manage"), http::Method::POST, Some("delete_redeem_code_batch"))
                | (Some("api_keys_manage"), http::Method::POST, Some("create_api_key"))
                | (Some("api_keys_manage"), http::Method::PUT, Some("update_api_key"))
                | (Some("api_keys_manage"), http::Method::PATCH, Some("toggle_api_key"))
                | (Some("adaptive_manage"), http::Method::PATCH, Some("toggle_mode"))
                | (Some("proxy_nodes_manage"), http::Method::POST, Some("create_manual_node"))
                | (Some("proxy_nodes_manage"), http::Method::POST, Some("register_node"))
                | (Some("proxy_nodes_manage"), http::Method::POST, Some("heartbeat_node"))
                | (Some("proxy_nodes_manage"), http::Method::POST, Some("unregister_node"))
                | (Some("proxy_nodes_manage"), http::Method::POST, Some("test_proxy_url"))
                | (Some("proxy_nodes_manage"), http::Method::POST, Some("batch_upgrade_nodes"))
                | (Some("proxy_nodes_manage"), http::Method::PATCH, Some("update_manual_node"))
                | (Some("proxy_nodes_manage"), http::Method::PUT, Some("update_node_config"))
                | (Some("security_manage"), http::Method::POST, Some("blacklist_add"))
                | (Some("security_manage"), http::Method::POST, Some("whitelist_add"))
                | (Some("users_manage"), http::Method::POST, Some("create_user"))
                | (Some("users_manage"), http::Method::PUT, Some("update_user"))
                | (Some("users_manage"), http::Method::POST, Some("create_user_api_key"))
                | (Some("users_manage"), http::Method::PUT, Some("update_user_api_key"))
                | (Some("users_manage"), http::Method::PATCH, Some("lock_user_api_key"))
                | (Some("pool_manage"), http::Method::POST, Some("batch_import_keys"))
                | (Some("pool_manage"), http::Method::POST, Some("batch_action_keys"))
                | (Some("pool_manage"), http::Method::POST, Some("resolve_selection"))
                | (Some("usage_manage"), http::Method::POST, Some("replay"))
                | (Some("wallets_manage"), http::Method::POST, Some("adjust_balance"))
                | (Some("wallets_manage"), http::Method::POST, Some("recharge_balance"))
                | (Some("wallets_manage"), http::Method::POST, Some("process_refund"))
                | (Some("wallets_manage"), http::Method::POST, Some("complete_refund"))
                | (Some("wallets_manage"), http::Method::POST, Some("fail_refund"))
                | (Some("gemini_files_manage"), http::Method::POST, Some("upload"))
                | (Some("ldap_manage"), http::Method::PUT, Some("set_config"))
                | (Some("ldap_manage"), http::Method::POST, Some("test_connection"))
                | (Some("global_models_manage"), http::Method::POST, Some("create_global_model"))
                | (
                    Some("global_models_manage"),
                    http::Method::PATCH,
                    Some("update_global_model"),
                )
                | (
                    Some("global_models_manage"),
                    http::Method::POST,
                    Some("batch_delete_global_models"),
                )
                | (Some("global_models_manage"), http::Method::POST, Some("assign_to_providers"))
                | (Some("providers_manage"), http::Method::POST, Some("create_provider"))
                | (Some("providers_manage"), http::Method::PATCH, Some("update_provider")) => true,
                _ => false,
            }
        })
}

pub(crate) fn internal_proxy_local_requires_buffered_body(
    request_context: &GatewayPublicRequestContext,
) -> bool {
    request_context
        .control_decision
        .as_ref()
        .is_some_and(|decision| {
            if decision.route_class.as_deref() != Some("internal_proxy")
                || request_context.request_method != http::Method::POST
            {
                return false;
            }
            matches!(
                (
                    decision.route_family.as_deref(),
                    decision.route_kind.as_deref()
                ),
                (Some(TUNNEL_ROUTE_FAMILY), Some("heartbeat" | "node_status"))
                    | (
                        Some("internal_gateway"),
                        Some(
                            "resolve"
                                | "auth_context"
                                | "decision_sync"
                                | "decision_stream"
                                | "execute_sync"
                                | "execute_stream"
                                | "plan_sync"
                                | "plan_stream"
                                | "report_sync"
                                | "report_stream"
                                | "finalize_sync"
                        )
                    )
            )
        })
}

pub(crate) fn public_support_local_requires_buffered_body(
    request_context: &GatewayPublicRequestContext,
) -> bool {
    request_context
        .control_decision
        .as_ref()
        .is_some_and(|decision| {
            if decision.route_class.as_deref() != Some("public_support") {
                return false;
            }

            matches!(
                (
                    decision.route_family.as_deref(),
                    request_context.request_method.clone(),
                    decision.route_kind.as_deref(),
                ),
                (
                    Some("auth"),
                    http::Method::POST,
                    Some(
                        "login"
                            | "register"
                            | "send_verification_code"
                            | "verify_email"
                            | "verification_status"
                    ),
                ) | (
                    Some("users_me"),
                    http::Method::PUT,
                    Some(
                        "update_detail"
                            | "model_capabilities_update"
                            | "preferences_update"
                            | "api_key_update"
                            | "management_token_update"
                            | "api_key_providers_update"
                            | "api_key_capabilities_update",
                    ),
                ) | (
                    Some("users_me"),
                    http::Method::PATCH,
                    Some("password" | "session_update" | "api_key_patch"),
                ) | (
                    Some("users_me"),
                    http::Method::POST,
                    Some("api_keys_create" | "management_tokens_create"),
                ) | (
                    Some("wallet"),
                    http::Method::POST,
                    Some("create_refund" | "create_recharge_order" | "redeem"),
                ) | (
                    Some("payment_callback"),
                    http::Method::POST,
                    Some("callback"),
                )
            )
        })
}

pub(crate) fn local_proxy_route_requires_buffered_body(
    request_context: &GatewayPublicRequestContext,
) -> bool {
    admin_proxy_local_requires_buffered_body(request_context)
        || internal_proxy_local_requires_buffered_body(request_context)
        || public_support_local_requires_buffered_body(request_context)
}
