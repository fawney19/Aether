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
        scope: "risk_control",
        label: "风控中心",
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
        return Ok(json!(all_assignable_management_token_permissions()));
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
        return Ok(None);
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
    Some(format!("admin:{scope}:{}", access_for_method(method)))
}

pub(crate) fn validate_management_token_admin_route_permission(
    method: &http::Method,
    decision: &GatewayControlDecision,
    token_permissions: Option<&[String]>,
) -> Result<(), ManagementTokenPermissionDenied> {
    let Some(required_permission) = management_token_required_permission(method, decision) else {
        return Ok(());
    };
    let Some(token_permissions) = token_permissions else {
        return Ok(());
    };
    let scope = required_permission
        .strip_prefix("admin:")
        .and_then(|value| value.rsplit_once(':').map(|(scope, _)| scope))
        .unwrap_or_default();
    let admin_permission = format!("admin:{scope}:admin");
    let has_full_assignable_access =
        management_token_permissions_cover_all_assignable_permissions(token_permissions);
    if token_permissions
        .iter()
        .any(|permission| permission == &required_permission || permission == &admin_permission)
        || (scope == "management_tokens" && has_full_assignable_access)
    {
        Ok(())
    } else {
        Err(ManagementTokenPermissionDenied {
            required_permission,
        })
    }
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
        ("risk_control", "read") => "admin:risk_control:read",
        ("risk_control", "write") => "admin:risk_control:write",
        ("risk_control", "admin") => "admin:risk_control:admin",
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

pub(crate) fn management_token_permissions_cover_all_assignable_permissions(
    token_permissions: &[String],
) -> bool {
    let permission_set = token_permissions
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    all_assignable_management_token_permissions()
        .iter()
        .all(|permission| permission_set.contains(permission.as_str()))
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
            "risk_control",
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
                "route/admin/risk_control_families.rs",
                include_str!("route/admin/risk_control_families.rs"),
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
    fn full_assignable_token_permissions_can_cover_management_tokens_scope() {
        let decision = GatewayControlDecision::synthetic(
            "/api/admin/management-tokens".to_string(),
            Some("admin_proxy".to_string()),
            Some("management_tokens_manage".to_string()),
            Some("list_tokens".to_string()),
            Some("admin:management_tokens".to_string()),
        );
        let permissions = all_assignable_management_token_permissions();

        assert!(validate_management_token_admin_route_permission(
            &http::Method::GET,
            &decision,
            Some(&permissions),
        )
        .is_ok());
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
