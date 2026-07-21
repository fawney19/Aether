-- Restore a normal lookup path before sqlx records this migration in the
-- same transaction. sqlx inserts into `_sqlx_migrations` unqualified.
SELECT pg_catalog.set_config('search_path', 'public', true);



--
-- PostgreSQL database dump complete
--
