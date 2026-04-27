# Provider Key Concurrent Limit

## TL;DR
> **Summary**: Add a hard concurrent in-flight request limit to upstream provider keys (`provider_api_keys.concurrent_limit`) and enforce it during scheduler candidate selection. When one key is saturated, the scheduler must skip only that key, try another key in the same provider, then fall through to the next provider if needed.
> **Deliverables**:
> - Nullable `provider_api_keys.concurrent_limit` persistence
> - Provider-key data contract/repository/API/frontend support
> - Scheduler enforcement and skip reason `provider_key_concurrency_limit_reached`
> - Backend and frontend tests after implementation
> **Effort**: Medium
> **Parallel**: YES - 4 waves
> **Critical Path**: DB/contracts → repository/API types → scheduler enforcement → frontend/API/tests → final verification

## Context

### Original Request
- User asked: “添加一个feat 是关于key级别的 并发请求数量限制”.
- User clarified target behavior: scheduler first chooses a supplier/provider, then provider-internal keys. If one key reaches its simultaneous request upper bound, use another key in the same provider; if no corresponding key is available, move to the next provider in the chain.
- User confirmed Admin API support is required.
- User added frontend configuration page support is required.
- User chose test strategy: implement first, then add tests.

### Interview Summary
- Target key means upstream provider key in `provider_api_keys`, not user-facing gateway API key in `api_keys`.
- `NULL` and `0` must mean unlimited, matching existing concurrency-limit patterns.
- Active request counting must reuse existing semantics: count unfinished `Pending` / `Streaming` request candidates, bounded by the existing active-request window.
- The feature must not introduce semaphores, locks, distributed counters, queues, or scheduler redesign.
- Frontend scope includes both normal provider-key edit/create and OAuth provider-key edit paths.

### Metis Review (gaps addressed)
- Use the exact provider-key identity recorded in `request_candidates.key_id`; verify scheduled candidates reliably record upstream key IDs.
- Do not let an exhausted key exhaust the whole provider; add tests for fallback to another key in the same provider and then next provider.
- Keep migration backward-compatible: nullable column, no default behavior change for existing rows.
- Normalize frontend empty values as `null` or omitted, never `""`.
- Ensure Admin API covers create, update, read/list, and edit hydration paths.
- Add concrete edge-case tests for `NULL`, `0`, positive limit, `Pending`, `Streaming`, stale active records, completed records, and all-exhausted behavior.

## Work Objectives

### Core Objective
Provider-key-level concurrent request limits must be configurable and enforced without changing provider-level, user API-key-level, RPM, quota, health, affinity, retry, or provider ordering behavior.

### Deliverables
- `provider_api_keys.concurrent_limit integer NULL` schema support.
- `StoredProviderCatalogKey.concurrent_limit: Option<i32>` and repository mapping.
- Admin provider-key create/update/read support for `concurrent_limit`.
- Scheduler active-count helper for upstream provider keys.
- Candidate skip reason `provider_key_concurrency_limit_reached`.
- Frontend provider-key forms/types/API payload support.
- Tests and verification evidence.

### Definition of Done (verifiable conditions with commands)
- `cargo test -p aether-scheduler-core provider_key_concurrency` passes.
- `cargo test -p aether-data provider_api_keys_concurrent_limit` passes, or closest matching repository round-trip tests added by implementer pass.
- `cargo test -p aether-gateway provider_key_concurrency` passes.
- `cargo test -p aether-gateway provider_key_concurrent_limit` passes for Admin API create/update/read tests.
- `npm run type-check --prefix frontend` passes.
- `npm run test:run --prefix frontend -- concurrent_limit` passes if frontend tests are added; otherwise `npm run test:run --prefix frontend` passes.
- `cargo test --workspace concurrent_limit` passes as a broad regression command if local resources allow.

### Must Have
- Key saturated → skip that key only.
- Same provider has another available key → select that other key.
- Same provider all keys saturated → try next provider.
- All provider keys across all providers saturated → return no candidate/no selectable candidate using existing no-candidate handling.
- `NULL` and `0` behave as unlimited.
- Positive integer limit behaves as hard in-flight cap.
- Admin API and frontend can configure the limit.

### Must NOT Have
- No scheduler redesign.
- No queueing behavior.
- No distributed semaphore/lock/counter implementation.
- No changes to RPM behavior (`rpm_limit`, `learned_rpm_limit`, `provider_key_rpm_allows_request_since`).
- No changes to existing `providers.concurrent_limit` or `api_keys.concurrent_limit` semantics.
- No required value for existing rows.
- No unrelated frontend UI redesign.
- No commits unless the user explicitly asks for commits.

