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
    build_admin_system_check_update_payload_from_release, build_admin_system_releases_list_payload,
    build_admin_system_settings_payload, build_admin_system_stats_payload, current_aether_version,
    fetch_admin_system_releases, fetch_latest_admin_system_release, resolve_update_target,
};
use crate::handlers::admin::system::shared::smtp::build_admin_smtp_test_payload;
use crate::handlers::admin::system::shared::update::{
    build_admin_system_update_capability_payload, current_self_update_blocker,
    prepare_admin_system_update_task, read_update_history, read_update_task_status,
    self_update_supported, start_admin_system_rollback_task, start_admin_system_update_task,
};
use crate::important_notification::build_important_notification_test_payload;
use crate::maintenance::{ManualUsageCleanupMode, ManualUsageCleanupOptions};
use crate::GatewayError;
use aether_data_contracts::repository::usage::UsageCleanupTargets;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::Instant;
use url::form_urlencoded;

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
        let force = query_flag(request_context.query_string(), "force");
        let (latest_release, error) = fetch_latest_admin_system_release(force).await;
        return Ok(Some(
            Json(build_admin_system_check_update_payload_from_release(
                latest_release,
                error,
            ))
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("releases")
        && request_method == http::Method::GET
        && request_path == "/api/admin/system/releases"
    {
        let force = query_flag(request_context.query_string(), "force");
        let (releases, error) = fetch_admin_system_releases(force).await;
        return Ok(Some(
            Json(build_admin_system_releases_list_payload(releases, error)).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("update_capability")
        && request_method == http::Method::GET
        && request_path == "/api/admin/system/update-capability"
    {
        return Ok(Some(
            Json(build_admin_system_update_capability_payload()).into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("prepare_update")
        && request_method == http::Method::POST
        && request_path == "/api/admin/system/prepare-update"
    {
        if !self_update_supported() {
            return Ok(Some(
                (
                    http::StatusCode::PRECONDITION_REQUIRED,
                    Json(json!({ "detail": current_self_update_blocker() })),
                )
                    .into_response(),
            ));
        }

        let target_version = request_body
            .filter(|b| !b.is_empty())
            .and_then(|body| serde_json::from_slice::<serde_json::Value>(body).ok())
            .and_then(|v| v.get("version").and_then(|v| v.as_str().map(String::from)));

        let (version, tarball_url, sha256sums_url) =
            match resolve_update_target(target_version).await {
                Ok(result) => result,
                Err((status, payload)) => {
                    return Ok(Some((status, Json(payload)).into_response()));
                }
            };

        return Ok(Some(
            match prepare_admin_system_update_task(version, tarball_url, sha256sums_url).await? {
                Ok(payload) => attach_admin_audit_response(
                    Json(payload).into_response(),
                    "admin_system_update_prepared",
                    "prepare_system_update",
                    "system_update",
                    "global",
                ),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("apply_update")
        && request_method == http::Method::POST
        && request_path == "/api/admin/system/apply-update"
    {
        let version = request_body
            .filter(|b| !b.is_empty())
            .and_then(|body| serde_json::from_slice::<serde_json::Value>(body).ok())
            .and_then(|v| v.get("version").and_then(|v| v.as_str().map(String::from)));

        return Ok(Some(
            match start_admin_system_update_task(version).await? {
                Ok(payload) => attach_admin_audit_response(
                    Json(payload).into_response(),
                    "admin_system_update_started",
                    "apply_system_update",
                    "system_update",
                    "global",
                ),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("rollback")
        && request_method == http::Method::POST
        && request_path == "/api/admin/system/rollback"
    {
        return Ok(Some(match start_admin_system_rollback_task().await? {
            Ok(payload) => attach_admin_audit_response(
                Json(payload).into_response(),
                "admin_system_rollback_started",
                "rollback_system_update",
                "system_rollback",
                "global",
            ),
            Err((status, payload)) => (status, Json(payload)).into_response(),
        }));
    }

    if decision.route_kind.as_deref() == Some("update_status")
        && request_method == http::Method::GET
        && request_path == "/api/admin/system/update-status"
    {
        let status = read_update_task_status();
        return Ok(Some(
            Json(json!({
                "phase": status.phase,
                "error": status.error,
                "output": status.output,
                "progress_label": status.progress_label,
                "downloaded_bytes": status.downloaded_bytes,
                "total_bytes": status.total_bytes,
                "progress_percent": status.progress_percent,
            }))
            .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("update_history")
        && request_method == http::Method::GET
        && request_path == "/api/admin/system/update-history"
    {
        let entries = read_update_history();
        return Ok(Some(Json(json!({ "entries": entries })).into_response()));
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

    if decision.route_kind.as_deref() == Some("data_export")
        && request_method == http::Method::GET
        && request_path == "/api/admin/system/data/export"
    {
        return Ok(Some(attach_admin_audit_response(
            Json(state.build_admin_system_data_export_payload().await?).into_response(),
            "admin_system_data_exported",
            "export_system_data",
            "system_data_export",
            "global",
        )));
    }

    if decision.route_kind.as_deref() == Some("data_import")
        && request_method == http::Method::POST
        && request_path == "/api/admin/system/data/import"
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
                .import_admin_system_data(
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
                    "admin_system_data_imported",
                    "import_system_data",
                    "system_data_import",
                    "global",
                ),
                Err((status, payload)) => (status, Json(payload)).into_response(),
            },
        ));
    }

    if decision.route_kind.as_deref() == Some("s3_backup_run")
        && request_method == http::Method::POST
        && request_path == "/api/admin/system/backups/s3/run"
    {
        return Ok(Some(
            match crate::backup::task::start_s3_backup_task(
                state.cloned_app(),
                "manual",
                decision
                    .admin_principal
                    .as_ref()
                    .map(|principal| principal.user_id.as_str()),
            )
            .await
            {
                Ok(task) => attach_admin_audit_response(
                    Json(json!({
                        "message": "S3 备份任务已提交",
                        "task": task,
                    }))
                    .into_response(),
                    "admin_system_s3_backup_task_started",
                    "run_s3_backup",
                    "s3_backup",
                    "global",
                ),
                Err(error) => {
                    (error.status(), Json(json!({ "detail": error.detail() }))).into_response()
                }
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

    if decision.route_kind.as_deref() == Some("important_notification_test")
        && request_method == http::Method::POST
        && request_path == "/api/admin/system/important-notification/test"
    {
        return Ok(Some(
            Json(build_important_notification_test_payload(state, request_body).await?)
                .into_response(),
        ));
    }

    if decision.route_kind.as_deref() == Some("cleanup") && request_method == http::Method::POST {
        return Ok(Some(attach_admin_audit_response(
            Json(build_admin_system_cleanup_payload(state).await?).into_response(),
            "admin_system_cleanup_completed",
            "cleanup_system_data",
            "system_cleanup",
            "global",
        )));
    }

    if decision.route_kind.as_deref() == Some("cleanup_runs")
        && request_method == http::Method::GET
        && request_path == "/api/admin/system/cleanup/runs"
    {
        let records = crate::maintenance::list_admin_cleanup_run_records(&state.app().data)
            .await
            .map_err(|err| GatewayError::Internal(err.to_string()))?;
        return Ok(Some(Json(json!({ "items": records })).into_response()));
    }

    if decision.route_kind.as_deref() == Some("cleanup_usage_manual")
        && request_method == http::Method::POST
        && request_path == "/api/admin/system/cleanup/usage/manual"
    {
        return Ok(Some(
            build_manual_usage_cleanup_response(state, request_context, request_body).await?,
        ));
    }

    if decision.route_kind.as_deref() == Some("cleanup_usage_preview")
        && request_method == http::Method::GET
        && request_path == "/api/admin/system/cleanup/usage/preview"
    {
        return Ok(Some(
            build_manual_usage_cleanup_preview_response(state, request_context).await?,
        ));
    }

    if let Some((task_kind, action, object_type, object_id)) =
        admin_system_purge_task_for_route_kind(decision.route_kind.as_deref())
    {
        if request_method != http::Method::POST {
            return Ok(None);
        }
        let task = crate::maintenance::start_admin_system_purge_task(state.cloned_app(), task_kind)
            .await?;
        return Ok(Some(attach_admin_audit_response(
            Json(json!({
                "message": task.message.clone(),
                "task": task,
            }))
            .into_response(),
            "admin_system_purge_task_started",
            action,
            object_type,
            object_id,
        )));
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

fn admin_system_purge_task_for_route_kind(
    route_kind: Option<&str>,
) -> Option<(
    crate::maintenance::AdminCleanupTaskKind,
    &'static str,
    &'static str,
    &'static str,
)> {
    match route_kind {
        Some("purge_config") => Some((
            crate::maintenance::AdminCleanupTaskKind::Config,
            "purge_system_config_async",
            "system_config",
            "global",
        )),
        Some("purge_users") => Some((
            crate::maintenance::AdminCleanupTaskKind::Users,
            "purge_non_admin_users_async",
            "users",
            "non_admin",
        )),
        Some("purge_usage") => Some((
            crate::maintenance::AdminCleanupTaskKind::Usage,
            "purge_usage_records_async",
            "usage",
            "all",
        )),
        Some("purge_audit_logs") => Some((
            crate::maintenance::AdminCleanupTaskKind::AuditLogs,
            "purge_audit_logs_async",
            "audit_logs",
            "all",
        )),
        Some("purge_request_bodies") | Some("purge_request_bodies_task") => Some((
            crate::maintenance::AdminCleanupTaskKind::RequestBodies,
            "purge_request_bodies_async",
            "request_bodies",
            "all",
        )),
        Some("purge_stats") => Some((
            crate::maintenance::AdminCleanupTaskKind::Stats,
            "purge_stats_async",
            "stats",
            "all",
        )),
        _ => None,
    }
}

async fn build_admin_system_cleanup_payload(
    state: &AdminAppState<'_>,
) -> Result<serde_json::Value, GatewayError> {
    let started_at_unix_secs = chrono::Utc::now().timestamp().max(0) as u64;
    let started_at = Instant::now();
    let summary = state.run_admin_system_cleanup_once().await?;
    let cleaned = json!({
        "audit_logs": summary.audit_logs_deleted,
        "request_candidates": summary.request_candidates_deleted,
        "proxy_node_metrics_1m": summary.proxy_node_metrics.deleted_1m_rows,
        "proxy_node_metrics_1h": summary.proxy_node_metrics.deleted_1h_rows,
        "pending_failed": summary.pending_failed,
        "pending_recovered": summary.pending_recovered,
        "usage_body_externalized": summary.usage.body_externalized,
        "usage_legacy_body_refs_migrated": summary.usage.legacy_body_refs_migrated,
        "usage_body_cleaned": summary.usage.body_cleaned,
        "usage_header_cleaned": summary.usage.header_cleaned,
        "usage_keys_cleaned": summary.usage.keys_cleaned,
        "usage_records_deleted": summary.usage.records_deleted,
    });
    let total = summary
        .audit_logs_deleted
        .saturating_add(summary.request_candidates_deleted)
        .saturating_add(summary.proxy_node_metrics.deleted_1m_rows)
        .saturating_add(summary.proxy_node_metrics.deleted_1h_rows)
        .saturating_add(summary.pending_failed)
        .saturating_add(summary.pending_recovered)
        .saturating_add(summary.usage.body_externalized)
        .saturating_add(summary.usage.legacy_body_refs_migrated)
        .saturating_add(summary.usage.body_cleaned)
        .saturating_add(summary.usage.header_cleaned)
        .saturating_add(summary.usage.keys_cleaned)
        .saturating_add(summary.usage.records_deleted);

    crate::maintenance::record_completed_cleanup_run(
        &state.app().data,
        "system_cleanup",
        "manual",
        started_at_unix_secs,
        started_at,
        cleaned.clone(),
        format!("系统清理已执行，影响 {total} 项"),
    )
    .await;

    Ok(json!({
        "message": format!("系统清理已执行，影响 {} 项", total),
        "cleaned": cleaned,
    }))
}

async fn build_manual_usage_cleanup_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let options = match parse_manual_usage_cleanup_request(request_body) {
        Ok(value) => value,
        Err(response) => return Ok(response),
    };
    let actor_user_id = request_context
        .decision()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.clone());

    match crate::maintenance::start_manual_usage_cleanup_task(
        std::sync::Arc::clone(&state.app().data),
        options,
        actor_user_id,
    )
    .await
    {
        Ok(task) => {
            let payload = json!({
                "message": task.message,
                "mode": options.mode.as_str(),
                "requested_older_than_days": options.requested_older_than_days,
                "targets": options.targets,
                "task": task,
            });
            Ok(attach_admin_audit_response(
                Json(payload).into_response(),
                "admin_system_usage_cleanup_started",
                "manual_usage_cleanup",
                "usage_cleanup",
                "global",
            ))
        }
        Err(crate::maintenance::ManualUsageCleanupError::AlreadyRunning) => Ok((
            http::StatusCode::CONFLICT,
            Json(json!({
                "detail": "usage_cleanup_already_running",
                "message": "已有一次清理正在进行中，请稍后再试",
            })),
        )
            .into_response()),
        Err(crate::maintenance::ManualUsageCleanupError::DataLayer(err)) => {
            Err(GatewayError::Internal(err.to_string()))
        }
    }
}

async fn build_manual_usage_cleanup_preview_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let older_than_days = match parse_older_than_days_query(request_context.query_string()) {
        Ok(value) => value,
        Err(response) => return Ok(response),
    };
    let options = match parse_manual_usage_cleanup_query_options(
        request_context.query_string(),
        older_than_days,
    ) {
        Ok(value) => value,
        Err(response) => return Ok(response),
    };
    let preview = crate::maintenance::preview_manual_usage_cleanup(&state.app().data, options)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    Ok(Json(json!({
        "mode": preview.mode.as_str(),
        "requested_older_than_days": preview.requested_older_than_days,
        "targets": preview.targets,
        "effective_cutoffs": {
            "detail": preview.detail_cutoff,
            "compressed": preview.compressed_cutoff,
            "header": preview.header_cutoff,
            "log": preview.log_cutoff,
        },
        "counts": {
            "detail": preview.detail_count,
            "compressed": preview.compressed_count,
            "header": preview.header_count,
            "log": preview.log_count,
        },
    }))
    .into_response())
}

fn parse_manual_usage_cleanup_request(
    request_body: Option<&Bytes>,
) -> Result<ManualUsageCleanupOptions, Response<Body>> {
    let Some(body) = request_body else {
        return Ok(ManualUsageCleanupOptions::policy());
    };
    if body.is_empty() {
        return Ok(ManualUsageCleanupOptions::policy());
    }
    let parsed: serde_json::Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(err) => {
            return Err((
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": format!("请求体无效 JSON: {err}") })),
            )
                .into_response())
        }
    };
    let Some(object) = parsed.as_object() else {
        return Err(bad_manual_cleanup_request("请求体必须为 JSON 对象"));
    };
    parse_manual_usage_cleanup_options(
        object.get("mode").and_then(serde_json::Value::as_str),
        object.get("older_than_days"),
        object.get("targets"),
    )
}

fn parse_manual_usage_cleanup_query_options(
    query_string: Option<&str>,
    older_than_days: Option<u32>,
) -> Result<ManualUsageCleanupOptions, Response<Body>> {
    let mode = query_param(query_string, "mode");
    let targets = query_param(query_string, "targets").map(serde_json::Value::String);
    let older_value =
        older_than_days.map(|days| serde_json::Value::Number(serde_json::Number::from(days)));
    parse_manual_usage_cleanup_options(mode.as_deref(), older_value.as_ref(), targets.as_ref())
}

fn parse_manual_usage_cleanup_options(
    raw_mode: Option<&str>,
    older_than_days: Option<&serde_json::Value>,
    targets_value: Option<&serde_json::Value>,
) -> Result<ManualUsageCleanupOptions, Response<Body>> {
    let (requested_older_than_days, requested_before_now) =
        parse_manual_cleanup_older_than_days(older_than_days)?;
    let mode =
        parse_manual_cleanup_mode(raw_mode, requested_older_than_days, requested_before_now)?;

    if mode == ManualUsageCleanupMode::OlderThanDays && requested_older_than_days.is_none() {
        return Err(bad_manual_cleanup_request(
            "older_than_days 模式必须提供正整数天数",
        ));
    }
    if mode == ManualUsageCleanupMode::BeforeNow && requested_older_than_days.is_some() {
        return Err(bad_manual_cleanup_request(
            "before_now 模式不能同时提供 older_than_days",
        ));
    }
    if raw_mode.is_some() && requested_before_now && mode != ManualUsageCleanupMode::BeforeNow {
        return Err(bad_manual_cleanup_request(
            "older_than_days 为 0 时必须使用 before_now 模式",
        ));
    }
    if raw_mode.is_some()
        && mode == ManualUsageCleanupMode::Policy
        && requested_older_than_days.is_some()
    {
        return Err(bad_manual_cleanup_request(
            "policy 模式不能同时提供 older_than_days",
        ));
    }

    let targets = parse_manual_cleanup_targets(targets_value, mode)?;
    if !targets.any_selected() {
        return Err(bad_manual_cleanup_request("至少选择一个清理范围"));
    }
    if mode == ManualUsageCleanupMode::BeforeNow
        && (targets.headers || targets.records || targets.expired_keys)
    {
        return Err(bad_manual_cleanup_request(
            "清理当前时刻之前只允许选择详细请求体和压缩请求体",
        ));
    }

    Ok(ManualUsageCleanupOptions {
        mode,
        requested_older_than_days,
        targets,
    })
}

fn parse_manual_cleanup_mode(
    raw_mode: Option<&str>,
    requested_older_than_days: Option<u32>,
    requested_before_now: bool,
) -> Result<ManualUsageCleanupMode, Response<Body>> {
    let Some(raw) = raw_mode.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(if requested_before_now {
            ManualUsageCleanupMode::BeforeNow
        } else if requested_older_than_days.is_some() {
            ManualUsageCleanupMode::OlderThanDays
        } else {
            ManualUsageCleanupMode::Policy
        });
    };
    match raw {
        "policy" => Ok(ManualUsageCleanupMode::Policy),
        "older_than_days" => Ok(ManualUsageCleanupMode::OlderThanDays),
        "before_now" => Ok(ManualUsageCleanupMode::BeforeNow),
        _ => Err(bad_manual_cleanup_request(
            "mode 必须为 policy、older_than_days 或 before_now",
        )),
    }
}

fn parse_manual_cleanup_older_than_days(
    value: Option<&serde_json::Value>,
) -> Result<(Option<u32>, bool), Response<Body>> {
    match value {
        None | Some(serde_json::Value::Null) => Ok((None, false)),
        Some(value) => {
            let Some(raw) = value.as_u64() else {
                return Err(bad_manual_cleanup_request("older_than_days 必须为非负整数"));
            };
            if raw == 0 {
                return Ok((None, true));
            }
            let days = u32::try_from(raw)
                .ok()
                .filter(|days| *days >= 1)
                .ok_or_else(|| bad_manual_cleanup_request("older_than_days 必须为正整数"))?;
            Ok((Some(days), false))
        }
    }
}

fn parse_manual_cleanup_targets(
    value: Option<&serde_json::Value>,
    mode: ManualUsageCleanupMode,
) -> Result<UsageCleanupTargets, Response<Body>> {
    let Some(value) = value else {
        return Ok(match mode {
            ManualUsageCleanupMode::BeforeNow => UsageCleanupTargets::body_targets(),
            ManualUsageCleanupMode::Policy | ManualUsageCleanupMode::OlderThanDays => {
                UsageCleanupTargets::all_policy_targets()
            }
        });
    };
    if value.is_null() {
        return Ok(match mode {
            ManualUsageCleanupMode::BeforeNow => UsageCleanupTargets::body_targets(),
            ManualUsageCleanupMode::Policy | ManualUsageCleanupMode::OlderThanDays => {
                UsageCleanupTargets::all_policy_targets()
            }
        });
    }

    let raw_targets = match value {
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| bad_manual_cleanup_request("targets 必须为字符串数组"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        serde_json::Value::String(raw) => raw
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        _ => return Err(bad_manual_cleanup_request("targets 必须为字符串数组")),
    };

    let mut targets = UsageCleanupTargets {
        detail_body: false,
        compressed_body: false,
        headers: false,
        records: false,
        expired_keys: false,
    };
    for raw in raw_targets {
        match raw.as_str() {
            "detail_body" | "detail" | "raw_body" => targets.detail_body = true,
            "compressed_body" | "compressed" => targets.compressed_body = true,
            "headers" | "header" => targets.headers = true,
            "records" | "log" | "logs" => targets.records = true,
            "expired_keys" => targets.expired_keys = true,
            "all" => targets = UsageCleanupTargets::all_policy_targets(),
            _ => {
                return Err(bad_manual_cleanup_request(
                    "targets 只能包含 detail_body、compressed_body、headers、records",
                ))
            }
        }
    }
    Ok(targets)
}

fn bad_manual_cleanup_request(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

fn query_param(query_string: Option<&str>, name: &str) -> Option<String> {
    let query = query_string.filter(|value| !value.is_empty())?;
    form_urlencoded::parse(query.as_bytes())
        .find_map(|(key, value)| (key == name && !value.is_empty()).then(|| value.into_owned()))
}

fn query_flag(query_string: Option<&str>, name: &str) -> bool {
    query_param(query_string, name).is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn parse_older_than_days_query(query_string: Option<&str>) -> Result<Option<u32>, Response<Body>> {
    let Some(value) = query_param(query_string, "older_than_days") else {
        return Ok(None);
    };
    value
        .parse::<u32>()
        .ok()
        .filter(|days| *days >= 1)
        .map(Some)
        .ok_or_else(|| {
            (
                http::StatusCode::BAD_REQUEST,
                Json(json!({
                    "detail": "older_than_days 必须为正整数",
                })),
            )
                .into_response()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_cleanup_request_defaults_to_policy_targets() {
        let options = parse_manual_usage_cleanup_request(None).expect("default request is valid");

        assert_eq!(options.mode, ManualUsageCleanupMode::Policy);
        assert_eq!(options.requested_older_than_days, None);
        assert_eq!(options.targets, UsageCleanupTargets::all_policy_targets());
    }

    #[test]
    fn manual_cleanup_request_treats_zero_days_as_before_now_body_only() {
        let body = Bytes::from_static(br#"{"older_than_days":0}"#);

        let options =
            parse_manual_usage_cleanup_request(Some(&body)).expect("before-now request is valid");

        assert_eq!(options.mode, ManualUsageCleanupMode::BeforeNow);
        assert_eq!(options.requested_older_than_days, None);
        assert_eq!(options.targets, UsageCleanupTargets::body_targets());
    }

    #[test]
    fn manual_cleanup_request_rejects_before_now_headers() {
        let body = Bytes::from_static(br#"{"mode":"before_now","targets":["headers"]}"#);

        assert!(parse_manual_usage_cleanup_request(Some(&body)).is_err());
    }

    #[test]
    fn manual_cleanup_preview_query_decodes_comma_separated_targets() {
        let options = parse_manual_usage_cleanup_query_options(
            Some("mode=before_now&targets=detail_body%2Ccompressed_body"),
            None,
        )
        .expect("encoded targets query is valid");

        assert_eq!(options.mode, ManualUsageCleanupMode::BeforeNow);
        assert_eq!(options.targets, UsageCleanupTargets::body_targets());
    }

    #[test]
    fn manual_cleanup_request_rejects_non_object_body() {
        let body = Bytes::from_static(br#"[]"#);

        assert!(parse_manual_usage_cleanup_request(Some(&body)).is_err());
    }
}
