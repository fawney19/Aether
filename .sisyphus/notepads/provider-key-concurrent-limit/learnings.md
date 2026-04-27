## 2026-04-27T06:32:00Z - Task 1: provider_api_keys.concurrent_limit schema support

- Added `crates/aether-data/migrations/20260427000000_add_provider_api_key_concurrent_limit.sql` with `ALTER TABLE public.provider_api_keys ADD COLUMN IF NOT EXISTS concurrent_limit integer;` so existing deployments add a nullable column without defaults/backfill.
- Added nullable `concurrent_limit integer` immediately after `rpm_limit integer` in `provider_api_keys` for both `crates/aether-data/migrations/20260403000000_baseline.sql` and `crates/aether-data/bootstrap/20260413020000_baseline_v2.sql`; left existing `api_keys.concurrent_limit` and `providers.concurrent_limit` untouched.
- Updated migration baseline-v2 bookkeeping in `crates/aether-data/src/migrate.rs` so the fresh baseline stamps migration `20260427000000` as covered by the baseline SQL.
- Diagnostics: `lsp_diagnostics` on SQL files could not run because no `.sql` LSP server is configured; `lsp_diagnostics` on `crates/aether-data/src/migrate.rs` could not initialize because `rust-analyzer` is unavailable in the active Rust toolchain.
- Verification: initial `rtk cargo test -p aether-data migrate` failed with 8 passed / 2 failed because the new migration version was not yet included in baseline-v2 cutoff/test expectations. After the bookkeeping update, `rtk cargo test -p aether-data migrate` passed: 10 passed, 261 filtered out.

## 2026-04-27T00:00:00Z - Task 2: provider_api_keys.concurrent_limit contract and repository mapping

- Added `StoredProviderCatalogKey.concurrent_limit: Option<i32>` next to `rpm_limit`, defaulting to `None`; `with_rate_limit_fields` now carries the nullable provider-key concurrent limit without changing existing RPM/adaptive counters.
- SQL provider-key select/list/summary paths now project `provider_api_keys.concurrent_limit`; insert/update bind the field so future Admin API writes persist it, and `map_key_row` reads it as `Option<i32>`.
- In-memory provider catalog repositories already clone full key records, so focused tests verify `None` defaults and create/update round-trips for `concurrent_limit`.
- Verification: `rtk cargo fmt --check && rtk cargo test -p aether-data-contracts provider_catalog_key && rtk cargo test -p aether-data provider_api_keys_concurrent_limit` passed (2 matching data-contracts tests, 2 matching data tests).
- Diagnostics: `lsp_diagnostics` on modified Rust files could not initialize because the active toolchain lacks `rust-analyzer` (`Unknown binary rust-analyzer in official toolchain 1.95.0-aarch64-apple-darwin`).

## 2026-04-27T00:00:00Z - Task 2 additional downstream verification

- Additional verification after shared builder signature update: `rtk cargo test -p aether-scheduler-core health` passed (14 passed, 27 filtered out) and `rtk cargo test -p aether-gateway trusted_admin_principal` passed (243 passed, 1658 filtered out).

## 2026-04-27T00:00:00Z - Task 3: Admin API provider-key concurrent_limit support

- Added `concurrent_limit: Option<i32>` to admin provider-key create/update payloads beside `rpm_limit`; create validates through `normalize_optional_api_key_concurrent_limit` and stores positive/null values, while update applies the same validator only when the patch field is present so omitted patches preserve the existing value and explicit `null` clears it.
- Added `concurrent_limit` to `build_admin_provider_key_response` immediately after `rpm_limit`, so create/update/list responses expose the stored provider-key value.
- Added focused gateway tests `provider_key_concurrent_limit_create_and_list_responses` and `provider_key_concurrent_limit_update_presence_semantics` covering create number/null storage, create negative rejection, list response serialization, update omitted preserve, positive set, explicit-null clear, and update negative rejection.
- Diagnostics: `lsp_diagnostics` on all modified Rust files failed to initialize because `rust-analyzer` is unavailable in the active toolchain: `Unknown binary 'rust-analyzer' in official toolchain '1.95.0-aarch64-apple-darwin'`.
- Verification: initial `rtk cargo fmt --check` reported formatting-only line wraps in the new test file, fixed with `rtk cargo fmt`; initial `rtk cargo test -p aether-gateway provider_key_concurrent_limit` failed with unresolved imports from `handlers::admin::shared`, fixed by importing the validator from `crate::handlers::shared`.
- Final verification: `rtk cargo test -p aether-gateway provider_key_concurrent_limit` passed (2 passed, 1901 filtered out); `rtk cargo fmt --check` passed with no output; `rtk git diff --check` passed with no output.


