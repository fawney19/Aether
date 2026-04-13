# Aether Proxy C++ Rewrite Design

**Date:** 2026-04-13

**Status:** Draft approved for planning

**Goal:** Rebuild `aether-proxy` in C++ with a new internal architecture that is strictly drop-in compatible with the current Aether backend, achieves materially better TTFB and connection quality on Linux, and reaches first-phase full feature parity with the current Rust implementation.

## 1. Context

The current Rust `aether-proxy` already solves the core product problem: an overseas tunnel node connects back to Aether, registers itself, accepts multiplexed tunnel traffic, relays upstream HTTP requests, and returns responses over the tunnel.

However, the current implementation has several architecture-level limits that constrain TTFB and tail latency:

- A single tunnel connection uses a single writer FIFO for control frames, response headers, and response body frames.
- The dispatcher can block on per-stream body delivery, allowing a single slow request body to stall other traffic sharing the same tunnel.
- Tunnel traffic runs over a small number of WebSocket-over-TCP connections, so per-connection head-of-line effects can amplify jitter under cross-border packet loss.
- Control plane and data plane responsibilities are packaged together tightly enough that the performance-critical path is harder to isolate and evolve independently.

This rewrite is not justified by language replacement alone. The value comes from a redesigned data plane, tighter hot-path isolation, and a more explicit control/data separation.

## 2. Confirmed Constraints

The design below is based on the following confirmed requirements:

- **Compatibility:** first release must be strict drop-in with the current, unmodified Aether backend.
- **Scope:** first release must reach full functional parity with the current `aether-proxy`, including setup, service management, upgrade, and TUI.
- **Priority:** performance is the primary objective, especially lower TTFB and better connection quality.
- **Platform:** Linux first, focused on `x86_64` and `arm64`.
- **Repository layout:** the rewrite will live under a new directory named `aether-proxy-cpp/`.

## 3. Non-Goals

The first release will not attempt the following:

- Changing the Aether server protocol before the new proxy is proven in production-like conditions.
- Shipping a QUIC-based tunnel in the first milestone.
- Prioritizing cross-platform abstractions over Linux hot-path performance.
- Introducing product features not already present in the Rust implementation.
- Performing an in-place replacement of the existing Rust directory during the design and planning phase.

## 4. Success Criteria

The rewrite is considered successful only if it meets all of the following:

### Functional

- Connects to the current Aether backend without server changes.
- Supports the current registration, heartbeat, remote config, multi-server, tunnel, setup, service, Docker, and upgrade workflows.
- Produces configuration and operational behavior close enough to permit side-by-side validation against the Rust implementation.

### Performance

- Lower proxy-added TTFB under warm connections.
- Greatly reduced cases where `ResponseHeaders` are delayed by queued body traffic.
- No tunnel-wide read-loop stalls caused by a single slow request body.
- Better tail latency and recovery behavior under packet loss, tunnel resets, or upstream slowdown.

### Operability

- Observable hot-path metrics that clearly distinguish DNS delay, tunnel delay, queue delay, connection acquisition, TLS time, and upstream wait time.
- Safe rollout path with clear fallback to the Rust implementation.
- Linux packaging and service management suitable for production VPS deployment.

## 5. Architecture Overview

The new proxy will be a **single binary with hard internal separation between control plane and data plane**.

This is the recommended architecture because it balances strict compatibility, Linux-first performance, operational simplicity, and first-release full feature parity.

### Top-Level Layers

- **Supervisor**
  Process startup, config loading, logging, signal handling, lifecycle, graceful drain, crash handling, and top-level orchestration.
- **Control Plane**
  Registration, unregister, heartbeat, remote config, multi-server management, setup wizard, service commands, self-upgrade, and config persistence.
- **Data Plane**
  Tunnel sessions, stream lifecycle, request-body ingest, upstream relay, response prioritization, fair scheduling, and queue management.
- **Common Runtime**
  Configuration models, TLS wrappers, JSON/TOML helpers, metrics, compression, disk spill helpers, memory pools, and error taxonomy.

### Core Principle

