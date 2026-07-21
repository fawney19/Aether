use aether_runtime::{init_service_runtime, ServiceRuntimeConfig};

pub fn init_test_runtime() {
    init_test_runtime_for("aether-testkit");
}

pub fn test_runtime_config(service_name: &'static str) -> ServiceRuntimeConfig {
    ServiceRuntimeConfig::new(service_name, "aether_testkit=debug")
        .with_metrics_namespace("aether_testkit")
}

pub fn init_test_runtime_for(service_name: &'static str) {
    let _ = init_service_runtime(test_runtime_config(service_name));
}
