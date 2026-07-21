use crate::observability::{FileLoggingConfig, LogDestination, ServiceObservabilityConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRuntimeConfig {
    pub service_name: &'static str,
    pub default_log_filter: &'static str,
    pub observability: ServiceObservabilityConfig,
}

impl ServiceRuntimeConfig {
    pub const fn new(service_name: &'static str, default_log_filter: &'static str) -> Self {
        Self {
            service_name,
            default_log_filter,
            observability: ServiceObservabilityConfig::new(crate::LogFormat::Pretty, service_name),
        }
    }

    pub const fn with_log_format(mut self, log_format: crate::LogFormat) -> Self {
        self.observability.log_format = log_format;
        self
    }

    pub const fn with_log_destination(mut self, log_destination: LogDestination) -> Self {
        self.observability.log_destination = log_destination;
        self
    }

    pub fn with_file_logging(mut self, file_logging: FileLoggingConfig) -> Self {
        self.observability.file_logging = Some(file_logging);
        self
    }

    pub fn with_node_role(mut self, node_role: impl Into<String>) -> Self {
        self.observability.node_role = Some(node_role.into());
        self
    }

    pub fn with_instance_id(mut self, instance_id: impl Into<String>) -> Self {
        self.observability.instance_id = Some(instance_id.into());
        self
    }

    pub const fn with_metrics_namespace(mut self, metrics_namespace: &'static str) -> Self {
        self.observability.metrics_namespace = metrics_namespace;
        self
    }
}
