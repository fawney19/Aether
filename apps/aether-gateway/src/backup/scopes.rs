use std::fmt;

const ENCRYPTED_BACKUP_FILE_SUFFIX: &str = ".json.zst.aes256gcm";
const LEGACY_PLAINTEXT_BACKUP_FILE_SUFFIX: &str = ".json.zst";

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

    pub(crate) fn from_encrypted_object_key(object_key: &str) -> Option<Self> {
        if object_key.is_empty()
            || object_key.starts_with('/')
            || object_key.contains('\0')
            || object_key.contains('\\')
        {
            return None;
        }
        let mut segments = object_key.split('/').peekable();
        let mut file_name = None;
        while let Some(segment) = segments.next() {
            if segment.is_empty()
                || segment == "."
                || segment == ".."
                || segment.chars().any(char::is_control)
            {
                return None;
            }
            if segments.peek().is_none() {
                file_name = Some(segment);
            }
        }
        let file_name = file_name?;

        [Self::Config, Self::Users, Self::Data]
            .into_iter()
            .find(|scope| {
                file_name
                    .strip_prefix(&format!("{}-", scope.file_stem()))
                    .and_then(|rest| rest.strip_suffix(ENCRYPTED_BACKUP_FILE_SUFFIX))
                    .is_some_and(is_aether_backup_object_id)
            })
    }

    #[cfg(test)]
    pub(crate) fn matching_backup_keys(
        self,
        prefix: &str,
        keys: impl IntoIterator<Item = String>,
    ) -> Vec<String> {
        self.matching_backup_keys_with_suffixes(
            prefix,
            keys,
            &[
                ENCRYPTED_BACKUP_FILE_SUFFIX,
                LEGACY_PLAINTEXT_BACKUP_FILE_SUFFIX,
            ],
        )
    }

    pub(crate) fn matching_encrypted_backup_keys(
        self,
        prefix: &str,
        keys: impl IntoIterator<Item = String>,
    ) -> Vec<String> {
        self.matching_backup_keys_with_suffixes(prefix, keys, &[ENCRYPTED_BACKUP_FILE_SUFFIX])
    }

    pub(crate) fn matching_legacy_plaintext_backup_keys(
        self,
        prefix: &str,
        keys: impl IntoIterator<Item = String>,
    ) -> Vec<String> {
        self.matching_backup_keys_with_suffixes(
            prefix,
            keys,
            &[LEGACY_PLAINTEXT_BACKUP_FILE_SUFFIX],
        )
    }

    fn matching_backup_keys_with_suffixes(
        self,
        prefix: &str,
        keys: impl IntoIterator<Item = String>,
        file_suffixes: &[&str],
    ) -> Vec<String> {
        let normalized_prefix = normalized_prefix(prefix);
        let expected_prefix = if normalized_prefix.is_empty() {
            String::new()
        } else {
            format!("{normalized_prefix}/")
        };
        let file_prefix = format!("{}-", self.file_stem());

        keys.into_iter()
            .filter(|key| {
                let Some(file_name) = key.strip_prefix(&expected_prefix) else {
                    return false;
                };
                if file_name.contains('/') {
                    return false;
                }
                let Some(timestamp) = file_name.strip_prefix(&file_prefix).and_then(|rest| {
                    file_suffixes
                        .iter()
                        .find_map(|suffix| rest.strip_suffix(suffix))
                }) else {
                    return false;
                };

                is_aether_backup_object_id(timestamp)
            })
            .collect()
    }

    fn file_name(self, timestamp: &str) -> String {
        format!(
            "{}-{timestamp}{ENCRYPTED_BACKUP_FILE_SUFFIX}",
            self.file_stem()
        )
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

fn is_aether_backup_object_id(value: &str) -> bool {
    if is_aether_backup_timestamp(value) {
        return true;
    }

    let Some((timestamp, collision_digest)) = value.split_once('-').and_then(|(date, rest)| {
        let (time, digest) = rest.split_once('-')?;
        Some((format!("{date}-{time}"), digest))
    }) else {
        return false;
    };

    is_aether_backup_timestamp(&timestamp)
        && collision_digest.len() == 64
        && collision_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
            "prod/aether-config-backup-20260524-031500.json.zst.aes256gcm"
        );
        assert_eq!(
            BackupScope::Users.object_key("prod/", "20260524-031500"),
            "prod/aether-users-backup-20260524-031500.json.zst.aes256gcm"
        );
        assert_eq!(
            BackupScope::Data.object_key("prod/", "20260524-031500"),
            "prod/aether-data-backup-20260524-031500.json.zst.aes256gcm"
        );
    }

    #[test]
    fn retention_filter_only_matches_same_scope() {
        let keys = vec![
            "prod/aether-config-backup-20260524-010000.json.zst".to_string(),
            "prod/aether-users-backup-20260524-010000.json.zst.aes256gcm".to_string(),
            "prod/aether-data-backup-20260524-010000.json.zst".to_string(),
            "prod/random.json.zst".to_string(),
        ];

        let matched = BackupScope::Users.matching_backup_keys("prod/", keys);

        assert_eq!(
            matched,
            vec!["prod/aether-users-backup-20260524-010000.json.zst.aes256gcm"]
        );
    }

    #[test]
    fn retention_filter_requires_aether_timestamp_format() {
        let collision_digest = "a".repeat(64);
        let keys = vec![
            "prod/aether-users-backup-20260524-010000.json.zst".to_string(),
            format!(
                "prod/aether-users-backup-20260524-010000-{collision_digest}.json.zst.aes256gcm"
            ),
            "prod/aether-users-backup-foo.json.zst".to_string(),
            "prod/aether-users-backup-2026052-010000.json.zst".to_string(),
            "prod/aether-users-backup-202605240-010000.json.zst".to_string(),
            "prod/aether-users-backup-20260524-01000.json.zst".to_string(),
            "prod/aether-users-backup-20260524-0100000.json.zst".to_string(),
            "prod/aether-users-backup-20260524010000.json.zst".to_string(),
            "prod/aether-users-backup-2026052a-010000.json.zst".to_string(),
            "prod/aether-users-backup-20260524-01000x.json.zst".to_string(),
            "prod/aether-users-backup-20260524-010000-short.json.zst.aes256gcm".to_string(),
        ];

        let matched = BackupScope::Users.matching_backup_keys("prod/", keys);

        assert_eq!(
            matched,
            vec![
                "prod/aether-users-backup-20260524-010000.json.zst".to_string(),
                format!(
                    "prod/aether-users-backup-20260524-010000-{collision_digest}.json.zst.aes256gcm"
                ),
            ]
        );
    }

    #[test]
    fn backup_key_prefix_boundaries_are_exact() {
        assert_eq!(
            BackupScope::Config.object_key("", "20260524-031500"),
            "aether-config-backup-20260524-031500.json.zst.aes256gcm"
        );
        assert_eq!(
            BackupScope::Config.object_key("prod", "20260524-031500"),
            "prod/aether-config-backup-20260524-031500.json.zst.aes256gcm"
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

    #[test]
    fn encrypted_object_key_parser_binds_scope_and_rejects_path_traversal() {
        let collision_digest = "a".repeat(64);
        assert_eq!(
            BackupScope::from_encrypted_object_key(
                "prod/aether-config-backup-20260524-010000.json.zst.aes256gcm"
            ),
            Some(BackupScope::Config)
        );
        assert_eq!(
            BackupScope::from_encrypted_object_key(&format!(
                "prod/aether-users-backup-20260524-010000-{collision_digest}.json.zst.aes256gcm"
            )),
            Some(BackupScope::Users)
        );
        for key in [
            "../aether-data-backup-20260524-010000.json.zst.aes256gcm",
            "/aether-data-backup-20260524-010000.json.zst.aes256gcm",
            "prod//aether-data-backup-20260524-010000.json.zst.aes256gcm",
            "prod/./aether-data-backup-20260524-010000.json.zst.aes256gcm",
            "prod\\aether-data-backup-20260524-010000.json.zst.aes256gcm",
            "prod/aether-data-backup-invalid.json.zst.aes256gcm",
            "prod/unrelated-20260524-010000.json.zst.aes256gcm",
        ] {
            assert_eq!(
                BackupScope::from_encrypted_object_key(key),
                None,
                "unsafe or unrelated key: {key}"
            );
        }
    }
}
