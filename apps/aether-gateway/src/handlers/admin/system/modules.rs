use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::system::shared::modules::{
    admin_module_by_name, admin_module_name_from_enabled_path, admin_module_name_from_status_path,
    build_admin_module_runtime_state, build_admin_module_status_payload,
    build_admin_module_validation_result, build_admin_modules_status_payload,
    AdminSetModuleEnabledRequest,
};
use crate::handlers::admin::system::shared::paths::{
    is_admin_system_configs_root, is_admin_system_email_templates_root,
};
use crate::handlers::shared::module_available_from_env;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(crate) async fn maybe_build_local_admin_modules_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("modules_manage") {
        return Ok(None);
    }

    if decision.route_kind.as_deref() == Some("status_list")
        && request_context.request_method == http::Method::GET
        && request_context.request_path == "/api/admin/modules/status"
    {
        let payload = build_admin_modules_status_payload(state).await?;
        return Ok(Some(Json(payload).into_response()));
    }

    if decision.route_kind.as_deref() == Some("status_detail")
        && request_context.request_method == http::Method::GET
        && request_context
            .request_path
            .starts_with("/api/admin/modules/status/")
    {
        let Some(module_name) = admin_module_name_from_status_path(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "模块不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(module) = admin_module_by_name(&module_name) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("模块 '{module_name}' 不存在") })),
                )
                    .into_response(),
            ));
        };
        let runtime = build_admin_module_runtime_state(state).await?;
        let payload = build_admin_module_status_payload(state, module, &runtime).await?;
        return Ok(Some(Json(payload).into_response()));
    }

    if decision.route_kind.as_deref() == Some("set_enabled")
        && request_context.request_method == http::Method::PUT
        && request_context
            .request_path
            .starts_with("/api/admin/modules/status/")
        && request_context.request_path.ends_with("/enabled")
    {
        let Some(module_name) = admin_module_name_from_enabled_path(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "模块不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(module) = admin_module_by_name(&module_name) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("模块 '{module_name}' 不存在") })),
                )
                    .into_response(),
            ));
        };
        let available = module_available_from_env(module.env_key, module.default_available);
        if !available {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({
                        "detail": format!(
                            "模块 '{}' 不可用，无法启用。请检查环境变量 {} 和依赖库。",
                            module.name, module.env_key
                        )
                    })),
                )
                    .into_response(),
            ));
        }
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体不能为空" })),
                )
                    .into_response(),
            ));
        };
        let payload = match serde_json::from_slice::<AdminSetModuleEnabledRequest>(request_body) {
            Ok(payload) => payload,
            Err(_) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": "请求体格式错误，需要 enabled 字段" })),
                    )
                        .into_response(),
                ));
            }
        };
        let runtime = build_admin_module_runtime_state(state).await?;
        if payload.enabled {
            let (config_validated, config_error) =
                build_admin_module_validation_result(module, &runtime);
            if !config_validated {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({
                            "detail": format!(
                                "模块配置未验证通过: {}",
                                config_error.unwrap_or_else(|| "未知错误".to_string())
                            )
                        })),
                    )
                        .into_response(),
                ));
            }
        }
        let _ = state
            .upsert_system_config_json_value(
                &format!("module.{}.enabled", module.name),
                &json!(payload.enabled),
                Some(&format!("模块 [{}] 启用状态", module.display_name)),
            )
            .await?;
        let updated_runtime = build_admin_module_runtime_state(state).await?;
        let payload = build_admin_module_status_payload(state, module, &updated_runtime).await?;
        return Ok(Some(Json(payload).into_response()));
    }

    Ok(None)
}
