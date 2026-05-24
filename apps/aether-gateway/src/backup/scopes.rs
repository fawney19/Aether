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

    pub(crate) fn as_config_value(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Users => "users",
            Self::Data => "data",
        }
    }

    pub(crate) fn route_kind(self) -> &'static str {
        match self {
            Self::Config => "config_export",
            Self::Users => "users_export",
            Self::Data => "data_export",
        }
    }

    pub(crate) fn file_stem(self) -> &'static str {
        match self {
            Self::Config => "aether-config-backup",
            Self::Users => "aether-users-backup",
            Self::Data => "aether-data-backup",
        }
    }

    pub(crate) fn object_key(self, prefix: &str, timestamp: &str) -> String {
        let file_name = self.file_name(timestamp);
        let prefix = normalized_prefix(prefix);

        if prefix.is_empty() {
            file_name
        } else {
            format!("{prefix}/{file_name}")
        }
    }

    pub(crate) fn matching_backup_keys(
        self,
        prefix: &str,
        keys: impl IntoIterator<Item = String>,
    ) -> Vec<String> {
        let normalized_prefix = normalized_prefix(prefix);
        let expected_prefix = if normalized_prefix.is_empty() {
            String::new()
        } else {
            format!("{normalized_prefix}/")
        };
        let file_prefix = format!("{}-", self.file_stem());
        let file_suffix = ".json.zst";

        keys.into_iter()
            .filter(|key| {
                let Some(file_name) = key.strip_prefix(&expected_prefix) else {
                    return false;
                };
                if file_name.contains('/') {
                    return false;
                }
                let Some(timestamp) = file_name
                    .strip_prefix(&file_prefix)
                    .and_then(|rest| rest.strip_suffix(file_suffix))
                else {
                    return false;
                };

                is_aether_backup_timestamp(timestamp)
            })
            .collect()
    }

    fn file_name(self, timestamp: &str) -> String {
        format!("{}-{timestamp}.json.zst", self.file_stem())
    }
}

impl fmt::Display for BackupScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_config_value())
    }
}

fn normalized_prefix(prefix: &str) -> &str {
    prefix.trim_end_matches('/')
}

fn is_aether_backup_timestamp(timestamp: &str) -> bool {
    let bytes = timestamp.as_bytes();

    bytes.len() == 15
        && bytes[8] == b'-'
        && bytes[..8].iter().all(|byte| byte.is_ascii_digit())
        && bytes[9..].iter().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::BackupScope;

    #[test]
    fn backup_scope_matches_export_routes_and_object_prefixes() {
        assert_eq!(BackupScope::Config.as_config_value(), "config");
        assert_eq!(BackupScope::Users.as_config_value(), "users");
        assert_eq!(BackupScope::Data.as_config_value(), "data");

        assert_eq!(BackupScope::Config.route_kind(), "config_export");
        assert_eq!(BackupScope::Users.route_kind(), "users_export");
        assert_eq!(BackupScope::Data.route_kind(), "data_export");

        assert_eq!(BackupScope::Config.file_stem(), "aether-config-backup");
        assert_eq!(BackupScope::Users.file_stem(), "aether-users-backup");
        assert_eq!(BackupScope::Data.file_stem(), "aether-data-backup");

        assert_eq!(
            BackupScope::Config.object_key("prod/", "20260524-031500"),
            "prod/aether-config-backup-20260524-031500.json.zst"
        );
        assert_eq!(
            BackupScope::Users.object_key("prod/", "20260524-031500"),
            "prod/aether-users-backup-20260524-031500.json.zst"
        );
        assert_eq!(
            BackupScope::Data.object_key("prod/", "20260524-031500"),
            "prod/aether-data-backup-20260524-031500.json.zst"
        );
    }

    #[test]
    fn retention_filter_only_matches_same_scope() {
        let keys = vec![
            "prod/aether-config-backup-20260524-010000.json.zst".to_string(),
            "prod/aether-users-backup-20260524-010000.json.zst".to_string(),
            "prod/aether-data-backup-20260524-010000.json.zst".to_string(),
            "prod/random.json.zst".to_string(),
        ];

        let matched = BackupScope::Users.matching_backup_keys("prod/", keys);

        assert_eq!(
            matched,
            vec!["prod/aether-users-backup-20260524-010000.json.zst"]
        );
    }

    #[test]
    fn retention_filter_requires_aether_timestamp_format() {
        let keys = vec![
            "prod/aether-users-backup-20260524-010000.json.zst".to_string(),
            "prod/aether-users-backup-foo.json.zst".to_string(),
            "prod/aether-users-backup-2026052-010000.json.zst".to_string(),
            "prod/aether-users-backup-202605240-010000.json.zst".to_string(),
            "prod/aether-users-backup-20260524-01000.json.zst".to_string(),
            "prod/aether-users-backup-20260524-0100000.json.zst".to_string(),
            "prod/aether-users-backup-20260524010000.json.zst".to_string(),
            "prod/aether-users-backup-2026052a-010000.json.zst".to_string(),
            "prod/aether-users-backup-20260524-01000x.json.zst".to_string(),
        ];

        let matched = BackupScope::Users.matching_backup_keys("prod/", keys);

        assert_eq!(
            matched,
            vec!["prod/aether-users-backup-20260524-010000.json.zst"]
        );
    }

    #[test]
    fn backup_key_prefix_boundaries_are_exact() {
        assert_eq!(
            BackupScope::Config.object_key("", "20260524-031500"),
            "aether-config-backup-20260524-031500.json.zst"
        );
        assert_eq!(
            BackupScope::Config.object_key("prod", "20260524-031500"),
            "prod/aether-config-backup-20260524-031500.json.zst"
        );

        let keys = vec![
            "prod/aether-config-backup-20260524-010000.json.zst".to_string(),
            "prod//aether-config-backup-20260524-010000.json.zst".to_string(),
            "prod-backups/aether-config-backup-20260524-010000.json.zst".to_string(),
            "prod/aether-config-backup-20260524-010000.json".to_string(),
            "prod/aether-config-backup-.json.zst".to_string(),
            "aether-config-backup-20260524-010000.json.zst".to_string(),
        ];

        let matched = BackupScope::Config.matching_backup_keys("prod", keys);

        assert_eq!(
            matched,
            vec!["prod/aether-config-backup-20260524-010000.json.zst"]
        );
    }
}
