# Risk Control Gateway Module Design

## Design goals

- Provide a single guardrail path for AI public request formats supported by Aether.
- Support safe rollout through `observe` mode before enabling `pre_block`.
- Avoid persisting full raw request bodies; store bounded excerpts and stable hashes instead.
- Keep Provider moderation optional so local keyword-only deployments still work.
- Protect Provider API Keys from disclosure in admin responses and health views.
- Keep database access behind repository interfaces so gateway logic is not tied to a SQL driver.

## Non-goals

- This module is not a full DLP classifier and does not inspect arbitrary binary uploads.
- It does not attempt to moderate assistant/tool output; it focuses on user-authored input before upstream forwarding.
- It does not replace provider-side safety systems.
- It does not manage long-term analytics; retention cleanup is bounded to risk-control log storage.

## Option analysis

### Option A: call Provider moderation for every request synchronously

Pros: simple mental model and best external classifier coverage.

Cons: adds latency to every request, fails closed/open decisions become operationally risky, and deployments without Provider Keys cannot use the module.

### Option B: local-only keyword/hash guardrail

Pros: fast, deterministic, works without external credentials.

Cons: limited semantic coverage and higher false-positive risk unless exemptions are tuned.

### Option C: layered pipeline (chosen)

The chosen path combines policy scope, sampling, flagged-hash precheck, keyword matching, and optional Provider moderation. It supports keyword-only, Provider-only, and hybrid modes. Observe mode can call Provider moderation in a background worker to avoid user-visible latency while still collecting logs.

## Key decisions and trade-offs

### Request extraction: all user input

All user messages/prompts are inspected for supported signatures, while assistant/tool content is ignored. Provider moderation receives a configurable truncated text view, but keyword matching and stable hashing use the full normalized user text. This avoids missing policy violations hidden earlier in the request context without persisting raw request bodies.

### Frontdoor order

Frontdoor RPM is consumed before pre-block risk-control enforcement. A request blocked by Risk Control therefore still counts against the caller's RPM budget, keeping abuse throttling consistent with other authenticated AI requests.

### Hash before Provider

Known flagged inputs are checked by normalized SHA-256 before keyword/API evaluation. This enables cheap repeat blocking and avoids extra Provider calls. Hashes are only useful for repeated or near-identical normalized text, not semantic variants.

### Bounded logs

Logs store metadata, decision fields, matched keywords, scores, and an excerpt capped by config. Raw request bodies and plaintext Provider Keys are not written to risk-control logs.

Admin log/hash APIs hide stored excerpts by default and return redaction metadata instead. Operators must explicitly request `include_excerpt=true`, and the request must come from a full admin principal with sufficient risk-control permission when using a management token. This keeps day-to-day triage usable without casually exposing prompt fragments.

### Config guardrails

Runtime config is sanitized on the gateway side even when the admin UI already clamps values. Keyword/exemption lists, threshold names/counts, block-message length, auto-action windows, retention days, and scheduled retention intervals are bounded to keep admin-provided config from creating excessive CPU, memory, storage, or response-size pressure.

Regex keyword mode has additional abuse guardrails: regex keyword count, pattern character length, static complexity score, compiled program size, empty-string matches, returned match ranges, per-pattern scan window, and total scan budget are all bounded. Frontend validation mirrors static limits for fast feedback, but the gateway remains the source of truth. If runtime scan budget is hit, the gateway emits `risk_control_regex_scan_budget_limited`; if the active Regex config exceeds the runtime pattern cap or contains invalid stale patterns, the gateway emits additional security warnings and treats the config as a keyword-mode risk instead of silently scanning past the limit. The admin test API returns `regex_scan_limited`, `regex_pattern_limited`, `regex_invalid_pattern_count`, `regex_scan_chars`, `regex_pattern_count`, and `regex_total_scan_budget_chars`.

### Policy scope

The runtime config stores scope in the existing `module.risk_control.config` JSON, so no additional policy tables are required for the first granular rollout. The gateway evaluates model filters plus include/exclude/all lists for user ID, user group ID, API Key ID, route family, route kind, and endpoint signature before hash, keyword, Provider, or logging work begins. User group membership is looked up only when the user-group scope is not `all`. Include/exclude lists are normalized, bounded, and case-insensitive; include mode without values is rejected by the admin API.

### Provider URL hardening

Provider Base URL validation requires HTTPS, disallows credentials/query/fragment, and rejects localhost/private/reserved/link-local DNS targets before making moderation calls. The blocked address set covers common IPv4 special ranges plus IPv6 loopback/ULA/link-local/site-local/multicast/documentation and special-purpose ranges such as NAT64, discard-only, IETF assignments, 6to4, SRv6, 6bone, and IPv4-compatible or IPv4-mapped forms. The moderation client also disables redirects and ambient proxy use, then pins the reqwest connection to the already validated socket addresses with `resolve_to_addrs`, closing the DNS validation versus connection-time TOCTOU gap that could otherwise enable DNS rebinding.

### Provider Key health is in-memory

