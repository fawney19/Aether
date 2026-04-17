use super::{classified, is_gemini_models_route, is_gemini_operation_route, ClassifiedRoute};

fn has_single_segment_after_prefix(path: &str, prefix: &str) -> bool {
    let trimmed = path.trim_end_matches('/');
    let Some(segment) = trimmed.strip_prefix(prefix) else {
        return false;
    };
    !segment.is_empty() && !segment.contains('/')
}

fn has_single_nested_suffix_after_prefix(path: &str, prefix: &str, suffix: &str) -> bool {
    let trimmed = path.trim_end_matches('/');
    let Some(rest) = trimmed.strip_prefix(prefix) else {
        return false;
    };
    let Some((segment, actual_suffix)) = rest.split_once('/') else {
        return false;
    };
    !segment.is_empty() && !segment.contains('/') && actual_suffix == suffix
}

pub(super) fn classify_public_support_route(
    method: &http::Method,
    normalized_path: &str,
    public_models_auth_signature: &str,
) -> Option<ClassifiedRoute> {
    if method == http::Method::GET && normalized_path == "/v1/models" {
        Some(classified(
            "public_support",
            "models",
            "list",
            public_models_auth_signature,
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/v1/models/")
        && !is_gemini_models_route(normalized_path)
        && !is_gemini_operation_route(normalized_path)
    {
        Some(classified(
            "public_support",
            "models",
            "detail",
            public_models_auth_signature,
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/v1beta/models" {
        Some(classified(
            "public_support",
            "models",
            "list",
            public_models_auth_signature,
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/v1beta/models/")
        && !is_gemini_models_route(normalized_path)
        && !is_gemini_operation_route(normalized_path)
    {
        Some(classified(
            "public_support",
            "models",
            "detail",
            public_models_auth_signature,
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/announcements" | "/api/announcements/"
        )
    {
        Some(classified(
            "admin_proxy",
            "announcements_manage",
            "create_announcement",
            "admin:announcements",
            false,
        ))
    } else if method == http::Method::PUT
        && normalized_path.starts_with("/api/announcements/")
        && normalized_path != "/api/announcements/active"
        && normalized_path != "/api/announcements/users/me/unread-count"
        && normalized_path != "/api/announcements/users/me/unread-count/"
        && !normalized_path.ends_with("/read-status")
    {
        Some(classified(
            "admin_proxy",
            "announcements_manage",
            "update_announcement",
            "admin:announcements",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/announcements/")
        && normalized_path != "/api/announcements/active"
        && normalized_path != "/api/announcements/users/me/unread-count"
        && normalized_path != "/api/announcements/users/me/unread-count/"
        && !normalized_path.ends_with("/read-status")
    {
        Some(classified(
            "admin_proxy",
            "announcements_manage",
            "delete_announcement",
            "admin:announcements",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/announcements" | "/api/announcements/"
        )
    {
        Some(classified(
            "public_support",
            "announcements",
            "list",
            "public:announcements",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/announcements/active" | "/api/announcements/active/"
        )
    {
        Some(classified(
            "public_support",
            "announcements",
            "active",
            "public:announcements",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/announcements/")
        && normalized_path != "/api/announcements/active"
        && normalized_path != "/api/announcements/users/me/unread-count"
        && normalized_path != "/api/announcements/users/me/unread-count/"
        && !normalized_path.ends_with("/read-status")
        && has_single_segment_after_prefix(normalized_path, "/api/announcements/")
    {
        Some(classified(
            "public_support",
            "announcements",
            "detail",
            "public:announcements",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/public/site-info"
                | "/api/public/providers"
                | "/api/public/models"
                | "/api/public/search/models"
                | "/api/public/stats"
                | "/api/public/global-models"
                | "/api/public/health/api-formats"
        )
    {
        let route_kind = match normalized_path {
            "/api/public/site-info" => "site_info",
            "/api/public/providers" => "providers",
            "/api/public/models" => "models",
            "/api/public/search/models" => "search_models",
            "/api/public/stats" => "stats",
            "/api/public/global-models" => "global_models",
            "/api/public/health/api-formats" => "health_api_formats",
            _ => "site_info",
        };
        Some(classified(
            "public_support",
            "public_catalog",
            route_kind,
            "public:catalog",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/auth/registration-settings" | "/api/auth/settings"
        )
    {
        let route_kind = match normalized_path {
            "/api/auth/registration-settings" => "registration_settings",
            "/api/auth/settings" => "settings",
            _ => "settings",
        };
        Some(classified(
            "public_support",
            "auth_public",
            route_kind,
            "public:auth",
            false,
        ))
    } else if matches!(method, &http::Method::GET | &http::Method::POST)
        && matches!(
            normalized_path,
            "/api/auth/login"
                | "/api/auth/refresh"
                | "/api/auth/register"
                | "/api/auth/me"
                | "/api/auth/logout"
                | "/api/auth/send-verification-code"
                | "/api/auth/verify-email"
                | "/api/auth/verification-status"
        )
    {
        let route_kind = match normalized_path {
            "/api/auth/login" => "login",
            "/api/auth/refresh" => "refresh",
            "/api/auth/register" => "register",
            "/api/auth/me" => "me",
            "/api/auth/logout" => "logout",
            "/api/auth/send-verification-code" => "send_verification_code",
            "/api/auth/verify-email" => "verify_email",
            "/api/auth/verification-status" => "verification_status",
            _ => "login",
        };
        Some(classified(
            "public_support",
            "auth",
            route_kind,
            "user:auth",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/dashboard/stats"
                | "/api/dashboard/recent-requests"
                | "/api/dashboard/provider-status"
                | "/api/dashboard/daily-stats"
        )
    {
        let route_kind = match normalized_path {
            "/api/dashboard/stats" => "stats",
            "/api/dashboard/recent-requests" => "recent_requests",
            "/api/dashboard/provider-status" => "provider_status",
            "/api/dashboard/daily-stats" => "daily_stats",
            _ => "stats",
        };
        Some(classified(
            "public_support",
            "dashboard",
            route_kind,
            "user:dashboard",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/monitoring/my-audit-logs" | "/api/monitoring/rate-limit-status"
        )
    {
        let route_kind = match normalized_path {
            "/api/monitoring/my-audit-logs" => "audit_logs",
            "/api/monitoring/rate-limit-status" => "rate_limit_status",
            _ => "audit_logs",
        };
        Some(classified(
            "public_support",
            "monitoring_user",
            route_kind,
            "user:monitoring",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/announcements/users/me/unread-count"
                | "/api/announcements/users/me/unread-count/"
        )
    {
        Some(classified(
            "public_support",
            "announcement_user",
            "unread_count",
            "user:announcements",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/announcements/read-all" | "/api/announcements/read-all/"
        )
    {
        Some(classified(
            "public_support",
            "announcement_user",
            "read_all",
            "user:announcements",
            false,
        ))
    } else if method == http::Method::PATCH
        && normalized_path.starts_with("/api/announcements/")
        && (normalized_path.ends_with("/read-status") || normalized_path.ends_with("/read-status/"))
        && has_single_nested_suffix_after_prefix(
            normalized_path,
            "/api/announcements/",
            "read-status",
        )
    {
        Some(classified(
            "public_support",
            "announcement_user",
            "read_status",
            "user:announcements",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/wallet/balance"
                | "/api/wallet/transactions"
                | "/api/wallet/flow"
                | "/api/wallet/today-cost"
                | "/api/wallet/recharge"
                | "/api/wallet/refunds"
        )
    {
        let route_kind = match normalized_path {
            "/api/wallet/balance" => "balance",
            "/api/wallet/transactions" => "transactions",
            "/api/wallet/flow" => "flow",
            "/api/wallet/today-cost" => "today_cost",
            "/api/wallet/recharge" => "list_recharge_orders",
            "/api/wallet/refunds" => "list_refunds",
            _ => "balance",
        };
        Some(classified(
            "public_support",
            "wallet",
            route_kind,
            "user:wallet",
            false,
        ))
    } else if method == http::Method::GET
        && has_single_segment_after_prefix(normalized_path, "/api/wallet/recharge/")
    {
        Some(classified(
            "public_support",
            "wallet",
            "recharge_detail",
            "user:wallet",
            false,
        ))
    } else if method == http::Method::GET
        && has_single_segment_after_prefix(normalized_path, "/api/wallet/refunds/")
    {
        Some(classified(
            "public_support",
            "wallet",
            "refund_detail",
            "user:wallet",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/wallet/recharge" | "/api/wallet/refunds" | "/api/wallet/redeem"
        )
    {
        let route_kind = match normalized_path {
            "/api/wallet/recharge" => "create_recharge_order",
            "/api/wallet/refunds" => "create_refund",
            "/api/wallet/redeem" => "redeem",
            _ => "create_recharge_order",
        };
        Some(classified(
            "public_support",
            "wallet",
            route_kind,
            "user:wallet",
            false,
        ))
    } else if method == http::Method::POST
        && has_single_segment_after_prefix(normalized_path, "/api/payment/callback/")
    {
        Some(classified(
            "public_support",
            "payment_callback",
            "callback",
            "public:payment",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/users/me"
                | "/api/users/me/sessions"
                | "/api/users/me/api-keys"
                | "/api/users/me/usage"
                | "/api/users/me/usage/active"
                | "/api/users/me/usage/interval-timeline"
                | "/api/users/me/usage/heatmap"
                | "/api/users/me/providers"
                | "/api/users/me/available-models"
                | "/api/users/me/endpoint-status"
                | "/api/users/me/preferences"
                | "/api/users/me/model-capabilities"
        )
    {
        let route_kind = match normalized_path {
            "/api/users/me" => "detail",
            "/api/users/me/sessions" => "sessions",
            "/api/users/me/api-keys" => "api_keys_list",
            "/api/users/me/usage" => "usage",
            "/api/users/me/usage/active" => "usage_active",
            "/api/users/me/usage/interval-timeline" => "usage_interval_timeline",
            "/api/users/me/usage/heatmap" => "usage_heatmap",
            "/api/users/me/providers" => "providers",
            "/api/users/me/available-models" => "available_models",
            "/api/users/me/endpoint-status" => "endpoint_status",
            "/api/users/me/preferences" => "preferences",
            "/api/users/me/model-capabilities" => "model_capabilities",
            _ => "detail",
        };
        Some(classified(
            "public_support",
            "users_me",
            route_kind,
            "user:self",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/me/management-tokens" | "/api/me/management-tokens/"
        )
    {
        Some(classified(
            "public_support",
            "users_me",
            "management_tokens_list",
            "user:self",
            false,
        ))
    } else if method == http::Method::PUT
        && matches!(
            normalized_path,
            "/api/users/me" | "/api/users/me/preferences" | "/api/users/me/model-capabilities"
        )
    {
        let route_kind = match normalized_path {
            "/api/users/me" => "update_detail",
            "/api/users/me/preferences" => "preferences_update",
            "/api/users/me/model-capabilities" => "model_capabilities_update",
            _ => "update_detail",
        };
        Some(classified(
            "public_support",
            "users_me",
            route_kind,
            "user:self",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/me/management-tokens" | "/api/me/management-tokens/"
        )
    {
        Some(classified(
            "public_support",
            "users_me",
            "management_tokens_create",
            "user:self",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/me/management-tokens/")
        && normalized_path.ends_with("/regenerate")
    {
        Some(classified(
            "public_support",
            "users_me",
            "management_token_regenerate",
            "user:self",
            false,
        ))
    } else if method == http::Method::PATCH && normalized_path == "/api/users/me/password" {
        Some(classified(
            "public_support",
            "users_me",
            "password",
            "user:self",
            false,
        ))
    } else if method == http::Method::PATCH
        && normalized_path.starts_with("/api/me/management-tokens/")
        && normalized_path.ends_with("/status")
    {
        Some(classified(
            "public_support",
            "users_me",
            "management_token_toggle",
            "user:self",
            false,
        ))
    } else if method == http::Method::DELETE && normalized_path == "/api/users/me/sessions/others" {
        Some(classified(
            "public_support",
            "users_me",
            "sessions_others_delete",
            "user:self",
            false,
        ))
    } else if matches!(method, &http::Method::PATCH | &http::Method::DELETE)
        && has_single_segment_after_prefix(normalized_path, "/api/users/me/sessions/")
    {
        let route_kind = if method == http::Method::PATCH {
            "session_update"
        } else {
            "session_delete"
        };
        Some(classified(
            "public_support",
            "users_me",
            route_kind,
            "user:self",
            false,
        ))
    } else if matches!(method, &http::Method::GET | &http::Method::POST)
        && normalized_path == "/api/users/me/api-keys"
    {
        let route_kind = if method == http::Method::GET {
            "api_keys_list"
        } else {
            "api_keys_create"
        };
        Some(classified(
            "public_support",
            "users_me",
            route_kind,
            "user:self",
            false,
        ))
    } else if method == http::Method::PUT
        && (has_single_nested_suffix_after_prefix(
            normalized_path,
            "/api/users/me/api-keys/",
            "providers",
        ) || has_single_nested_suffix_after_prefix(
            normalized_path,
            "/api/users/me/api-keys/",
            "capabilities",
        ))
    {
        let route_kind = if normalized_path.ends_with("/providers") {
            "api_key_providers_update"
        } else {
            "api_key_capabilities_update"
        };
        Some(classified(
            "public_support",
            "users_me",
            route_kind,
            "user:self",
            false,
        ))
    } else if matches!(
        method,
        &http::Method::GET | &http::Method::PUT | &http::Method::PATCH | &http::Method::DELETE
    ) && has_single_segment_after_prefix(normalized_path, "/api/users/me/api-keys/")
    {
        let route_kind = match *method {
            http::Method::GET => "api_key_detail",
            http::Method::PUT => "api_key_update",
            http::Method::PATCH => "api_key_patch",
            http::Method::DELETE => "api_key_delete",
            _ => "api_key_detail",
        };
        Some(classified(
            "public_support",
            "users_me",
            route_kind,
            "user:self",
            false,
        ))
    } else if matches!(
        method,
        &http::Method::GET | &http::Method::PUT | &http::Method::DELETE
    ) && has_single_segment_after_prefix(normalized_path, "/api/me/management-tokens/")
    {
        let route_kind = match *method {
            http::Method::GET => "management_token_detail",
            http::Method::PUT => "management_token_update",
            http::Method::DELETE => "management_token_delete",
            _ => "management_token_detail",
        };
        Some(classified(
            "public_support",
            "users_me",
            route_kind,
            "user:self",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/capabilities" | "/api/capabilities/user-configurable"
        )
    {
        let route_kind = match normalized_path {
            "/api/capabilities" => "list",
            "/api/capabilities/user-configurable" => "user_configurable",
            _ => "list",
        };
        Some(classified(
            "public_support",
            "capabilities",
            route_kind,
            "public:capabilities",
            false,
        ))
    } else if method == http::Method::GET && normalized_path.starts_with("/api/capabilities/model/")
    {
        Some(classified(
            "public_support",
            "capabilities",
            "model",
            "public:capabilities",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/modules/auth-status" {
        Some(classified(
            "public_support",
            "modules",
            "auth_status",
            "public:modules",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/" | "/health" | "/v1/health" | "/v1/providers" | "/v1/test-connection"
        )
    {
        let route_kind = match normalized_path {
            "/" => "root",
            "/health" | "/v1/health" => "health",
            "/v1/providers" => "providers",
            "/v1/test-connection" => "test_connection",
            _ => "root",
        };
        Some(classified(
            "public_support",
            "system_catalog",
            route_kind,
            "public:system_catalog",
            false,
        ))
    } else if method == http::Method::GET && normalized_path.starts_with("/v1/providers/") {
        Some(classified(
            "public_support",
            "system_catalog",
            "provider_detail",
            "public:system_catalog",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/test-connection" {
        Some(classified(
            "public_support",
            "system_catalog",
            "test_connection",
            "public:system_catalog",
            false,
        ))
    } else {
        None
    }
}
