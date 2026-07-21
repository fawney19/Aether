mod fixtures;
mod redis;
mod server;
mod tracing;
mod wait;

#[cfg(feature = "gateway")]
mod execution_runtime;
#[cfg(feature = "gateway")]
mod gateway;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "gateway")]
mod tunnel;

pub use aether_loadtools::{
    fetch_prometheus_samples, find_metric_value_u64, parse_prometheus_samples, PrometheusSample,
};
pub use aether_loadtools::{
    json_body, run_http_load_probe, run_multi_url_http_load_probe, test_http_client,
    test_http_client_config, HttpLoadProbeConfig, HttpLoadProbeResponseMode, HttpLoadProbeResult,
    MultiUrlHttpLoadProbeResult,
};
pub use aether_loadtools::{BenchmarkRuntimeSampler, BenchmarkRuntimeSnapshot};
pub use fixtures::test_trace_id;
pub use redis::ManagedRedisServer;
pub use server::{reserve_local_port, SpawnedServer};
pub use tracing::{init_test_runtime, init_test_runtime_for, test_runtime_config};
pub use wait::wait_until;

#[cfg(feature = "gateway")]
pub use execution_runtime::{ExecutionRuntimeHarness, ExecutionRuntimeHarnessConfig};
#[cfg(feature = "gateway")]
pub use gateway::{GatewayHarness, GatewayHarnessConfig, GATEWAY_HARNESS_API_KEY};
#[cfg(feature = "postgres")]
pub use postgres::{prepare_aether_postgres_schema, ManagedPostgresServer};
#[cfg(feature = "gateway")]
pub use tunnel::{TunnelHarness, TunnelHarnessConfig};
