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

const ADMIN_SECURITY_DATA_UNAVAILABLE_DETAIL: &str = "Admin security data unavailable";

#[derive(Debug, serde::Deserialize)]
struct AdminSecurityBlacklistAddRequest {
    ip_address: String,
    reason: String,
    #[serde(default)]
    ttl: Option<u64>,
}

#[derive(Debug, serde::Deserialize)]
struct AdminSecurityWhitelistAddRequest {
    ip_address: String,
}

fn build_admin_security_data_unavailable_response() -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": ADMIN_SECURITY_DATA_UNAVAILABLE_DETAIL })),
    )
        .into_response()
}

fn build_admin_security_bad_request_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

fn build_admin_security_not_found_response(detail: impl Into<String>) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail.into() })),
    )
        .into_response()
}

fn admin_security_blacklist_ip_from_path(request_path: &str) -> Option<String> {
    let value = request_path
        .strip_prefix("/api/admin/security/ip/blacklist/")?
        .trim()
        .trim_matches('/')
        .to_string();
    if value.is_empty() || value.contains('/') {
        None
    } else {
        Some(value)
    }
}

fn admin_security_whitelist_ip_from_path(request_path: &str) -> Option<String> {
    let value = request_path
        .strip_prefix("/api/admin/security/ip/whitelist/")?
        .trim()
        .trim_matches('/')
        .to_string();
    if value.is_empty() || value.contains('/') {
        None
    } else {
        Some(value)
    }
}

fn admin_security_validate_ip_or_cidr(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    if value.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    let Some((host, prefix)) = value.split_once('/') else {
        return false;
    };
    let Ok(ip) = host.trim().parse::<std::net::IpAddr>() else {
        return false;
    };
    let Ok(prefix) = prefix.trim().parse::<u8>() else {
        return false;
    };
    match ip {
        std::net::IpAddr::V4(_) => prefix <= 32,
        std::net::IpAddr::V6(_) => prefix <= 128,
    }
}

async fn build_admin_security_blacklist_add_response(
    state: &AdminAppState<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(request_body) = request_body else {
        return Ok(build_admin_security_bad_request_response(
            "请求数据验证失败",
        ));
    };
    let payload = match serde_json::from_slice::<AdminSecurityBlacklistAddRequest>(request_body) {
        Ok(value) if !value.ip_address.trim().is_empty() && !value.reason.trim().is_empty() => {
            value
        }
        _ => {
            return Ok(build_admin_security_bad_request_response(
                "请求数据验证失败",
            ));
        }
    };

    if !state
        .add_admin_security_blacklist(
            payload.ip_address.trim(),
            payload.reason.trim(),
            payload.ttl,
        )
        .await?
    {
        return Ok((
            http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "detail": "添加 IP 到黑名单失败（Redis 不可用）" })),
        )
            .into_response());
    }

    Ok(attach_admin_audit_response(
        Json(json!({
            "success": true,
            "message": format!("IP {} 已加入黑名单", payload.ip_address.trim()),
            "reason": payload.reason.trim(),
            "ttl": payload.ttl.map(serde_json::Value::from).unwrap_or_else(|| json!("永久")),
        }))
        .into_response(),
        "admin_security_blacklist_added",
        "add_security_blacklist_entry",
        "security_blacklist_entry",
        payload.ip_address.trim(),
    ))
}

async fn build_admin_security_blacklist_remove_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(ip_address) = admin_security_blacklist_ip_from_path(request_context.path()) else {
        return Ok(build_admin_security_bad_request_response("缺少 ip_address"));
    };

    if !state.remove_admin_security_blacklist(&ip_address).await? {
        return Ok(build_admin_security_not_found_response(format!(
            "IP {ip_address} 不在黑名单中"
        )));
    }

    Ok(attach_admin_audit_response(
        Json(json!({
            "success": true,
            "message": format!("IP {ip_address} 已从黑名单移除"),
        }))
        .into_response(),
        "admin_security_blacklist_removed",
        "remove_security_blacklist_entry",
        "security_blacklist_entry",
        &ip_address,
    ))
}