## Verification Strategy
> ZERO HUMAN INTERVENTION - all verification is agent-executed.
- Test decision: implement first, then add tests, per user choice.
- Backend test frameworks: Rust `cargo test`, existing `tokio::test` and in-repo test fixtures.
- Frontend test/type frameworks: `vue-tsc` via `npm run type-check --prefix frontend`; Vitest via `npm run test:run --prefix frontend`.
- QA policy: Every task has agent-executed scenarios.
- Evidence path: `.sisyphus/evidence/task-{N}-{slug}.txt` or `.json`.

## Execution Strategy

### Parallel Execution Waves
> Target: 5-8 tasks per wave. This plan uses fewer tasks per wave because schema/contracts gate much of the work.

Wave 1: Foundation data model and exact API path discovery
- Task 1: Schema migration/baseline support
- Task 2: Data contracts and repository mapping
- Task 3: Backend Admin API route/payload mapping inspection and update plan execution

Wave 2: Scheduler enforcement
- Task 4: Provider-key active request counting and skip reason
- Task 5: Gateway candidate selection integration and fallback behavior validation

Wave 3: UI + tests
- Task 6: Frontend provider-key configuration fields
- Task 7: Backend tests after implementation
- Task 8: Frontend typecheck/tests after implementation

Wave 4: Final verification
- F1-F4 review agents in parallel.

### Dependency Matrix (full, all tasks)
| Task | Blocks | Blocked By |
|---|---|---|
| 1. Schema migration/baseline | 2, 3, 7 | None |
| 2. Data contracts/repository | 4, 5, 7 | 1 |
| 3. Admin API backend support | 6, 7 | 1, 2 |
| 4. Scheduler core enforcement | 5, 7 | 2 |
| 5. Gateway selection/fallback validation | 7 | 4 |
| 6. Frontend forms/types/API payloads | 8 | 3 |
| 7. Backend tests | Final | 3, 4, 5 |
| 8. Frontend tests/typecheck | Final | 6 |

### Agent Dispatch Summary (wave → task count → categories)
- Wave 1 → 3 tasks → `unspecified-high`, `unspecified-high`, `unspecified-high`
- Wave 2 → 2 tasks → `unspecified-high`, `deep`
- Wave 3 → 3 tasks → `visual-engineering`, `unspecified-high`, `visual-engineering`
- Wave 4 → 4 review tasks → `oracle`, `unspecified-high`, `unspecified-high`, `deep`

## TODOs

- [x] 1. Add `provider_api_keys.concurrent_limit` schema support

  **What to do**: Add a nullable integer `concurrent_limit` column to `provider_api_keys`. Create the incremental migration file exactly as `crates/aether-data/migrations/20260427000000_add_provider_api_key_concurrent_limit.sql` with `ALTER TABLE public.provider_api_keys ADD COLUMN IF NOT EXISTS concurrent_limit integer;`. Also update `crates/aether-data/migrations/20260403000000_baseline.sql` and `crates/aether-data/bootstrap/20260413020000_baseline_v2.sql` so fresh installs contain the column.
  **Must NOT do**: Do not set a non-null default. Do not backfill existing rows. Do not alter `providers.concurrent_limit` or `api_keys.concurrent_limit`.

  **Recommended Agent Profile**:
  - Category: `unspecified-high` - Reason: schema changes must match repository migration conventions.
  - Skills: `[]`
  - Omitted: [`git-master`] - No commit requested.

  **Parallelization**: Can Parallel: YES | Wave 1 | Blocks: [2, 3, 7] | Blocked By: []

  **References** (executor has NO interview context - be exhaustive):
  - Pattern: `crates/aether-data/migrations/20260424000000_add_billing_v3_settlement_snapshot.sql` - latest incremental migration naming style; new file must use `20260427000000_add_provider_api_key_concurrent_limit.sql`.
  - Pattern: `crates/aether-data/migrations/20260403000000_baseline.sql:465-499` - current `provider_api_keys` table; add nullable `concurrent_limit integer` near `rpm_limit`/rate-limit fields.
  - Pattern: `crates/aether-data/bootstrap/20260413020000_baseline_v2.sql:466-476` - same baseline_v2 table shape must stay aligned.
  - Pattern: `crates/aether-data/migrations/20260403000000_baseline.sql:162-175` - `api_keys.concurrent_limit` existing nullable integer.
  - Pattern: `crates/aether-data/migrations/20260403000000_baseline.sql:573-587` - `providers.concurrent_limit` existing nullable integer.

  **Acceptance Criteria** (agent-executable only):
  - [ ] `provider_api_keys` schema contains nullable `concurrent_limit integer`.
  - [ ] Existing rows have no changed behavior because the new value is `NULL` unless explicitly set.
  - [ ] `cargo test -p aether-data migrate` or the closest existing migration tests pass.
  - [ ] `cargo test -p aether-data concurrent_limit` passes if matching schema tests exist or are added.

  **QA Scenarios** (MANDATORY - task incomplete without these):
  ```
  Scenario: Existing provider key remains unlimited after schema change
    Tool: Bash
    Steps: Run `cargo test -p aether-data provider_api_keys_concurrent_limit_null_unlimited`
    Expected: Test exits 0 and verifies an omitted/NULL concurrent_limit reads as None.
    Evidence: .sisyphus/evidence/task-1-schema-null.txt

  Scenario: Positive concurrent_limit column can be persisted by SQL fixture
    Tool: Bash
    Steps: Run `cargo test -p aether-data provider_api_keys_concurrent_limit_round_trip`
    Expected: Test exits 0 and verifies value 1 or 5 persists and reads back.
    Evidence: .sisyphus/evidence/task-1-schema-roundtrip.txt
  ```

  **Commit**: NO | Message: `feat(data): add provider key concurrent limit schema` | Files: [`crates/aether-data/migrations/*`, `crates/aether-data/bootstrap/20260413020000_baseline_v2.sql`]

