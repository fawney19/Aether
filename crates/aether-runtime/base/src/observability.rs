use crate::tracing::LogFormat;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogDestination {
    Stdout,
    File,
    Both,
}

impl LogDestination {
    pub const fn needs_file_sink(self) -> bool {
        matches!(self, Self::File | Self::Both)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogRotation {
    Hourly,
    Daily,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLoggingConfig {
    pub dir: PathBuf,
    pub rotation: LogRotation,
    pub retention_days: u64,
    pub max_files: usize,
}

impl FileLoggingConfig {
    pub fn new(
        dir: impl Into<PathBuf>,
        rotation: LogRotation,
        retention_days: u64,
        max_files: usize,
    ) -> Self {
        Self {
            dir: dir.into(),
            rotation,
            retention_days,
            max_files,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceObservabilityConfig {
    pub log_format: LogFormat,
    pub metrics_namespace: &'static str,
    pub log_destination: LogDestination,
    pub file_logging: Option<FileLoggingConfig>,
    pub node_role: Option<String>,
    pub instance_id: Option<String>,
}

impl ServiceObservabilityConfig {
    pub const fn new(log_format: LogFormat, metrics_namespace: &'static str) -> Self {
        Self {
            log_format,
            metrics_namespace,
            log_destination: LogDestination::Stdout,
            file_logging: None,
            node_role: None,
            instance_id: None,
        }
    }

    pub const fn with_log_destination(mut self, log_destination: LogDestination) -> Self {
        self.log_destination = log_destination;
        self
    }

    pub fn with_file_logging(mut self, file_logging: FileLoggingConfig) -> Self {
        self.file_logging = Some(file_logging);
        self
    }

    pub fn with_node_role(mut self, node_role: impl Into<String>) -> Self {
        self.node_role = Some(node_role.into());
        self
    }

    pub fn with_instance_id(mut self, instance_id: impl Into<String>) -> Self {
        self.instance_id = Some(instance_id.into());
        self
    }
}
