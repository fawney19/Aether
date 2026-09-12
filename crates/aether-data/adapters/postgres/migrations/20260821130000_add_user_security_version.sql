ALTER TABLE public.users
    ADD COLUMN security_version BIGINT NOT NULL DEFAULT 0;

ALTER TABLE public.user_sessions
    ADD COLUMN security_version BIGINT NOT NULL DEFAULT 0;