- [x] 2. Add provider-key `concurrent_limit` to contracts and repository mapping

  **What to do**: Add `concurrent_limit: Option<i32>` to `StoredProviderCatalogKey`, initialize it as `None`, include it in relevant builder/update helpers, and wire SQL/memory repository read/write paths. Include selects, inserts, updates, row mapping, and test repository implementations. Negative values must follow existing provider/user API-key normalization: do not allow negative values through Admin/API validation; repository should not introduce new behavior beyond storing typed values.
  **Must NOT do**: Do not rename `rpm_limit`; do not change `concurrent_429_count`; do not alter provider-key health/rpm learning behavior.

  **Recommended Agent Profile**:
  - Category: `unspecified-high` - Reason: cross-crate data type and repository update.
  - Skills: `[]`
  - Omitted: [`frontend-skill`] - Backend-only task.

  **Parallelization**: Can Parallel: YES | Wave 1 | Blocks: [4, 5, 7] | Blocked By: [1]

  **References**:
  - API/Type: `crates/aether-data-contracts/src/repository/provider_catalog/types.rs:250-294` - `StoredProviderCatalogKey` fields; add `concurrent_limit: Option<i32>` near `rpm_limit`.
  - Pattern: `crates/aether-data-contracts/src/repository/provider_catalog/types.rs:316-367` - constructor defaults; initialize to `None`.
  - Pattern: `crates/aether-data/src/repository/provider_catalog/sql.rs:2279-2329` - `map_key_row`; parse `concurrent_limit` using `row_get::<Option<i32>>`.
  - Pattern: `crates/aether-data/src/repository/auth/sql.rs:35` and `:1410` - existing user API key `concurrent_limit` alias/mapping pattern.
  - Search target: `StoredProviderCatalogKey` references across repo; update all constructors/fixtures.

  **Acceptance Criteria**:
  - [ ] `StoredProviderCatalogKey` exposes `concurrent_limit: Option<i32>`.
  - [ ] SQL provider-key select/list/read paths include `concurrent_limit`.
  - [ ] Provider-key create/update repository paths persist `concurrent_limit`.
  - [ ] In-memory repositories and tests compile with the new field.
  - [ ] `cargo test -p aether-data-contracts provider_catalog_key` passes.
  - [ ] `cargo test -p aether-data provider_api_keys_concurrent_limit` passes.

  **QA Scenarios**:
  ```
  Scenario: Data contract default is unlimited
    Tool: Bash
    Steps: Run `cargo test -p aether-data-contracts provider_catalog_key_concurrent_limit_default`
    Expected: Newly constructed StoredProviderCatalogKey has concurrent_limit == None.
    Evidence: .sisyphus/evidence/task-2-contract-default.txt

  Scenario: Repository round trip preserves positive limit
    Tool: Bash
    Steps: Run `cargo test -p aether-data provider_api_keys_concurrent_limit_round_trip`
    Expected: Create/update/read returns concurrent_limit == Some(5).
    Evidence: .sisyphus/evidence/task-2-repository-roundtrip.txt
  ```

  **Commit**: NO | Message: `feat(data): persist provider key concurrent limit` | Files: [`crates/aether-data-contracts/src/repository/provider_catalog/types.rs`, `crates/aether-data/src/repository/provider_catalog/*.rs`]