Provider Key health/freeze state is process-local and derived from test/runtime calls. This avoids persisting secret-adjacent operational metadata and keeps failover simple. A restart clears health counters and freeze state.

### Auto action scope

Auto action counts flagged logs by user within a configured window. User disabling and API Key locking are optional sub-actions because not every deployment wants automated account enforcement.

When pre-block mode applies an automatic user disable or API Key lock, the gateway emits a structured audit event with the risk log ID, target type, target ID, action, applied flag, and violation count. The admin detail drawer exposes matching recovery actions: restore the disabled user or unlock the affected API Key, and those recovery mutations also attach admin audit events.

### Notification and retention

Risk-control notifications use the Important Notification module and are opt-in. Notification templates receive metadata such as trace ID, user label, decision source, matched keywords, and auto action. Input excerpts are hidden by default and must be explicitly enabled.

Admin status reports whether the selected risk-control notification events are dispatch-ready. Missing notification channels or disabled notification items are shown as warnings instead of invalidating the Risk Control config, so enforcement can still run when alert delivery is not ready.

Notification delivery is durable. The request path writes `risk_control_notification_outbox` rows and does not wait for delivery. A single risk log may have multiple notification items, such as `risk_control_auto_action` and `risk_control_user_action_notice`, keyed by `(log_id, item_key)`. A daemon worker claims due rows with a short lease, checks Important Notification readiness, sends the notification through the item/global channel selected by the notification service, and updates both the outbox row and the legacy risk-log notification fields. If the notification center or target channel is not ready, the row is deferred back to `pending` with a later `next_attempt_at` and a visible last error without consuming the send retry budget. Actual delivery failures use bounded backoff; rows that exhaust the retry budget move to `dead` and keep their last error. Admin status exposes pending/processing/sent/dead counts and the newest non-sent error. The log detail drawer requeues all non-sent notification items for that risk log together, then refreshes the complete outbox list so multi-item logs stay consistent.

Retention cleanup runs through the maintenance task runtime when a risk-control writer is available. `auto_run_interval_minutes = 0` disables the scheduled sweep; manual cleanup remains available from the admin module. The worker persists last run time, success/error, delete counts, and the next scheduled run in system config so the admin UI can show retention health after restarts.

### Observe-mode queue

Observe-mode Provider moderation is asynchronous, but it is not spawned once per request. Requests enqueue a bounded background inspection job and continue forwarding. A single worker drains the queue, records queue delay on the resulting risk log, and increments queue drop/failure counters when the queue is full or processing fails. This keeps observe rollout from creating unbounded Tokio task growth under high traffic.

## Data and trust boundaries

- Admin config input is trusted only after validation and secret masking/preservation.
- Public request bodies are untrusted; parsing failures result in no inspection rather than request failure.
- Provider moderation responses are untrusted; parsing errors become Provider errors and are logged as such.
- Provider API Keys are secrets. Admin read APIs return masked values; tests with masked keys preserve stored secrets instead of sending `****` as a key.

## Known limitations

- Regex complexity scoring is deliberately conservative and static; runtime scan windows cap cost further, and keyword-mode requests with Regex policies now fail closed on budget pressure instead of silently skipping the excess.
- Observe-mode Provider checks run asynchronously, so audit records can appear after the proxied request has already continued.
- When observe traffic exceeds the bounded queue capacity, background Provider checks are dropped and counted rather than piling up.
- In-memory Provider Key health is not shared across multiple gateway replicas.
- Current policy scope targets auth and route metadata available at the gateway front door. User-group scope performs an extra membership lookup only when configured, so the default all-scope path stays cheap.

## Verification matrix

| Concern | Coverage |
|---|---|
| Keyword contains/exact/regex matching | `cargo test -p aether-gateway risk_control` |
| Keyword exemptions | `cargo test -p aether-gateway risk_control` |
| Stable sampling | `cargo test -p aether-gateway risk_control` |
| Input extraction for OpenAI/Claude/Gemini/Responses | `cargo test -p aether-gateway risk_control` |
| Policy scope, Provider URL validation, DNS pinning, and key health | `cargo test -p aether-gateway risk_control` |
| Observe queue, notification outbox/attempt state, retention status | `cargo test -p aether-gateway risk_control` |
| RPM ordering before risk-control blocking | `cargo test -p aether-gateway risk_control_blocked_requests_still_consume_frontdoor_rpm_before_next_attempt` |
| Admin UI type safety | `npm run type-check` in `frontend` |
| Production frontend bundle | `npm run build` in `frontend` |

## Change history

- 2026-05-23: Initial gateway Risk Control module with layered local/Provider decision pipeline, admin-configurable runtime policy, and durable log/hash repositories.
- 2026-05-26: Added notification dispatch/retry status, scheduled retention cleanup/status, all-user-input extraction, observe bounded queue, and RPM-before-risk-control ordering.
- 2026-05-27: Reworked risk-control notifications into a durable multi-item outbox with leased worker delivery, readiness deferral, backoff, dead-letter state, admin status metrics, and manual retry.
- 2026-05-27: Added granular policy scope in runtime JSON and hardened Provider SSRF defenses with redirect blocking plus validated DNS address pinning.
