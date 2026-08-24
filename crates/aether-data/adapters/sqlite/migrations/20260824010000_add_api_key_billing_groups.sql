ALTER TABLE user_groups
    ADD COLUMN sell_rate_multiplier REAL NOT NULL DEFAULT 1
    CHECK (sell_rate_multiplier >= 0 AND sell_rate_multiplier <= 100);

ALTER TABLE api_keys ADD COLUMN group_id TEXT;

UPDATE api_keys
SET group_id = '00000000-0000-0000-0000-000000000001'
WHERE is_standalone = 0 AND group_id IS NULL;

CREATE INDEX api_keys_group_id_idx ON api_keys (group_id);
