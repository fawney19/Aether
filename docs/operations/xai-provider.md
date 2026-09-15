# xAI provider behavior

The following rules preserve the provider-specific behavior of the `xai` provider
across Aether's request and transport layers.

## Responses and tools

- HTTP requests drop `previous_response_id`. Clients must supply conversation
  history; this provider does not add an HTTP response-ID history store.
- `metadata.user_id` is removed. Claude clients copy it onto converted Responses
  bodies and xAI rejects the field.
- Preserve requested `reasoning.encrypted_content`. On a native Responses-to-Responses
  hop, keep provider-owned input items instead of rebuilding them through the canonical
  format. xAI encrypted reasoning may have IDs that do not use OpenAI's `rs` prefix.
  Aether's Gemini signature carriers remain excluded from xAI replay.
- The replay policy is selected from the configured provider type. A model called
  `grok-*` on another provider does not opt into that policy. WebSocket continuation
  metadata retains the selected policy across reconnects.
- A regular client function called `web_search` remains a function. Claude hosted
  search choices are resolved against the original typed tool declaration, including
  declarations with a different name.
- When only `image_generation` is allowed, keep only that tool and retain the requested
  `auto` or `required` mode. For mixed allowed-tool lists, remove the image choice while
  preserving the other allowed entries, as required by xAI's tool-choice schema.
- Reasoning effort is stripped for models that do not accept it.
- OpenAI-style image reference aliases in a request body are rewritten to xAI's
  shape without touching chat message parts.

## Routing and credentials

OAuth requests default to `https://cli-chat-proxy.grok.com/v1`; API-key or
`using_api=true` requests default to `https://api.x.ai/v1`. Explicit custom gateways
are preserved. Compact remains on the official endpoint. CLI identity headers are
applied where the selected upstream requires them.

Account binding uses the xAI device code flow: the gateway requests a device code,
the operator authorizes it out of band, and the gateway polls for the token set.
There is no local callback listener, so headless deployments can bind accounts.
Refresh tokens can also be imported individually or in batches, and are rotated
on refresh.

Quota refresh reads `/user` and `/billing?format=credits` and stores a structured
usage snapshot. A prepaid balance keeps an account selectable after the weekly
allowance is exhausted. API-key accounts skip the subscription billing surface.

## Images and videos

OAuth media requests default to `https://cli-chat-proxy.grok.com/v1`; API-key or
`using_api=true` requests default to `https://api.x.ai/v1`. Explicit custom gateways
are preserved. Compact remains on the official endpoint. CLI identity headers are
applied to media requests and restored when a persisted video task's polling transport
is reconstructed.

Aether's OpenAI-compatible task parser accepts xAI's `request_id` creation field,
status aliases such as `pending` and `done`, nested `video.url` and `video.duration`,
and failure payloads containing `code` / `error` without a status. Existing OpenAI
`id` takes precedence. The client receives Aether's local task ID; polling uses the
upstream task ID and selected credential. Completed video downloads use the returned
media URL without forwarding provider authentication headers to the media host.

### Public video protocols

The xAI provider supports two video surfaces:

| Operation | xAI native | OpenAI compatible |
| --- | --- | --- |
| Create | `POST /v1/videos/generations` | `POST /openai/v1/videos` |
| Edit / extend | `POST /v1/videos/edits`, `POST /v1/videos/extensions` | — |
| Retrieve | `GET /v1/videos/{request_id}` | `GET /openai/v1/videos/{id}` |
| Download | use the returned `video.url` | `GET /openai/v1/videos/{id}/content` |

For xAI, `POST /v1/videos` is a native creation alias. Other providers retain
Aether's existing OpenAI-compatible `/v1/videos` behavior. xAI callers using
OpenAI `seconds` / `size` parameters must use `/openai/v1/videos`. The adapter
maps these to numeric `duration`, `aspect_ratio`, and `resolution`; it also adapts
image references. This implementation defaults to 4 seconds, portrait, and 720p,
clamps `duration` to 1-15, and validates inputs. Explicit native requests retain
native parameters and additional provider fields.

Default xAI creation targets `/videos/generations` on the selected upstream host.
Explicit custom endpoint paths still take precedence. Native generation, editing,
and extension paths only select xAI provider candidates.

Native creation returns `request_id`; native retrieval preserves `done`, nested
`video.url`, and provider fields such as `respect_moderation`. The identifier is
an opaque Aether task ID so queries remain scoped to the owning user and pinned
to the original upstream task and credential. The explicit `/openai/v1/videos`
surface projects `id`, `completed`, and `video_url`.

The task row records the native client protocol as `xai:video`, while its provider
transport remains `openai:video`. This survives restart without storing request
bodies or credentials. Raw native responses are cached only in memory; after
reconstruction the gateway refreshes from the original provider to recover its
response fields, including for completed tasks. If refreshing is unavailable,
the stored task still provides the native status and media URL projection.

OpenAI/xAI task persistence supplies a stable 16-character `short_id`, as required
by the PostgreSQL schema. Existing rows retain their original short ID across
reconstruction, including legacy embedded snapshots. This internal identifier is
separate from the opaque local task ID returned to clients; no schema change or
historical row rewrite is needed.

Task retrieval and content downloads are admitted by the production GET execution
gate. Reconstructed tasks resolve proxy nodes, system proxy defaults, tunnel affinity,
and transport profiles through the same deployment resolver used for creation;
configured proxy routes must not silently turn into direct requests after restart.

### Runtime configuration

Standalone Rust deployments must set
`AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE=rust-authoritative` and restart the
gateway to enable video task retrieval, polling, and content downloads. The CLI's
legacy default is `python-sync-report`: creation can return a task ID in that mode,
but the local task read/refresh paths are disabled and may return HTTP 503.

When the gateway also serves the frontend, `/openai/v1/videos` and its subpaths
must bypass the static SPA handler and be mounted as API routes. Otherwise a
successful-looking HTTP 200 response to a video query may contain `text/html`
instead of the task's JSON response. The lifecycle regression includes the static
frontend to cover this production configuration.

## Regression coverage

The format tests cover client and hosted search choices, image-only and mixed tool
restrictions, encrypted reasoning replay, image reference rewriting, and unchanged
OpenAI replay restrictions. Transport tests cover OAuth/API-key/custom routing and
media identity headers. Video-task tests exercise creation, polling, terminal
projection, persistence fields, content-download planning, and status-less errors
using local fixtures. They do not make paid generation requests.

The HTTP regression exercises all native creation paths and the compatibility
prefix through the public router and candidate planner, then checks polling,
cross-user denial, persistence, retrieval from a fresh gateway instance, and downloads
through both prefixes without leaking authorization to the media host. It uses the
real HTTP executor and a managed proxy node backed by a local test server, with no
execution-runtime override. The background poller also has a real HTTP proxy-node
regression, so production method guards and transport reconstruction are exercised.
CI also runs the same HTTP lifecycle with the PostgreSQL repository and the
production column constraints/indexes in an isolated temporary table. This catches
persistence failures that the in-memory repository cannot expose. The test uses
local `initdb`, `postgres`, and `pg_ctl` (already provided by the gateway CI job),
or an explicit `AETHER_TEST_DATABASE_URL` pointing to an isolated test database.

```sh
cargo test -p aether-ai-formats -p aether-provider-transport -p aether-video-tasks-core --lib
cargo test -p aether-gateway --lib xai
```
