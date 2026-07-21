# aether-data

`aether-data` is the runtime data-access crate. It owns concrete SQL/database
drivers, concrete repository implementations, migration/backfill/export
workflows, and the composition layer that wires those pieces into the rest of
the application.

It does not own the cross-crate DTO contracts. Shared repository records and
errors that are consumed by scheduler, billing, admin, usage runtime, and video
task crates live in the sibling `../contracts` crate (`aether-data-contracts`).

## Directory Map

| Path | Responsibility |
|---|---|
| `src/database.rs` | Logical SQL driver selection and shared pool configuration. |
| `src/config.rs` | Data-layer config for SQL drivers and repository wiring. |
| `src/maintenance.rs` | Maintenance DTOs and aggregation summaries used by backend dispatch and runtime maintenance entrypoints. |
| `src/driver/{postgres,mysql,sqlite}.rs` | Thin compatibility facades for the selected adapter crates. Low-level pools, transactions, and leases live in `aether-data-postgres`, `aether-data-mysql`, and `aether-data-sqlite`. |
| `src/repository` | Compatibility modules that re-export contracts and selected SQL adapters, plus concrete in-memory implementations. Driver-specific request-path SQL lives in the adapter crates. |
| `src/backend` | Composition root. Builds concrete driver backends and exposes app-facing read/write/worker/lock/lease handles. |
| `src/backend/{maintenance,stats,wallet,system}.rs` | Backend-owned maintenance, aggregation, wallet ledger, and system config workflows that are not normal request-path repositories. |
| `src/lifecycle/migrate.rs` and `src/lifecycle/migrate/*` | Compatibility entry points and Postgres snapshot bootstrap adapter. |
| `src/lifecycle/backfill.rs` | Backfill entry points and backfill discovery. |
| `src/lifecycle/export.rs` | Cross-database export/import workflows. |
| `../adapters/{postgres,mysql,sqlite}/migrations` | Executable `sqlx` migrations owned and embedded by each adapter crate. |
| `schema` | Schema maintenance workspace for logical definitions, driver fragments, and generated output. |
| `schema/logical` | Human-maintained logical table definitions used by `aether-data-schema`. |
| `schema/drivers/{postgres,mysql,sqlite}` | Human-maintained driver fragments that compose back into executable SQL while generation is being promoted. |
| `schema/bootstrap/postgres` | Human-maintained source fragments for the Postgres bootstrap snapshot. `build.rs` composes them into the runtime embedded artifact during crate builds. |
| `schema/generated` | Machine-written SQL generated from logical schema for audit and drift detection only. |
| `schema/overrides` | Rare driver-specific SQL escape hatch. Keep README-only until a real override is needed. |
| `backfills/{postgres,mysql,sqlite}` | Executable backfill SQL grouped by driver. |

## Layering Rules

The crate is easiest to read as five layers:

1. Contracts: DTOs, input structs, repository traits, and `DataLayerError`.
   Prefer `aether-data-contracts` for anything that another crate needs to
   compile against.
2. Driver primitives: `aether-data-postgres`, `aether-data-mysql`, and
   `aether-data-sqlite` connect to infrastructure and expose pools, runners,
   and executable migrations;
   the matching `src/driver/*.rs` files only preserve the existing import paths.
3. Repository implementations: `aether-data-{postgres,mysql,sqlite}` translate
   contract types to driver-specific SQL. `src/repository/<domain>` keeps the
   compatibility import path and owns the in-memory implementation where one
   exists.
4. Backend composition: `backend` chooses one SQL driver from config and wires
   repository implementations into app-facing handles. Backend-owned runtime
   maintenance workflows live in focused backend modules rather than in the
   driver pool files.
5. Maintenance workflows: `lifecycle/migrate`, `lifecycle/backfill`,
   `lifecycle/export`, and `schema` manage database lifecycle outside normal
   request handling.

Do not add domain queries to low-level pool modules. Do not add driver selection
logic inside individual repository implementations. Keep cross-crate contracts
out of `aether-data` unless they are implementation-only.

## Repository Layout

Most domain repositories use this shape:

```text
src/repository/<domain>/
  mod.rs      # exports trait/type names and concrete implementations
  types.rs    # implementation-local DTOs when they are not already in contracts
  postgres.rs # Postgres implementation
  mysql.rs    # MySQL implementation
  sqlite.rs   # SQLite implementation
  memory.rs   # tests/dev in-memory implementation
```

Use explicit driver filenames for repository implementations. Do not introduce
new generic `sql.rs` modules for driver-specific code.

