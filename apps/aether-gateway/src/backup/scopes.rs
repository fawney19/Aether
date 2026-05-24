use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackupScope {
    Config,
    Users,
    Data,
}

impl BackupScope {
    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        match value.trim() {
            "config" => Some(Self::Config),
            "users" => Some(Self::Users),
            "data" => Some(Self::Data),
            _ => None,
        }
    }
}

impl fmt::Display for BackupScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Config => "config",
            Self::Users => "users",
            Self::Data => "data",
        };
        formatter.write_str(value)
    }
}
