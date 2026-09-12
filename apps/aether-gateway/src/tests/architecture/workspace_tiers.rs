use super::{collect_workspace_rust_files, read_workspace_file, workspace_file_exists};

fn assert_manifest_excludes(manifest_path: &str, forbidden: &[&str]) {
    let manifest = read_workspace_file(manifest_path);
    let violations = forbidden
        .iter()
        .filter(|dependency| manifest.contains(**dependency))
        .copied()
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "{manifest_path} crosses its dependency tier through: {}",
        violations.join(", ")
    );
}

#[test]
fn pure_policy_crates_do_not_depend_on_runtime_adapters() {
    let pure_manifests = [
        "crates/aether-admission-core/Cargo.toml",
        "crates/aether-provider/core/Cargo.toml",
        "crates/aether-task/core/Cargo.toml",
        "crates/aether-usage/core/Cargo.toml",
    ];
    let forbidden = [
        "axum",
        "sqlx",
        "redis",
        "reqwest",
        "wreq",
        "tokio",
        "aether-data =",
        "aether-gateway",
    ];

    for manifest in pure_manifests {
        assert_manifest_excludes(manifest, &forbidden);
    }
}

#[test]
fn database_adapters_are_independent_driver_boundaries() {
    let path = "crates/aether-data/adapters/postgres/Cargo.toml";
    let manifest = read_workspace_file(path);
    assert!(manifest.contains("aether-data-contracts.workspace = true"));
    assert!(manifest.contains("features = [\"postgres\""));
    assert_manifest_excludes(path, &["aether-gateway", "axum"]);
}

#[test]
fn data_facade_preserves_legacy_driver_paths_without_owning_driver_code() {
    let source = read_workspace_file("crates/aether-data/runtime/src/driver/postgres.rs");
    assert!(source.contains("pub use aether_data_postgres::*;"));
    assert!(!source.contains("sqlx::"));
}

#[test]
fn gateway_runtime_components_keep_focused_dependency_surfaces() {
    assert_manifest_excludes(
        "crates/aether-gateway/frontdoor/Cargo.toml",
        &[
            "aether-data",
            "aether-provider-transport",
            "aether-gateway-workers",
            "sqlx",
            "redis",
        ],
    );
    assert_manifest_excludes(
        "crates/aether-gateway/workers/Cargo.toml",
        &[
            "axum",
            "aether-gateway-frontdoor",
            "aether-provider-transport",
        ],
    );
    assert_manifest_excludes(
        "crates/aether-gateway/execution/Cargo.toml",
        &[
            "axum",
            "sqlx",
            "redis",
            "aether-data",
            "aether-gateway-frontdoor",
            "aether-gateway-workers",
        ],
    );
    assert_manifest_excludes(
        "crates/aether-gateway/control/Cargo.toml",
        &[
            "sqlx",
            "redis",
            "reqwest",
            "aether-data",
            "aether-provider-transport",
            "aether-gateway-workers",
        ],
    );
    assert_manifest_excludes(
        "crates/aether-gateway/tunnel/Cargo.toml",
        &[
            "axum",
            "sqlx",
            "redis",
            "reqwest",
            "wreq",
            "aether-data",
            "aether-provider-transport",
            "aether-gateway-workers",
        ],
    );
    assert_manifest_excludes(
        "crates/aether-testing/loadtools/Cargo.toml",
        &[
            "aether-gateway",
            "aether-testkit",
            "aether-data",
            "axum",
            "redis",
        ],
    );
}

#[test]
fn tunnel_binary_uses_shared_tunnel_boundary_without_gateway_runtime_dependency() {
    let manifest = read_workspace_file("apps/aether-tunnel/Cargo.toml");
    let dependencies = manifest
        .split_once("[dependencies]")
        .expect("tunnel manifest should declare dependencies")
        .1
        .split("[dev-dependencies]")
        .next()
        .expect("normal dependency section should exist");

    assert!(dependencies.contains("aether-gateway-tunnel.workspace = true"));
    assert!(!dependencies.contains("aether-gateway.workspace = true"));

    let protocol_facade = read_workspace_file("apps/aether-tunnel/src/tunnel/protocol.rs");
    assert!(protocol_facade.contains("aether_gateway_tunnel::protocol::*"));
}