- [x] 3. Add backend Admin API create/update/read support for provider-key concurrent limit

  **What to do**: Extend backend provider-key Admin API request/response structs so create, update, list/detail/read responses include `concurrent_limit`. Add the field to `AdminProviderKeyCreateRequest` and `AdminProviderKeyUpdateRequest`, set it in `build_admin_create_provider_key_record`, update it with field-presence semantics in `build_admin_update_provider_key_record`, and add it to `build_admin_provider_key_response`. Use the same validation semantics as existing `normalize_optional_api_key_concurrent_limit`: negative values are rejected; `null`/omitted means unlimited or unchanged according to create/update semantics. OAuth edit uses the same update endpoint, so supporting the shared update request/record path covers it.
  **Must NOT do**: Do not add a new endpoint. Do not require the field. Do not alter authentication/authorization semantics.

  **Recommended Agent Profile**:
  - Category: `unspecified-high` - Reason: route/type/repository coordination.
  - Skills: `[]`
  - Omitted: [`frontend-skill`] - Frontend handled separately.

  **Parallelization**: Can Parallel: YES | Wave 1 | Blocks: [6, 7] | Blocked By: [1, 2]

  **References**:
  - API/Type: `apps/aether-gateway/src/handlers/admin/provider/shared/payloads.rs:6-41` - `AdminProviderKeyCreateRequest`; add `concurrent_limit: Option<i32>` near `rpm_limit` at lines 21-23.
  - API/Type: `apps/aether-gateway/src/handlers/admin/provider/shared/payloads.rs:43-89` - `AdminProviderKeyUpdateRequest` and typed patch; add `concurrent_limit: Option<i32>` near `rpm_limit` at lines 61-63.
  - Pattern: `apps/aether-gateway/src/handlers/admin/provider/write/keys/create.rs:160-166` - create record sets `key.rpm_limit`; set `key.concurrent_limit` immediately adjacent after validating non-negative.
  - Pattern: `apps/aether-gateway/src/handlers/admin/provider/write/keys/update.rs:232-237` - update record uses `fields.contains("rpm_limit")`; add `fields.contains("concurrent_limit")` handling immediately after this block.
  - Response: `apps/aether-gateway/src/handlers/shared/catalog.rs:1300-1305` - provider key response inserts `rpm_limit`; insert `concurrent_limit` immediately after it.
  - Route: `apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/mutations/create.rs:43-64` - create endpoint deserializes request and calls record builder.
  - Route: `apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/mutations/update.rs:43-82` - update endpoint builds typed patch and calls record builder.
  - List response: `apps/aether-gateway/src/handlers/admin/provider/write/keys/payload.rs:43-57` and `apps/aether-gateway/src/handlers/admin/provider/endpoint_keys/reads.rs:128-164` - list provider keys uses `build_admin_provider_key_response`, so response field addition propagates here.
  - Pattern: `apps/aether-gateway/src/handlers/shared/api_keys.rs:74-80` - negative `concurrent_limit` validation for user API keys; mirror or extract shared provider-key helper.
  - Pattern: `apps/aether-gateway/src/handlers/admin/auth/api_keys/mutation_routes.rs:117-193` and `:280-371` - create/update API-key concurrent_limit handling pattern.
  - Frontend API target: `frontend/src/api/endpoints/keys.ts:136-196` calls `/api/admin/endpoints/providers/${providerId}/keys` and `/api/admin/endpoints/keys/${keyId}`; backend routes serving these must accept/return `concurrent_limit`.

  **Acceptance Criteria**:
  - [ ] Create provider key accepts `concurrent_limit: null | number` and rejects negative numbers.
  - [ ] Update provider key accepts `concurrent_limit: null | number` and rejects negative numbers.
  - [ ] Read/list/detail provider-key responses include `concurrent_limit`.
  - [ ] Omitted create field stores `None`; omitted update field preserves existing value.
  - [ ] Explicit `null` clears the limit where update payloads support field presence semantics.
  - [ ] `cargo test -p aether-gateway provider_key_concurrent_limit` passes.

  **QA Scenarios**:
  ```
  Scenario: Admin creates provider key with concurrent_limit
    Tool: Bash
    Steps: Run `cargo test -p aether-gateway admin_provider_key_create_concurrent_limit`
    Expected: Response includes concurrent_limit 5 and repository row stores Some(5).
    Evidence: .sisyphus/evidence/task-3-api-create.txt

  Scenario: Admin rejects negative concurrent_limit
    Tool: Bash
    Steps: Run `cargo test -p aether-gateway admin_provider_key_rejects_negative_concurrent_limit`
    Expected: Test exits 0 and API returns validation failure for -1.
    Evidence: .sisyphus/evidence/task-3-api-negative.txt
  ```

  **Commit**: NO | Message: `feat(gateway): expose provider key concurrent limit in admin api` | Files: [`apps/aether-gateway/src/handlers/**`, `apps/aether-gateway/src/tests/**`]

