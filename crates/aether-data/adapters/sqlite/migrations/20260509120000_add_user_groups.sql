ALTER TABLE users ADD COLUMN allowed_providers_mode TEXT NOT NULL DEFAULT 'unrestricted';
ALTER TABLE users ADD COLUMN allowed_api_formats_mode TEXT NOT NULL DEFAULT 'unrestricted';
ALTER TABLE users ADD COLUMN allowed_models_mode TEXT NOT NULL DEFAULT 'unrestricted';
ALTER TABLE users ADD COLUMN rate_limit_mode TEXT NOT NULL DEFAULT 'system';

UPDATE users
SET allowed_providers_mode = CASE WHEN allowed_providers IS NULL THEN 'unrestricted' ELSE 'specific' END
WHERE allowed_providers_mode = 'unrestricted';

UPDATE users
SET allowed_api_formats_mode = CASE WHEN allowed_api_formats IS NULL THEN 'unrestricted' ELSE 'specific' END
WHERE allowed_api_formats_mode = 'unrestricted';

UPDATE users
SET allowed_models_mode = CASE WHEN allowed_models IS NULL THEN 'unrestricted' ELSE 'specific' END
WHERE allowed_models_mode = 'unrestricted';

UPDATE users
SET rate_limit_mode = CASE WHEN rate_limit IS NULL THEN 'system' ELSE 'custom' END
WHERE rate_limit_mode = 'system';

CREATE TABLE IF NOT EXISTS user_groups (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    normalized_name TEXT NOT NULL UNIQUE,
    description TEXT,
    priority INTEGER NOT NULL DEFAULT 0,
    allowed_providers TEXT,
    allowed_providers_mode TEXT NOT NULL DEFAULT 'inherit',
    allowed_api_formats TEXT,
    allowed_api_formats_mode TEXT NOT NULL DEFAULT 'inherit',
    allowed_models TEXT,
    allowed_models_mode TEXT NOT NULL DEFAULT 'inherit',
    rate_limit INTEGER,
    rate_limit_mode TEXT NOT NULL DEFAULT 'inherit',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS user_group_members (
    group_id TEXT NOT NULL REFERENCES user_groups(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (group_id, user_id)
);

CREATE INDEX IF NOT EXISTS user_group_members_user_id_idx
    ON user_group_members (user_id);

CREATE INDEX IF NOT EXISTS user_groups_priority_name_idx
    ON user_groups (priority DESC, name ASC, id ASC);

INSERT OR IGNORE INTO user_groups (
    id,
    name,
    normalized_name,
    description,
    priority,
    allowed_providers_mode,
    allowed_api_formats_mode,
    allowed_models_mode,
    rate_limit_mode,
    created_at,
    updated_at
)
VALUES (
    '00000000-0000-0000-0000-000000000001',
    'Default',
    'default',
    'Default group for all users',
    0,
    'unrestricted',
    'unrestricted',
    'unrestricted',
    'system',
    CAST(strftime('%s', 'now') AS INTEGER),
    CAST(strftime('%s', 'now') AS INTEGER)
);

INSERT OR IGNORE INTO system_configs (
    id,
    key,
    value,
    description,
    created_at,
    updated_at
)
VALUES (
    '00000000-0000-0000-0000-000000000002',
    'default_user_group_id',
    '"00000000-0000-0000-0000-000000000001"',
    'Default user group',
    CAST(strftime('%s', 'now') AS INTEGER),
    CAST(strftime('%s', 'now') AS INTEGER)
);

INSERT OR IGNORE INTO user_group_members (group_id, user_id, created_at)
SELECT '00000000-0000-0000-0000-000000000001', id, CAST(strftime('%s', 'now') AS INTEGER)
FROM users
WHERE is_deleted = 0
  AND LOWER(role) <> 'admin';
