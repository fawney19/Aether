use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{self, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use tracing::warn;

use crate::async_task::{
    cancel_video_task, get_video_task_detail, get_video_task_stats, get_video_task_video,
    list_video_tasks,
};
use crate::audit::{get_auth_api_key_snapshot, get_decision_trace, get_request_candidate_trace};
use crate::hooks::{get_request_audit_bundle, get_request_usage_audit};
use crate::router::metrics;
use crate::state::AppState;

#[derive(Clone, Copy)]
struct OperationalPermission {
    required_permissions: &'static [&'static str],
    write: bool,
    requires_full_admin_role: bool,
}

pub(crate) fn mount_operational_routes(
    router: Router<AppState>,
    state: AppState,
) -> Router<AppState> {
    let operational = Router::<AppState>::new()
        .route("/_gateway/metrics", get(metrics))
        .route("/_gateway/async-tasks/video-tasks", get(list_video_tasks))
        .route(
            "/_gateway/async-tasks/video-tasks/stats",
            get(get_video_task_stats),
        )
        .route(
            "/_gateway/async-tasks/video-tasks/{task_id}/video",
            get(get_video_task_video),
        )
        .route(
            "/_gateway/async-tasks/video-tasks/{task_id}/cancel",
            post(cancel_video_task),
        )
        .route(
            "/_gateway/async-tasks/video-tasks/{task_id}",
            get(get_video_task_detail),
        )
        .route(
            "/_gateway/audit/auth/users/{user_id}/api-keys/{api_key_id}",
            get(get_auth_api_key_snapshot),
        )
        .route(
            "/_gateway/audit/decision-trace/{request_id}",
            get(get_decision_trace),
        )
        .route(
            "/_gateway/audit/request-candidates/{request_id}",
            get(get_request_candidate_trace),
        )
        .route(
            "/_gateway/audit/request-audit/{request_id}",
            get(get_request_audit_bundle),
        )
        .route(
            "/_gateway/audit/request-usage/{request_id}",
            get(get_request_usage_audit),
        )
        .route_layer(middleware::from_fn_with_state(
            state,
            authorize_operational_request,
        ));
    router.merge(operational)
}

async fn authorize_operational_request(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response<Body> {
    let Some(permission) = operational_permission(request.method(), request.uri().path()) else {
        return operational_error_response(
            StatusCode::FORBIDDEN,
            "operational route permission is not configured",
            None,
        );
    };
    let Some(remote_addr) = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|value| value.0)
    else {
        return operational_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "operational authentication unavailable",
            None,
        );
    };
    let headers = request.headers().clone();
    let uri = request.uri().clone();
    if headers.get_all(http::header::AUTHORIZATION).iter().count() > 1 {
        return operational_auth_required_response();
    }

    match crate::control::resolve_local_admin_session_principal(&state, &headers, &uri).await {
        Ok(Some(principal)) => {
            if permission.requires_full_admin_role
                && !crate::roles::is_full_admin_role(&principal.user_role)
            {
                return operational_permission_denied_response(permission.required_permissions[0]);
            }
            if permission.write && !crate::roles::can_write_admin_console(&principal.user_role) {
                return operational_permission_denied_response(permission.required_permissions[0]);
            }
        }
        Ok(None) => {
            let client_ip = crate::headers::effective_client_ip(&headers, &remote_addr);
            let authenticated = match crate::management_token_auth::authenticate_management_token(
                &state, &headers, client_ip,
            )
            .await
            {
                Ok(authenticated) => authenticated,
                Err(
                    crate::management_token_auth::ManagementTokenAuthError::Missing
                    | crate::management_token_auth::ManagementTokenAuthError::Invalid,
                ) => return operational_auth_required_response(),
                Err(crate::management_token_auth::ManagementTokenAuthError::Unavailable) => {
                    return operational_error_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "operational authentication unavailable",
                        None,
                    )
                }
            };

            if permission.requires_full_admin_role
                && !crate::roles::is_full_admin_role(&authenticated.user.role)
            {
                return operational_permission_denied_response(permission.required_permissions[0]);
            }
            if permission.write && !crate::roles::can_write_admin_console(&authenticated.user.role)
            {
                return operational_permission_denied_response(permission.required_permissions[0]);
            }
            let missing_permission =
                permission
                    .required_permissions
                    .iter()
                    .copied()
                    .find(|required| {
                        !management_token_has_operational_permission(
                            &authenticated.permissions,
                            required,
                        )
                    });
            if let Some(required_permission) = missing_permission {
                return operational_permission_denied_response(required_permission);
            }

            let client_ip = client_ip.to_string();
            if let Err(err) = state
                .record_management_token_usage(&authenticated.token.id, Some(client_ip.as_str()))
                .await
            {
                warn!(
                    token_id = %authenticated.token.id,
                    error = ?err,
                    "gateway failed to record operational management token usage"
                );
            }
        }
        Err(err) => {
            warn!(error = %crate::error::redact_error_debug(&err), "operational admin session authentication failed");
            return operational_error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "operational authentication unavailable",
                None,
            );
        }
    }

    let mut response = next.run(request).await;
    response.headers_mut().insert(
        http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    response
}