Control plane and data plane must communicate through explicit message channels and immutable snapshots wherever possible. The data plane must not perform blocking work on behalf of control plane tasks, and control plane correctness must not depend on hot-path shared locks.

## 6. Technology Choices

### Language and Toolchain

- **Language:** C++20
- **Build system:** CMake
- **Compiler targets:** GCC and Clang on Linux

### Libraries

- **Eventing:** Linux `epoll`
- **TLS / WSS:** OpenSSL
- **Upstream HTTP:** libcurl multi interface
- **Compression:** zlib
- **JSON:** simdjson for hot-path parsing, with conventional serialization helpers where needed
- **TOML config:** toml++
- **Logging:** spdlog + fmt
- **Testing:** GoogleTest for unit tests; integration tests via standalone harnesses and compatibility fixtures

### Why This Stack

- `epoll` gives direct Linux-first control over the event loop and queue behavior.
- `libcurl multi` provides mature HTTP/1.1 and HTTP/2 connection pooling without forcing the project to build a full upstream client stack from scratch.
- OpenSSL is operationally familiar, Linux-friendly, and supports the required WSS and TLS behaviors.

## 7. Repository Layout

The rewrite will live in a parallel tree:

```text
aether-proxy-cpp/
  CMakeLists.txt
  cmake/
  cmd/
    aether-proxy/
  src/
    supervisor/
    control/
    data/
    tunnel/
    upstream/
    config/
    setup/
    common/
  tests/
    unit/
    integration/
    compat/
  bench/
  packaging/
    systemd/
    docker/
```

### Directory Responsibilities

- `cmd/aether-proxy/`: thin binary entrypoint
- `src/supervisor/`: bootstrap, shutdown, restart, signal flow
- `src/control/`: registration, heartbeat, server registry, upgrade
- `src/data/`: shard management and queue scheduler
- `src/tunnel/`: tunnel protocol, session state, frame I/O
- `src/upstream/`: DNS validation, upstream connection pool, HTTP relay
- `src/config/`: static config, dynamic config, config file compatibility
- `src/setup/`: TUI, service commands, installer, config wizard
- `src/common/`: shared utilities and platform abstractions
- `tests/compat/`: protocol and behavior parity against current Rust proxy

## 8. Data Plane Design

This is the main reason to do the rewrite.

### 8.1 Data Plane Units

- **DataPlaneManager**
  Owns all shards and maps tunnel sessions to shards.
- **DataPlaneShard**
  A single-threaded event loop owning a subset of tunnel sessions and stream state.
- **TunnelSession**
  A single WebSocket tunnel connection to one Aether server.
- **StreamState**
  State machine for one logical proxied request/response.
- **EgressScheduler**
  Per-tunnel prioritized sender with fair scheduling.
- **UpstreamRequestContext**
  Per-request execution state for DNS, connection acquisition, upstream relay, and timing.

### 8.2 Sharding Model

The data plane will be sharded. Each shard owns:

- its own event loop
- its own tunnel sessions
- its own stream maps
- its own buffers and queue accounting
- its own upstream relay tasks

No request may bounce between shards once assigned. This reduces lock contention and keeps memory ownership explicit.

### 8.3 Tunnel Session Model

Each configured Aether server creates a pool of tunnel sessions. Unlike the current implementation, session management is designed around queue isolation and prioritization from the start.

Each `TunnelSession` contains:

- ingress frame parser
- stream registry
- heartbeat/control path
- prioritized egress queues
- congestion counters
- local timing and health stats

### 8.4 Egress Priorities

The current Rust design allows body traffic to delay response headers. The new design prevents that.

Each tunnel uses three logical egress queues:

- **P0 Control**
  `Ping`, `Pong`, `HeartbeatData`, `HeartbeatAck`, `GoAway`, `StreamError`
- **P1 Response Headers**
  `ResponseHeaders`
- **P2 Response Body**
  `ResponseBody`, `StreamEnd`

The scheduler always serves higher priority classes first, with fairness inside each class. This means:

- new response headers are not trapped behind megabytes of body frames
- keepalive and control traffic are not starved by data bursts
- error and cancel frames can cut through congestion faster

### 8.5 Request Body Ingest

