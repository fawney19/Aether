-- PostgreSQL 15/16 cannot enter the function's EXCEPTION subtransactions in a
-- parallel operation. Keep its validation and immutable amount semantics intact.
ALTER FUNCTION public.usage_customer_billable_amount(jsonb, numeric, numeric) PARALLEL UNSAFE;
