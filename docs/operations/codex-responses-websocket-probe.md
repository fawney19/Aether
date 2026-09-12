# Codex Responses WebSocket probe

`aether-codex-ws-probe` is a P0 compatibility probe for a Codex-compatible
Responses WebSocket upstream. It verifies two sequential `response.create`
warmups on one socket, with the second request continuing from the first
response ID.

The command, environment variables, JSON report shape, and Codex-specific
handshake headers remain stable. It now shares only the protocol-driving core
with the separate [OpenAI Responses WebSocket probe](openai-responses-websocket-probe.md);
the two probes intentionally retain independent authentication profiles and
provider-specific assertions.

The probe is intentionally not a production proxy. It does not persist,
refresh, log, or print credentials, account IDs, response IDs, request bodies,
or response bodies.

## Prerequisites

Use a dedicated, non-production Codex test account. Rotate any credential that
has been pasted into a chat, terminal history, issue, or source file before
using this probe.

Set these values only in the process environment or your secret manager:

```bash
export AETHER_CODEX_WS_PROBE_URL='wss://your-codex-upstream.example/backend-api/codex/responses'
export AETHER_CODEX_WS_PROBE_ACCESS_TOKEN='your-short-lived-access-token'
export AETHER_CODEX_WS_PROBE_ACCOUNT_ID='your-account-id'
export AETHER_CODEX_WS_PROBE_MODEL='your-codex-model'
```

The endpoint must use `ws://` or `wss://`, with no credentials, query string,
or fragment. The access token is accepted only through
`AETHER_CODEX_WS_PROBE_ACCESS_TOKEN`; there is deliberately no command-line
flag for it.

## Run

```bash
cargo run -p aether-gateway --bin aether-codex-ws-probe
```

Use `--url` to override only the endpoint and `--timeout-secs` to set a
per-turn receive timeout (1–120 seconds):

```bash
cargo run -p aether-gateway --bin aether-codex-ws-probe -- \
  --url 'wss://your-codex-upstream.example/backend-api/codex/responses' \
  --timeout-secs 30
```

The probe emits one JSON line. A successful run has
`"continuation_confirmed":true`; its event and header fields contain names
only, never values. A failure emits a stable error code such as
`"handshake_failed"`, `"upstream_error_event"`, or
`"response_id_not_observed"`.

## Interpretation

A successful probe establishes that the selected upstream accepts the
Responses WebSocket handshake and retains continuation state on one socket.
It does not establish that all Codex models, account plans, or tunnel egress
paths are supported. In particular, the current `aether-tunnel` HTTP relay
does not forward WebSocket upgrades, so a successful direct probe is a
prerequisite rather than tunnel support.

## Gateway bridge

The gateway exposes WebSocket mode at the same public Responses path:

```text
wss://<aether-gateway>/v1/responses
```

It is disabled by default per provider. In **添加提供商** or **编辑提供商**,
enable **Responses WebSocket 模式** under **功能开关** only after the selected
upstream has passed a compatible WebSocket probe. The setting takes effect for
new WebSocket connections without a gateway restart. It is available to every
provider type; candidate planning still requires a selected
`openai:responses` endpoint.

Authenticate the upgrade request with the normal Aether API key. The first
client frame must be a text JSON `response.create` containing a non-empty
`model`. Aether then applies its regular Responses candidate selection, but
accepts only an eligible, WebSocket-enabled endpoint using `openai:responses`.
It opens an upstream WebSocket using the selected provider key.

The selected provider's model mapping and request headers are applied to every
turn, along with the rest of that candidate's provider-body normalization:
model-directive patches, endpoint body rules, and the Codex body contract
(unsupported-field stripping, its HTTP `store: false` default, and
`tool_choice` defaulting). An explicitly supplied WebSocket `store` value is
restored unchanged after that HTTP-oriented normalization. A
continuation turn with a non-null `previous_response_id` is revalidated through
the current scheduler and normalized against its pinned binding. It can never
move to another provider key, and it is rejected if that exact candidate is no
longer eligible or its physical binding changed.
`store`, `previous_response_id`, and `generate` are re-applied after
normalization because they are WebSocket protocol state that the provider body
contract may otherwise rewrite or strip. `stream` and `background` are removed
because they are HTTP transport fields, not WebSocket-mode fields. Every
independent `response.create` (one without a non-null `previous_response_id`)
runs access checks and candidate planning again, even when the public model is unchanged.
It keeps the existing upstream when the same target remains eligible, or
transparently replaces the upstream between responses when the selected target
changes. Overlapping responses on one client socket remain rejected.