async fn build_admin_security_blacklist_stats_response(
    state: &AdminAppState<'_>,
) -> Result<Response<Body>, GatewayError> {
    let (available, total, error) = state.admin_security_blacklist_stats().await?;
    let mut payload = json!({
        "available": available,
        "total": total,
    });
    if let Some(error) = error {
        payload["error"] = json!(error);
    }
    Ok(attach_admin_audit_response(
        Json(payload).into_response(),
        "admin_security_blacklist_stats_viewed",
        "view_security_blacklist_stats",
        "security_blacklist",
        "global",
    ))
}

async fn build_admin_security_blacklist_list_response(
    state: &AdminAppState<'_>,
) -> Result<Response<Body>, GatewayError> {
    let entries = state.list_admin_security_blacklist().await?;
    let total = entries.len();
    Ok(attach_admin_audit_response(
        Json(json!({ "items": entries, "total": total })).into_response(),
        "admin_security_blacklist_viewed",
        "view_security_blacklist",
        "security_blacklist",
        "global",
    ))
}

async fn build_admin_security_whitelist_add_response(
    state: &AdminAppState<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(request_body) = request_body else {
        return Ok(build_admin_security_bad_request_response(
            "请求数据验证失败",
        ));
    };
    let payload = match serde_json::from_slice::<AdminSecurityWhitelistAddRequest>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return Ok(build_admin_security_bad_request_response(
                "请求数据验证失败",
            ));
        }
    };
    let ip_address = payload.ip_address.trim();
    if !admin_security_validate_ip_or_cidr(ip_address)
        || !state.add_admin_security_whitelist(ip_address).await?
    {
        return Ok(build_admin_security_bad_request_response(
            "添加 IP 到白名单失败（无效的 IP 格式或 Redis 不可用）",
        ));
    }

    Ok(attach_admin_audit_response(
        Json(json!({
            "success": true,
            "message": format!("IP {ip_address} 已加入白名单"),
        }))
        .into_response(),
        "admin_security_whitelist_added",
        "add_security_whitelist_entry",
        "security_whitelist_entry",
        ip_address,
    ))
}

async fn build_admin_security_whitelist_remove_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(ip_address) = admin_security_whitelist_ip_from_path(request_context.path()) else {
        return Ok(build_admin_security_bad_request_response("缺少 ip_address"));
    };

    if !state.remove_admin_security_whitelist(&ip_address).await? {
        return Ok(build_admin_security_not_found_response(format!(
            "IP {ip_address} 不在白名单中"
        )));
    }

    Ok(attach_admin_audit_response(
        Json(json!({
            "success": true,
            "message": format!("IP {ip_address} 已从白名单移除"),
        }))
        .into_response(),
        "admin_security_whitelist_removed",
        "remove_security_whitelist_entry",
        "security_whitelist_entry",
        &ip_address,
    ))
}

async fn build_admin_security_whitelist_list_response(
    state: &AdminAppState<'_>,
) -> Result<Response<Body>, GatewayError> {
    let whitelist = state.list_admin_security_whitelist().await?;
    let total = whitelist.len();
    Ok(attach_admin_audit_response(
        Json(json!({
            "whitelist": whitelist,
            "total": total,
        }))
        .into_response(),
        "admin_security_whitelist_viewed",
        "view_security_whitelist",
        "security_whitelist",
        "global",
    ))
}

pub(crate) async fn maybe_build_local_admin_security_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("security_manage") {
        return Ok(None);
    }

    match decision.route_kind.as_deref() {
        Some("blacklist_add") => Ok(Some(
            build_admin_security_blacklist_add_response(state, request_body).await?,
        )),
        Some("blacklist_remove") => Ok(Some(
            build_admin_security_blacklist_remove_response(state, request_context).await?,
        )),
        Some("blacklist_stats") => Ok(Some(
            build_admin_security_blacklist_stats_response(state).await?,
        )),
        Some("blacklist_list") => Ok(Some(
            build_admin_security_blacklist_list_response(state).await?,
        )),
        Some("whitelist_add") => Ok(Some(
            build_admin_security_whitelist_add_response(state, request_body).await?,
        )),
        Some("whitelist_remove") => Ok(Some(
            build_admin_security_whitelist_remove_response(state, request_context).await?,
        )),
        Some("whitelist_list") => Ok(Some(
            build_admin_security_whitelist_list_response(state).await?,
        )),
        _ => Ok(Some(build_admin_security_data_unavailable_response())),
    }
}
