SET @aether_provider_key_name_index_sql := IF(
    (
        SELECT COUNT(*)
        FROM information_schema.statistics
        WHERE table_schema = DATABASE()
          AND table_name = 'provider_api_keys'
          AND index_name = 'idx_provider_api_keys_provider_name_id'
    ) = 0,
    'CREATE INDEX idx_provider_api_keys_provider_name_id ON provider_api_keys (provider_id, name, id)',
    'DO 0'
);

PREPARE aether_provider_key_name_index_stmt FROM @aether_provider_key_name_index_sql;
EXECUTE aether_provider_key_name_index_stmt;
DEALLOCATE PREPARE aether_provider_key_name_index_stmt;
