CREATE TABLE IF NOT EXISTS public.usage_cost_reservations (
    request_id character varying(128) NOT NULL,
    subject_id character varying(128) NOT NULL,
    reservation_token character varying(128) NOT NULL,
    admitted_at timestamp with time zone NOT NULL,
    reserved_cost_units bigint NOT NULL,
    actual_cost_units bigint,
    state character varying(20) NOT NULL,
    reservation_expires_at timestamp with time zone NOT NULL,
    retain_until timestamp with time zone NOT NULL,
    finalized_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT usage_cost_reservations_pkey PRIMARY KEY (reservation_token),
    CONSTRAINT usage_cost_reservations_state_check
        CHECK (state IN ('reserved', 'finalized', 'released')),
    CONSTRAINT usage_cost_reservations_reserved_cost_units_check
        CHECK (reserved_cost_units >= 0),
    CONSTRAINT usage_cost_reservations_actual_cost_units_check
        CHECK (actual_cost_units IS NULL OR actual_cost_units >= 0),
    CONSTRAINT usage_cost_reservations_expiry_check
        CHECK (reservation_expires_at > admitted_at),
    CONSTRAINT usage_cost_reservations_retention_check
        CHECK (retain_until >= reservation_expires_at),
    CONSTRAINT usage_cost_reservations_lifecycle_check CHECK (
        (state = 'reserved' AND actual_cost_units IS NULL AND finalized_at IS NULL)
        OR (state = 'finalized' AND actual_cost_units IS NOT NULL AND finalized_at IS NOT NULL)
        OR (state = 'released' AND actual_cost_units IS NOT NULL
            AND actual_cost_units = 0 AND finalized_at IS NOT NULL)
    )
);

CREATE INDEX IF NOT EXISTS usage_cost_reservations_request_id_idx
    ON public.usage_cost_reservations USING btree (request_id);

CREATE INDEX IF NOT EXISTS usage_cost_reservations_subject_admitted_at_idx
    ON public.usage_cost_reservations USING btree (subject_id, admitted_at);

CREATE INDEX IF NOT EXISTS usage_cost_reservations_reservation_expires_at_idx
    ON public.usage_cost_reservations USING btree (reservation_expires_at);

CREATE INDEX IF NOT EXISTS usage_cost_reservations_retain_until_token_idx
    ON public.usage_cost_reservations USING btree (retain_until, reservation_token);
