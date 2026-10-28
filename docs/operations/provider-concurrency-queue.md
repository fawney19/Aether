# Provider concurrency queue

When a provider or upstream credential reaches its configured `concurrent_limit`, Aether can wait for capacity instead of immediately failing the request. Requests sharing a credential also share its FIFO queue across models. Limits that are unset or zero remain unlimited.

The scheduler tries selectable candidates before concurrency-blocked candidates. A credential pool scans idle keys before replaying busy keys. Admission reserves both credential and provider capacity; a failed group reservation releases partial permits before waiting. Waiting does not occupy the gateway-wide upstream execution gate. The permit remains attached to the execution, including a streaming response or WebSocket turn, until completion or cancellation.

## Configuration

Each upstream key has a `concurrency_queue_enabled` boolean setting beside its concurrency limit in the ordinary key, OAuth key and shared import settings forms. It defaults to `true` for existing and new keys. Setting it to `false` makes that key attempt both key and provider capacity once without waiting; other candidates can still be tried. The setting is persisted and included in key responses and configuration exports. Updating other key fields without this property preserves its current value. Only JSON booleans are accepted.

`AETHER_GATEWAY_PROVIDER_CONCURRENCY_QUEUE_TIMEOUT_MS` defaults to `15000` (15 seconds), with a maximum of `120000`. This is the budget for each candidate admission, not the entire request across retries.

`AETHER_GATEWAY_PROVIDER_CONCURRENCY_QUEUE_CAPACITY` defaults to `100` waiting requests per upstream credential, with a maximum of `10000`. Executing requests do not consume waiting capacity. Setting either value to `0` disables waiting and retains immediate capacity rejection. Configuration is loaded when the gateway starts.

Queue overflow or timeout allows candidate fallback. If all usable candidates are capacity-blocked, HTTP requests return `429`. Cancellation removes the waiter and releases any reserved permits. Queuing does not create an upstream failure cooldown. After admission, current catalog availability, quota and credential restrictions are checked again before upstream execution.

FIFO ordering is process-local. With the Redis runtime backend, active concurrency permits are shared across gateway instances, but waiting order is not globally FIFO. Admission polls runtime capacity every 50 milliseconds. No streaming response headers or SSE heartbeat are emitted while waiting, so clients and reverse proxies must allow for the configured wait budget plus upstream first-byte time.

The `provider_concurrency_queue_wait` stage metric records admission wait time.

Usage records display a static “排队中” (Queued) badge while a request is waiting for provider concurrency admission. The admin and user usage list/active APIs expose the transient `is_queued` flag without changing the request lifecycle status. Ordinary pending requests retain their existing waiting label; admitted, timed-out and cancelled requests no longer display the queue badge. This does not add queue positions, wait timers or client-facing SSE progress events.

## Reference

The per-credential FIFO, bounded waiting and cancellation behavior is inspired by [yetone/magpie concurrency admission](https://github.com/yetone/magpie/blob/d9c6a60d8a0b023f77bdf16d4d896316c67ec7c6/internal/gateway/concurrency.go). Unlike Magpie's optional unlimited queue and wait, Aether defaults to finite capacity and timeout and reuses its runtime semaphore backend.
