use super::builders::{
    admin_ldap_test_connection, build_admin_ldap_test_config, build_admin_ldap_update_config,
    AdminLdapConfigTestRequest, AdminLdapConfigUpdateRequest,
};
use super::shared::*;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn maybe_build_local_admin_ldap_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("ldap_manage") {
        return Ok(None);
    }

    match decision.route_kind.as_deref() {
        Some("get_config")
            if request_context.method() == http::Method::GET
                && is_admin_ldap_config_root(request_context.path()) =>
        {
            return Ok(Some(attach_admin_audit_response(
                Json(build_admin_ldap_config_payload(
                    state.get_ldap_module_config().await?.as_ref(),
                ))
                .into_response(),
                "admin_ldap_config_viewed",
                "view_ldap_config",
                "ldap_config",
                "ldap",
            )));
        }
        Some("set_config")
            if request_context.method() == http::Method::PUT
                && is_admin_ldap_config_root(request_context.path()) =>
        {
            if !state.has_auth_module_writer() {
                return Ok(Some(admin_ldap_unavailable_response()));
            }
            let Some(request_body) = request_body else {
                return Ok(Some(admin_ldap_bad_request_response("请求数据验证失败")));
            };
            let payload = match serde_json::from_slice::<AdminLdapConfigUpdateRequest>(request_body)
            {
                Ok(payload) => payload,
                Err(_) => return Ok(Some(admin_ldap_bad_request_response("请求数据验证失败"))),
            };
            let update = match build_admin_ldap_update_config(state, payload).await {
                Ok(config) => config,
                Err(detail) => return Ok(Some(admin_ldap_bad_request_response(detail))),
            };
            let saved = state.upsert_ldap_module_config(&update).await?;
            if saved.is_none() {
                return Ok(Some(admin_ldap_unavailable_response()));
            }
            return Ok(Some(
                Json(json!({ "message": "LDAP配置更新成功" })).into_response(),
            ));
        }
        Some("test_connection")
            if request_context.method() == http::Method::POST
                && is_admin_ldap_test_root(request_context.path()) =>
        {
            let payload = match request_body {
                Some(body) if !body.is_empty() => match serde_json::from_slice::<
                    AdminLdapConfigTestRequest,
                >(body)
                {
                    Ok(value) => value,
                    Err(_) => return Ok(Some(admin_ldap_bad_request_response("请求数据验证失败"))),
                },
                _ => AdminLdapConfigTestRequest::default(),
            };
            let merged = match build_admin_ldap_test_config(state, payload).await {
                Ok(config) => config,
                Err(detail) => return Ok(Some(admin_ldap_bad_request_response(detail))),
            };
            let response = match merged {
                Some(config) => {
                    let (success, message) = admin_ldap_test_connection(config).await?;
                    json!({ "success": success, "message": message })
                }
                None => json!({
                    "success": false,
                    "message": "缺少必要字段: server_url, bind_dn, base_dn, bind_password",
                }),
            };
            return Ok(Some(attach_admin_audit_response(
                Json(response).into_response(),
                "admin_ldap_connection_tested",
                "test_ldap_connection",
                "ldap_config",
                "ldap",
            )));
        }
        _ => {}
    }

    Ok(None)
}