- [x] 4. Enforce provider-key active-request concurrent limit in scheduler core

  **What to do**: Add a provider-key active request count helper mirroring existing provider/user API key helpers. In candidate runtime skip logic, when the candidate's provider key has `concurrent_limit > 0` and active request count for that same upstream key is `>= limit`, return `Some("provider_key_concurrency_limit_reached")`. Place this after provider-level concurrency checks and before RPM checks so hard in-flight limits short-circuit before RPM/health checks for that key.
  **Must NOT do**: Do not change `provider_key_rpm_allows_request_since`, health score, circuit breaker, or cooldown logic. Do not make key exhaustion skip an entire provider.

  **Recommended Agent Profile**:
  - Category: `unspecified-high` - Reason: core scheduling behavior and fallback semantics.
  - Skills: `[]`
  - Omitted: [`visual-engineering`] - Backend-only task.

  **Parallelization**: Can Parallel: NO | Wave 2 | Blocks: [5, 7] | Blocked By: [2]

  **References**:
  - Pattern: `crates/aether-scheduler-core/src/health.rs:91-113` - existing `count_recent_active_requests_for_provider` and `count_recent_active_requests_for_api_key` helpers.
  - Pattern: `crates/aether-scheduler-core/src/health.rs:457-474` - `is_recently_active`: only unfinished `Pending`/`Streaming`, bounded by `ACTIVE_REQUEST_WINDOW_SECS`.
  - Pattern: `crates/aether-scheduler-core/src/candidate.rs:400-410` - provider concurrent limit skip check.
  - Pattern: `crates/aether-scheduler-core/src/candidate.rs:415-432` - provider-key RPM checks; add concurrency check before this block.
  - Existing exports: `crates/aether-scheduler-core/src/lib.rs:27-30` - export new helper if needed by tests or gateway.

  **Acceptance Criteria**:
  - [ ] New helper counts active requests by `StoredRequestCandidate.key_id` / candidate upstream provider key ID.
  - [ ] `NULL`/missing/0 key limit does not skip.
  - [ ] Positive limit skips only that key when active count `>= limit`.
  - [ ] Skip reason is exactly `provider_key_concurrency_limit_reached`.
  - [ ] `cargo test -p aether-scheduler-core provider_key_concurrency` passes.

  **QA Scenarios**:
  ```
  Scenario: Same provider second key remains selectable
    Tool: Bash
    Steps: Run `cargo test -p aether-scheduler-core provider_key_concurrency_falls_back_to_same_provider_key`
    Expected: Saturated `provider-key-a` is skipped and `provider-key-b` remains selectable.
    Evidence: .sisyphus/evidence/task-4-same-provider-fallback.txt

  Scenario: Completed requests do not count
    Tool: Bash
    Steps: Run `cargo test -p aether-scheduler-core provider_key_concurrency_ignores_finished_candidates`
    Expected: Success/Failed/Cancelled or finished request candidates do not contribute to active count.
    Evidence: .sisyphus/evidence/task-4-finished-ignored.txt
  ```

  **Commit**: NO | Message: `feat(scheduler): enforce provider key concurrent limit` | Files: [`crates/aether-scheduler-core/src/health.rs`, `crates/aether-scheduler-core/src/candidate.rs`, `crates/aether-scheduler-core/src/lib.rs`]

