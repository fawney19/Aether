CREATE TABLE IF NOT EXISTS routing_groups (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    is_system_default INTEGER NOT NULL DEFAULT 0,
    config_json TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    published_at INTEGER,
    UNIQUE (name)
);

CREATE INDEX IF NOT EXISTS routing_groups_system_default_idx
    ON routing_groups (is_system_default, enabled);

CREATE TABLE IF NOT EXISTS routing_group_bindings (
    id TEXT PRIMARY KEY NOT NULL,
    group_id TEXT NOT NULL,
    subject_type TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    is_default INTEGER NOT NULL DEFAULT 0,
    allow_explicit_select INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS routing_group_bindings_group_id_idx
    ON routing_group_bindings (group_id);
CREATE INDEX IF NOT EXISTS routing_group_bindings_subject_idx
    ON routing_group_bindings (subject_type, subject_id);

CREATE TABLE IF NOT EXISTS routing_group_versions (
    id TEXT PRIMARY KEY NOT NULL,
    group_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    config_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    created_by TEXT,
    UNIQUE (group_id, version)
);

CREATE INDEX IF NOT EXISTS routing_group_versions_group_id_idx
    ON routing_group_versions (group_id);
