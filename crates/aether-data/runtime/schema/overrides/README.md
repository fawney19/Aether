# Schema Overrides

This directory is reserved for rare driver-specific SQL that cannot be
represented by `schema/logical/*.toml` yet.

It is not a normal schema source tree. Leave it with this README only until a
real override exists.

Use overrides for:

- Postgres-only indexes such as GIN/GiST expression indexes
- views
- custom trigger/function SQL
- dialect-specific bootstrap details
- temporary compatibility SQL while a domain is being moved to logical schema

Do not place ordinary table/column/index definitions here when the logical
schema generator can express them.

Do not add `.gitkeep` placeholder directories. When an override is introduced,
create only the directory and SQL file that are actually needed, then add that
file to the relevant source manifest so `compose_schema.sh check` covers it.