- [x] 5. Validate gateway candidate fallback behavior with provider-key limits

  **What to do**: Ensure gateway candidate selection naturally continues after `provider_key_concurrency_limit_reached` and does not treat that reason as provider-wide exhaustion. Update skip reporting helpers if they enumerate or special-case skip reasons. Verify active request records always include upstream `key_id` for executed candidates.
  **Must NOT do**: Do not alter provider ordering, priority mode, cache affinity, required capabilities, model mapping, or retry semantics.

  **Recommended Agent Profile**:
  - Category: `deep` - Reason: must verify cross-layer scheduler behavior without redesign.
  - Skills: `[]`
  - Omitted: [`frontend-skill`] - Backend selection task.

  **Parallelization**: Can Parallel: NO | Wave 2 | Blocks: [7] | Blocked By: [4]

  **References**:
  - Pattern: `apps/aether-gateway/src/scheduler/candidate/runtime.rs:88-121` - `is_candidate_selectable` delegates to core skip logic.
  - Pattern: `apps/aether-gateway/src/scheduler/candidate/runtime.rs:124-160` - skip reason reporting wrapper.
  - Pattern: `apps/aether-gateway/src/scheduler/candidate/selection.rs:240-254` - skipped candidates are collected per candidate/key.
  - Test pattern: `apps/aether-gateway/src/scheduler/candidate/tests/selection.rs:763-848` - provider limit fallback test; mirror for provider-key limit.
  - Test pattern: `apps/aether-gateway/src/scheduler/candidate/tests/selection.rs:851-934` - API-key concurrency no-candidate test.
  - Data lifecycle: `request_candidates.key_id` in existing fixtures around `apps/aether-gateway/src/scheduler/candidate/tests/selection.rs:803-806` and `:871-874`.

  **Acceptance Criteria**:
  - [ ] Saturated key A and available key B in same provider selects key B.
  - [ ] Saturated all keys in provider A and available provider B selects provider B.
  - [ ] Saturated all keys in all providers returns no selected candidate through existing no-candidate path.
  - [ ] Skip reason collection includes `provider_key_concurrency_limit_reached` for saturated keys.
  - [ ] `cargo test -p aether-gateway provider_key_concurrency` passes.

  **QA Scenarios**:
  ```
  Scenario: Same-provider fallback
    Tool: Bash
    Steps: Run `cargo test -p aether-gateway selects_next_key_when_first_provider_key_concurrent_limit_is_reached`
    Expected: Selected candidate has same provider_id and different key_id from saturated key.
    Evidence: .sisyphus/evidence/task-5-same-provider.txt

  Scenario: Next-provider fallback
    Tool: Bash
    Steps: Run `cargo test -p aether-gateway selects_next_provider_when_all_provider_keys_concurrent_limit_reached`
    Expected: Selected candidate provider_id is provider-b after provider-a keys are saturated.
    Evidence: .sisyphus/evidence/task-5-next-provider.txt
  ```

  **Commit**: NO | Message: `test(gateway): verify provider key concurrency fallback` | Files: [`apps/aether-gateway/src/scheduler/candidate/**`]

- [x] 6. Add frontend provider-key `concurrent_limit` configuration fields

  **What to do**: Update frontend provider-key types, API payload builders, and create/edit forms. Add a numeric optional field next to `rpm_limit` in the normal provider key dialog and OAuth key edit dialog. Hydrate existing values on edit. Serialize empty value as `null` or omit according to existing update semantics; never serialize `""`. Label should clearly distinguish this from RPM: e.g. Chinese label `并发请求上限` with helper text `同一时间允许使用该 Key 的最大请求数，留空或 0 表示不限制`.
  **Must NOT do**: Do not redesign the dialog. Do not add charts/dashboards. Do not change existing `rpm_limit` UI behavior.

  **Recommended Agent Profile**:
  - Category: `visual-engineering` - Reason: frontend form/UI changes with UX clarity.
  - Skills: [`frontend-skill`] - Use restrained form addition consistent with existing UI.
  - Omitted: [`docx`] - Not a document task.

  **Parallelization**: Can Parallel: YES | Wave 3 | Blocks: [8] | Blocked By: [3]

  **References**:
  - Form: `frontend/src/features/providers/components/KeyFormDialog.vue:190-207` - existing `rpm_limit` field; add `concurrent_limit` adjacent to it.
  - Form state: `frontend/src/features/providers/components/KeyFormDialog.vue:518` - add form model field.
  - Edit hydration: `frontend/src/features/providers/components/KeyFormDialog.vue:658` - load existing field into form.
  - Submit payload: `frontend/src/features/providers/components/KeyFormDialog.vue:783-799` and `:813-828` - include normalized field.
  - OAuth form: `frontend/src/features/providers/components/OAuthKeyEditDialog.vue:57-74` - existing `rpm_limit`; add adjacent `concurrent_limit`.
  - OAuth form state: `frontend/src/features/providers/components/OAuthKeyEditDialog.vue:245`; hydration `:297`; submit payload `:358-369`.
  - Types: `frontend/src/api/endpoints/types/provider.ts:226` - `EndpointAPIKey`; add `concurrent_limit?: number | null` near `rpm_limit` at `:243`.
  - Types: `frontend/src/api/endpoints/types/provider.ts:383` - `EndpointAPIKeyUpdate`; add `concurrent_limit?: number | null` near `rpm_limit` at `:392`.
  - API: `frontend/src/api/endpoints/keys.ts:136-158` and `:164-196` - include field in create/update payload types.
  - Mounting: `frontend/src/features/providers/components/ProviderDetailDrawer.vue:983-1013` - dialogs mounted here; no mount changes expected.

  **Acceptance Criteria**:
  - [ ] Normal key create form displays `并发请求上限`.
  - [ ] Normal key edit form hydrates existing `concurrent_limit`.
  - [ ] OAuth key edit form displays and submits `concurrent_limit`.
  - [ ] Frontend types include `concurrent_limit` for read and update payloads.
  - [ ] Empty field serializes as `null` or omitted, not `""`.
  - [ ] `npm run type-check --prefix frontend` passes.

  **QA Scenarios**:
  ```
  Scenario: Frontend typecheck accepts provider key concurrent_limit
    Tool: Bash
    Steps: Run `npm run type-check --prefix frontend`
    Expected: Exit code 0.
    Evidence: .sisyphus/evidence/task-6-frontend-typecheck.txt

  Scenario: Frontend tests verify serialization
    Tool: Bash
    Steps: Run `npm run test:run --prefix frontend -- concurrent_limit`
    Expected: Tests pass and show empty input serializes as null/omitted and positive value serializes as number.
    Evidence: .sisyphus/evidence/task-6-frontend-tests.txt
  ```

  **Commit**: NO | Message: `feat(frontend): configure provider key concurrent limit` | Files: [`frontend/src/api/endpoints/keys.ts`, `frontend/src/api/endpoints/types/provider.ts`, `frontend/src/features/providers/components/KeyFormDialog.vue`, `frontend/src/features/providers/components/OAuthKeyEditDialog.vue`]

