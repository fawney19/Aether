# Aether Schema Source

This directory is the schema maintenance workspace. Executable migrations are
owned by the Postgres, MySQL, and SQLite adapters under `../../adapters/`.
The Postgres bootstrap snapshot is compiled from the source fragments here
during `aether-data` builds, so there is no checked-in bootstrap artifact.

The maintenance flow is:

```bash
bash crates/aether-data/runtime/schema/compose_schema.sh generate
bash crates/aether-data/runtime/schema/compose_schema.sh compose
bash crates/aether-data/runtime/schema/compose_schema.sh check
```

- `generate` renders `logical/*.toml` through `aether-data-schema` into
  `generated/{postgres,mysql,sqlite}`. This is a build output, not another SQL
  source to maintain.
- `compose` rewrites the executable SQL from the manifest order.
- `check` verifies generated output is current, confirms the bootstrap source
  fragments still compose cleanly, and diffs each executable migration manifest
  against the checked-in SQL.
- `split` regenerates fragments from the executable SQL and is mostly for
  rebaselining after a deliberate bulk rewrite.

## What To Edit

The schema workspace has three normal source areas:

| Path | Role | Edit policy |
|---|---|---|
| `logical/*.toml` | Long-term logical table model shared by all SQL drivers. | Edit first for portable table-shape changes. |
| `drivers/{postgres,mysql,sqlite}/` | Current maintenance fragments for executable SQL. | Edit only for deployment compatibility, ordering, or generator gaps. |
| `bootstrap/postgres/` | Source fragments for the Postgres empty-database bootstrap snapshot. | Edit here when the bootstrap snapshot changes, then rebuild `aether-data` so `build.rs` regenerates the embedded snapshot. |

Everything else is output:

| Path | Role | Edit policy |
|---|---|---|
| `generated/{postgres,mysql,sqlite}/` | Machine-written SQL emitted from `logical/*.toml` for audit and drift detection. | Do not edit; regenerate with `compose_schema.sh generate`. |
| `../../adapters/{postgres,mysql,sqlite}/migrations/` | Runtime SQL artifacts embedded by each database adapter. | Regenerate through `compose_schema.sh compose`; do not edit independently. |

`generated/**` is deliberately checked in so reviews and CI can see exactly
what the logical schema compiler emits for each driver. It is not a fourth SQL
source of truth, and runtime code never loads migrations from it.

`overrides/` is an exception bucket, not a regular source tree. Keep it empty
except for its README until a real driver-specific SQL file is needed and added
to a manifest.

The generator can also be called directly:

```bash
cargo run -p aether-data-schema --bin aether-schema -- check
cargo run -p aether-data-schema --bin aether-schema -- generate
cargo run -p aether-data-schema --bin aether-schema -- print --driver postgres
```

## Logical Schema

`logical/*.toml` is the long-term source for table definitions. It covers the
clean baseline table set and the portable MySQL/SQLite table-creation
migrations. The generator emits driver-specific SQL under `generated/`; those
files include a directory README plus `Do not edit` headers and should only
change through `compose_schema.sh generate`.

`compose_schema.sh check` enforces two things:

- generated SQL must match the current logical TOML source
- bootstrap source fragments must still compose cleanly for the runtime build
- required executable SQL tables must have logical definitions, so new portable
  tables cannot bypass the single-maintenance-source path

The migration path is incremental:

1. Add a table/domain to `logical/*.toml`.
2. Run `compose_schema.sh generate`.
3. Compare generated SQL to the current driver fragments.
4. Promote generated output into driver fragments only when that domain is
   intentionally ready to stop being handwritten.
5. Keep driver-specific special cases in explicit override fragments under
   `overrides/` only when they cannot live cleanly in a driver fragment.
6. Once a domain matches, move its baseline maintenance to generated output.

The existing `drivers/postgres`, `drivers/mysql`, and `drivers/sqlite`
fragment trees remain authoritative for executable migrations until a generated
fragment is deliberately promoted.

`overrides/` is reserved for rare driver-specific SQL that cannot be represented
by logical schema or the normal driver fragments. Keep it small and explicit.

## Targets

| Target | Executable SQL | Source manifest |
|---|---|---|
| Postgres baseline | `../../adapters/postgres/migrations/20260403000000_baseline.sql` | `drivers/postgres/baseline/manifest.txt` |
| Postgres empty-database snapshot | `aether-data` build output (`OUT_DIR/empty_database_snapshot.sql`) | `bootstrap/postgres/manifest.txt` |
| MySQL baseline | `../../adapters/mysql/migrations/20260403000000_baseline.sql` | `drivers/mysql/baseline/manifest.txt` |
| SQLite baseline | `../../adapters/sqlite/migrations/20260403000000_baseline.sql` | `drivers/sqlite/baseline/manifest.txt` |

Driver baseline source manifests are kept as a small set of numbered SQL
fragments. Postgres uses execution-phase fragments so the pg_dump ordering
remains byte-for-byte stable when composed:

- `001_types_and_tables.sql`
- `002_defaults.sql`
- `003_constraints.sql`
- `004_indexes.sql`
- `005_foreign_keys.sql`
- `006_footer.sql`
- `100_*` extension files for empty-database snapshot-only additions

MySQL and SQLite use similarly numbered domain fragments (`001_identity.sql`
through `006_usage.sql`) because their baselines are shorter and already
organized by domain.

The Rust migration tests compose these manifests too, so fragment drift is
caught during `cargo test -p aether-data split_baseline_sources_match_executable_migrations`.