The new design must eliminate the current “one slow body stalls the tunnel read loop” failure mode.

The tunnel ingress thread must:

- decode the incoming frame
- locate the destination `StreamState`
- attempt a non-blocking append into that stream’s body ring buffer
- immediately return to processing more tunnel frames

If the per-stream in-memory body buffer is full:

- first apply bounded backpressure at the stream level
- then optionally spill to disk for large uploads
- never stall the entire tunnel ingress loop waiting on one stream consumer

### 8.6 Stream Lifecycle

Each stream follows an explicit state machine:

- `Created`
- `ReceivingRequestHeaders`
- `ReceivingRequestBody`
- `DispatchingUpstream`
- `AwaitingUpstreamHeaders`
- `StreamingResponseBody`
- `Completed`
- `Failed`
- `Cancelled`

The state machine is owned by the shard, not by shared global maps.

### 8.7 Upstream Relay

The upstream relay path will:

1. validate target host and port
2. resolve DNS through the safe resolver/cache
3. acquire or create an upstream connection via libcurl multi
4. start the upstream request without waiting on unrelated streams
5. push response headers immediately to `P1`
6. stream response body via `P2`

### 8.8 TTFB-Specific Improvements

The redesign explicitly optimizes the following:

- response headers emitted before body backlog
- smaller hot-path critical sections
- no tunnel-wide blocking on per-stream request body handling
- per-tunnel congestion visibility
- per-shard queue isolation
- more tunnel connections with fewer streams per connection, reducing the blast radius of TCP-level HOL

## 9. Control Plane Design

The control plane must remain full-featured but stay off the hot path.

### Control Plane Modules

- **ServerRegistry**
  Holds configured Aether servers, node IDs, and per-server metadata.
- **RegistrationClient**
  Handles register and unregister requests.
- **HeartbeatManager**
  Builds heartbeat payloads, receives acknowledgements, applies server feedback.
- **RemoteConfigApplier**
  Applies allowed-port, log-level, interval, and similar runtime updates.
- **UpgradeManager**
  Handles self-upgrade and restart semantics.
- **SetupWizard**
  Interactive setup/TUI.
- **ServiceManager**
  systemd integration and command wrappers.
- **ConfigStore**
  TOML config persistence and migration compatibility.

### Control/Data Interface

The control plane may affect the data plane only through:

- immutable config snapshot refresh
- explicit command messages
- graceful drain / reconnect directives
- metric reads

Direct mutation of data-plane state from arbitrary control-plane threads is forbidden.

## 10. Compatibility Strategy

This is a strict drop-in first release.

### Must Remain Compatible

- current registration endpoints
- current unregister endpoint
- current heartbeat and remote config behavior
- current multi-server semantics
- current tunnel endpoint path
- current tunnel frame types and framing rules
- current compression expectations
- current environment variables where feasible
- current user-facing CLI command model

### May Change Internally

- queue structure
- worker model
- event loop architecture
- memory layout
- internal metrics model
- disk spill implementation
- packaging internals

## 11. Configuration Design

The new config layer should preserve the current operator experience where practical.

### Static Config

Static config includes:

- server list
- management tokens
- node naming
- tunnel connection counts
- upstream client settings
- reconnect policy
- logging
- service behavior

### Dynamic Config

Dynamic config received from heartbeat acknowledgements includes:

- allowed ports
- node display name
- heartbeat interval
- log level

The data plane consumes dynamic config through versioned snapshots to avoid hot-path locking.

## 12. Setup, Service, and Upgrade

First release must preserve the all-in-one operational story.

### Setup

- Linux terminal TUI for initial config
- multi-server editing
- secure config file permissions
- validation before save

### Service

- systemd unit generation
- start / stop / restart / status / logs commands
- Linux file descriptor and restart policy tuning

### Upgrade

- release download
- checksum verification
- atomic replace
- required restart path for service mode

These features are not the main performance objective, but they are required for parity.

## 13. Observability

The new proxy must expose better evidence for TTFB and connection quality than the current implementation.

### Required Metrics

