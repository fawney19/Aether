//! Standalone load and benchmark tooling.
//!
//! This crate intentionally has no dependency on gateway internals or the
//! integration test harness, so ordinary load-tool builds remain lightweight.

mod http;
mod load;
mod metrics;
mod runtime;

pub use aether_test_support::ManagedRedisServer;
pub use http::{json_body, test_http_client, test_http_client_config};
pub use load::{
    run_http_load_probe, run_http_load_probe_with_options, run_multi_url_http_load_probe,
    run_multi_url_http_load_probe_with_options, HttpLoadProbeConfig, HttpLoadProbeErrorSample,
    HttpLoadProbeOptions, HttpLoadProbeResponseMode, HttpLoadProbeResult,
    HttpLoadProbeStatusSample, MultiUrlHttpLoadProbeResult,
};
pub use metrics::{
    fetch_prometheus_samples, find_metric_value_u64, parse_prometheus_samples, PrometheusSample,
};
pub use runtime::{BenchmarkRuntimeSampler, BenchmarkRuntimeSnapshot};

pub fn init_load_runtime_for(service_name: &'static str) {
    let _ = aether_runtime::init_service_runtime(
        aether_runtime::ServiceRuntimeConfig::new(service_name, "aether_loadtools=debug")
            .with_metrics_namespace("aether_loadtools"),
    );
}
