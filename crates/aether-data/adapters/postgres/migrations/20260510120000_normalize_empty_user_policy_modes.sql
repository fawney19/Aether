UPDATE public.users
SET allowed_providers_mode = 'unrestricted'
WHERE allowed_providers_mode = 'specific'
  AND (
    allowed_providers IS NULL
    OR json_typeof(allowed_providers) = 'null'
    OR CASE
      WHEN json_typeof(allowed_providers) = 'array' THEN
        json_array_length(allowed_providers) = 0
      ELSE FALSE
    END
  );

UPDATE public.users
SET allowed_api_formats_mode = 'unrestricted'
WHERE allowed_api_formats_mode = 'specific'
  AND (
    allowed_api_formats IS NULL
    OR json_typeof(allowed_api_formats) = 'null'
    OR CASE
      WHEN json_typeof(allowed_api_formats) = 'array' THEN
        json_array_length(allowed_api_formats) = 0
      ELSE FALSE
    END
  );

UPDATE public.users
SET allowed_models_mode = 'unrestricted'
WHERE allowed_models_mode = 'specific'
  AND (
    allowed_models IS NULL
    OR json_typeof(allowed_models) = 'null'
    OR CASE
      WHEN json_typeof(allowed_models) = 'array' THEN
        json_array_length(allowed_models) = 0
      ELSE FALSE
    END
  );
