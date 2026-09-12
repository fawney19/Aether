use axum::http;

use super::{classified, ClassifiedRoute};

pub(super) fn classify_admin_operations_family_route(
    method: &http::Method,
    normalized_path: &str,
    normalized_path_no_trailing: &str,
) -> Option<ClassifiedRoute> {
    if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/referrals" | "/api/admin/referrals/"
        )
    {
        Some(classified(
            "admin_proxy",
            "referrals_manage",
            "list_referrals",
            "admin:billing",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/referral-rewards" | "/api/admin/referral-rewards/"
        )
    {
        Some(classified(
            "admin_proxy",
            "referrals_manage",
            "list_referral_rewards",
            "admin:billing",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/referral-rewards/")
        && normalized_path.ends_with("/retry")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "referrals_manage",
            "retry_referral_reward",
            "admin:billing",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/referral-rewards/")
        && normalized_path.ends_with("/void")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "referrals_manage",
            "void_referral_reward",
            "admin:billing",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/provider-ops/architectures" | "/api/admin/provider-ops/architectures/"
        )
    {
        Some(classified(
            "admin_proxy",
            "provider_ops_manage",
            "list_architectures",
            "admin:provider_ops",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/video-tasks" | "/api/admin/video-tasks/"
        )
    {
        Some(classified(
            "admin_proxy",
            "video_tasks_manage",
            "list_tasks",
            "admin:video_tasks",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/video-tasks/stats" | "/api/admin/video-tasks/stats/"
        )
    {
        Some(classified(
            "admin_proxy",
            "video_tasks_manage",
            "stats",
            "admin:video_tasks",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(normalized_path, "/api/admin/tasks" | "/api/admin/tasks/")
    {
        Some(classified(
            "admin_proxy",
            "tasks_manage",
            "list_tasks",
            "admin:tasks",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/tasks/stats" | "/api/admin/tasks/stats/"
        )
    {
        Some(classified(
            "admin_proxy",
            "tasks_manage",
            "stats",
            "admin:tasks",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/tasks/")
        && normalized_path.ends_with("/events")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "tasks_manage",
            "events",
            "admin:tasks",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/tasks/")
        && normalized_path.ends_with("/cancel")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "tasks_manage",
            "cancel",
            "admin:tasks",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/tasks/")
        && normalized_path.ends_with("/trigger")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "tasks_manage",
            "trigger",
            "admin:tasks",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/tasks/")
        && normalized_path["/api/admin/tasks/".len()..]
            .split('/')
            .count()
            == 1
        && !matches!(
            normalized_path,
            "/api/admin/tasks/stats" | "/api/admin/tasks/stats/"
        )
    {
        Some(classified(
            "admin_proxy",
            "tasks_manage",
            "detail",
            "admin:tasks",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/video-tasks/")
        && normalized_path.ends_with("/video")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "video_tasks_manage",
            "video",
            "admin:video_tasks",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/video-tasks/")
        && normalized_path.ends_with("/cancel")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "video_tasks_manage",
            "cancel",
            "admin:video_tasks",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/video-tasks/")
        && normalized_path["/api/admin/video-tasks/".len()..]
            .split('/')
            .count()
            == 1
        && !matches!(
            normalized_path,
            "/api/admin/video-tasks/stats" | "/api/admin/video-tasks/stats/"
        )
    {
        Some(classified(
            "admin_proxy",
            "video_tasks_manage",
            "detail",
            "admin:video_tasks",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/provider-ops/architectures/")
        && !normalized_path.ends_with('/')
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "provider_ops_manage",
            "get_architecture",
            "admin:provider_ops",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/proxy-nodes" | "/api/admin/proxy-nodes/"
        )
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "list_nodes",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/proxy-nodes/metrics/fleet" | "/api/admin/proxy-nodes/metrics/fleet/"
        )
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "list_fleet_metrics",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path_no_trailing.starts_with("/api/admin/proxy-nodes/")
        && normalized_path_no_trailing.ends_with("/metrics")
        && normalized_path_no_trailing["/api/admin/proxy-nodes/".len()..]
            .split('/')
            .count()
            == 2
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "list_node_metrics",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path_no_trailing.starts_with("/api/admin/proxy-nodes/")
        && normalized_path_no_trailing["/api/admin/proxy-nodes/".len()..]
            .split('/')
            .count()
            == 1
        && !matches!(
            &normalized_path_no_trailing["/api/admin/proxy-nodes/".len()..],
            "register" | "heartbeat" | "unregister" | "manual" | "upgrade" | "test-url"
        )
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "get_node",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/proxy-nodes/register" | "/api/admin/proxy-nodes/register/"
        )
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "register_node",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/proxy-nodes/heartbeat" | "/api/admin/proxy-nodes/heartbeat/"
        )
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "heartbeat_node",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/proxy-nodes/unregister" | "/api/admin/proxy-nodes/unregister/"
        )
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "unregister_node",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/proxy-nodes/manual" | "/api/admin/proxy-nodes/manual/"
        )
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "create_manual_node",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/proxy-nodes/install-sessions" | "/api/admin/proxy-nodes/install-sessions/"
        )
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "create_proxy_node_install_session",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/proxy-nodes/upgrade" | "/api/admin/proxy-nodes/upgrade/"
        )
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "batch_upgrade_nodes",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/proxy-nodes/upgrade/cancel" | "/api/admin/proxy-nodes/upgrade/cancel/"
        )
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "cancel_upgrade_rollout",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/proxy-nodes/upgrade/clear-conflicts"
                | "/api/admin/proxy-nodes/upgrade/clear-conflicts/"
        )
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "clear_upgrade_rollout_conflicts",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/proxy-nodes/upgrade/restore-skipped"
                | "/api/admin/proxy-nodes/upgrade/restore-skipped/"
        )
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "restore_skipped_upgrade_rollout_nodes",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/proxy-nodes/")
        && normalized_path.ends_with("/upgrade/skip")
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "skip_upgrade_rollout_node",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/proxy-nodes/")
        && normalized_path.ends_with("/upgrade/retry")
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "retry_upgrade_rollout_node",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/proxy-nodes/test-url" | "/api/admin/proxy-nodes/test-url/"
        )
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "test_proxy_url",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::PATCH
        && normalized_path.starts_with("/api/admin/proxy-nodes/")
        && !normalized_path.ends_with("/test")
        && !normalized_path.ends_with("/config")
        && !normalized_path.ends_with("/events")
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "update_manual_node",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/proxy-nodes/")
        && !normalized_path.ends_with("/test")
        && !normalized_path.ends_with("/config")
        && !normalized_path.ends_with("/events")
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "delete_node",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/proxy-nodes/")
        && normalized_path.ends_with("/test")
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "test_node",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::PUT
        && normalized_path.starts_with("/api/admin/proxy-nodes/")
        && normalized_path.ends_with("/config")
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "update_node_config",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/proxy-nodes/")
        && normalized_path.ends_with("/events")
    {
        Some(classified(
            "admin_proxy",
            "proxy_nodes_manage",
            "list_node_events",
            "admin:proxy_nodes",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/wallets" | "/api/admin/wallets/"
        )
    {
        Some(classified(
            "admin_proxy",
            "wallets_manage",
            "list_wallets",
            "admin:wallets",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/wallets/ledger" | "/api/admin/wallets/ledger/"
        )
    {
        Some(classified(
            "admin_proxy",
            "wallets_manage",
            "ledger",
            "admin:wallets",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/wallets/refund-requests" | "/api/admin/wallets/refund-requests/"
        )
    {
        Some(classified(
            "admin_proxy",
            "wallets_manage",
            "list_refund_requests",
            "admin:wallets",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/wallets/")
        && normalized_path.ends_with("/transactions")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "wallets_manage",
            "list_wallet_transactions",
            "admin:wallets",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/wallets/")
        && normalized_path.ends_with("/refunds")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "wallets_manage",
            "list_wallet_refunds",
            "admin:wallets",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/wallets/")
        && !normalized_path.ends_with("/transactions")
        && !normalized_path.ends_with("/refunds")
        && normalized_path.matches('/').count() == 4
    {
        Some(classified(
            "admin_proxy",
            "wallets_manage",
            "wallet_detail",
            "admin:wallets",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/wallets/")
        && normalized_path.ends_with("/adjust")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "wallets_manage",
            "adjust_balance",
            "admin:wallets",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/wallets/")
        && normalized_path.ends_with("/recharge")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "wallets_manage",
            "recharge_balance",
            "admin:wallets",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/wallets/")
        && normalized_path.contains("/refunds/")
        && normalized_path.ends_with("/process")
        && normalized_path.matches('/').count() == 7
    {
        Some(classified(
            "admin_proxy",
            "wallets_manage",
            "process_refund",
            "admin:wallets",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/wallets/")
        && normalized_path.contains("/refunds/")
        && normalized_path.ends_with("/complete")
        && normalized_path.matches('/').count() == 7
    {
        Some(classified(
            "admin_proxy",
            "wallets_manage",
            "complete_refund",
            "admin:wallets",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/wallets/")
        && normalized_path.contains("/refunds/")
        && normalized_path.ends_with("/fail")
        && normalized_path.matches('/').count() == 7
    {
        Some(classified(
            "admin_proxy",
            "wallets_manage",
            "fail_refund",
            "admin:wallets",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(
            normalized_path,
            "/api/admin/user-groups" | "/api/admin/user-groups/"
        )
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "list_user_groups",
            "admin:users",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/user-groups" | "/api/admin/user-groups/"
        )
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "create_user_group",
            "admin:users",
            false,
        ))
    } else if method == http::Method::PUT
        && matches!(
            normalized_path,
            "/api/admin/user-groups/default" | "/api/admin/user-groups/default/"
        )
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "set_default_user_group",
            "admin:users",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/user-groups/")
        && normalized_path.ends_with("/members")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "list_user_group_members",
            "admin:users",
            false,
        ))
    } else if method == http::Method::PUT
        && normalized_path.starts_with("/api/admin/user-groups/")
        && normalized_path.ends_with("/members")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "replace_user_group_members",
            "admin:users",
            false,
        ))
    } else if method == http::Method::PUT
        && normalized_path.starts_with("/api/admin/user-groups/")
        && normalized_path.matches('/').count() == 4
        && !normalized_path.ends_with("/default")
        && !normalized_path.ends_with("/members")
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "update_user_group",
            "admin:users",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/user-groups/")
        && normalized_path.matches('/').count() == 4
        && !normalized_path.ends_with("/default")
        && !normalized_path.ends_with("/members")
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "delete_user_group",
            "admin:users",
            false,
        ))
    } else if method == http::Method::GET
        && matches!(normalized_path, "/api/admin/users" | "/api/admin/users/")
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "list_users",
            "admin:users",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(normalized_path, "/api/admin/users" | "/api/admin/users/")
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "create_user",
            "admin:users",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/users/resolve-selection" | "/api/admin/users/resolve-selection/"
        )
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "resolve_user_selection",
            "admin:users",
            false,
        ))
    } else if method == http::Method::POST
        && matches!(
            normalized_path,
            "/api/admin/users/batch-action" | "/api/admin/users/batch-action/"
        )
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "batch_action_users",
            "admin:users",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path_no_trailing.starts_with("/api/admin/users/")
        && normalized_path_no_trailing.ends_with("/billing/entitlements")
        && normalized_path_no_trailing.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "list_user_billing_entitlements",
            "admin:users",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path_no_trailing.starts_with("/api/admin/users/")
        && normalized_path_no_trailing.ends_with("/billing/grant-plan")
        && normalized_path_no_trailing.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "grant_user_billing_plan",
            "admin:users",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path_no_trailing.starts_with("/api/admin/users/")
        && normalized_path_no_trailing.contains("/billing/entitlements/")
        && normalized_path_no_trailing.matches('/').count() == 7
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "revoke_user_billing_entitlement",
            "admin:users",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/users/")
        && normalized_path.ends_with("/sessions")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "list_user_sessions",
            "admin:users",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/users/")
        && normalized_path.ends_with("/sessions")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "delete_user_sessions",
            "admin:users",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/users/")
        && normalized_path.contains("/sessions/")
        && normalized_path.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "delete_user_session",
            "admin:users",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/users/")
        && normalized_path.ends_with("/api-keys")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "list_user_api_keys",
            "admin:users",
            false,
        ))
    } else if method == http::Method::POST
        && normalized_path.starts_with("/api/admin/users/")
        && normalized_path.ends_with("/api-keys")
        && normalized_path.matches('/').count() == 5
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "create_user_api_key",
            "admin:users",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/users/")
        && normalized_path.contains("/api-keys/")
        && !normalized_path.ends_with("/lock")
        && !normalized_path.ends_with("/full-key")
        && normalized_path.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "delete_user_api_key",
            "admin:users",
            false,
        ))
    } else if method == http::Method::PUT
        && normalized_path.starts_with("/api/admin/users/")
        && normalized_path.contains("/api-keys/")
        && !normalized_path.ends_with("/lock")
        && !normalized_path.ends_with("/full-key")
        && normalized_path.matches('/').count() == 6
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "update_user_api_key",
            "admin:users",
            false,
        ))
    } else if method == http::Method::PATCH
        && normalized_path.starts_with("/api/admin/users/")
        && normalized_path.ends_with("/lock")
        && normalized_path.matches('/').count() == 7
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "lock_user_api_key",
            "admin:users",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/users/")
        && normalized_path.ends_with("/full-key")
        && normalized_path.matches('/').count() == 7
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "reveal_user_api_key",
            "admin:users",
            false,
        ))
    } else if method == http::Method::GET
        && normalized_path.starts_with("/api/admin/users/")
        && !normalized_path.ends_with("/sessions")
        && !normalized_path.contains("/sessions/")
        && !normalized_path.ends_with("/api-keys")
        && !normalized_path.contains("/api-keys/")
        && normalized_path.matches('/').count() == 4
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "get_user",
            "admin:users",
            false,
        ))
    } else if method == http::Method::PUT
        && normalized_path.starts_with("/api/admin/users/")
        && !normalized_path.ends_with("/sessions")
        && !normalized_path.contains("/sessions/")
        && !normalized_path.ends_with("/api-keys")
        && !normalized_path.contains("/api-keys/")
        && normalized_path.matches('/').count() == 4
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "update_user",
            "admin:users",
            false,
        ))
    } else if method == http::Method::DELETE
        && normalized_path.starts_with("/api/admin/users/")
        && !normalized_path.ends_with("/sessions")
        && !normalized_path.contains("/sessions/")
        && !normalized_path.ends_with("/api-keys")
        && !normalized_path.contains("/api-keys/")
        && normalized_path.matches('/').count() == 4
    {
        Some(classified(
            "admin_proxy",
            "users_manage",
            "delete_user",
            "admin:users",
            false,
        ))
    } else {
        None
    }
}
