use axum::http;

use super::{classified, ClassifiedRoute};

pub(super) fn classify_admin_risk_control_family_route(
    method: &http::Method,
    normalized_path: &str,
    normalized_path_no_trailing: &str,
) -> Option<ClassifiedRoute> {
    if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/risk-control/status" | "/api/admin/risk-control/status/"
        )
    {
        Some(classified(
            "admin_proxy",
            "risk_control_manage",
            "status",
            "admin:risk_control",
            false,
        ))
    } else if matches!(
        normalized_path,
        "/api/admin/risk-control/config" | "/api/admin/risk-control/config/"
    ) && matches!(*method, http::Method::GET | http::Method::PUT)
    {
        Some(classified(
            "admin_proxy",
            "risk_control_manage",
            "config",
            "admin:risk_control",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/risk-control/logs" | "/api/admin/risk-control/logs/"
        )
    {
        Some(classified(
            "admin_proxy",
            "risk_control_manage",
            "logs",
            "admin:risk_control",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path_no_trailing.starts_with("/api/admin/risk-control/logs/")
        && normalized_path_no_trailing.ends_with("/notification/retry")
        && matches!(
            normalized_path_no_trailing["/api/admin/risk-control/logs/".len()..]
                .split('/')
                .collect::<Vec<_>>()
                .as_slice(),
            [log_id, "notification", "retry"] if !log_id.is_empty()
        )
    {
        Some(classified(
            "admin_proxy",
            "risk_control_manage",
            "retry_notification",
            "admin:risk_control",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/risk-control/hashes" | "/api/admin/risk-control/hashes/"
        )
    {
        Some(classified(
            "admin_proxy",
            "risk_control_manage",
            "hashes",
            "admin:risk_control",
            false,
        ))
    } else if method == http::Method::DELETE
        && matches!(
            normalized_path,
            "/api/admin/risk-control/hashes" | "/api/admin/risk-control/hashes/"
        )
    {
        Some(classified(
            "admin_proxy",
            "risk_control_manage",
            "clear_hashes",
            "admin:risk_control",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path_no_trailing.starts_with("/api/admin/risk-control/hashes/")
        && normalized_path_no_trailing["/api/admin/risk-control/hashes/".len()..]
            .split('/')
            .count()
            == 1
    {
        Some(classified(
            "admin_proxy",
            "risk_control_manage",
            "delete_hash",
            "admin:risk_control",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/risk-control/test" | "/api/admin/risk-control/test/"
        )
    {
        Some(classified(
            "admin_proxy",
            "risk_control_manage",
            "test",
            "admin:risk_control",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/risk-control/provider-keys/test"
                | "/api/admin/risk-control/provider-keys/test/"
        )
    {
        Some(classified(
            "admin_proxy",
            "risk_control_manage",
            "provider_keys_test",
            "admin:risk_control",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/risk-control/retention/run" | "/api/admin/risk-control/retention/run/"
        )
    {
        Some(classified(
            "admin_proxy",
            "risk_control_manage",
            "retention_run",
            "admin:risk_control",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path_no_trailing.starts_with("/api/admin/risk-control/users/")
        && normalized_path_no_trailing.ends_with("/unlock")
        && matches!(
            normalized_path_no_trailing["/api/admin/risk-control/users/".len()..]
                .split('/')
                .collect::<Vec<_>>()
                .as_slice(),
            [user_id, "api-keys", api_key_id, "unlock"]
                if !user_id.is_empty() && !api_key_id.is_empty()
        )
    {
        Some(classified(
            "admin_proxy",
            "risk_control_manage",
            "unlock_user_api_key",
            "admin:risk_control",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path_no_trailing.starts_with("/api/admin/risk-control/users/")
        && normalized_path_no_trailing.ends_with("/unban")
        && normalized_path_no_trailing["/api/admin/risk-control/users/".len()..]
            .split('/')
            .count()
            == 2
    {
        Some(classified(
            "admin_proxy",
            "risk_control_manage",
            "unban_user",
            "admin:risk_control",
            false,
        ))
    } else {
        None
    }
}