#[test]
fn data_facade_defaults_to_postgres_and_gateway_selects_all_drivers_explicitly() {
    let data_manifest = read_workspace_file("crates/aether-data/runtime/Cargo.toml");
    assert!(data_manifest.contains("default = [\"postgres\"]"));
    assert!(data_manifest.contains("aether-data-postgres = { workspace = true, optional = true }"));
    assert!(data_manifest.contains("postgres = [\"dep:aether-data-postgres\", \"sqlx/postgres\"]"));
    assert!(data_manifest.contains("all-drivers = [\"postgres\"]"));

    let gateway_manifest = read_workspace_file("apps/aether-gateway/Cargo.toml");
    assert!(gateway_manifest
        .contains("aether-data = { workspace = true, features = [\"all-drivers\"] }"));

    let data_lib = read_workspace_file("crates/aether-data/runtime/src/lib.rs");
    assert!(data_lib.contains("pub use backend::PostgresBackend;"));
}

#[test]
fn data_query_helpers_belong_to_adapters_not_the_runtime_facade() {
    let data_manifest = read_workspace_file("crates/aether-data/runtime/Cargo.toml");
    assert!(
        !data_manifest.contains("aether-data-query.workspace = true"),
        "aether-data should not keep a direct query-helper dependency after SQL repositories move to adapters"
    );

    let manifest = read_workspace_file("crates/aether-data/adapters/postgres/Cargo.toml");
    assert!(manifest.contains("aether-data-query.workspace = true"));

    let query_helpers = read_workspace_file("crates/aether-data/query/src/lib.rs");
    assert!(query_helpers.contains("Postgres"));
}

#[test]
fn gateway_tunnel_protocol_path_is_a_thin_compatibility_facade() {
    let source = read_workspace_file("apps/aether-gateway/src/tunnel/embedded/protocol.rs");
    assert_eq!(
        source.trim(),
        "pub use aether_gateway_tunnel::embedded::protocol::*;"
    );
}

#[test]
fn frontdoor_owns_bounded_request_body_buffering() {
    let frontdoor = read_workspace_file("crates/aether-gateway/frontdoor/src/body.rs");
    assert!(frontdoor.contains("acquire_many_owned"));
    assert!(frontdoor.contains("body.into_data_stream()"));
    assert!(frontdoor.contains("try_reserve_bytes"));
    assert!(frontdoor.contains("BodyBufferReservation"));

    let gateway = read_workspace_file("apps/aether-gateway/src/handlers/proxy/body_buffer.rs");
    assert!(gateway.contains("FrontdoorBodyBufferPolicy"));
    assert!(!gateway.contains("acquire_many_owned"));
    assert!(!gateway.contains("request_body_collection_exceeded_limit"));
}

#[test]
fn benchmark_binaries_are_outside_the_reusable_testkit() {
    let testkit_bin = "crates/aether-testing/testkit/src/bin";
    assert!(
        !workspace_file_exists(testkit_bin) || collect_workspace_rust_files(testkit_bin).is_empty(),
        "aether-testkit must not own benchmark binaries"
    );
    assert!(
        !collect_workspace_rust_files("crates/aether-testing/loadtools/src/bin").is_empty(),
        "standalone load tools should live in aether-loadtools"
    );
    assert!(
        !collect_workspace_rust_files("crates/aether-testing/integration/src/bin").is_empty(),
        "gateway-backed scenarios should live in aether-integration-tests"
    );
}

#[test]
fn testkit_gateway_harness_is_opt_in() {
    let testkit_manifest = read_workspace_file("crates/aether-testing/testkit/Cargo.toml");
    assert!(
        testkit_manifest.contains("default = []"),
        "aether-testkit should keep the default feature set dependency-light"
    );
    assert!(
        testkit_manifest
            .contains("gateway = [\"dep:aether-gateway\", \"dep:aether-runtime-state\"]"),
        "gateway harnesses should be behind the explicit gateway feature"
    );
    assert!(
        testkit_manifest.contains("postgres = [\"dep:aether-data\", \"dep:sqlx\"]"),
        "Postgres schema helpers should be behind the explicit postgres feature"
    );
    assert!(testkit_manifest.contains(
        "aether-gateway = { workspace = true, features = [\"testkit\"], optional = true }"
    ));

    let testkit_lib =
        read_workspace_file("crates/aether-testing/testkit/src/lib.rs").replace("\r\n", "\n");
    for module in ["execution_runtime", "gateway", "tunnel"] {
        assert!(
            testkit_lib.contains(&format!("#[cfg(feature = \"gateway\")]\nmod {module};")),
            "aether-testkit::{module} should be feature-gated"
        );
    }
    assert!(
        testkit_lib.contains("#[cfg(feature = \"postgres\")]\nmod postgres;"),
        "the Postgres helper should be feature-gated"
    );

    let integration_manifest = read_workspace_file("crates/aether-testing/integration/Cargo.toml");
    assert!(integration_manifest
        .contains("aether-testkit = { workspace = true, features = [\"gateway\", \"postgres\"] }"));
}
