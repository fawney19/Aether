# Risk Control Gateway Module

## What this module does

`risk_control` is the gateway-side guardrail for AI public requests. It extracts user-provided text from supported request formats, evaluates local keyword/hash rules and optional Provider moderation, writes audit logs, and returns a block decision when the module is running in pre-block mode.

The module is intentionally runtime-local to `aether-gateway`: it owns request inspection and decision composition, while durable logs and flagged hashes live behind `aether-data` repository interfaces.

## Why it exists

Aether proxies multiple AI request formats. Risk policy must run before forwarding sensitive prompts upstream, but the gateway also needs an observe mode for safe rollout. This module provides one consistent decision path for OpenAI Chat/Responses/Image, Claude Messages, Gemini Generate Content, and compatible JSON endpoints.

## Core responsibilities

- Decide whether an incoming public AI request should be inspected.
- Sanitize runtime policy bounds before applying admin-provided config.
- Extract and normalize user-authored input without storing full raw bodies.
- Apply model/user/user-group/API Key/route/endpoint scope, stable sampling, flagged-hash precheck, keyword matching, and optional Provider moderation.
- Write bounded audit records and reusable flagged hashes.
- Enforce pre-block responses and optional auto actions such as disabling a user or locking an API Key.
- Hide hit excerpts in admin list APIs by default; only a full admin request with `include_excerpt=true` can retrieve them.
- Emit explicit audit events for automatic user disabling/API Key locking and expose admin recovery actions for restoring users or unlocking Keys.
- Bound regex keyword mode by pattern count, pattern length, static complexity, compiled program size, match range count, and runtime scan budget; stale/oversized Regex configs fail closed in keyword modes instead of silently bypassing rules.
- Limit policy rollout by model, user ID, user group ID, API Key ID, route family, route kind, and endpoint signature without changing database schema.
- Harden Provider moderation calls by validating public HTTPS URLs, rejecting unsafe DNS targets including special-purpose IPv4/IPv6 ranges, disabling redirects/proxies, and pinning reqwest connections to the already validated socket addresses.
- Queue opt-in important notifications through a durable multi-item outbox with readiness deferral, worker retry, dead-letter state, and manual requeue from log detail.
- Emit `risk_control_user_action_notice` as a separately toggleable notification-service item when automatic user action notices are enabled.
- Run scheduled retention cleanup and expose persisted cleanup status for operators.
- Queue observe-mode Provider moderation through a bounded worker instead of spawning unbounded tasks per request.
- Surface notification readiness in admin status without making notification setup block core enforcement.
- Expose Provider Key health without returning plaintext keys.

## Dependencies

| Direction | Dependency | Purpose |
|---|---|---|
| Upstream | `GatewayPublicRequestContext` | Route family, auth context, endpoint signature, trace metadata. |
| Upstream | `headers::decoded_request_body_bytes` | Handles decoded request bodies before JSON extraction. |
| Upstream | Per-call `reqwest::Client` | Calls Provider moderation APIs after DNS target validation and address pinning. |
| Downstream | `aether-data::repository::risk_control` | Persists logs, flagged input hashes, and notification outbox rows. |
| Downstream | Admin risk-control handler | Reads/updates runtime config and exposes testing/status APIs. |

## Quick usage path

1. Admin enables `module.risk_control.enabled` and saves `module.risk_control.config` through the 风控中心 page.
2. The proxy path buffers eligible bodies only when Risk Control is enabled and not off.
3. The module returns `None` for pass/observe, or `Some(RiskControlBlockDecision)` for pre-blocked traffic.
4. Audit logs and flagged hashes can be queried from the admin Risk Control page.

## Local verification

```bash
cargo test -p aether-gateway risk_control -- --nocapture
cargo check -p aether-gateway
```

Frontend/API verification for the admin page lives in `frontend/src/views/admin/modules/RiskControl.vue` and `frontend/src/api/risk-control.ts`.

The admin test API returns Regex budget metadata when Regex mode is active: `regex_scan_limited`, `regex_pattern_limited`, `regex_invalid_pattern_count`, `regex_scan_chars`, `regex_pattern_count`, and `regex_total_scan_budget_chars`.
