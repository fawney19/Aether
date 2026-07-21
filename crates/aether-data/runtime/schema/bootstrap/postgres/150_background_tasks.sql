CREATE TABLE IF NOT EXISTS public.background_task_runs (
    id character varying(64) PRIMARY KEY,
    task_key character varying(200) NOT NULL,
    kind character varying(32) NOT NULL,
    "trigger" character varying(64) NOT NULL,
    status character varying(32) NOT NULL,
    attempt integer NOT NULL DEFAULT 0,
    max_attempts integer NOT NULL DEFAULT 0,
    owner_instance character varying(200),
    progress_percent integer NOT NULL DEFAULT 0,
    progress_message text,
    payload_json jsonb,
    result_json jsonb,
    error_message text,
    cancel_requested boolean NOT NULL DEFAULT false,
    created_by character varying(200),
    created_at_unix_secs bigint NOT NULL,
    started_at_unix_secs bigint,
    finished_at_unix_secs bigint,
    updated_at_unix_secs bigint NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_background_task_runs_task_key
    ON public.background_task_runs (task_key);
CREATE INDEX IF NOT EXISTS idx_background_task_runs_status
    ON public.background_task_runs (status);
CREATE INDEX IF NOT EXISTS idx_background_task_runs_kind
    ON public.background_task_runs (kind);
CREATE INDEX IF NOT EXISTS idx_background_task_runs_created_at
    ON public.background_task_runs (created_at_unix_secs DESC);
CREATE INDEX IF NOT EXISTS idx_background_task_runs_status_created
    ON public.background_task_runs (status, created_at_unix_secs DESC, updated_at_unix_secs DESC);
CREATE INDEX IF NOT EXISTS idx_background_task_runs_kind_created
    ON public.background_task_runs (kind, created_at_unix_secs DESC, updated_at_unix_secs DESC);

CREATE TABLE IF NOT EXISTS public.background_task_events (
    id character varying(64) PRIMARY KEY,
    run_id character varying(64) NOT NULL REFERENCES public.background_task_runs(id) ON DELETE CASCADE,
    event_type character varying(64) NOT NULL,
    message text NOT NULL,
    payload_json jsonb,
    created_at_unix_secs bigint NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_background_task_events_run_id
    ON public.background_task_events (run_id, created_at_unix_secs ASC);
