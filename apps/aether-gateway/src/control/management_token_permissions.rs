use axum::http;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;

use super::GatewayControlDecision;

#[derive(Debug, Clone, Copy)]
struct PermissionGroup {
    scope: &'static str,
    label: &'static str,
    assignable: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct ManagementTokenPermissionCatalogItem {
    pub(crate) key: &'static str,
    pub(crate) scope: &'static str,
    pub(crate) scope_label: &'static str,
    pub(crate) access: &'static str,
    pub(crate) access_label: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagementTokenPermissionDenied {
    pub(crate) required_permission: String,
}

const PERMISSION_GROUPS: &[PermissionGroup] = &[
    PermissionGroup {
        scope: "adaptive",
        label: "自适应调度",
        assignable: true,
    },
    PermissionGroup {
        scope: "announcements",
        label: "公告",
        assignable: true,
    },
    PermissionGroup {
        scope: "api_keys",
        label: "API 密钥",
        assignable: true,
    },
    PermissionGroup {
        scope: "billing",
        label: "账单",
        assignable: true,
    },
    PermissionGroup {
        scope: "endpoints_health",
        label: "端点健康",
        assignable: true,
    },
    PermissionGroup {
        scope: "endpoints_manage",
        label: "端点配置",
        assignable: true,
    },
    PermissionGroup {
        scope: "endpoints_rpm",
        label: "端点 RPM",
        assignable: true,
    },
    PermissionGroup {
        scope: "gemini_files",
        label: "Gemini 文件",
        assignable: true,
    },
    PermissionGroup {
        scope: "ldap",
        label: "LDAP",
        assignable: true,
    },
    PermissionGroup {
        scope: "management_tokens",
        label: "访问令牌",
        assignable: false,
    },
    PermissionGroup {
        scope: "models",
        label: "模型",
        assignable: true,
    },
    PermissionGroup {
        scope: "modules",
        label: "模块管理",
        assignable: true,
    },
    PermissionGroup {
        scope: "monitoring",
        label: "监控",
        assignable: true,
    },
    PermissionGroup {
        scope: "oauth",
        label: "OAuth 配置",
        assignable: true,
    },
    PermissionGroup {
        scope: "payments",
        label: "支付",
        assignable: true,
    },
    PermissionGroup {
        scope: "pool",
        label: "号池",
        assignable: true,
    },
    PermissionGroup {
        scope: "provider_ops",
        label: "Provider 运维",
        assignable: true,
    },
    PermissionGroup {
        scope: "provider_oauth",
        label: "Provider OAuth",
        assignable: true,
    },
    PermissionGroup {
        scope: "provider_query",
        label: "Provider 查询",
        assignable: true,
    },
    PermissionGroup {
        scope: "provider_strategy",
        label: "Provider 策略",
        assignable: true,
    },
    PermissionGroup {
        scope: "providers",
        label: "供应商与模型",
        assignable: true,
    },
    PermissionGroup {
        scope: "proxy_nodes",
        label: "代理节点",
        assignable: true,
    },
    PermissionGroup {
        scope: "routing_profiles",
        label: "调度分组",
        assignable: true,
    },
    PermissionGroup {
        scope: "security",
        label: "安全",
        assignable: true,
    },
    PermissionGroup {
        scope: "stats",
        label: "统计",
        assignable: true,
    },
    PermissionGroup {
        scope: "system",
        label: "系统",
        assignable: true,
    },
    PermissionGroup {
        scope: "tasks",
        label: "后台任务",
        assignable: true,
    },
    PermissionGroup {
        scope: "usage",
        label: "用量",
        assignable: true,
    },
    PermissionGroup {
        scope: "users",
        label: "用户",
        assignable: true,
    },
    PermissionGroup {
        scope: "video_tasks",
        label: "视频任务",
        assignable: true,
    },
    PermissionGroup {
        scope: "wallets",
        label: "钱包",
        assignable: true,
    },
];

const ACCESS_LEVELS: &[(&str, &str)] = &[("read", "读取"), ("write", "写入"), ("admin", "管理")];

// Freeze the implicit permissions granted before per-token permissions were
// introduced. New scopes must never expand legacy NULL-permission tokens. The
// management_tokens scope is deliberately withheld so a legacy token cannot
// mint an unrestricted replacement that outlives its own IP or expiry bounds.
const LEGACY_FULL_PERMISSION_SCOPES: &[&str] = &[
    "adaptive",
    "announcements",
    "api_keys",
    "billing",
    "endpoints_health",
    "endpoints_manage",
    "endpoints_rpm",
    "gemini_files",
    "ldap",
    "models",
    "modules",
    "monitoring",
    "oauth",
    "payments",
    "pool",
    "provider_ops",
    "provider_oauth",
    "provider_query",
    "provider_strategy",
    "providers",
    "proxy_nodes",
    "security",
    "stats",
    "system",
    "usage",
    "users",
    "video_tasks",
    "wallets",
];

pub(crate) fn legacy_full_management_token_permissions() -> Vec<String> {
    LEGACY_FULL_PERMISSION_SCOPES
        .iter()
        .flat_map(|scope| {
            ACCESS_LEVELS
                .iter()
                .map(move |(access, _)| permission_key(scope, access).to_string())
        })
        .collect()
}

pub(crate) fn management_token_permission_catalog_items(
) -> Vec<ManagementTokenPermissionCatalogItem> {
    PERMISSION_GROUPS
        .iter()
        .filter(|group| group.assignable)
        .flat_map(|group| {
            ACCESS_LEVELS.iter().map(move |(access, access_label)| {
                ManagementTokenPermissionCatalogItem {
                    key: permission_key(group.scope, access),
                    scope: group.scope,
                    scope_label: group.label,
                    access,
                    access_label,
                }
            })
        })
        .collect()
}

pub(crate) fn management_token_permission_catalog_payload() -> Value {
    let items = management_token_permission_catalog_items();
    json!({
        "items": items,
        "all_permissions": all_assignable_management_token_permissions(),
        "read_only_permissions": read_only_management_token_permissions(),
    })
}

pub(crate) fn all_assignable_management_token_permissions() -> Vec<String> {
    management_token_permission_catalog_items()
        .into_iter()
        .map(|item| item.key.to_string())
        .collect()
}

pub(crate) fn read_only_management_token_permissions() -> Vec<String> {
    PERMISSION_GROUPS
        .iter()
        .filter(|group| group.assignable)
        .map(|group| permission_key(group.scope, "read").to_string())
        .collect()
}

pub(crate) fn audit_admin_read_only_management_token_permissions() -> Vec<String> {
    let mut permissions = read_only_management_token_permissions()
        .into_iter()
        .collect::<BTreeSet<_>>();
    permissions.extend(
        PERMISSION_GROUPS
            .iter()
            .filter(|group| !group.assignable)
            .map(|group| permission_key(group.scope, "read").to_string()),
    );
    permissions.into_iter().collect()
}

pub(crate) fn normalize_assignable_management_token_permissions(
    value: Option<&Value>,
) -> Result<Value, String> {
    let Some(value) = value else {
        return Ok(json!(all_assignable_management_token_permissions()));
    };
    if value.is_null() {
        return Err("permissions 必须是非空字符串数组；省略该字段可使用默认权限".to_string());
    }
    let Some(items) = value.as_array() else {
        return Err("permissions 必须是字符串数组".to_string());
    };
    if items.is_empty() {
        return Err("permissions 不能为空".to_string());
    }

    let mut normalized = BTreeSet::new();
    for item in items {
        let Some(raw) = item.as_str() else {
            return Err("permissions 必须是字符串数组".to_string());
        };
        let key = raw.trim();
        if key.is_empty() {
            return Err("permissions 不能包含空字符串".to_string());
        }
        if !is_assignable_management_token_permission(key) {
            return Err(format!("无效的管理令牌权限: {key}"));
        }
        normalized.insert(key.to_string());
    }

    Ok(json!(normalized.into_iter().collect::<Vec<_>>()))
}

pub(crate) fn management_token_permission_keys_from_value(
    value: Option<&Value>,
) -> Result<Option<Vec<String>>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Err("management token permissions JSON null is invalid".to_string());
    }
    let Some(items) = value.as_array() else {
        return Err("management token permissions must be an array".to_string());
    };
    if items.is_empty() {
        return Err("management token permissions must not be empty".to_string());
    }
    let mut keys = Vec::with_capacity(items.len());
    for item in items {
        let Some(key) = item.as_str() else {
            return Err("management token permissions must contain strings".to_string());
        };
        if !is_assignable_management_token_permission(key) {
            return Err(format!("unknown management token permission: {key}"));
        }
        keys.push(key.to_string());
    }
    Ok(Some(keys))
}

