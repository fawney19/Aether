use axum::http;

use super::{classified, ClassifiedRoute};

pub(super) fn classify_admin_basic_family_route(
    method: &http::Method,
    normalized_path: &str,
    normalized_path_no_trailing: &str,
) -> Option<ClassifiedRoute> {
    if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/management-tokens" | "/api/admin/management-tokens/"
        )
    {
        Some(classified(
            "admin_proxy",
            "management_tokens_manage",
            "list_tokens",
            "admin:management_tokens",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/management-tokens/")
    {
        Some(classified(
            "admin_proxy",
            "management_tokens_manage",
            "get_token",
            "admin:management_tokens",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/management-tokens/")
    {
        Some(classified(
            "admin_proxy",
            "management_tokens_manage",
            "delete_token",
            "admin:management_tokens",
            false,
        ))
    } else if method == http::Method::PATCH
        && normalized_path.starts_with("/api/admin/management-tokens/")
        && normalized_path.ends_with("/status")
    {
        Some(classified(
            "admin_proxy",
            "management_tokens_manage",
            "toggle_status",
            "admin:management_tokens",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/ldap/config" | "/api/admin/ldap/config/"
        )
    {
        Some(classified(
            "admin_proxy",
            "ldap_manage",
            "get_config",
            "admin:ldap",
            false,
        ))
    } else if method == http::Method::PUT
        && matches!(
            normalized_path,
            "/api/admin/ldap/config" | "/api/admin/ldap/config/"
        )
    {
        Some(classified(
            "admin_proxy",
            "ldap_manage",
            "set_config",
            "admin:ldap",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/ldap/test" | "/api/admin/ldap/test/"
        )
    {
        Some(classified(
            "admin_proxy",
            "ldap_manage",
            "test_connection",
            "admin:ldap",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/gemini-files/mappings" | "/api/admin/gemini-files/mappings/"
        )
    {
        Some(classified(
            "admin_proxy",
            "gemini_files_manage",
            "list_mappings",
            "admin:gemini_files",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/gemini-files/stats" | "/api/admin/gemini-files/stats/"
        )
    {
        Some(classified(
            "admin_proxy",
            "gemini_files_manage",
            "stats",
            "admin:gemini_files",
            false,
        ))
    } else if method == http::Method::DELETE
        && matches!(
            normalized_path,
            "/api/admin/gemini-files/mappings" | "/api/admin/gemini-files/mappings/"
        )
    {
        Some(classified(
            "admin_proxy",
            "gemini_files_manage",
            "cleanup_mappings",
            "admin:gemini_files",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/gemini-files/mappings/")
    {
        Some(classified(
            "admin_proxy",
            "gemini_files_manage",
            "delete_mapping",
            "admin:gemini_files",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/gemini-files/capable-keys" | "/api/admin/gemini-files/capable-keys/"
        )
    {
        Some(classified(
            "admin_proxy",
            "gemini_files_manage",
            "capable_keys",
            "admin:gemini_files",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/gemini-files/upload" | "/api/admin/gemini-files/upload/"
        )
    {
        Some(classified(
            "admin_proxy",
            "gemini_files_manage",
            "upload",
            "admin:gemini_files",
            false,
        ))
    } else if method == http::Method::GET && normalized_path == "/api/admin/modules/status" {
        Some(classified(
            "admin_proxy",
            "modules_manage",
            "status_list",
            "admin:modules",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/modules/status/")
    {
        Some(classified(
            "admin_proxy",
            "modules_manage",
            "status_detail",
            "admin:modules",
            false,
        ))
    } else if method == http::Method::PUT
        && normalized_path.starts_with("/api/admin/modules/status/")
        && normalized_path.ends_with("/enabled")
    {
        Some(classified(
            "admin_proxy",
            "modules_manage",
            "set_enabled",
            "admin:modules",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/adaptive/keys" | "/api/admin/adaptive/keys/"
        )
    {
        Some(classified(
            "admin_proxy",
            "adaptive_manage",
            "list_keys",
            "admin:adaptive",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/adaptive/summary" | "/api/admin/adaptive/summary/"
        )
    {
        Some(classified(
            "admin_proxy",
            "adaptive_manage",
            "summary",
            "admin:adaptive",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/adaptive/keys/")
        && normalized_path.ends_with("/stats")
        && normalized_path.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "adaptive_manage",
            "get_stats",
            "admin:adaptive",
            false,
        ))
    } else if method == http::Method::PATCH
        && normalized_path.starts_with("/api/admin/adaptive/keys/")
        && normalized_path.ends_with("/mode")
        && normalized_path.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "adaptive_manage",
            "toggle_mode",
            "admin:adaptive",
            false,
        ))
    } else if method == http::Method::PATCH
        && normalized_path.starts_with("/api/admin/adaptive/keys/")
        && normalized_path.ends_with("/limit")
        && normalized_path.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "adaptive_manage",
            "set_limit",
            "admin:adaptive",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/adaptive/keys/")
        && normalized_path.ends_with("/learning")
        && normalized_path.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "adaptive_manage",
            "reset_learning",
            "admin:adaptive",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/provider-strategy/strategies" | "/api/admin/provider-strategy/strategies/"
        )
    {
        Some(classified(
            "admin_proxy",
            "provider_strategy_manage",
            "list_strategies",
            "admin:provider_strategy",
            false,
        ))
    } else if method == http::Method::PUT
        && normalized_path.starts_with("/api/admin/provider-strategy/providers/")
        && normalized_path.ends_with("/billing")
    {
        Some(classified(
            "admin_proxy",
            "provider_strategy_manage",
            "update_provider_billing",
            "admin:provider_strategy",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/provider-strategy/providers/")
        && normalized_path.ends_with("/stats")
    {
        Some(classified(
            "admin_proxy",
            "provider_strategy_manage",
            "get_provider_stats",
            "admin:provider_strategy",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/provider-strategy/providers/")
        && normalized_path.ends_with("/quota")
        && normalized_path.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "provider_strategy_manage",
            "reset_provider_quota",
            "admin:provider_strategy",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/billing/presets" | "/api/admin/billing/presets/"
        )
    {
        Some(classified(
            "admin_proxy",
            "billing_manage",
            "list_presets",
            "admin:billing",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/billing/presets/apply" | "/api/admin/billing/presets/apply/"
        )
    {
        Some(classified(
            "admin_proxy",
            "billing_manage",
            "apply_preset",
            "admin:billing",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/billing/rules" | "/api/admin/billing/rules/"
        )
    {
        Some(classified(
            "admin_proxy",
            "billing_manage",
            "list_rules",
            "admin:billing",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/billing/rules/")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "billing_manage",
            "get_rule",
            "admin:billing",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/billing/rules" | "/api/admin/billing/rules/"
        )
    {
        Some(classified(
            "admin_proxy",
            "billing_manage",
            "create_rule",
            "admin:billing",
            false,
        ))
    } else if method == http::Method::PUT
        && normalized_path.starts_with("/api/admin/billing/rules/")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "billing_manage",
            "update_rule",
            "admin:billing",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/billing/collectors" | "/api/admin/billing/collectors/"
        )
    {
        Some(classified(
            "admin_proxy",
            "billing_manage",
            "list_collectors",
            "admin:billing",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/billing/collectors/")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "billing_manage",
            "get_collector",
            "admin:billing",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/billing/collectors" | "/api/admin/billing/collectors/"
        )
    {
        Some(classified(
            "admin_proxy",
            "billing_manage",
            "create_collector",
            "admin:billing",
            false,
        ))
    } else if method == http::Method::PUT
        && normalized_path.starts_with("/api/admin/billing/collectors/")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "billing_manage",
            "update_collector",
            "admin:billing",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/payments/orders" | "/api/admin/payments/orders/"
        )
    {
        Some(classified(
            "admin_proxy",
            "payments_manage",
            "list_orders",
            "admin:payments",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path_no_trailing.starts_with("/api/admin/payments/orders/")
        && normalized_path_no_trailing.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "payments_manage",
            "get_order",
            "admin:payments",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path_no_trailing.starts_with("/api/admin/payments/orders/")
        && normalized_path_no_trailing.ends_with("/expire")
        && normalized_path_no_trailing.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "payments_manage",
            "expire_order",
            "admin:payments",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path_no_trailing.starts_with("/api/admin/payments/orders/")
        && normalized_path_no_trailing.ends_with("/credit")
        && normalized_path_no_trailing.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "payments_manage",
            "credit_order",
            "admin:payments",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path_no_trailing.starts_with("/api/admin/payments/orders/")
        && normalized_path_no_trailing.ends_with("/fail")
        && normalized_path_no_trailing.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "payments_manage",
            "fail_order",
            "admin:payments",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/payments/callbacks" | "/api/admin/payments/callbacks/"
        )
    {
        Some(classified(
            "admin_proxy",
            "payments_manage",
            "list_callbacks",
            "admin:payments",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/payments/redeem-codes/batches"
                | "/api/admin/payments/redeem-codes/batches/"
        )
    {
        Some(classified(
            "admin_proxy",
            "payments_manage",
            "list_redeem_code_batches",
            "admin:payments",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/payments/redeem-codes/batches"
                | "/api/admin/payments/redeem-codes/batches/"
        )
    {
        Some(classified(
            "admin_proxy",
            "payments_manage",
            "create_redeem_code_batch",
            "admin:payments",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path_no_trailing.starts_with("/api/admin/payments/redeem-codes/batches/")
        && normalized_path_no_trailing.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "payments_manage",
            "get_redeem_code_batch",
            "admin:payments",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path_no_trailing.starts_with("/api/admin/payments/redeem-codes/batches/")
        && normalized_path_no_trailing.ends_with("/codes")
        && normalized_path_no_trailing.matches('/').count() == 7
    {
        Some(classified(
            "admin_proxy",
            "payments_manage",
            "list_redeem_codes",
            "admin:payments",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path_no_trailing.starts_with("/api/admin/payments/redeem-codes/batches/")
        && normalized_path_no_trailing.ends_with("/disable")
        && normalized_path_no_trailing.matches('/').count() == 7
    {
        Some(classified(
            "admin_proxy",
            "payments_manage",
            "disable_redeem_code_batch",
            "admin:payments",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path_no_trailing.starts_with("/api/admin/payments/redeem-codes/batches/")
        && normalized_path_no_trailing.ends_with("/delete")
        && normalized_path_no_trailing.matches('/').count() == 7
    {
        Some(classified(
            "admin_proxy",
            "payments_manage",
            "delete_redeem_code_batch",
            "admin:payments",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path_no_trailing.starts_with("/api/admin/payments/redeem-codes/codes/")
        && normalized_path_no_trailing.ends_with("/disable")
        && normalized_path_no_trailing.matches('/').count() == 7
    {
        Some(classified(
            "admin_proxy",
            "payments_manage",
            "disable_redeem_code",
            "admin:payments",
            false,
        ))
    } else {
        None
    }
}