## 2026-04-27T07:02:09Z - Task 4: scheduler provider-key concurrent limit enforcement

- Added `count_recent_active_requests_for_provider_key` in `crates/aether-scheduler-core/src/health.rs`; it filters by `StoredRequestCandidate.key_id` and reuses the existing `is_recently_active` semantics (`Pending`/`Streaming`, unfinished, within `ACTIVE_REQUEST_WINDOW_SECS`).
- Inserted the runtime provider-key concurrency check in `candidate_runtime_skip_reason_with_state` after provider-level concurrency and before provider-key circuit/health/RPM checks; saturated positive key limits return exactly `provider_key_concurrency_limit_reached` and absent/`None`/`0` limits remain unlimited.
- Added `provider_key_concurrency_*` scheduler-core tests for unset and zero unlimited behavior, Pending and Streaming active counts, finished/stale ignored candidates, missing key state, exact skip reason, and preserving key circuit/RPM behavior when concurrency is not saturated.
- Diagnostics: `lsp_diagnostics` on modified Rust files could not initialize because the active Rust toolchain lacks `rust-analyzer` (`Unknown binary 'rust-analyzer' in official toolchain '1.95.0-aarch64-apple-darwin'`).
- Verification: `rtk cargo test -p aether-scheduler-core provider_key_concurrency` passed (7 passed, 41 filtered out); `rtk cargo test -p aether-scheduler-core health` passed (15 passed, 33 filtered out); `rtk cargo fmt --check`, `rtk cargo check -p aether-scheduler-core`, `rtk cargo build -p aether-scheduler-core`, and `rtk git diff --check` passed.

## 2026-04-27T00:00:00Z - Task 5: gateway provider-key concurrency fallback validation

- Added gateway candidate selection tests in `apps/aether-gateway/src/scheduler/candidate/tests/selection.rs` for provider-key concurrency fallback: saturated `provider-key-a` falls through to `provider-key-b` in `test-provider-a`, saturated all keys in `test-provider-a` falls through to `test-provider-b`, and saturated keys across all tested providers returns no selected candidate through the existing no-candidate path.
- Provider-key active request fixtures explicitly populate `StoredRequestCandidate.key_id` with the saturated provider-key IDs and use existing active `Pending`/`Streaming` status semantics; provider-key limits are set on `StoredProviderCatalogKey.concurrent_limit` without changing provider-level or user API-key-level concurrency behavior.
- Skip reason collection now has a focused gateway assertion that saturated provider keys emit exactly `provider_key_concurrency_limit_reached` while available sibling keys remain selectable.
- Verification: `rtk cargo test -p aether-gateway provider_key_concurrency` passed (4 passed, 1903 filtered out); `rtk cargo test -p aether-gateway selects_next_key_when_first_provider_key_concurrent_limit_is_reached` passed (1 passed, 1906 filtered out); `rtk cargo test -p aether-gateway selects_next_provider_when_all_provider_keys_concurrent_limit_reached` passed (1 passed, 1906 filtered out); `rtk cargo fmt --check`, `rtk git diff --check`, and `rtk cargo check -p aether-gateway` passed.

## 2026-04-27T00:00:00Z - Task 6: frontend provider-key concurrent_limit fields

- Added `concurrent_limit?: number | null` immediately after `rpm_limit` in frontend provider-key read/update/create types (`EndpointAPIKey`, `EndpointAPIKeyUpdate`, `addProviderKey`, `updateProviderKey`).
- Added compact numeric `并发请求上限` fields immediately after `rpm_limit` in `KeyFormDialog.vue` and `OAuthKeyEditDialog.vue`, preserving the existing `Label`/`Input`/helper-text pattern and using `parseNullableNumberInput(v, { min: 0 })` so `0` is accepted as unlimited.
- Normal provider-key create/update and OAuth key update forms now hydrate `editingKey.concurrent_limit` and submit `form.concurrent_limit`; empty input follows existing nullable semantics (`null` when cleared by input, otherwise omitted when untouched) and never sends an empty string.
- Verification after installing locked frontend dependencies with `rtk npm ci --prefix frontend`: `rtk npm run type-check --prefix frontend` passed, `rtk npm run build --prefix frontend` passed, and the requested focused Vitest command found no matching `concurrent_limit` frontend test files.

## 2026-04-27T00:00:00Z - Task 6 Atlas QA addendum

