use super::*;

#[test]
fn admin_model_global_owner_is_split() {
    let global_mod =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/model/global/mod.rs");
    for pattern in ["mod helpers;", "mod payloads;", "mod providers;"] {
        assert!(
            global_mod.contains(pattern),
            "handlers/admin/model/global/mod.rs should register {pattern}"
        );
    }
    for path in [
        "apps/aether-gateway/src/handlers/admin/model/global/helpers.rs",
        "apps/aether-gateway/src/handlers/admin/model/global/payloads.rs",
        "apps/aether-gateway/src/handlers/admin/model/global/providers.rs",
    ] {
        assert!(
            workspace_file_exists(path),
            "{path} should exist after model/global owner split"
        );
    }
    assert!(
        !workspace_file_exists("apps/aether-gateway/src/handlers/admin/model/global.rs"),
        "handlers/admin/model/global.rs should be removed after owner split"
    );
}

#[test]
fn admin_model_root_exposes_single_route_seam() {
    let model_mod = read_workspace_file("apps/aether-gateway/src/handlers/admin/model/mod.rs");
    assert!(
        model_mod.contains("mod routes;"),
        "handlers/admin/model/mod.rs should register routes.rs as the model route seam"
    );
    assert!(
        model_mod.contains("pub(super) use self::routes::maybe_build_local_admin_model_response;"),
        "handlers/admin/model/mod.rs should expose maybe_build_local_admin_model_response"
    );

    let model_routes =
        read_workspace_file("apps/aether-gateway/src/handlers/admin/model/routes.rs");
    for pattern in [
        "catalog_routes::maybe_build_local_admin_model_catalog_response(",
        "global_models::maybe_build_local_admin_global_models_response(",
    ] {
        assert!(
            model_routes.contains(pattern),
            "handlers/admin/model/routes.rs should dispatch through {pattern}"
        );
    }
}
