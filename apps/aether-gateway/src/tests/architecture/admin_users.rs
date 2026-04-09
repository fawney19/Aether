use super::{read_workspace_file, workspace_file_exists};

#[test]
fn admin_users_lifecycle_mod_stays_thin() {
    let lifecycle_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/users/lifecycle/mod.rs");
    for pattern in [
        "mod create;",
        "mod delete;",
        "mod reads;",
        "mod support;",
        "mod update;",
    ] {
        assert!(
            lifecycle_mod.contains(pattern),
            "users/lifecycle/mod.rs should keep explicit owner seam {pattern}"
        );
    }
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/users/lifecycle/core.rs"),
        "users/lifecycle/core.rs should be removed once lifecycle is split into explicit owners"
    );
}

#[test]
fn admin_users_api_key_responses_mod_stays_wrapped() {
    let responses_mod = read_workspace_file(
        "apps/aether-gateway/src/handlers/admin/users/api_keys/responses/mod.rs",
    );
    for pattern in [
        "mod create;",
        "mod delete;",
        "mod list;",
        "mod reveal;",
        "mod toggle_lock;",
        "mod update;",
        "pub(in super::super::super) use create::build_admin_create_user_api_key_response;",
        "pub(in super::super::super) use update::build_admin_update_user_api_key_response;",
    ] {
        assert!(
            responses_mod.contains(pattern),
            "users/api_keys/responses/mod.rs should keep wrapped response seam {pattern}"
        );
    }
    for forbidden in [
        "pub mod create;",
        "pub mod delete;",
        "pub mod list;",
        "pub mod reveal;",
        "pub mod toggle_lock;",
        "pub mod update;",
    ] {
        assert!(
            !responses_mod.contains(forbidden),
            "users/api_keys/responses/mod.rs should not expose raw public response module seam {forbidden}"
        );
    }
}
