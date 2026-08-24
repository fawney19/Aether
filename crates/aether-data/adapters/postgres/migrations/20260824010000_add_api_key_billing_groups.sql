ALTER TABLE user_groups
    ADD COLUMN sell_rate_multiplier DOUBLE PRECISION NOT NULL DEFAULT 1
    CHECK (sell_rate_multiplier >= 0 AND sell_rate_multiplier <= 100);

ALTER TABLE api_keys ADD COLUMN group_id VARCHAR(64);

UPDATE api_keys
SET group_id = '00000000-0000-0000-0000-000000000001'
WHERE is_standalone = FALSE AND group_id IS NULL;

CREATE INDEX api_keys_group_id_idx ON api_keys (group_id);
ALTER TABLE api_keys
    ADD CONSTRAINT api_keys_group_id_fkey
    FOREIGN KEY (group_id) REFERENCES user_groups(id) ON DELETE RESTRICT;
