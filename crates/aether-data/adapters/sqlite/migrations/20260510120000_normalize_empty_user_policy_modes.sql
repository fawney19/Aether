UPDATE users
SET allowed_providers_mode = 'unrestricted'
WHERE allowed_providers_mode = 'specific'
  AND (
    allowed_providers IS NULL
    OR trim(allowed_providers) = ''
    OR lower(trim(allowed_providers)) = 'null'
    OR trim(allowed_providers) = '[]'
  );

UPDATE users
SET allowed_api_formats_mode = 'unrestricted'
WHERE allowed_api_formats_mode = 'specific'
  AND (
    allowed_api_formats IS NULL
    OR trim(allowed_api_formats) = ''
    OR lower(trim(allowed_api_formats)) = 'null'
    OR trim(allowed_api_formats) = '[]'
  );

UPDATE users
SET allowed_models_mode = 'unrestricted'
WHERE allowed_models_mode = 'specific'
  AND (
    allowed_models IS NULL
    OR trim(allowed_models) = ''
    OR lower(trim(allowed_models)) = 'null'
    OR trim(allowed_models) = '[]'
  );
