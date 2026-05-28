# Risk Control Repository

## What this module does

`crates/aether-data/src/repository/risk_control` stores and queries the durable data used by the gateway 风控中心 module:

- risk-control audit logs
- reusable flagged input hashes
- durable per-item notification outbox rows
- retention cleanup summaries

It provides one repository contract with in-memory, Postgres, MySQL, and SQLite implementations.

## Why it exists

The gateway needs to audit decisions and reuse known flagged inputs without coupling request-path logic to a specific SQL driver. This repository isolates driver-specific SQL while keeping the runtime contract stable for `aether-gateway`.

## Core responsibilities

- Insert validated risk-control log records.
- List logs with filters for admin UI review.
- Count flagged logs in a time window for auto action decisions.
- Upsert/list/delete/clear flagged input hashes.
- Insert/claim/defer/update/reset notification outbox rows for readiness delay and delivery retry; retry reset returns the full per-log outbox set so callers do not collapse multi-item logs into one row.
- Delete expired hit and non-hit logs according to retention settings.

## Dependencies

| Direction | Dependency | Purpose |
|---|---|---|
| Upstream | `aether-gateway::risk_control` | Produces log and hash records. |
| Upstream | Admin risk-control handler | Reads log/hash pages and runs retention cleanup. |
| Downstream | `driver/{postgres,mysql,sqlite}` | Executes driver-specific SQL. |
| Downstream | `DataLayerError` | Normalizes repository errors for callers. |

## Quick usage path

```rust
let page = data
    .list_risk_control_logs(&RiskControlLogListQuery {
        limit: 20,
        ..Default::default()
    })
    .await?;
```

Runtime code should use the backend-level read/write methods instead of constructing repository implementations directly.

## Local verification

```bash
cargo test -p aether-gateway risk_control -- --nocapture
cargo check -p aether-gateway
```

Migration compatibility is covered by the driver-specific SQL migration files under `crates/aether-data/migrations/{postgres,mysql,sqlite}`.
