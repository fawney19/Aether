UPDATE users
SET allowed_providers_mode = 'unrestricted'
WHERE allowed_providers_mode = 'specific'
  AND (
    allowed_providers IS NULL
    OR TRIM(allowed_providers) = ''
    OR CASE
        WHEN JSON_VALID(allowed_providers) = 1 THEN
            JSON_TYPE(allowed_providers) = 'NULL'
            OR (
                JSON_TYPE(allowed_providers) = 'ARRAY'
                AND JSON_LENGTH(allowed_providers) = 0
            )
        ELSE FALSE
    END
  );

UPDATE users
SET allowed_api_formats_mode = 'unrestricted'
WHERE allowed_api_formats_mode = 'specific'
  AND (
    allowed_api_formats IS NULL
    OR TRIM(allowed_api_formats) = ''
    OR CASE
        WHEN JSON_VALID(allowed_api_formats) = 1 THEN
            JSON_TYPE(allowed_api_formats) = 'NULL'
            OR (
                JSON_TYPE(allowed_api_formats) = 'ARRAY'
                AND JSON_LENGTH(allowed_api_formats) = 0
            )
        ELSE FALSE
    END
  );

UPDATE users
SET allowed_models_mode = 'unrestricted'
WHERE allowed_models_mode = 'specific'
  AND (
    allowed_models IS NULL
    OR TRIM(allowed_models) = ''
    OR CASE
        WHEN JSON_VALID(allowed_models) = 1 THEN
            JSON_TYPE(allowed_models) = 'NULL'
            OR (
                JSON_TYPE(allowed_models) = 'ARRAY'
                AND JSON_LENGTH(allowed_models) = 0
            )
        ELSE FALSE
    END
  );
