-- Match upgraded PostgreSQL installations to the portable Gemini Files
-- metadata contract already used by the logical schema and MySQL.
ALTER TABLE public.gemini_file_mappings
    ALTER COLUMN file_name TYPE character varying(512),
    ALTER COLUMN display_name TYPE character varying(512),
    ALTER COLUMN mime_type TYPE character varying(255);