- DNS resolution time
- tunnel queue delay before `ResponseHeaders`
- tunnel queue delay before first `ResponseBody`
- upstream connection acquisition time
- upstream TCP connect time
- upstream TLS handshake time
- upstream first-byte wait time
- per-tunnel queue depth by priority class
- per-tunnel active stream count
- tunnel ping RTT
- reconnect counts and reconnect causes
- body spill usage
- dropped / timed-out frame counts

### Required Logs

- tunnel connect and disconnect causes
- reconnect reason and delay
- heartbeat send/ack anomalies
- writer congestion events
- disk spill activation
- upstream timeout and connect failures

## 14. Performance Tuning Model

The new implementation should make these knobs explicit and testable:

- tunnel connection count per server
- max streams per tunnel
- per-stream body buffer size
- body spill threshold
- response body chunk size
- queue high-water marks by priority class
- reconnect base and cap
- DNS TTL and cache capacity
- upstream connection pool sizing

The TUI does not need to expose every advanced knob in the first pass, but the config model must support them and the operational docs must describe them.

## 15. Validation Strategy

The rewrite cannot be trusted without both compatibility and performance validation.

### Test Layers

- **Unit tests**
  frame parsing, state machines, queue behavior, config migration, scheduler fairness
- **Integration tests**
  tunnel session lifecycle, upstream relay, reconnect, upgrade flow
- **Compatibility tests**
  compare C++ proxy behavior with current Rust proxy against the same Aether backend behaviors
- **Benchmark tests**
  TTFB under warm/cold connections, body backlog, packet loss simulation, reconnect storms

### Mandatory Benchmark Scenarios

- many small concurrent requests
- mixed small and large request bodies
- SSE / streaming responses
- slow upstream origin
- tunnel packet loss / latency injection
- Aether-side tunnel reconnect churn

## 16. Rollout and Migration

The rollout plan should be reversible from day one.

### Recommended Rollout Path

1. build C++ proxy in parallel under `aether-proxy-cpp/`
2. validate protocol compatibility in isolated environments
3. run A/B proxy tests against the same backend
4. compare metrics and failure modes
5. deploy to a small number of Linux nodes
6. keep Rust proxy as the rollback target until the C++ path proves superior

### Rollback Requirement

No production migration step should make rollback to the Rust implementation difficult or stateful.

## 17. Risks and Mitigations

### Risk: C++ complexity increases bug surface

Mitigation:

- strict ownership boundaries
- shard-local state
- minimal cross-thread shared state
- aggressive testing around queueing and shutdown

### Risk: full feature parity delays data-plane work

Mitigation:

- treat setup/service/upgrade as separate control-plane modules
- build and verify data-plane core before polishing CLI workflows

### Risk: strict drop-in compatibility constrains ideal protocol choices

Mitigation:

- accept current tunnel protocol for first release
- design internals to allow future protocol upgrades later

### Risk: upstream client stack becomes the next bottleneck

Mitigation:

- benchmark libcurl multi early
- isolate upstream layer so it can be replaced later if needed

## 18. Delivery Phases

### Phase 0: Foundations

- repository skeleton
- build system
- config compatibility
- logging and metrics skeleton
- tunnel frame codec tests

### Phase 1: Data Plane Core

- shard manager
- tunnel session
- prioritized egress scheduler
- stream state machine
- upstream relay
- timing instrumentation

### Phase 2: Control Plane Parity

- registration
- unregister
- heartbeat
- remote config
- multi-server support
- setup TUI
- systemd service tooling
- self-upgrade

### Phase 3: Compatibility and Performance Hardening

- compatibility fixtures
- benchmark harness
- failure injection
- rollout guardrails
- packaging and docs

## 19. Final Recommendation

Proceed with a **single-binary C++20 rewrite in `aether-proxy-cpp/` using a hard internal separation between control plane and data plane, plus a sharded, prioritized tunnel data plane designed specifically to reduce proxy-added TTFB and improve connection quality while staying strict drop-in compatible with the current Aether backend**.

This approach gives the best balance of:

- performance improvement potential
- operational simplicity
- rollback safety
- first-release feature parity
- room for future protocol evolution once compatibility constraints are lifted