- Atlas re-read the changed frontend files and verified the field is typed in `keys.ts` / `provider.ts`, rendered in both normal and OAuth dialogs, hydrated from `editingKey.concurrent_limit`, and submitted through create/update payloads without serializing empty values as strings.
- Playwright demo-mode QA mounted the actual Vue dialog components through the Vite dev server: `KeyFormDialog.vue` exposed `并发请求上限` with `type="number"`, `min="0"`, placeholder `不限制`, and the required helper text; `OAuthKeyEditDialog.vue` exposed the same field and hydrated a sample value of `3`.
- Atlas verification: `rtk npm run type-check --prefix frontend`, `rtk npm run build --prefix frontend`, and `rtk git diff --check` passed; focused `rtk npm run test:run --prefix frontend -- concurrent_limit` reported no matching test files, as expected before Task 8.

## 2026-04-27T00:00:00Z - Task 7: backend provider-key concurrent_limit test audit and additions

- Audited existing Task 3-5 backend coverage and found scheduler-core already covered `None`/`0` unlimited, `Pending` and `Streaming` saturation with limit `1`, stale active-window behavior, exact `provider_key_concurrency_limit_reached`, and gateway same-provider/next-provider/all-exhausted fallback using `test-provider-a`, `provider-key-a`, and `provider-key-b`.
- Added missing explicit completed/failed/cancelled coverage for provider-key active counting and skip decisions: `health.rs` now asserts failed/cancelled finished records do not count, and `candidate.rs` now includes failed/cancelled alongside success/stale records in the no-skip case.
- Strengthened data tests by using concrete `test-provider-a` / `provider-key-a` repository fixtures with limit `1`, adding a nullable/no-default schema SQL assertion for the provider-key `concurrent_limit` migration/baselines, and keeping SQL query/bind/row mapping checks under the `provider_api_keys_concurrent_limit` filter.
- Strengthened Admin API concurrent-limit coverage by reading created provider keys back by ID after create/list assertions, confirming positive and `null` values persist while existing negative create/update rejection coverage remains in the focused gateway tests.
- Verification: `rtk cargo test -p aether-scheduler-core provider_key_concurrency`, `rtk cargo test -p aether-gateway provider_key_concurrent_limit`, `rtk cargo test -p aether-gateway provider_key_concurrency`, `rtk cargo test -p aether-data provider_api_keys_concurrent_limit`, `rtk cargo fmt --check`, and `GIT_MASTER=1 rtk git diff --check` passed.

## 2026-04-27T00:00:00Z - Task 7 Admin API read/list addendum

- Added `provider_key_concurrent_limit_reads_existing_list_response` so the focused Admin API filter reads seeded `test-provider-a` keys through the local list endpoint and asserts `provider-key-a` has concurrent limit `1` while `provider-key-b` serializes `null`.
- Re-ran `rtk cargo test -p aether-gateway provider_key_concurrent_limit`; final output is `4 passed, 1904 filtered out`.

## 2026-04-27T00:00:00Z - Task 8: frontend provider-key concurrent_limit tests/type validation

- Added `frontend/src/features/providers/components/__tests__/provider-key-concurrent_limit.spec.ts` so the requested `rtk npm run test:run --prefix frontend -- concurrent_limit` subset now finds a matching frontend test file.
- The tests mount the actual `KeyFormDialog.vue` and `OAuthKeyEditDialog.vue` SFCs through Vue `createApp`/jsdom with lightweight local UI/API stubs; this avoids adding `@vue/test-utils`, which is not present in the project dependencies.
- Coverage verifies normal and OAuth dialog hydration from `editingKey.concurrent_limit`, positive numeric input serializing as numbers (`5` / `7`), cleared input serializing as `null` rather than `""`, `0` remaining numeric/unlimited, and existing `rpm_limit` payload values staying unchanged.
- Verification: `rtk npm run type-check --prefix frontend` passed; `rtk npm run test:run --prefix frontend -- concurrent_limit` passed with 1 file / 5 tests; `GIT_MASTER=1 rtk git diff --check` passed before evidence/notepad recording.

## 2026-04-27T00:00:00Z - Final Verification Wave

- F1 Plan Compliance Audit returned `VERDICT: APPROVE`: Tasks 1-8 are implemented and provider-level/user API-key-level concurrency behavior remains unchanged.
- F2 Code Quality Review returned `VERDICT: APPROVE`: data/SQL mapping, scheduler logic, Admin API validation, frontend normalization, and tests were reviewed without blocking quality issues.
- F3 Real Manual QA returned `VERDICT: APPROVE`: Definition-of-Done commands passed, including broad `rtk cargo test --workspace concurrent_limit` with `16 passed, 2866 filtered out`; evidence files exist under `.sisyphus/evidence/`.
- F4 Scope Fidelity Check returned `VERDICT: APPROVE`: no queues, locks, semaphores, dashboards, dynamic throttling, or scheduler redesign were introduced; fallback semantics match the clarified user behavior.
- Atlas marked F1-F4 checked in `.sisyphus/plans/provider-key-concurrent-limit.md` after the continuation directive required completing remaining plan tasks.
