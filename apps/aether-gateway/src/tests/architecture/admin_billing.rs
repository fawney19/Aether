use super::*;

#[test]
fn admin_billing_wallets_boundaries_are_split() {
    let wallets_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/billing/wallets/mod.rs");
    for pattern in ["mod mutations;", "mod reads;", "mod routes;", "mod shared;"] {
        assert!(
            wallets_mod.contains(pattern),
            "handlers/admin/billing/wallets/mod.rs should register {pattern}"
        );
    }

    let shared_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/billing/wallets/shared/mod.rs");
    for pattern in [
        "mod normalizers;",
        "mod payloads;",
        "mod requests;",
        "mod responses;",
        "mod support;",
    ] {
        assert!(
            shared_mod.contains(pattern),
            "handlers/admin/billing/wallets/shared/mod.rs should register {pattern}"
        );
    }

    let mutations_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/billing/wallets/mutations/mod.rs",
    );
    for pattern in [
        "mod adjust;",
        "mod complete_refund;",
        "mod fail_refund;",
        "mod process_refund;",
        "mod recharge;",
    ] {
        assert!(
            mutations_mod.contains(pattern),
            "handlers/admin/billing/wallets/mutations/mod.rs should register {pattern}"
        );
    }

    let reads_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/billing/wallets/reads/mod.rs");
    for pattern in [
        "mod detail;",
        "mod ledger;",
        "mod list;",
        "mod refund_requests;",
        "mod refunds;",
        "mod transactions;",
    ] {
        assert!(
            reads_mod.contains(pattern),
            "handlers/admin/billing/wallets/reads/mod.rs should register {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/handlers/admin/billing/wallets/shared/core.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/mutations/core.rs",
        "apps/aether-gateway/src/handlers/admin/billing/wallets/reads.rs",
    ] {
        assert!(
            !workspace_file_exists(path),
            "{path} should be removed after wallets boundaries are split"
        );
    }
}

#[test]
fn admin_billing_collectors_owner_is_split() {
    let collectors_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/billing/collectors/mod.rs");
    for pattern in ["mod reads;", "mod support;", "mod writes;"] {
        assert!(
            collectors_mod.contains(pattern),
            "handlers/admin/billing/collectors/mod.rs should register {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/handlers/admin/billing/collectors/support.rs",
        "apps/aether-gateway/src/handlers/admin/billing/collectors/reads.rs",
        "apps/aether-gateway/src/handlers/admin/billing/collectors/writes.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist after collectors owner split"
        );
    }

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/billing/collectors.rs"),
        "handlers/admin/billing/collectors.rs should be removed after collectors owner split"
    );

    let collectors_support =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/billing/collectors/support.rs");
    assert!(
        !collectors_support.contains("pub(super) use super::super::{"),
        "handlers/admin/billing/collectors/support.rs should not keep wildcard bridge re-export from billing root"
    );
}

#[test]
fn admin_billing_presets_owner_is_split() {
    let presets_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/billing/presets/mod.rs");
    for pattern in ["mod apply;", "mod support;"] {
        assert!(
            presets_mod.contains(pattern),
            "handlers/admin/billing/presets/mod.rs should register {pattern}"
        );
    }

    for path in [
        "apps/aether-gateway/src/handlers/admin/billing/presets/support.rs",
        "apps/aether-gateway/src/handlers/admin/billing/presets/apply.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist after presets owner split"
        );
    }

    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/billing/presets.rs"),
        "handlers/admin/billing/presets.rs should be removed after presets owner split"
    );
}

#[test]
fn admin_billing_wallets_support_uses_wrapped_request_context() {
    let wallets_support = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/billing/wallets/shared/support.rs",
    );
    assert!(
        wallets_support.contains("use crate::handlers::admin::request::AdminRequestContext;"),
        "handlers/admin/billing/wallets/shared/support.rs should consume wrapped AdminRequestContext"
    );
    assert!(
        !wallets_support.contains("GatewayPublicRequestContext"),
        "handlers/admin/billing/wallets/shared/support.rs should not keep raw GatewayPublicRequestContext seam"
    );
}
