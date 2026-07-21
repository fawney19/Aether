SELECT pg_advisory_xact_lock(hashtext($1)::BIGINT)
