DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'providers'
          AND column_name = 'enabled'
    ) THEN
        UPDATE public.providers
        SET
            is_active = enabled,
            updated_at = NOW()
        WHERE enabled IS NOT NULL
          AND is_active IS DISTINCT FROM enabled;
    END IF;

    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'provider_endpoints'
          AND column_name = 'enabled'
    ) THEN
        UPDATE public.provider_endpoints
        SET
            is_active = enabled,
            updated_at = NOW()
        WHERE enabled IS NOT NULL
          AND is_active IS DISTINCT FROM enabled;
    END IF;

    IF EXISTS (
        SELECT 1
        FROM information_schema.columns
        WHERE table_schema = 'public'
          AND table_name = 'models'
          AND column_name = 'enabled'
    ) THEN
        UPDATE public.models
        SET
            is_active = enabled,
            updated_at = NOW()
        WHERE enabled IS NOT NULL
          AND is_active IS DISTINCT FROM enabled;
    END IF;
END $$;