Each `response.create` is tracked as an independent Aether logical request:
it receives its own request/candidate identity, usage lifecycle, and terminal
audit record. `response.completed`, `response.failed`,
`response.incomplete`, `response.cancelled`, client disconnects, and upstream
transport failures all settle that turn through the existing stream reporting
path.

Example client setup:

```python
from websocket import create_connection
import json
import os

ws = create_connection(
    "wss://gateway.example/v1/responses",
    header=[f"Authorization: Bearer {os.environ['AETHER_API_KEY']}"],
)
ws.send(json.dumps({
    "type": "response.create",
    "model": "your-public-model",
    "store": False,
    "input": "Explain this repository.",
}))
```

### Operating limits

- Maximum frame and message size: 16 MiB.
- An idle connection must send its first `response.create` within 60 seconds.
- A connection is closed after 60 minutes; reconnect before then for long runs.
- Each `response.create` must receive its first upstream event within the
  selected provider's `stream_first_byte_timeout` (30 seconds by default),
  and finish within its `request_timeout` (20 minutes by default). Aether
  sends `responses_websocket_first_event_timeout` or
  `responses_websocket_turn_timeout` and closes the bound socket when either
  deadline expires.
- Responses are sequential; no multiplexing is supported on one socket.
- Each `response.create` consumes the normal Aether user/API-key RPM budget.
- Continuations with a non-null `previous_response_id` stay on the bound
  provider key. Independent turns are re-authorized and re-planned each time;
  they reuse the socket only when planning selects the same physical target.
- Direct provider proxy settings are honored through the selected transport
  profile. Tunnel-mode proxy nodes are not supported for this bridge yet.

### DNS and proxy behavior

The gateway's plain and browser-profile WebSocket clients share the HTTP/SSE
provider DNS resolver. Provider hostname answers are not filtered by address
range, including Fake-IP answers such as `198.18.0.0/15`. DNS lookup timeout,
answer-count limits, and rejection of empty answers still apply. Resolution
happens during connection establishment, not while building the client; DNS
failures therefore surface as upstream handshake failures rather than invalid
upstream URLs.

This policy applies only to configured provider hostnames. URL validation still
rejects credentials, fragments, and literal private/reserved IP targets (except
loopback `ws://`), and the gateway frontdoor self-loop guard remains active.
Tunnel owner-relay DNS address filtering is unchanged.

Configure an explicit HTTP(S) or SOCKS proxy on the provider when needed; these
clients do not automatically use system proxy environment variables. A proxy
connection does not trigger a separate gateway-side lookup of the provider
hostname. Existing SOCKS remote-DNS normalization remains in effect.

The [DNS egress audit](dns-egress-audit-2026-09-08.md) documents the separate
tunnel and untrusted-download policies, regression coverage, and remaining
deployment limitations; not every outbound path uses the provider DNS policy.

### Usage and logging

Usage and audit finalization now runs for every accepted `response.create`.
Existing usage body-capture and header-redaction policies apply to the resulting
records. Newly created WebSocket usage records expose `is_websocket=true`, and
the usage-record type column renders them as `WS`. For diagnosis, enable debug
logging for `aether_gateway::handlers::proxy::responses_ws`; event logs contain
only the event type and frame size, never request or response contents. Codex
quota-extension logs remain under `aether_gateway::handlers::proxy::codex_ws`.
Every WebSocket-specific log carries `transport="websocket"` and
`websocket=true`; keep `log_type` for its existing access/event/ops
classification, and render the transport flag as a `WS` label in a log viewer
if desired.