- [x] 7. Add backend tests after implementation

  **What to do**: Add or update tests after implementation to cover schema/repository, scheduler-core helper/skip reason, gateway candidate fallback, and Admin API create/update/read validation. Use concrete data names: provider `test-provider-a`, keys `provider-key-a` and `provider-key-b`, limit value `1`, statuses `Pending` and `Streaming`.
  **Must NOT do**: Do not rely on manual verification. Do not skip all-exhausted/no-candidate tests.

  **Recommended Agent Profile**:
  - Category: `unspecified-high` - Reason: broad backend test coverage across crates.
  - Skills: `[]`
  - Omitted: [`frontend-skill`] - Frontend tests handled in Task 8.

  **Parallelization**: Can Parallel: YES | Wave 3 | Blocks: [Final] | Blocked By: [3, 4, 5]

  **References**:
  - Provider fallback test pattern: `apps/aether-gateway/src/scheduler/candidate/tests/selection.rs:763-848`.
  - API-key limit test pattern: `apps/aether-gateway/src/scheduler/candidate/tests/selection.rs:851-934`.
  - Core active count tests: `crates/aether-scheduler-core/src/health.rs:776-780` and nearby tests for active counting.
  - Core candidate skip tests: `crates/aether-scheduler-core/src/candidate.rs:942-1045` and nearby tests.
  - Admin API existing provider/user key tests: search in `apps/aether-gateway/src/tests/control/admin/` for provider key create/update tests.

  **Acceptance Criteria**:
  - [ ] Backend tests cover `NULL` unlimited.
  - [ ] Backend tests cover `0` unlimited.
  - [ ] Backend tests cover `1` + one active `Pending` request skips key.
  - [ ] Backend tests cover `1` + one active `Streaming` request skips key.
  - [ ] Backend tests cover old/stale active outside active window does not skip.
  - [ ] Backend tests cover completed/failed/cancelled not counted.
  - [ ] Backend tests cover same-provider key fallback, next-provider fallback, and all-exhausted no-candidate.
  - [ ] Backend API tests cover create/update/read and negative rejection.

  **QA Scenarios**:
  ```
  Scenario: Scheduler core provider-key concurrency suite
    Tool: Bash
    Steps: Run `cargo test -p aether-scheduler-core provider_key_concurrency`
    Expected: Exit code 0.
    Evidence: .sisyphus/evidence/task-7-scheduler-core.txt

  Scenario: Gateway/Admin API provider-key concurrency suite
    Tool: Bash
    Steps: Run `cargo test -p aether-gateway provider_key_concurrent_limit` and `cargo test -p aether-gateway provider_key_concurrency`
    Expected: Both commands exit 0.
    Evidence: .sisyphus/evidence/task-7-gateway-api.txt
  ```

  **Commit**: NO | Message: `test: cover provider key concurrent limit` | Files: [`crates/aether-scheduler-core/src/*.rs`, `crates/aether-data/src/**`, `apps/aether-gateway/src/**`]

