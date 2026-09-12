use super::{classified, ClassifiedRoute};
use crate::tunnel::{is_tunnel_heartbeat_path, is_tunnel_node_status_path, TUNNEL_ROUTE_FAMILY};

pub(super) fn classify_internal_route(
    method: &http::Method,
    normalized_path: &str,
) -> Option<ClassifiedRoute> {
    if normalized_path == "/api/internal/gateway"
        || normalized_path.starts_with("/api/internal/gateway/")
    {
        let route_kind = match (method, normalized_path) {
            (&http::Method::POST, "/api/internal/gateway/resolve") => "resolve",
            (&http::Method::POST, "/api/internal/gateway/auth-context") => "auth_context",
            (&http::Method::POST, "/api/internal/gateway/decision-sync") => "decision_sync",
            (&http::Method::POST, "/api/internal/gateway/decision-stream") => "decision_stream",
            (&http::Method::POST, "/api/internal/gateway/plan-sync") => "plan_sync",
            (&http::Method::POST, "/api/internal/gateway/plan-stream") => "plan_stream",
            (&http::Method::POST, "/api/internal/gateway/report-sync") => "report_sync",
            (&http::Method::POST, "/api/internal/gateway/report-stream") => "report_stream",
            (&http::Method::POST, "/api/internal/gateway/finalize-sync") => "finalize_sync",
            (&http::Method::POST, "/api/internal/gateway/execute-sync") => "execute_sync",
            (&http::Method::POST, "/api/internal/gateway/execute-stream") => "execute_stream",
            _ => "unhandled",
        };
        Some(classified(
            "internal_proxy",
            "internal_gateway",
            route_kind,
            "",
            false,
        ))
    } else if method == http::Method::POST && is_tunnel_heartbeat_path(normalized_path) {
        Some(classified(
            "internal_proxy",
            TUNNEL_ROUTE_FAMILY,
            "heartbeat",
            "",
            false,
        ))
    } else if method == http::Method::POST && is_tunnel_node_status_path(normalized_path) {
        Some(classified(
            "internal_proxy",
            TUNNEL_ROUTE_FAMILY,
            "node_status",
            "",
            false,
        ))
    } else {
        None
    }
}
