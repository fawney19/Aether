use super::ADMIN_AWS_REGIONS;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::handlers::admin::shared::build_proxy_error_response;
use crate::handlers::admin::system::shared::configs::{
    apply_admin_system_config_update, build_admin_system_config_detail_payload,
    build_admin_system_configs_payload, delete_admin_system_config,
};
use crate::handlers::admin::system::shared::paths::{
    admin_system_config_key_from_path, admin_system_email_template_preview_type_from_path,
    admin_system_email_template_reset_type_from_path, admin_system_email_template_type_from_path,
    is_admin_system_configs_root, is_admin_system_email_templates_root,
};
use crate::handlers::admin::system::shared::settings::{
    apply_admin_system_settings_update, build_admin_api_formats_payload,
    build_admin_system_check_update_payload, build_admin_system_settings_payload,
    build_admin_system_stats_payload, current_aether_version,
};
use crate::handlers::admin::system::shared::smtp::build_admin_smtp_test_payload;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn maybe_build_local_admin_core_system_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    let request_method = request_context.method();
    let request_path = request_context.path();
    if decision.route_family.as_deref() != Some("system_manage") {
        return Ok(None);
    }

    if decision.route_kind.as_deref() == Some("version")
        && request_method == http::Method::GET
        && request_path == "/api/admin/system/version"
    {
        return Ok(Some(
            Json(json!({ "version": current_aether_version() })).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("check_update")
        && request_method == http::Method::GET
        && request_path == "/api/admin/system/check-update"
    {
        return Ok(Some(
            Json(build_admin_system_check_update_payload()).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("aws_regions")
        && request_method == http::Method::GET
        && request_path == "/api/admin/system/aws-regions"
    {
        return Ok(Some(
            Json(json!({ "regions": ADMIN_AWS_REGIONS })).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("stats")
        && request_method == http::Method::GET
        && request_path == "/api/admin/system/stats"
    {
        return Ok(Some(
            Json(build_admin_system_stats_payload(state).await?).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("settings_get")
        && request_method == http::Method::GET
        && request_path == "/api/admin/system/settings"
    {
        return Ok(Some(
            Json(build_admin_system_settings_payload(state).await?).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("config_export")
        && request_method == http::Method::GET
        && request_path == "/api/admin/system/config/export"
    {
        return Ok(Some(attach_admin_audit_response(
            Json(state.build_admin_system_config_export_payload().await?).into_response(),
            "admin_system_config_exported",
            "export_system_config",
            "system_config_export",
            "global",
        )));
    }

    if decision.route_kind.as_deref() == Some("config_import")
        && request_method == http::Method::POST
        && request_path == "/api/admin/system/config/import"
    {
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求数据验证失败" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match state.import_admin_system_config(request_body).await? {
                Ok(payload) => attach_admin_audit_response(
                    Json(payload).into_response(),
                    "admin_system_config_imported",
                    "import_system_config",
                    "system_config_import",
                    "global",
                ),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("users_export")
        && request_method == http::Method::GET
        && request_path == "/api/admin/system/users/export"
    {
        return Ok(Some(attach_admin_audit_response(
            Json(state.build_admin_system_users_export_payload().await?).into_response(),
            "admin_system_users_exported",
            "export_system_users",
            "user_export",
            "all_users",
        )));
    }

    if decision.route_kind.as_deref() == Some("users_import")
        && request_method == http::Method::POST
        && request_path == "/api/admin/system/users/import"
    {
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求数据验证失败" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match state
                .import_admin_system_users(
                    request_body,
                    decision
                        .admin_principal
                        .as_ref()
                        .map(|principal| principal.user_id.as_str()),
                )
                .await?
            {
                Ok(payload) => attach_admin_audit_response(
                    Json(payload).into_response(),
                    "admin_system_users_imported",
                    "import_system_users",
                    "system_users_import",
                    "global",
                ),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("smtp_test")
        && request_method == http::Method::POST
        && request_path == "/api/admin/system/smtp/test"
    {
        return Ok(Some(
            Json(build_admin_smtp_test_payload(state, request_body).await?).into_response(),
        ));
    }

    if matches!(
        decision.route_kind.as_deref(),
        Some(
            "cleanup"
                | "purge_config"
                | "purge_users"
                | "purge_usage"
                | "purge_audit_logs"
                | "purge_request_bodies"
                | "purge_stats"
        )
    ) && request_method == http::Method::POST
    {
        return Ok(Some(
            (
                http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "detail": "Admin system data unavailable" })),
            )
                .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("settings_set")
        && request_method == http::Method::PUT
        && request_path == "/api/admin/system/settings"
    {
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求数据验证失败" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match apply_admin_system_settings_update(state, request_body).await? {
                Ok(payload) => attach_admin_audit_response(
                    Json(payload).into_response(),
                    "admin_system_settings_updated",
                    "update_system_settings",
                    "system_settings",
                    "global",
                ),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("configs_list")
        && request_method == http::Method::GET
        && is_admin_system_configs_root(request_path)
    {
        let entries = state.list_system_config_entries().await?;
        return Ok(Some(
            Json(build_admin_system_configs_payload(&entries)).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("config_get") && request_method == http::Method::GET {
        let Some(config_key) = admin_system_config_key_from_path(request_path) else {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::NOT_FOUND,
                "not_found",
                "配置项不存在",
                None,
            )));
        };
        return Ok(Some(
            match build_admin_system_config_detail_payload(state, &config_key).await? {
                Ok(payload) => Json(payload).into_response(),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("config_set") && request_method == http::Method::PUT {
        let Some(config_key) = admin_system_config_key_from_path(request_path) else {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::NOT_FOUND,
                "not_found",
                "配置项不存在",
                None,
            )));
        };
        let Some(request_body) = request_body else {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::BAD_REQUEST,
                "invalid_request",
                "请求数据验证失败",
                None,
            )));
        };
        return Ok(Some(
            match apply_admin_system_config_update(state, &config_key, request_body).await? {
                Ok(payload) => attach_admin_audit_response(
                    Json(payload).into_response(),
                    "admin_system_config_updated",
                    "update_system_config",
                    "system_config",
                    &config_key,
                ),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("config_delete")
        && request_method == http::Method::DELETE
    {
        let Some(config_key) = admin_system_config_key_from_path(request_path) else {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::NOT_FOUND,
                "not_found",
                "配置项不存在",
                None,
            )));
        };
        return Ok(Some(
            match delete_admin_system_config(state, &config_key).await? {
                Ok(payload) => attach_admin_audit_response(
                    Json(payload).into_response(),
                    "admin_system_config_deleted",
                    "delete_system_config",
                    "system_config",
                    &config_key,
                ),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("api_formats")
        && request_method == http::Method::GET
        && request_path == "/api/admin/system/api-formats"
    {
        return Ok(Some(
            Json(build_admin_api_formats_payload()).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("email_templates_list")
        && request_method == http::Method::GET
        && is_admin_system_email_templates_root(request_path)
    {
        return Ok(Some(
            Json(state.build_admin_email_templates_payload().await?).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("email_template_get")
        && request_method == http::Method::GET
    {
        let Some(template_type) = admin_system_email_template_type_from_path(request_path) else {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::NOT_FOUND,
                "not_found",
                "模板类型不存在",
                None,
            )));
        };
        return Ok(Some(
            match state
                .build_admin_email_template_payload(&template_type)
                .await?
            {
                Ok(payload) => Json(payload).into_response(),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("email_template_set")
        && request_method == http::Method::PUT
    {
        let Some(template_type) = admin_system_email_template_type_from_path(request_path) else {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::NOT_FOUND,
                "not_found",
                "模板类型不存在",
                None,
            )));
        };
        let Some(request_body) = request_body else {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::BAD_REQUEST,
                "invalid_request",
                "请求数据验证失败",
                None,
            )));
        };
        return Ok(Some(
            match state
                .apply_admin_email_template_update(&template_type, request_body)
                .await?
            {
                Ok(payload) => Json(payload).into_response(),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("email_template_preview")
        && request_method == http::Method::POST
    {
        let Some(template_type) = admin_system_email_template_preview_type_from_path(request_path)
        else {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::NOT_FOUND,
                "not_found",
                "模板类型不存在",
                None,
            )));
        };
        return Ok(Some(
            match state
                .preview_admin_email_template(&template_type, request_body)
                .await?
            {
                Ok(payload) => Json(payload).into_response(),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("email_template_reset")
        && request_method == http::Method::POST
    {
        let Some(template_type) = admin_system_email_template_reset_type_from_path(request_path)
        else {
            return Ok(Some(build_proxy_error_response(
                http::StatusCode::NOT_FOUND,
                "not_found",
                "模板类型不存在",
                None,
            )));
        };
        return Ok(Some(
            match state.reset_admin_email_template(&template_type).await? {
                Ok(payload) => Json(payload).into_response(),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            },
        ));
    }

    Ok(None)
}
