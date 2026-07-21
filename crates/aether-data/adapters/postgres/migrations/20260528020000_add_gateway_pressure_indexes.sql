-- High-concurrency gateway read/cleanup paths.
-- These indexes keep request audit, candidate cleanup, and background task list
-- queries bounded as append-only tables grow under 6k+ long-lived requests.

CREATE INDEX IF NOT EXISTS idx_usage_created_id_desc
    ON public.usage USING btree (created_at DESC, id ASC);

CREATE INDEX IF NOT EXISTS idx_usage_user_created_id_desc
    ON public.usage USING btree (user_id, created_at DESC, id ASC);

CREATE INDEX IF NOT EXISTS idx_usage_api_format_created_id_desc
    ON public.usage USING btree (api_format, created_at DESC, id ASC);

CREATE INDEX IF NOT EXISTS idx_usage_status_created_id_desc
    ON public.usage USING btree (status, created_at DESC, id ASC);

CREATE INDEX IF NOT EXISTS idx_usage_monitoring_errors_created_id_desc
    ON public.usage USING btree (created_at DESC, id ASC)
    WHERE (
        lower(BTRIM(COALESCE(status, ''))) IN ('failed', 'error')
        OR (error_category IS NOT NULL AND BTRIM(error_category) <> '')
        OR (
            BTRIM(COALESCE(status, '')) = ''
            AND (
                COALESCE(status_code, 0) >= 400
                OR (error_message IS NOT NULL AND BTRIM(error_message) <> '')
            )
        )
    );

CREATE INDEX IF NOT EXISTS idx_request_candidates_endpoint_status_created
    ON public.request_candidates USING btree (endpoint_id, status, created_at DESC, id ASC);

CREATE INDEX IF NOT EXISTS idx_request_candidates_provider_created
    ON public.request_candidates USING btree (provider_id, created_at DESC, id ASC);

CREATE INDEX IF NOT EXISTS idx_request_candidates_api_key_created
    ON public.request_candidates USING btree (api_key_id, created_at ASC, id ASC);

CREATE INDEX IF NOT EXISTS idx_background_task_runs_status_created
    ON public.background_task_runs USING btree (status, created_at_unix_secs DESC, updated_at_unix_secs DESC);

CREATE INDEX IF NOT EXISTS idx_background_task_runs_kind_created
    ON public.background_task_runs USING btree (kind, created_at_unix_secs DESC, updated_at_unix_secs DESC);
