DO $migration$
DECLARE
    policy_column record;
BEGIN
    FOR policy_column IN
        SELECT *
        FROM (VALUES
            ('api_keys', 'allowed_providers'),
            ('api_keys', 'allowed_api_formats'),
            ('api_keys', 'allowed_models'),
            ('api_keys', 'ip_rules'),
            ('users', 'allowed_providers'),
            ('users', 'allowed_api_formats'),
            ('users', 'allowed_models'),
            ('user_groups', 'allowed_providers'),
            ('user_groups', 'allowed_api_formats'),
            ('user_groups', 'allowed_models'),
            ('provider_api_keys', 'api_formats'),
            ('provider_api_keys', 'allowed_models')
        ) AS policy_columns(table_name, column_name)
    LOOP
        EXECUTE format(
            $statement$
                UPDATE public.%1$I
                SET %2$I = NULL
                WHERE json_typeof(%2$I::json) = 'null'
                   OR (
                       json_typeof(%2$I::json) = 'string'
                       AND (%2$I::json #>> '{}') ~* '^[[:space:]]*(null)?[[:space:]]*$'
                   )
            $statement$,
            policy_column.table_name,
            policy_column.column_name
        );
    END LOOP;
END;
$migration$;

UPDATE public.management_tokens
SET allowed_ips = NULL
WHERE json_typeof(allowed_ips::json) = 'null';

UPDATE public.management_tokens
SET permissions = NULL
WHERE json_typeof(permissions::json) = 'null';