- [x] 8. Add frontend tests/type validation after implementation

  **What to do**: Add frontend tests where existing patterns support it; at minimum run typecheck. Preferred tests should cover normal key form and OAuth key edit form serialization/hydration for `concurrent_limit`. If no suitable component test harness exists for these forms, add focused utility/unit tests for payload normalization and record evidence explaining why component-level test was not added.
  **Must NOT do**: Do not use manual browser verification as the only proof. Do not leave failing lint/typecheck.

  **Recommended Agent Profile**:
  - Category: `visual-engineering` - Reason: frontend verification and form behavior.
  - Skills: [`frontend-skill`]
  - Omitted: [`playwright`] - Only use if project has existing browser test setup; otherwise Vitest/typecheck is enough.

  **Parallelization**: Can Parallel: YES | Wave 3 | Blocks: [Final] | Blocked By: [6]

  **References**:
  - Scripts: `frontend/package.json:11-15` - `test:run` and `type-check` commands.
  - Normal form: `frontend/src/features/providers/components/KeyFormDialog.vue`.
  - OAuth edit form: `frontend/src/features/providers/components/OAuthKeyEditDialog.vue`.
  - Types/API: `frontend/src/api/endpoints/types/provider.ts`, `frontend/src/api/endpoints/keys.ts`.

  **Acceptance Criteria**:
  - [ ] `npm run type-check --prefix frontend` passes.
  - [ ] `npm run test:run --prefix frontend -- concurrent_limit` passes if tests are added with matching names.
  - [ ] Positive numeric input serializes as a number.
  - [ ] Empty input serializes as `null` or omitted.
  - [ ] Existing `rpm_limit` behavior remains unchanged.

  **QA Scenarios**:
  ```
  Scenario: TypeScript contract validation
    Tool: Bash
    Steps: Run `npm run type-check --prefix frontend`
    Expected: Exit code 0 and EndpointAPIKey/EndpointAPIKeyUpdate accept concurrent_limit.
    Evidence: .sisyphus/evidence/task-8-typecheck.txt

  Scenario: Frontend test suite subset
    Tool: Bash
    Steps: Run `npm run test:run --prefix frontend -- concurrent_limit`
    Expected: Exit code 0 if subset exists; otherwise run `npm run test:run --prefix frontend` and record output.
    Evidence: .sisyphus/evidence/task-8-vitest.txt
  ```

  **Commit**: NO | Message: `test(frontend): cover provider key concurrent limit form behavior` | Files: [`frontend/src/**`]

## Final Verification Wave (MANDATORY — after ALL implementation tasks)
> 4 review agents run in PARALLEL. ALL must APPROVE. Present consolidated results to user and get explicit "okay" before completing.
> **Do NOT auto-proceed after verification. Wait for user's explicit approval before marking work complete.**
> **Never mark F1-F4 as checked before getting user's okay.** Rejection or user feedback -> fix -> re-run -> present again -> wait for okay.
- [x] F1. Plan Compliance Audit — oracle
  - Verify every task in this plan is implemented or explicitly deferred with user approval.
  - Confirm no provider-level/API-key-level existing behavior changed.
- [x] F2. Code Quality Review — unspecified-high
  - Review data model, SQL mapping, scheduler logic, validation, frontend normalization, and tests.
  - Check no AI-slop patterns, unrelated refactors, or duplicated logic beyond accepted existing patterns.
- [x] F3. Real Manual QA — unspecified-high (+ browser only if existing setup supports it)
  - Execute the commands listed in Definition of Done.
  - Confirm evidence files exist under `.sisyphus/evidence/`.
- [x] F4. Scope Fidelity Check — deep
  - Confirm the feature does not introduce queues, locks, dashboards, dynamic throttling, or scheduler redesign.
  - Confirm fallback semantics match the user’s clarified behavior exactly.

## Commit Strategy
- Do not commit unless the user explicitly asks.
- If the user asks for commits later, use author/committer identity required by repo instructions:
  - `GIT_AUTHOR_NAME=Kayphoon`
  - `GIT_AUTHOR_EMAIL=109347466+Kayphoon@users.noreply.github.com`
  - `GIT_COMMITTER_NAME=Kayphoon`
  - `GIT_COMMITTER_EMAIL=109347466+Kayphoon@users.noreply.github.com`
- Recommended commit grouping if requested later:
  1. `feat(data): persist provider key concurrent limit`
  2. `feat(scheduler): enforce provider key concurrent limit`
  3. `feat(gateway): expose provider key concurrent limit`
  4. `feat(frontend): configure provider key concurrent limit`
  5. `test: cover provider key concurrent limit`

## Success Criteria
- `provider_api_keys.concurrent_limit` is available and backward-compatible.
- Admin create/update/read supports the field.
- Frontend normal provider-key and OAuth key edit forms support the field.
- Scheduler skips only saturated provider keys and continues to same-provider alternative keys before moving to next provider.
- All relevant backend/frontend tests and typechecks pass.
- Final verification agents approve, and user explicitly approves consolidated verification results.
