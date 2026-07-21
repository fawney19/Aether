CREATE TABLE IF NOT EXISTS routing_groups (
    `id` VARCHAR(64) NOT NULL,
    `name` VARCHAR(255) NOT NULL,
    `description` LONGTEXT,
    `enabled` TINYINT(1) NOT NULL DEFAULT 1,
    `is_system_default` TINYINT(1) NOT NULL DEFAULT 0,
    `config_json` JSON NOT NULL,
    `version` BIGINT NOT NULL DEFAULT 1,
    `created_at` BIGINT NOT NULL,
    `updated_at` BIGINT NOT NULL,
    `published_at` BIGINT,
    PRIMARY KEY (`id`),
    UNIQUE KEY routing_groups_name_key (`name`),
    KEY routing_groups_system_default_idx (`is_system_default`, `enabled`)
);

CREATE TABLE IF NOT EXISTS routing_group_bindings (
    `id` VARCHAR(64) NOT NULL,
    `group_id` VARCHAR(64) NOT NULL,
    `subject_type` VARCHAR(32) NOT NULL,
    `subject_id` VARCHAR(64) NOT NULL,
    `is_default` TINYINT(1) NOT NULL DEFAULT 0,
    `allow_explicit_select` TINYINT(1) NOT NULL DEFAULT 1,
    `created_at` BIGINT NOT NULL,
    `updated_at` BIGINT NOT NULL,
    PRIMARY KEY (`id`),
    KEY routing_group_bindings_group_id_idx (`group_id`),
    KEY routing_group_bindings_subject_idx (`subject_type`, `subject_id`)
);

CREATE TABLE IF NOT EXISTS routing_group_versions (
    `id` VARCHAR(64) NOT NULL,
    `group_id` VARCHAR(64) NOT NULL,
    `version` BIGINT NOT NULL,
    `config_json` JSON NOT NULL,
    `created_at` BIGINT NOT NULL,
    `created_by` VARCHAR(64),
    PRIMARY KEY (`id`),
    UNIQUE KEY routing_group_versions_group_version_key (`group_id`, `version`),
    KEY routing_group_versions_group_id_idx (`group_id`)
);