pub(crate) fn management_token_permission_mode_and_summary(
    permissions: Option<&Value>,
) -> (&'static str, String) {
    let keys = match management_token_permission_keys_from_value(permissions) {
        Ok(Some(keys)) => keys,
        Ok(None) => return ("legacy_full", "旧版全权限".to_string()),
        Err(_) => return ("custom", "权限配置异常".to_string()),
    };
    let key_set = keys.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let all = all_assignable_management_token_permissions();
    let read_only = read_only_management_token_permissions();
    if all.iter().all(|key| key_set.contains(key.as_str())) {
        return ("full", "全权限".to_string());
    }
    if !keys.is_empty() && keys.iter().all(|key| key.ends_with(":read")) {
        return ("read_only", "只读".to_string());
    }
    if read_only.iter().all(|key| key_set.contains(key.as_str()))
        && keys.iter().any(|key| !key.ends_with(":read"))
    {
        return ("custom", format!("自定义 {} 项（含全部读取）", keys.len()));
    }
    ("custom", format!("自定义 {} 项", keys.len()))
}

pub(crate) fn management_token_required_permission(
    method: &http::Method,
    decision: &GatewayControlDecision,
) -> Option<String> {
    let signature = decision.auth_endpoint_signature.as_deref()?.trim();
    let scope = signature.strip_prefix("admin:")?.trim();
    if scope.is_empty() {
        return None;
    }
    Some(format!(
        "admin:{scope}:{}",
        access_for_route(method, decision)
    ))
}

pub(crate) fn validate_management_token_admin_route_permission(
    method: &http::Method,
    decision: &GatewayControlDecision,
    token_permissions: Option<&[String]>,
) -> Result<(), ManagementTokenPermissionDenied> {
    let Some(token_permissions) = token_permissions else {
        return Ok(());
    };
    let Some(required_permission) = management_token_required_permission(method, decision) else {
        return if decision.route_class.as_deref() == Some("admin_proxy") {
            Err(ManagementTokenPermissionDenied {
                required_permission: "admin:unknown:admin".to_string(),
            })
        } else {
            Ok(())
        };
    };
    let scope = required_permission
        .strip_prefix("admin:")
        .and_then(|value| value.rsplit_once(':').map(|(scope, _)| scope))
        .unwrap_or_default();
    let admin_permission = format!("admin:{scope}:admin");
    if token_permissions
        .iter()
        .any(|permission| permission == &required_permission || permission == &admin_permission)
    {
        Ok(())
    } else {
        Err(ManagementTokenPermissionDenied {
            required_permission,
        })
    }
}

pub(crate) fn management_token_principal_has_permission(
    decision: &GatewayControlDecision,
    required_permission: &str,
) -> bool {
    let Some(principal) = decision.admin_principal.as_ref() else {
        return false;
    };
    if principal.management_token_id.is_none() {
        return true;
    }

    let legacy_permissions;
    let permissions = match principal.management_token_permissions.as_deref() {
        Some(permissions) => permissions,
        None => {
            legacy_permissions = legacy_full_management_token_permissions();
            legacy_permissions.as_slice()
        }
    };
    permissions
        .iter()
        .any(|permission| permission == required_permission)
}

fn access_for_method(method: &http::Method) -> &'static str {
    if matches!(
        *method,
        http::Method::GET | http::Method::HEAD | http::Method::OPTIONS
    ) {
        "read"
    } else {
        "write"
    }
}

fn access_for_route(method: &http::Method, decision: &GatewayControlDecision) -> &'static str {
    let signature = decision.auth_endpoint_signature.as_deref();
    let route_kind = decision.route_kind.as_deref();
    let requires_admin_permission = matches!(
        (signature, route_kind),
        (
            Some("admin:system"),
            Some(
                "prepare_update"
                    | "apply_update"
                    | "rollback"
                    | "config_export"
                    | "users_export"
                    | "data_export"
                    | "s3_backup_run"
                    | "config_import"
                    | "users_import"
                    | "data_import"
                    | "smtp_test"
                    | "important_notification_test"
                    | "cleanup"
                    | "cleanup_usage_manual"
                    | "purge_config"
                    | "purge_users"
                    | "purge_usage"
                    | "purge_audit_logs"
                    | "purge_request_bodies"
                    | "purge_request_bodies_task"
                    | "purge_stats"
                    | "settings_set"
                    | "config_set"
                    | "config_delete"
            )
        ) | (
            Some("admin:endpoints_manage"),
            Some(
                "reveal_key"
                    | "reveal_endpoint_rules"
                    | "export_key"
                    | "create_provider_key"
                    | "update_key"
                    | "create_endpoint"
                    | "update_endpoint"
                    | "refresh_quota"
                    | "codex_reset_credit_consume"
            )
        ) | (Some("admin:providers"), Some("update_provider"))
            | (
                Some("admin:provider_query"),
                Some("query_models" | "test_model" | "test_model_failover")
            )
            | (
                Some("admin:provider_oauth"),
                Some(
                    "complete_key_oauth"
                        | "refresh_key_oauth"
                        | "complete_provider_oauth"
                        | "import_refresh_token"
                        | "cookie_authorize"
                        | "start_cookie_authorize_task"
                        | "start_agent_identity_import_task"
                        | "batch_import_oauth"
                        | "start_batch_import_oauth_task"
                        | "device_poll"
                )
            )
            | (
                Some("admin:management_tokens"),
                Some("create_token" | "regenerate_token")
            )
            | (
                Some("admin:provider_ops"),
                Some(
                    "connect_provider"
                        | "verify_provider"
                        | "get_provider_balance"
                        | "refresh_provider_balance"
                        | "provider_checkin"
                        | "execute_provider_action"
                        | "batch_balance"
                )
            )
            | (
                Some("admin:security"),
                Some("blacklist_add" | "blacklist_remove" | "whitelist_add" | "whitelist_remove")
            )
            | (Some("admin:ldap"), Some("set_config" | "test_connection"))
            | (Some("admin:modules"), Some("set_enabled"))
            | (
                Some("admin:users"),
                Some("reveal_user_api_key" | "create_user_api_key")
            )
            | (
                Some("admin:oauth"),
                Some("upsert_provider" | "delete_provider")
            )
            | (
                Some("admin:payments"),
                Some("update_epay_gateway" | "update_payment_gateway")
            )
            | (
                Some("admin:api_keys"),
                Some("create_api_key" | "create_api_key_install_session")
            )
            | (
                Some("admin:proxy_nodes"),
                Some("create_proxy_node_install_session")
            )
            | (Some("admin:usage"), Some("detail" | "curl" | "replay"))
            | (Some("admin:monitoring"), Some("trace_request"))
            | (Some("admin:tasks"), Some("detail" | "events"))
            | (
                Some("admin:pool"),
                Some("batch_action_keys" | "batch_update_keys")
            )
    ) || (*method == http::Method::GET
        && signature == Some("admin:api_keys")
        && route_kind == Some("api_key_detail")
        && query_param_enabled(decision.public_query_string.as_deref(), "include_key"));

    if requires_admin_permission {
        // These routes reveal usable credentials or replace security-critical
        // identity and system configuration. They are not delegated writes.
        "admin"
    } else {
        access_for_method(method)
    }
}

