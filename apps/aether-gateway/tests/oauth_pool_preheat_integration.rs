use std::path::Path;
use std::process::{Command, Output};
use std::sync::OnceLock;

const UNDERLYING_TESTS: &[&str] = &[
    "q1_cache_hit_no_sql_on_second_request",
    "q2_admin_mutation_invalidates_cache",
    "q3_backfill_maintains_128_on_evict",
    "q4_probe_starts_before_k1_first_byte_and_concurrent_with_dispatch",
    "q5_probe_dedup_skips_within_5min",
    "q6_per_provider_rate_limit_crossnode",
    "q7_probe_circuit_on_global_5xx",
    "q8_hedge_swap_on_k1_401_saves_request",
    "q9_hedge_no_healthy_falls_back_original_loop",
    "q10_body_1mb_eligibility_check",
];

static LIB_SUITE_OUTPUT: OnceLock<Output> = OnceLock::new();

fn gateway_manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn lib_suite_output() -> &'static Output {
    LIB_SUITE_OUTPUT.get_or_init(|| {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        Command::new(cargo)
            .current_dir(gateway_manifest_dir())
            .env("NO_COLOR", "1")
            .args([
                "test",
                "-p",
                "aether-gateway",
                "--lib",
                "oauth_pool_preheat_integration",
                "--color=never",
                "--",
                "--nocapture",
            ])
            .output()
            .expect("cargo should run the in-crate oauth_pool_preheat_integration suite")
    })
}

fn combined_output(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{stdout}\n{stderr}")
}

fn assert_underlying_q_test_passed(test_name: &str) {
    assert!(
        UNDERLYING_TESTS.contains(&test_name),
        "unknown oauth pool preheat test bridge target: {test_name}"
    );

    let output = lib_suite_output();
    let combined = combined_output(output);
    assert!(
        output.status.success(),
        "in-crate oauth_pool_preheat_integration suite failed for bridge target {test_name}\n\n{combined}"
    );

    let pass_marker = format!("{test_name} ... ok");
    assert!(
        combined.contains(&pass_marker),
        "in-crate oauth_pool_preheat_integration output did not show {test_name} passing\n\nexpected marker: {pass_marker}\n\n{combined}"
    );
}

// The Q1-Q10 behavior tests need crate-private fixtures, so this external target
// preserves the private API boundary by running the lib-filtered suite once.
#[test]
fn q1_cache_hit_no_sql_on_second_request() {
    assert_underlying_q_test_passed("q1_cache_hit_no_sql_on_second_request");
}

#[test]
fn q2_admin_mutation_invalidates_cache() {
    assert_underlying_q_test_passed("q2_admin_mutation_invalidates_cache");
}

#[test]
fn q3_backfill_maintains_128_on_evict() {
    assert_underlying_q_test_passed("q3_backfill_maintains_128_on_evict");
}

#[test]
fn q4_probe_starts_before_k1_first_byte_and_concurrent_with_dispatch() {
    assert_underlying_q_test_passed(
        "q4_probe_starts_before_k1_first_byte_and_concurrent_with_dispatch",
    );
}

#[test]
fn q5_probe_dedup_skips_within_5min() {
    assert_underlying_q_test_passed("q5_probe_dedup_skips_within_5min");
}

#[test]
fn q6_per_provider_rate_limit_crossnode() {
    assert_underlying_q_test_passed("q6_per_provider_rate_limit_crossnode");
}

#[test]
fn q7_probe_circuit_on_global_5xx() {
    assert_underlying_q_test_passed("q7_probe_circuit_on_global_5xx");
}

#[test]
fn q8_hedge_swap_on_k1_401_saves_request() {
    assert_underlying_q_test_passed("q8_hedge_swap_on_k1_401_saves_request");
}

#[test]
fn q9_hedge_no_healthy_falls_back_original_loop() {
    assert_underlying_q_test_passed("q9_hedge_no_healthy_falls_back_original_loop");
}

#[test]
fn q10_body_1mb_eligibility_check() {
    assert_underlying_q_test_passed("q10_body_1mb_eligibility_check");
}