fn operational_permission(method: &http::Method, path: &str) -> Option<OperationalPermission> {
    if path == "/_gateway/metrics" {
        return Some(OperationalPermission {
            required_permissions: &["admin:monitoring:read"],
            write: false,
            requires_full_admin_role: false,
        });
    }
    if path.starts_with("/_gateway/async-tasks/video-tasks") {
        let write = *method == http::Method::POST && path.ends_with("/cancel");
        return Some(OperationalPermission {
            required_permissions: if write {
                &["admin:video_tasks:write"]
            } else {
                &["admin:video_tasks:read"]
            },
            write,
            requires_full_admin_role: false,
        });
    }
    if path.starts_with("/_gateway/audit/auth/users/") {
        return Some(OperationalPermission {
            required_permissions: &["admin:api_keys:read"],
            write: false,
            requires_full_admin_role: false,
        });
    }
    if path.starts_with("/_gateway/audit/request-audit/") {
        return Some(OperationalPermission {
            required_permissions: &[
                "admin:monitoring:admin",
                "admin:usage:read",
                "admin:api_keys:read",
            ],
            write: false,
            requires_full_admin_role: true,
        });
    }
    if path.starts_with("/_gateway/audit/request-candidates/")
        || path.starts_with("/_gateway/audit/decision-trace/")
    {
        return Some(OperationalPermission {
            required_permissions: &["admin:monitoring:admin"],
            write: false,
            requires_full_admin_role: true,
        });
    }
    if path.starts_with("/_gateway/audit/") {
        return Some(OperationalPermission {
            required_permissions: &["admin:usage:read"],
            write: false,
            requires_full_admin_role: false,
        });
    }
    None
}

fn management_token_has_operational_permission(
    permissions: &[String],
    required_permission: &str,
) -> bool {
    let scope = required_permission
        .rsplit_once(':')
        .map(|(scope, _)| scope)
        .unwrap_or(required_permission);
    let admin_permission = format!("{scope}:admin");
    permissions
        .iter()
        .any(|permission| permission == required_permission || permission == &admin_permission)
}

fn operational_auth_required_response() -> Response<Body> {
    let mut response = operational_error_response(
        StatusCode::UNAUTHORIZED,
        "admin authentication required",
        None,
    );
    response.headers_mut().insert(
        http::header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer"),
    );
    response
}

fn operational_permission_denied_response(required_permission: &'static str) -> Response<Body> {
    operational_error_response(
        StatusCode::FORBIDDEN,
        "operational permission denied",
        Some(required_permission),
    )
}

fn operational_error_response(
    status: StatusCode,
    detail: &'static str,
    required_permission: Option<&'static str>,
) -> Response<Body> {
    let mut response = (
        status,
        Json(json!({
            "detail": detail,
            "required_permission": required_permission,
        })),
    )
        .into_response();
    response.headers_mut().insert(
        http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    response
}
