-- A feature branch shipped the two ledger migrations before their user
-- ownership relationship was enforced. Preserve those migration checksums and
-- add the cascade in a follow-up that also upgrades databases which ran that
-- branch. Rows whose user was already deleted cannot be retained safely.
DELETE FROM public.usage_cost_reservations AS reservation
WHERE NOT EXISTS (
    SELECT 1
    FROM public.users AS app_user
    WHERE app_user.id = reservation.subject_id
);

DELETE FROM public.usage_request_admissions AS admission
WHERE NOT EXISTS (
    SELECT 1
    FROM public.users AS app_user
    WHERE app_user.id = admission.subject_id
);

ALTER TABLE public.usage_cost_reservations
    ADD CONSTRAINT usage_cost_reservations_subject_id_fkey
    FOREIGN KEY (subject_id) REFERENCES public.users(id) ON DELETE CASCADE;

ALTER TABLE public.usage_request_admissions
    ADD CONSTRAINT usage_request_admissions_subject_id_fkey
    FOREIGN KEY (subject_id) REFERENCES public.users(id) ON DELETE CASCADE;
