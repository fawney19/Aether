ALTER TABLE public.usage_http_audits
    ADD COLUMN IF NOT EXISTS request_body_state character varying(32),
    ADD COLUMN IF NOT EXISTS provider_request_body_state character varying(32),
    ADD COLUMN IF NOT EXISTS response_body_state character varying(32),
    ADD COLUMN IF NOT EXISTS client_response_body_state character varying(32);
