# Risk Control Repository Design

## Design goals

- Keep the risk-control storage contract independent of Postgres/MySQL/SQLite SQL differences.
- Support admin filtering without exposing raw SQL to the gateway.
- Keep input hashes reusable and cheap to query for repeat-blocking.
- Make retention cleanup explicit and reversible at the code-review level.

## Non-goals

- This repository does not implement moderation or keyword policy.
- It does not store plaintext Provider API Keys.
- It does not provide analytics rollups beyond the admin log/hash pages and auto-action counts.

## Option analysis

### Option A: store logs in an existing audit-log table

Pros: fewer tables and existing UI patterns.

Cons: risk-control decisions have domain-specific fields such as decision source, scores, matched keywords, input hash, violation counts, and retention policy. Overloading generic audit logs would make filtering and cleanup brittle.

### Option B: one table for logs only

Pros: simpler schema.

Cons: repeat-blocking would need to scan historical logs by hash or duplicate logic in the gateway. This is too slow and mixes audit history with active block state.

### Option C: separate logs, flagged hashes, and notification outbox (chosen)

`risk_control_logs` is append-oriented audit history. `risk_control_flagged_hashes` is active reusable state keyed by normalized input hash. `risk_control_notification_outbox` stores durable notification jobs per risk log and notification item. This keeps admin review, repeat-blocking, delivery retry, and retention cleanup separate.

## Key decisions and trade-offs

### Driver-specific SQL implementations

Postgres, MySQL, and SQLite each have their own implementation file. This follows `aether-data` repository policy and avoids hiding differences in JSON, boolean, pagination, and upsert syntax behind fragile string builders.

### JSON fields remain at repository boundary

Scores, thresholds, and matched keywords are represented as `serde_json::Value`. This avoids schema churn for Provider-specific category maps while keeping callers typed for core fields.

### Retention separates hit and non-hit logs

Hit logs and non-hit logs can have different retention windows. This lets deployments preserve evidence for blocked/flagged traffic longer while keeping observation noise short-lived.

### Hash upsert updates last-seen metadata

Flagged hash upsert preserves first-seen data and increments `hit_count`, while updating last-seen metadata. This makes repeated attacks visible without duplicating active hash rows.

### Notification outbox uses leases

Outbox rows move through `pending`, `processing`, `sent`, and `dead`. Rows are unique by `(log_id, item_key)` so one risk event can emit separate admin and user-action notification items without overwriting each other. Workers claim due rows with `lease_until`; stale `processing` rows become claimable again after the lease expires. Readiness deferrals return a row to `pending` with a later `next_attempt_at` and preserved `last_error` without incrementing `attempt_count`; actual delivery attempts increment the counter and can dead-letter. The repository also keeps a reset operation so the admin UI can requeue a non-sent row without rebuilding notification payloads from sensitive request bodies.

## Data and trust boundaries

- Repository records are validated before insert/upsert for required identifiers.
- Query filters are bound parameters, not interpolated SQL values.
- Stored excerpts are already bounded by gateway config before insertion.
- `input_hash` is a normalized SHA-256 value, not plaintext prompt content.

## Known limitations

- Full-text search is implemented as portable substring matching across selected fields, not database-native FTS.
- JSON score/filter semantics are intentionally not queryable yet; they are displayed in admin detail views.
- The in-memory implementation is for tests/dev only and does not model SQL transaction isolation.
- Notification outbox payloads intentionally store rendered notification metadata, not full prompt bodies. If excerpt delivery is enabled, the stored excerpt is the already-bounded gateway excerpt.

## Verification matrix

| Concern | Coverage |
|---|---|
| Repository contract compiles across selected backend wiring | `cargo check -p aether-gateway` |
| Risk-control runtime behavior using repository calls | `cargo test -p aether-gateway risk_control` |
| Driver migration presence | SQL files in all three migration directories |
| Admin UI list/detail consumption | `npm run type-check` and `npm run build` in `frontend` |

## Change history

- 2026-05-23: Initial repository for risk-control logs and flagged hashes across memory, Postgres, MySQL, and SQLite.
- 2026-05-23: Added logical schema and baseline generation coverage for risk-control tables.
- 2026-05-27: Added durable risk-control notification outbox storage, leased claiming, readiness deferral, retry/dead-letter mutation, and summary reads.