fn query_param_enabled(query: Option<&str>, key: &str) -> bool {
    query
        .into_iter()
        .flat_map(|query| url::form_urlencoded::parse(query.as_bytes()))
        .find(|(entry_key, _)| entry_key == key)
        .is_some_and(|(_, value)| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

fn permission_key(scope: &str, access: &str) -> &'static str {
    match (scope, access) {
        ("adaptive", "read") => "admin:adaptive:read",
        ("adaptive", "write") => "admin:adaptive:write",
        ("adaptive", "admin") => "admin:adaptive:admin",
        ("announcements", "read") => "admin:announcements:read",
        ("announcements", "write") => "admin:announcements:write",
        ("announcements", "admin") => "admin:announcements:admin",
        ("api_keys", "read") => "admin:api_keys:read",
        ("api_keys", "write") => "admin:api_keys:write",
        ("api_keys", "admin") => "admin:api_keys:admin",
        ("billing", "read") => "admin:billing:read",
        ("billing", "write") => "admin:billing:write",
        ("billing", "admin") => "admin:billing:admin",
        ("endpoints_health", "read") => "admin:endpoints_health:read",
        ("endpoints_health", "write") => "admin:endpoints_health:write",
        ("endpoints_health", "admin") => "admin:endpoints_health:admin",
        ("endpoints_manage", "read") => "admin:endpoints_manage:read",
        ("endpoints_manage", "write") => "admin:endpoints_manage:write",
        ("endpoints_manage", "admin") => "admin:endpoints_manage:admin",
        ("endpoints_rpm", "read") => "admin:endpoints_rpm:read",
        ("endpoints_rpm", "write") => "admin:endpoints_rpm:write",
        ("endpoints_rpm", "admin") => "admin:endpoints_rpm:admin",
        ("gemini_files", "read") => "admin:gemini_files:read",
        ("gemini_files", "write") => "admin:gemini_files:write",
        ("gemini_files", "admin") => "admin:gemini_files:admin",
        ("ldap", "read") => "admin:ldap:read",
        ("ldap", "write") => "admin:ldap:write",
        ("ldap", "admin") => "admin:ldap:admin",
        ("management_tokens", "read") => "admin:management_tokens:read",
        ("management_tokens", "write") => "admin:management_tokens:write",
        ("management_tokens", "admin") => "admin:management_tokens:admin",
        ("models", "read") => "admin:models:read",
        ("models", "write") => "admin:models:write",
        ("models", "admin") => "admin:models:admin",
        ("modules", "read") => "admin:modules:read",
        ("modules", "write") => "admin:modules:write",
        ("modules", "admin") => "admin:modules:admin",
        ("monitoring", "read") => "admin:monitoring:read",
        ("monitoring", "write") => "admin:monitoring:write",
        ("monitoring", "admin") => "admin:monitoring:admin",
        ("oauth", "read") => "admin:oauth:read",
        ("oauth", "write") => "admin:oauth:write",
        ("oauth", "admin") => "admin:oauth:admin",
        ("payments", "read") => "admin:payments:read",
        ("payments", "write") => "admin:payments:write",
        ("payments", "admin") => "admin:payments:admin",
        ("pool", "read") => "admin:pool:read",
        ("pool", "write") => "admin:pool:write",
        ("pool", "admin") => "admin:pool:admin",
        ("provider_ops", "read") => "admin:provider_ops:read",
        ("provider_ops", "write") => "admin:provider_ops:write",
        ("provider_ops", "admin") => "admin:provider_ops:admin",
        ("provider_oauth", "read") => "admin:provider_oauth:read",
        ("provider_oauth", "write") => "admin:provider_oauth:write",
        ("provider_oauth", "admin") => "admin:provider_oauth:admin",
        ("provider_query", "read") => "admin:provider_query:read",
        ("provider_query", "write") => "admin:provider_query:write",
        ("provider_query", "admin") => "admin:provider_query:admin",
        ("provider_strategy", "read") => "admin:provider_strategy:read",
        ("provider_strategy", "write") => "admin:provider_strategy:write",
        ("provider_strategy", "admin") => "admin:provider_strategy:admin",
        ("providers", "read") => "admin:providers:read",
        ("providers", "write") => "admin:providers:write",
        ("providers", "admin") => "admin:providers:admin",
        ("proxy_nodes", "read") => "admin:proxy_nodes:read",
        ("proxy_nodes", "write") => "admin:proxy_nodes:write",
        ("proxy_nodes", "admin") => "admin:proxy_nodes:admin",
        ("routing_profiles", "read") => "admin:routing_profiles:read",
        ("routing_profiles", "write") => "admin:routing_profiles:write",
        ("routing_profiles", "admin") => "admin:routing_profiles:admin",
        ("security", "read") => "admin:security:read",
        ("security", "write") => "admin:security:write",
        ("security", "admin") => "admin:security:admin",
        ("stats", "read") => "admin:stats:read",
        ("stats", "write") => "admin:stats:write",
        ("stats", "admin") => "admin:stats:admin",
        ("system", "read") => "admin:system:read",
        ("system", "write") => "admin:system:write",
        ("system", "admin") => "admin:system:admin",
        ("tasks", "read") => "admin:tasks:read",
        ("tasks", "write") => "admin:tasks:write",
        ("tasks", "admin") => "admin:tasks:admin",
        ("usage", "read") => "admin:usage:read",
        ("usage", "write") => "admin:usage:write",
        ("usage", "admin") => "admin:usage:admin",
        ("users", "read") => "admin:users:read",
        ("users", "write") => "admin:users:write",
        ("users", "admin") => "admin:users:admin",
        ("video_tasks", "read") => "admin:video_tasks:read",
        ("video_tasks", "write") => "admin:video_tasks:write",
        ("video_tasks", "admin") => "admin:video_tasks:admin",
        ("wallets", "read") => "admin:wallets:read",
        ("wallets", "write") => "admin:wallets:write",
        ("wallets", "admin") => "admin:wallets:admin",
        _ => "admin:unknown:read",
    }
}

fn is_known_management_token_permission_scope(scope: &str) -> bool {
    PERMISSION_GROUPS.iter().any(|group| group.scope == scope)
}

fn is_assignable_management_token_permission(key: &str) -> bool {
    management_token_permission_catalog_items()
        .iter()
        .any(|item| item.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_covers_known_admin_auth_scopes() {
        let scopes = [
            "adaptive",
            "announcements",
            "api_keys",
            "billing",
            "endpoints_health",
            "endpoints_manage",
            "endpoints_rpm",
            "gemini_files",
            "ldap",
            "management_tokens",
            "models",
            "modules",
            "monitoring",
            "oauth",
            "payments",
            "pool",
            "provider_oauth",
            "provider_ops",
            "provider_query",
            "provider_strategy",
            "providers",
            "proxy_nodes",
            "security",
            "stats",
            "system",
            "tasks",
            "usage",
            "users",
            "video_tasks",
            "wallets",
        ];

        for scope in scopes {
            assert!(
                is_known_management_token_permission_scope(scope),
                "missing scope {scope}"
            );
            if scope == "management_tokens" {
                continue;
            }
            for access in ["read", "write", "admin"] {
                let key = format!("admin:{scope}:{access}");
                assert!(
                    is_assignable_management_token_permission(&key),
                    "missing permission key {key}"
                );
            }
        }
    }

    #[test]
    fn catalog_covers_admin_route_signatures_from_route_sources() {
        let route_sources = [
            ("route/admin.rs", include_str!("route/admin.rs")),
            ("route/oauth.rs", include_str!("route/oauth.rs")),
            (
                "route/public_support.rs",
                include_str!("route/public_support.rs"),
            ),
            (
                "route/admin/basic_families.rs",
                include_str!("route/admin/basic_families.rs"),
            ),
            (
                "route/admin/endpoints_families.rs",
                include_str!("route/admin/endpoints_families.rs"),
            ),
            (
                "route/admin/model_provider_families.rs",
                include_str!("route/admin/model_provider_families.rs"),
            ),
            (
                "route/admin/observability_families.rs",
                include_str!("route/admin/observability_families.rs"),
            ),
            (
                "route/admin/operations_families.rs",
                include_str!("route/admin/operations_families.rs"),
            ),
            (
                "route/admin/provider_ops_routes.rs",
                include_str!("route/admin/provider_ops_routes.rs"),
            ),
            (
                "route/admin/system_families.rs",
                include_str!("route/admin/system_families.rs"),
            ),
        ];
        let mut route_scopes = BTreeSet::new();

        for (file, source) in route_sources {
            for scope in extract_admin_route_scopes(source) {
                assert!(
                    is_known_management_token_permission_scope(scope),
                    "missing management token permission scope {scope} referenced by {file}"
                );
                route_scopes.insert(scope);
            }
        }

        assert!(
            !route_scopes.is_empty(),
            "admin route scope scanner did not find any route signatures"
        );
    }

    #[test]
    fn full_assignable_token_permissions_cannot_cover_management_tokens_scope() {
        let decision = GatewayControlDecision::synthetic(
            "/api/admin/management-tokens".to_string(),
            Some("admin_proxy".to_string()),
            Some("management_tokens_manage".to_string()),
            Some("list_tokens".to_string()),
            Some("admin:management_tokens".to_string()),
        );
        let permissions = all_assignable_management_token_permissions();

        assert_eq!(
            validate_management_token_admin_route_permission(
                &http::Method::GET,
                &decision,
                Some(&permissions),
            )
            .expect_err("management-token administration is not assignable")
            .required_permission,
            "admin:management_tokens:read"
        );
    }

    #[test]
    fn legacy_tokens_cannot_administer_management_tokens() {
        let decision = GatewayControlDecision::synthetic(
            "/api/admin/management-tokens".to_string(),
            Some("admin_proxy".to_string()),
            Some("management_tokens_manage".to_string()),
            Some("create_token".to_string()),
            Some("admin:management_tokens".to_string()),
        );
        let permissions = legacy_full_management_token_permissions();

        assert_eq!(
            validate_management_token_admin_route_permission(
                &http::Method::POST,
                &decision,
                Some(&permissions),
            )
            .expect_err("legacy tokens must not mint replacement credentials")
            .required_permission,
            "admin:management_tokens:admin"
        );
        assert!(!permissions
            .iter()
            .any(|permission| permission.starts_with("admin:management_tokens:")));
    }

    #[test]
    fn read_only_permissions_allow_reads_and_reject_writes() {
        let decision = GatewayControlDecision::synthetic(
            "/api/admin/providers".to_string(),
            Some("admin_proxy".to_string()),
            Some("providers_manage".to_string()),
            Some("create_provider".to_string()),
            Some("admin:providers".to_string()),
        );
        let permissions = read_only_management_token_permissions();

        assert!(validate_management_token_admin_route_permission(
            &http::Method::GET,
            &decision,
            Some(&permissions),
        )
        .is_ok());
        assert_eq!(
            validate_management_token_admin_route_permission(
                &http::Method::POST,
                &decision,
                Some(&permissions),
            )
            .expect_err("read-only permissions should reject writes")
            .required_permission,
            "admin:providers:write"
        );
    }

    #[test]
    fn scoped_tokens_fail_closed_for_unclassified_admin_permissions() {
        let decision = GatewayControlDecision::synthetic(
            "/api/admin/unclassified".to_string(),
            Some("admin_proxy".to_string()),
            Some("unclassified_manage".to_string()),
            Some("unclassified".to_string()),
            None,
        );
        let permissions = vec!["admin:system:admin".to_string()];

        assert_eq!(
            validate_management_token_admin_route_permission(
                &http::Method::GET,
                &decision,
                Some(&permissions),
            )
            .expect_err("scoped tokens must not bypass an unclassified admin route")
            .required_permission,
            "admin:unknown:admin"
        );
        assert!(validate_management_token_admin_route_permission(
            &http::Method::GET,
            &decision,
            None,
        )
        .is_ok());
    }

    #[test]
    fn legacy_full_permissions_are_frozen_to_the_original_scope_catalog() {
        let permissions = legacy_full_management_token_permissions();

        for permission in [
            "admin:system:admin",
            "admin:oauth:admin",
            "admin:proxy_nodes:admin",
        ] {
            assert!(permissions.iter().any(|item| item == permission));
        }
        for permission in [
            "admin:routing_profiles:read",
            "admin:routing_profiles:admin",
            "admin:tasks:read",
            "admin:tasks:admin",
        ] {
            assert!(
                !permissions.iter().any(|item| item == permission),
                "legacy token unexpectedly gained {permission}"
            );
        }
    }

    #[test]
    fn json_null_permissions_never_inherit_legacy_full_access() {
        assert!(normalize_assignable_management_token_permissions(Some(&Value::Null)).is_err());
        assert!(management_token_permission_keys_from_value(Some(&Value::Null)).is_err());

        assert_eq!(
            management_token_permission_keys_from_value(None)
                .expect("SQL NULL remains the explicit legacy representation"),
            None
        );
    }

    #[test]
    fn sensitive_action_permission_distinguishes_sessions_and_scoped_tokens() {
        let mut decision = GatewayControlDecision::synthetic(
            "/api/admin/users/user-1".to_string(),
            Some("admin_proxy".to_string()),
            Some("users_manage".to_string()),
            Some("update_user".to_string()),
            Some("admin:users".to_string()),
        );

        assert!(!management_token_principal_has_permission(
            &decision,
            "admin:users:admin"
        ));

        decision.admin_principal = Some(crate::control::GatewayAdminPrincipalContext {
            user_id: "admin-1".to_string(),
            user_role: "admin".to_string(),
            session_id: Some("session-1".to_string()),
            management_token_id: None,
            management_token_permissions: None,
        });
        assert!(management_token_principal_has_permission(
            &decision,
            "admin:users:admin"
        ));

        let principal = decision
            .admin_principal
            .as_mut()
            .expect("principal should exist");
        principal.session_id = None;
        principal.management_token_id = Some("management-token-1".to_string());
        principal.management_token_permissions = Some(vec!["admin:users:write".to_string()]);
        assert!(!management_token_principal_has_permission(
            &decision,
            "admin:users:admin"
        ));

        decision
            .admin_principal
            .as_mut()
            .expect("principal should exist")
            .management_token_permissions = Some(vec!["admin:users:admin".to_string()]);
        assert!(management_token_principal_has_permission(
            &decision,
            "admin:users:admin"
        ));
    }

    #[test]
    fn sensitive_system_data_transfers_require_admin_permission() {
        let delegated_permissions = vec![
            "admin:system:read".to_string(),
            "admin:system:write".to_string(),
        ];
        let admin_permissions = vec!["admin:system:admin".to_string()];

        for (method, route_kind) in [
            (http::Method::GET, "config_export"),
            (http::Method::GET, "users_export"),
            (http::Method::GET, "data_export"),
            (http::Method::POST, "s3_backup_run"),
            (http::Method::POST, "config_import"),
            (http::Method::POST, "users_import"),
            (http::Method::POST, "data_import"),
        ] {
            let decision = GatewayControlDecision::synthetic(
                format!("/api/admin/system/{route_kind}"),
                Some("admin_proxy".to_string()),
                Some("system_manage".to_string()),
                Some(route_kind.to_string()),
                Some("admin:system".to_string()),
            );

            assert_eq!(
                management_token_required_permission(&method, &decision).as_deref(),
                Some("admin:system:admin")
            );
            assert_eq!(
                validate_management_token_admin_route_permission(
                    &method,
                    &decision,
                    Some(&delegated_permissions),
                )
                .expect_err("delegated system access must not transfer sensitive data")
                .required_permission,
                "admin:system:admin"
            );
            assert!(validate_management_token_admin_route_permission(
                &method,
                &decision,
                Some(&admin_permissions),
            )
            .is_ok());
        }
    }

    #[test]
    fn oauth_provider_upsert_requires_oauth_admin_permission() {
        let decision = GatewayControlDecision::synthetic(
            "/api/admin/oauth/providers/custom".to_string(),
            Some("admin_proxy".to_string()),
            Some("oauth_manage".to_string()),
            Some("upsert_provider".to_string()),
            Some("admin:oauth".to_string()),
        );
        let write_permissions = vec!["admin:oauth:write".to_string()];
        let admin_permissions = vec!["admin:oauth:admin".to_string()];

        assert_eq!(
            management_token_required_permission(&http::Method::PUT, &decision).as_deref(),
            Some("admin:oauth:admin")
        );
        assert_eq!(
            validate_management_token_admin_route_permission(
                &http::Method::PUT,
                &decision,
                Some(&write_permissions),
            )
            .expect_err("oauth write must not replace security-sensitive provider configuration")
            .required_permission,
            "admin:oauth:admin"
        );
        assert!(validate_management_token_admin_route_permission(
            &http::Method::PUT,
            &decision,
            Some(&admin_permissions),
        )
        .is_ok());
    }

    #[test]
    fn oauth_provider_delete_requires_oauth_admin_permission() {
        let decision = GatewayControlDecision::synthetic(
            "/api/admin/oauth/providers/custom".to_string(),
            Some("admin_proxy".to_string()),
            Some("oauth_manage".to_string()),
            Some("delete_provider".to_string()),
            Some("admin:oauth".to_string()),
        );
        let write_permissions = vec!["admin:oauth:write".to_string()];
        let admin_permissions = vec!["admin:oauth:admin".to_string()];

        assert_eq!(
            management_token_required_permission(&http::Method::DELETE, &decision).as_deref(),
            Some("admin:oauth:admin")
        );
        assert_eq!(
            validate_management_token_admin_route_permission(
                &http::Method::DELETE,
                &decision,
                Some(&write_permissions),
            )
            .expect_err("oauth write must not delete an authentication provider")
            .required_permission,
            "admin:oauth:admin"
        );
        assert!(validate_management_token_admin_route_permission(
            &http::Method::DELETE,
            &decision,
            Some(&admin_permissions),
        )
        .is_ok());
    }

    #[test]
    fn destructive_system_actions_require_system_admin_permission() {
        let write_permissions = vec!["admin:system:write".to_string()];
        let admin_permissions = vec!["admin:system:admin".to_string()];

        for (method, route_kind) in [
            (http::Method::POST, "prepare_update"),
            (http::Method::POST, "apply_update"),
            (http::Method::POST, "rollback"),
            (http::Method::POST, "cleanup"),
            (http::Method::POST, "cleanup_usage_manual"),
            (http::Method::POST, "purge_config"),
            (http::Method::POST, "purge_users"),
            (http::Method::POST, "purge_usage"),
            (http::Method::POST, "purge_audit_logs"),
            (http::Method::POST, "purge_request_bodies"),
            (http::Method::POST, "purge_request_bodies_task"),
            (http::Method::POST, "purge_stats"),
            (http::Method::PUT, "settings_set"),
            (http::Method::PUT, "config_set"),
            (http::Method::DELETE, "config_delete"),
        ] {
            let decision = GatewayControlDecision::synthetic(
                format!("/api/admin/system/{route_kind}"),
                Some("admin_proxy".to_string()),
                Some("system_manage".to_string()),
                Some(route_kind.to_string()),
                Some("admin:system".to_string()),
            );

            assert_eq!(
                management_token_required_permission(&method, &decision).as_deref(),
                Some("admin:system:admin"),
                "unexpected permission for {method} {route_kind}"
            );
            assert_eq!(
                validate_management_token_admin_route_permission(
                    &method,
                    &decision,
                    Some(&write_permissions),
                )
                .expect_err("system write must not perform destructive or security-critical action")
                .required_permission,
                "admin:system:admin"
            );
            assert!(validate_management_token_admin_route_permission(
                &method,
                &decision,
                Some(&admin_permissions),
            )
            .is_ok());
        }
    }

    #[test]
    fn provider_operations_that_use_stored_credentials_require_provider_ops_admin_permission() {
        let read_permissions = vec!["admin:provider_ops:read".to_string()];
        let write_permissions = vec!["admin:provider_ops:write".to_string()];
        let admin_permissions = vec!["admin:provider_ops:admin".to_string()];

        for (method, route_kind) in [
            (http::Method::POST, "connect_provider"),
            (http::Method::POST, "verify_provider"),
            (http::Method::GET, "get_provider_balance"),
            (http::Method::POST, "refresh_provider_balance"),
            (http::Method::POST, "provider_checkin"),
            (http::Method::POST, "execute_provider_action"),
            (http::Method::POST, "batch_balance"),
        ] {
            let decision = GatewayControlDecision::synthetic(
                format!("/api/admin/provider-ops/{route_kind}"),
                Some("admin_proxy".to_string()),
                Some("provider_ops_manage".to_string()),
                Some(route_kind.to_string()),
                Some("admin:provider_ops".to_string()),
            );

            assert_eq!(
                management_token_required_permission(&method, &decision).as_deref(),
                Some("admin:provider_ops:admin")
            );
            for delegated_permissions in [&read_permissions, &write_permissions] {
                assert_eq!(
                    validate_management_token_admin_route_permission(
                        &method,
                        &decision,
                        Some(delegated_permissions),
                    )
                    .expect_err("delegated access must not execute stored provider credentials")
                    .required_permission,
                    "admin:provider_ops:admin"
                );
            }
            assert!(validate_management_token_admin_route_permission(
                &method,
                &decision,
                Some(&admin_permissions),
            )
            .is_ok());
        }
    }

    #[test]
    fn provider_queries_that_execute_stored_credentials_require_admin_permission() {
        let write_permissions = vec!["admin:provider_query:write".to_string()];
        let admin_permissions = vec!["admin:provider_query:admin".to_string()];

        for route_kind in ["query_models", "test_model", "test_model_failover"] {
            let decision = GatewayControlDecision::synthetic(
                format!("/api/admin/provider-query/{route_kind}"),
                Some("admin_proxy".to_string()),
                Some("provider_query_manage".to_string()),
                Some(route_kind.to_string()),
                Some("admin:provider_query".to_string()),
            );

            assert_eq!(
                management_token_required_permission(&http::Method::POST, &decision).as_deref(),
                Some("admin:provider_query:admin")
            );
            assert!(validate_management_token_admin_route_permission(
                &http::Method::POST,
                &decision,
                Some(&write_permissions),
            )
            .is_err());
            assert!(validate_management_token_admin_route_permission(
                &http::Method::POST,
                &decision,
                Some(&admin_permissions),
            )
            .is_ok());
        }
    }

    #[test]
    fn endpoint_actions_that_execute_stored_credentials_require_admin_permission() {
        let write_permissions = vec!["admin:endpoints_manage:write".to_string()];
        let admin_permissions = vec!["admin:endpoints_manage:admin".to_string()];

        for (method, route_kind) in [
            (http::Method::PUT, "update_key"),
            (http::Method::POST, "create_endpoint"),
            (http::Method::PUT, "update_endpoint"),
            (http::Method::POST, "refresh_quota"),
            (http::Method::POST, "codex_reset_credit_consume"),
        ] {
            let decision = GatewayControlDecision::synthetic(
                format!("/api/admin/endpoints/{route_kind}"),
                Some("admin_proxy".to_string()),
                Some("endpoints_manage".to_string()),
                Some(route_kind.to_string()),
                Some("admin:endpoints_manage".to_string()),
            );

            assert_eq!(
                management_token_required_permission(&method, &decision).as_deref(),
                Some("admin:endpoints_manage:admin")
            );
            assert!(validate_management_token_admin_route_permission(
                &method,
                &decision,
                Some(&write_permissions),
            )
            .is_err());
            assert!(validate_management_token_admin_route_permission(
                &method,
                &decision,
                Some(&admin_permissions),
            )
            .is_ok());
        }
    }

    #[test]
    fn stored_credential_egress_configuration_requires_admin_permission() {
        for (method, signature, route_kind) in [
            (http::Method::PATCH, "admin:providers", "update_provider"),
            (http::Method::POST, "admin:pool", "batch_action_keys"),
            (http::Method::PATCH, "admin:pool", "batch_update_keys"),
        ] {
            let scope = signature.trim_start_matches("admin:");
            let decision = GatewayControlDecision::synthetic(
                format!("/api/admin/{scope}/{route_kind}"),
                Some("admin_proxy".to_string()),
                Some(format!("{scope}_manage")),
                Some(route_kind.to_string()),
                Some(signature.to_string()),
            );
            let write_permissions = vec![format!("{signature}:write")];
            let admin_permissions = vec![format!("{signature}:admin")];

            assert_eq!(
                management_token_required_permission(&method, &decision).as_deref(),
                Some(format!("{signature}:admin").as_str())
            );
            assert_eq!(
                validate_management_token_admin_route_permission(
                    &method,
                    &decision,
                    Some(&write_permissions),
                )
                .expect_err("delegated writes must not redirect stored provider credentials")
                .required_permission,
                format!("{signature}:admin")
            );
            assert!(validate_management_token_admin_route_permission(
                &method,
                &decision,
                Some(&admin_permissions),
            )
            .is_ok());
        }
    }

    #[test]
    fn connection_tests_that_transmit_stored_secrets_require_admin_permission() {
        for (signature, route_kind, read_permission, write_permission, admin_permission) in [
            (
                "admin:system",
                "smtp_test",
                "admin:system:read",
                "admin:system:write",
                "admin:system:admin",
            ),
            (
                "admin:system",
                "important_notification_test",
                "admin:system:read",
                "admin:system:write",
                "admin:system:admin",
            ),
            (
                "admin:ldap",
                "test_connection",
                "admin:ldap:read",
                "admin:ldap:write",
                "admin:ldap:admin",
            ),
        ] {
            let decision = GatewayControlDecision::synthetic(
                "/api/admin/security-sensitive-test".to_string(),
                Some("admin_proxy".to_string()),
                Some("security_test".to_string()),
                Some(route_kind.to_string()),
                Some(signature.to_string()),
            );
            let read_permissions = vec![read_permission.to_string()];
            let write_permissions = vec![write_permission.to_string()];
            let admin_permissions = vec![admin_permission.to_string()];

            assert_eq!(
                management_token_required_permission(&http::Method::POST, &decision).as_deref(),
                Some(admin_permission)
            );
            for delegated_permissions in [&read_permissions, &write_permissions] {
                assert_eq!(
                    validate_management_token_admin_route_permission(
                        &http::Method::POST,
                        &decision,
                        Some(delegated_permissions),
                    )
                    .expect_err("delegated access must not transmit stored service secrets")
                    .required_permission,
                    admin_permission
                );
            }
            assert!(validate_management_token_admin_route_permission(
                &http::Method::POST,
                &decision,
                Some(&admin_permissions),
            )
            .is_ok());
        }
    }

    #[test]
    fn security_control_plane_mutations_require_admin_permission() {
        for (method, signature, route_kind) in [
            (http::Method::POST, "admin:security", "blacklist_add"),
            (http::Method::DELETE, "admin:security", "blacklist_remove"),
            (http::Method::POST, "admin:security", "whitelist_add"),
            (http::Method::DELETE, "admin:security", "whitelist_remove"),
            (http::Method::PUT, "admin:ldap", "set_config"),
            (http::Method::PUT, "admin:modules", "set_enabled"),
        ] {
            let scope = signature.trim_start_matches("admin:");
            let decision = GatewayControlDecision::synthetic(
                format!("/api/admin/{scope}/{route_kind}"),
                Some("admin_proxy".to_string()),
                Some(format!("{scope}_manage")),
                Some(route_kind.to_string()),
                Some(signature.to_string()),
            );
            let read_permissions = vec![format!("{signature}:read")];
            let write_permissions = vec![format!("{signature}:write")];
            let admin_permissions = vec![format!("{signature}:admin")];

            assert_eq!(
                management_token_required_permission(&method, &decision).as_deref(),
                Some(format!("{signature}:admin").as_str())
            );
            for delegated_permissions in [&read_permissions, &write_permissions] {
                assert_eq!(
                    validate_management_token_admin_route_permission(
                        &method,
                        &decision,
                        Some(delegated_permissions),
                    )
                    .expect_err("delegated tokens must not change security controls")
                    .required_permission,
                    format!("{signature}:admin")
                );
            }
            assert!(validate_management_token_admin_route_permission(
                &method,
                &decision,
                Some(&admin_permissions),
            )
            .is_ok());
        }
    }

    #[test]
    fn payment_gateway_updates_require_payments_admin_permission() {
        for route_kind in ["update_epay_gateway", "update_payment_gateway"] {
            let decision = GatewayControlDecision::synthetic(
                "/api/admin/payments/gateways/epay".to_string(),
                Some("admin_proxy".to_string()),
                Some("payments_manage".to_string()),
                Some(route_kind.to_string()),
                Some("admin:payments".to_string()),
            );
            let write_permissions = vec!["admin:payments:write".to_string()];
            let admin_permissions = vec!["admin:payments:admin".to_string()];

            assert_eq!(
                management_token_required_permission(&http::Method::PUT, &decision).as_deref(),
                Some("admin:payments:admin")
            );
            assert!(validate_management_token_admin_route_permission(
                &http::Method::PUT,
                &decision,
                Some(&write_permissions),
            )
            .is_err());
            assert!(validate_management_token_admin_route_permission(
                &http::Method::PUT,
                &decision,
                Some(&admin_permissions),
            )
            .is_ok());
        }
    }

    #[test]
    fn plaintext_credential_reads_require_admin_permission() {
        let read_only_permissions = read_only_management_token_permissions();
        let cases = [
            (
                "admin:endpoints_manage",
                "reveal_endpoint_rules",
                None,
                "admin:endpoints_manage:admin",
            ),
            (
                "admin:endpoints_manage",
                "reveal_key",
                None,
                "admin:endpoints_manage:admin",
            ),
            (
                "admin:endpoints_manage",
                "export_key",
                None,
                "admin:endpoints_manage:admin",
            ),
            (
                "admin:users",
                "reveal_user_api_key",
                None,
                "admin:users:admin",
            ),
            (
                "admin:api_keys",
                "api_key_detail",
                Some("include_key=true"),
                "admin:api_keys:admin",
            ),
            (
                "admin:api_keys",
                "create_api_key_install_session",
                None,
                "admin:api_keys:admin",
            ),
            (
                "admin:proxy_nodes",
                "create_proxy_node_install_session",
                None,
                "admin:proxy_nodes:admin",
            ),
        ];

        for (signature, route_kind, query, expected) in cases {
            let mut decision = GatewayControlDecision::synthetic(
                "/api/admin/sensitive-read".to_string(),
                Some("admin_proxy".to_string()),
                Some("security_test".to_string()),
                Some(route_kind.to_string()),
                Some(signature.to_string()),
            );
            decision.public_query_string = query.map(str::to_string);

            let method = if route_kind.ends_with("install_session") {
                http::Method::POST
            } else {
                http::Method::GET
            };
            assert_eq!(
                management_token_required_permission(&method, &decision).as_deref(),
                Some(expected)
            );
            assert_eq!(
                validate_management_token_admin_route_permission(
                    &method,
                    &decision,
                    Some(&read_only_permissions),
                )
                .expect_err("read-only access must not reveal plaintext credentials")
                .required_permission,
                expected
            );
        }
    }

    #[test]
    fn raw_usage_and_trace_reads_require_admin_permission() {
        let cases = [
            ("admin:usage", "detail", http::Method::GET),
            ("admin:usage", "curl", http::Method::GET),
            ("admin:usage", "replay", http::Method::POST),
            ("admin:monitoring", "trace_request", http::Method::GET),
        ];

        for (signature, route_kind, method) in cases {
            let scope = signature.trim_start_matches("admin:");
            let read_permissions = vec![format!("{signature}:read")];
            let admin_permissions = vec![format!("{signature}:admin")];
            let decision = GatewayControlDecision::synthetic(
                format!("/api/admin/{scope}/sensitive-record"),
                Some("admin_proxy".to_string()),
                Some(format!("{scope}_manage")),
                Some(route_kind.to_string()),
                Some(signature.to_string()),
            );

            assert_eq!(
                management_token_required_permission(&method, &decision).as_deref(),
                Some(format!("{signature}:admin").as_str())
            );
            assert_eq!(
                validate_management_token_admin_route_permission(
                    &method,
                    &decision,
                    Some(&read_permissions),
                )
                .expect_err("delegated read access must not expose raw request diagnostics")
                .required_permission,
                format!("{signature}:admin")
            );
            assert!(validate_management_token_admin_route_permission(
                &method,
                &decision,
                Some(&admin_permissions),
            )
            .is_ok());
        }

        for (signature, route_kind) in [
            ("admin:usage", "records"),
            ("admin:monitoring", "trace_provider_stats"),
        ] {
            let decision = GatewayControlDecision::synthetic(
                "/api/admin/summary".to_string(),
                Some("admin_proxy".to_string()),
                Some("summary".to_string()),
                Some(route_kind.to_string()),
                Some(signature.to_string()),
            );
            assert_eq!(
                management_token_required_permission(&http::Method::GET, &decision).as_deref(),
                Some(format!("{signature}:read").as_str())
            );
        }
    }

    #[test]
    fn task_read_permission_excludes_raw_detail_and_events() {
        let read_permissions = vec!["admin:tasks:read".to_string()];
        let admin_permissions = vec!["admin:tasks:admin".to_string()];

        for route_kind in ["list_tasks", "stats"] {
            let decision = GatewayControlDecision::synthetic(
                "/api/admin/tasks".to_string(),
                Some("admin_proxy".to_string()),
                Some("tasks_manage".to_string()),
                Some(route_kind.to_string()),
                Some("admin:tasks".to_string()),
            );
            assert_eq!(
                management_token_required_permission(&http::Method::GET, &decision).as_deref(),
                Some("admin:tasks:read")
            );
            assert!(validate_management_token_admin_route_permission(
                &http::Method::GET,
                &decision,
                Some(&read_permissions),
            )
            .is_ok());
        }

        for route_kind in ["detail", "events"] {
            let decision = GatewayControlDecision::synthetic(
                "/api/admin/tasks/run-1".to_string(),
                Some("admin_proxy".to_string()),
                Some("tasks_manage".to_string()),
                Some(route_kind.to_string()),
                Some("admin:tasks".to_string()),
            );
            assert_eq!(
                management_token_required_permission(&http::Method::GET, &decision).as_deref(),
                Some("admin:tasks:admin")
            );
            assert_eq!(
                validate_management_token_admin_route_permission(
                    &http::Method::GET,
                    &decision,
                    Some(&read_permissions),
                )
                .expect_err("task read access must not expose raw task diagnostics")
                .required_permission,
                "admin:tasks:admin"
            );
            assert!(validate_management_token_admin_route_permission(
                &http::Method::GET,
                &decision,
                Some(&admin_permissions),
            )
            .is_ok());
            assert!(validate_management_token_admin_route_permission(
                &http::Method::GET,
                &decision,
                None,
            )
            .is_ok());
        }
    }

    #[test]
    fn user_lifecycle_routes_preserve_write_permission_for_field_level_checks() {
        let write_permissions = vec!["admin:users:write".to_string()];
        let cases = [
            (http::Method::POST, "/api/admin/users", "create_user"),
            (http::Method::PUT, "/api/admin/users/user-1", "update_user"),
            (
                http::Method::POST,
                "/api/admin/users/batch-action",
                "batch_action_users",
            ),
        ];

        for (method, path, route_kind) in cases {
            let decision = GatewayControlDecision::synthetic(
                path.to_string(),
                Some("admin_proxy".to_string()),
                Some("users_manage".to_string()),
                Some(route_kind.to_string()),
                Some("admin:users".to_string()),
            );

            assert_eq!(
                management_token_required_permission(&method, &decision).as_deref(),
                Some("admin:users:write"),
                "unexpected permission for {method} {path}"
            );
            assert!(validate_management_token_admin_route_permission(
                &method,
                &decision,
                Some(&write_permissions),
            )
            .is_ok());
        }
    }

    #[test]
    fn ordinary_user_resource_mutations_still_require_users_write_permission() {
        let write_permissions = vec!["admin:users:write".to_string()];

        for (method, route_kind) in [
            (http::Method::PUT, "update_user_api_key"),
            (http::Method::DELETE, "delete_user_session"),
        ] {
            let decision = GatewayControlDecision::synthetic(
                "/api/admin/users/user-1/resource".to_string(),
                Some("admin_proxy".to_string()),
                Some("users_manage".to_string()),
                Some(route_kind.to_string()),
                Some("admin:users".to_string()),
            );

            assert_eq!(
                management_token_required_permission(&method, &decision).as_deref(),
                Some("admin:users:write")
            );
            assert!(validate_management_token_admin_route_permission(
                &method,
                &decision,
                Some(&write_permissions),
            )
            .is_ok());
        }
    }

    #[test]
    fn usable_credential_creation_requires_admin_permission() {
        for (signature, route_kind, write_permission, admin_permission) in [
            (
                "admin:management_tokens",
                "create_token",
                "admin:management_tokens:write",
                "admin:management_tokens:admin",
            ),
            (
                "admin:management_tokens",
                "regenerate_token",
                "admin:management_tokens:write",
                "admin:management_tokens:admin",
            ),
            (
                "admin:endpoints_manage",
                "create_provider_key",
                "admin:endpoints_manage:write",
                "admin:endpoints_manage:admin",
            ),
            (
                "admin:users",
                "create_user_api_key",
                "admin:users:write",
                "admin:users:admin",
            ),
            (
                "admin:api_keys",
                "create_api_key",
                "admin:api_keys:write",
                "admin:api_keys:admin",
            ),
        ] {
            let decision = GatewayControlDecision::synthetic(
                "/api/admin/credential".to_string(),
                Some("admin_proxy".to_string()),
                Some("credential_security_test".to_string()),
                Some(route_kind.to_string()),
                Some(signature.to_string()),
            );
            let write_permissions = vec![write_permission.to_string()];
            let admin_permissions = vec![admin_permission.to_string()];

            assert_eq!(
                management_token_required_permission(&http::Method::POST, &decision).as_deref(),
                Some(admin_permission)
            );
            assert_eq!(
                validate_management_token_admin_route_permission(
                    &http::Method::POST,
                    &decision,
                    Some(&write_permissions),
                )
                .expect_err("delegated write access must not issue usable credentials")
                .required_permission,
                admin_permission
            );
            assert!(validate_management_token_admin_route_permission(
                &http::Method::POST,
                &decision,
                Some(&admin_permissions),
            )
            .is_ok());
        }
    }

    #[test]
    fn ordinary_system_reads_still_require_read_permission() {
        let decision = GatewayControlDecision::synthetic(
            "/api/admin/system/settings".to_string(),
            Some("admin_proxy".to_string()),
            Some("system_manage".to_string()),
            Some("settings_get".to_string()),
            Some("admin:system".to_string()),
        );

        assert_eq!(
            management_token_required_permission(&http::Method::GET, &decision).as_deref(),
            Some("admin:system:read")
        );

        let mut api_key_detail = GatewayControlDecision::synthetic(
            "/api/admin/api-keys/key-1".to_string(),
            Some("admin_proxy".to_string()),
            Some("api_keys_manage".to_string()),
            Some("api_key_detail".to_string()),
            Some("admin:api_keys".to_string()),
        );
        for query in [None, Some("include_key=false"), Some("include_key=invalid")] {
            api_key_detail.public_query_string = query.map(str::to_string);
            assert_eq!(
                management_token_required_permission(&http::Method::GET, &api_key_detail)
                    .as_deref(),
                Some("admin:api_keys:read")
            );
        }
    }

    #[test]
    fn audit_admin_read_only_permissions_allow_management_tokens_reads_and_reject_writes() {
        let decision = GatewayControlDecision::synthetic(
            "/api/admin/management-tokens".to_string(),
            Some("admin_proxy".to_string()),
            Some("management_tokens_manage".to_string()),
            Some("list_tokens".to_string()),
            Some("admin:management_tokens".to_string()),
        );
        let permissions = audit_admin_read_only_management_token_permissions();

        assert!(validate_management_token_admin_route_permission(
            &http::Method::GET,
            &decision,
            Some(&permissions),
        )
        .is_ok());
        assert_eq!(
            validate_management_token_admin_route_permission(
                &http::Method::POST,
                &decision,
                Some(&permissions),
            )
            .expect_err("read-only permissions should reject management token writes")
            .required_permission,
            "admin:management_tokens:write"
        );
    }

    fn extract_admin_route_scopes(source: &'static str) -> BTreeSet<&'static str> {
        let mut scopes = BTreeSet::new();
        let mut remaining = source;

        while let Some(start) = remaining.find("\"admin:") {
            let signature_start = start + 1;
            let after_start = &remaining[signature_start..];
            let Some(end) = after_start.find('"') else {
                break;
            };
            let signature = &after_start[..end];
            let mut parts = signature.split(':');
            if parts.next() == Some("admin") {
                if let (Some(scope), None) = (parts.next(), parts.next()) {
                    scopes.insert(scope);
                }
            }
            remaining = &after_start[end + 1..];
        }

        scopes
    }
}
