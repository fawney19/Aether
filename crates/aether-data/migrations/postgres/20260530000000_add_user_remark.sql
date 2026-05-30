ALTER TABLE public.users
  ADD COLUMN IF NOT EXISTS remark character varying(500);
