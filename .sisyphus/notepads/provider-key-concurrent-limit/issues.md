## 2026-04-27T00:00:00Z - Task 5 diagnostics

- `lsp_diagnostics` on `apps/aether-gateway/src/scheduler/candidate/tests/selection.rs` failed before Rust diagnostics could run because the active toolchain cannot initialize rust-analyzer: `Error: LSP request timeout (method: initialize)` with stderr `error: Unknown binary 'rust-analyzer' in official toolchain '1.95.0-aarch64-apple-darwin'.`

## 2026-04-27T00:00:00Z - Task 6 frontend diagnostics and test availability

- `lsp_diagnostics` on `frontend/src/features/providers/components/KeyFormDialog.vue` and `frontend/src/features/providers/components/OAuthKeyEditDialog.vue` failed because the configured Vue LSP is not installed: `Command not found: vue-language-server`.
- `lsp_diagnostics` on `frontend/src/api/endpoints/types/provider.ts` and `frontend/src/api/endpoints/keys.ts` failed because the configured TypeScript LSP is not installed: `Command not found: typescript-language-server`.
- Initial frontend verification scripts failed before dependencies were installed with `sh: vue-tsc: command not found` and `sh: vitest: command not found`; running `rtk npm ci --prefix frontend` installed the existing lockfile dependencies and allowed verification to proceed.
- `rtk npm run test:run --prefix frontend -- concurrent_limit` reported `No test files found`, so Task 6 has no matching focused frontend tests to run; Task 8 remains the place to add/verify frontend tests.

## 2026-04-27T00:00:00Z - Task 7 backend diagnostics

- `lsp_diagnostics` on `crates/aether-scheduler-core/src/candidate.rs`, `crates/aether-scheduler-core/src/health.rs`, `crates/aether-data/src/repository/provider_catalog/memory.rs`, `crates/aether-data/src/repository/provider_catalog/sql.rs`, and `apps/aether-gateway/src/tests/control/admin/endpoints/keys.rs` failed before Rust diagnostics could run because rust-analyzer is unavailable: `Error: LSP request timeout (method: initialize)` with stderr `error: Unknown binary 'rust-analyzer' in official toolchain '1.95.0-aarch64-apple-darwin'.`
- Initial `rtk cargo fmt --check` reported a formatting-only wrap in the new `sql.rs` schema assertion; `rtk cargo fmt` fixed it and final `rtk cargo fmt --check` passed.
- Initial `rtk cargo test -p aether-data provider_api_keys_concurrent_limit` surfaced incorrect `include_str!` paths and then a baseline-table assertion mismatch; the test was corrected to use `../../../...` fixture paths and `CREATE TABLE IF NOT EXISTS public.provider_api_keys`, after which the data filter passed.

## 2026-04-27T00:00:00Z - Task 8 frontend diagnostics

- `lsp_diagnostics` on `frontend/src/features/providers/components/__tests__/provider-key-concurrent_limit.spec.ts` failed because the configured TypeScript LSP is not installed: `LSP server 'typescript' is configured but NOT INSTALLED. Command not found: typescript-language-server`.
