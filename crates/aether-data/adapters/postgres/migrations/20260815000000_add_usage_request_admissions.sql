CREATE TABLE IF NOT EXISTS public.usage_request_admissions (
    request_id character varying(128) NOT NULL,
    subject_id character varying(128) NOT NULL,
    event_token character varying(128) NOT NULL,
    admitted_at timestamp with time zone NOT NULL,
    retain_until timestamp with time zone NOT NULL,
    state character varying(20) NOT NULL,
    released_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT usage_request_admissions_pkey PRIMARY KEY (event_token),
    CONSTRAINT usage_request_admissions_retention_check
        CHECK (retain_until > admitted_at),
    CONSTRAINT usage_request_admissions_state_check
        CHECK (state IN ('active', 'released')),
    CONSTRAINT usage_request_admissions_lifecycle_check CHECK (
        (state = 'active' AND released_at IS NULL)
        OR (state = 'released' AND released_at IS NOT NULL AND released_at >= admitted_at)
    )
);

CREATE INDEX IF NOT EXISTS usage_request_admissions_subject_admitted_at_idx
    ON public.usage_request_admissions USING btree (subject_id, admitted_at);

CREATE INDEX IF NOT EXISTS usage_request_admissions_retain_until_token_idx
    ON public.usage_request_admissions USING btree (retain_until, event_token);