## SQL Driver Policy

The project supports three SQL drivers at the repository/backend boundary:
Postgres, MySQL, and SQLite. That does not mean every raw SQL file is shared.
The portable contract is the Rust shape and behavior; the physical SQL stays
driver-specific where syntax, indexes, JSON support, timestamps, locking, or
upsert semantics differ.

Use logical types in design docs and reviews:

| Logical type | Postgres | MySQL | SQLite |
|---|---|---|---|
| `json` | `json` or `jsonb` | `json` or text JSON | text JSON |
| `bool` | `boolean` | `boolean` / `tinyint(1)` | integer |
| `time_unix` | `bigint` or legacy timestamp | `bigint` | integer |
| `money_decimal` | `numeric` / legacy double | `double` | real |

`jsonb` is acceptable only in Postgres SQL. MySQL and SQLite migrations must not
contain `jsonb`; this is guarded by migration tests. Prefer `serde_json::Value`
or typed Rust structs at the repository boundary so callers do not depend on the
physical storage type.

### Feature Matrix

`aether-data` enables only PostgreSQL by default so local checks do not compile
all SQLx drivers:

```bash
cargo check -p aether-data
cargo check -p aether-data --no-default-features --features mysql
cargo check -p aether-data --no-default-features --features sqlite
cargo check -p aether-data --no-default-features --features all-drivers
```

Services selecting MySQL or SQLite in `Cargo.toml` must also disable the default
Postgres feature:

```toml
aether-data = { workspace = true, default-features = false, features = ["mysql"] }
```

The gateway explicitly enables `all-drivers` for deployment compatibility. New
services should select only the driver they deploy with. A configured driver
that is not enabled in the build returns an explicit configuration error rather
than silently constructing an empty backend.

## Schema Maintenance

Executable migrations live under each adapter crate's `migrations/` directory.
Moving their physical files does not change SQLx migration versions or deployed
database history because SQLx records migration version and checksum, not the
workspace source path.

The large baseline SQL files are maintained through `schema` fragments. New
table-structure work should start in `schema/logical/*.toml` and be generated
into driver-specific SQL before it is composed into executable migrations:

```bash
bash crates/aether-data/runtime/schema/compose_schema.sh generate
bash crates/aether-data/runtime/schema/compose_schema.sh compose
bash crates/aether-data/runtime/schema/compose_schema.sh check
```

`schema/generated/**` is machine-written by `aether-data-schema`; it is checked
in to make generator drift reviewable, not because runtime reads it. Edit
`schema/logical/*.toml` instead. Use `schema/overrides/**` only as an exception
bucket for driver-specific SQL that cannot live cleanly in logical schema or the
normal driver fragments.

`compose_schema.sh check` also verifies that required baseline/portable
table-creation SQL is represented in `schema/logical`. This is the guardrail
that keeps table structure from drifting back into three manually maintained
definitions.

For executable fragments that have not been promoted to generated output yet,
edit fragments under `schema/drivers/{postgres,mysql,sqlite}` directly, run
`compose`, then run `check`. Do not edit baseline executable SQL and fragments
independently.

When adding a table:

1. Add or update `schema/logical/*.toml` first for the table structure.
2. Run `schema/compose_schema.sh generate` and inspect the generated driver SQL.
3. Add or update the executable driver-specific migration/fragments only for
   deployment compatibility or generator gaps.
4. Add or update repository contracts in `aether-data-contracts` if other crates
   need the new shape.
5. Add driver repository implementations only for the drivers that are actually
   supported for that domain.
6. Wire new repositories through `src/backend/read.rs` or `src/backend/write.rs`
   only after the implementation exists for each selected driver.
7. Update `docs/architecture/data-layer.md` when ownership, import, or logical
   type policy changes.

## Known Cleanup Targets

These are intentionally staged to keep the multi-database refactor reviewable:

1. Retire the compatibility re-export paths once downstream crates use
   `aether-data-contracts` for contracts and `aether-data` only as the runtime
   composition facade.
2. Group repository domains once file-level names are stable. Likely groups:
   identity, auth config, provider catalog, runtime tasks, wallet/billing,
   usage, stats, and proxy nodes.
3. Continue shrinking Postgres stats SQL modules where useful by moving shared
   SQL fragments and row-mapping helpers behind focused `backend/stats/*`
   modules.
4. Split very large `usage`, `wallet`, and stats implementations into focused
   query/mapping/read/write/test modules before considering any additional
   crates. New crate boundaries should follow stable ownership, not file size
   alone.
