# Runtime Redis Operations Runbook

This runbook covers Aether runtime Redis connection pressure incidents. It is
not a substitute for fixing application-level connection churn.

## Persistence Policy

The bundled `docker-compose.yml` treats Redis as a low-latency runtime
coordination layer by default: locks, cache affinity, semaphores, and runtime
streams. Postgres remains the source of truth. The default Redis persistence
policy is passed directly to `redis-server` in `docker-compose.yml`:

```sh
--dir /tmp --appendonly no --save ""
```

This avoids request-path latency spikes from AOF fsync and background snapshot
forks. The trade-off is that Redis runtime state can be lost if the Redis
container or host crashes before workers have flushed queued records to the
database. The default `dir /tmp` also prevents old files in the mounted
data directory from being loaded as stale runtime state; the persistence
disable itself is `--appendonly no` and `--save ""`.

Only deployments that intentionally want Redis runtime streams to survive a
crash should restore persistence in the Redis command:

```sh
--dir /data --appendonly yes --appendfsync everysec --save 60 1000
```

Expect higher tail latency when Redis persistence shares disks with Postgres or
application logs.

### Daily usage limit counters

Daily usage limits intentionally remain compatible with the default
non-persistent Redis policy. Every positive finalized request updates the user
and API key counters even when that scope is currently unlimited, so changing a
limit only changes the threshold and never starts a database query.

The gateway reads a global daily-usage runtime-state marker together with the
applicable counters in the same Redis `MGET`. If Redis loses its runtime state,
the missing marker makes daily limits temporarily fail open and starts one
distributed background recovery. That recovery performs one grouped scan of
the current `APP_TIMEZONE` day, restores counters in Redis batches without
lowering concurrent values, and marks the runtime state ready. It does not run
a query per user or API key, does not put SQL on the request path, and does not
require usage-table indexes dedicated to this feature.

Gateway process restarts do not trigger recovery while Redis still contains the
marker. Redis container or host restarts do trigger recovery on the first
limited request. A small amount of undercounting or over-limit traffic is
acceptable while recovery overlaps in-flight settlements; this feature is a
traffic policy, not a financial balance ledger.

### OpenAI Responses continuation history

When an OpenAI Responses request is converted to an OpenAI Chat provider,
Aether stores the completed continuation transcript in `RuntimeState` under the
`ai:responses:history:v1` namespace. Records are immutable, scoped by a hashed
API key identity, limited to 8 MiB, and expire after six hours. Redis `SET` with
TTL makes completion writes atomic and idempotent.

All gateway instances must use the same Redis URL and key prefix. This allows a
continuation request to land on another instance and allows gateway processes
to restart without losing history. `AETHER_RUNTIME_BACKEND=memory` remains a
single-process development mode and cannot provide either guarantee; multi-node
startup rejects it.

The bundled non-persistent Redis policy survives gateway restarts but not a
Redis container or host restart. Deployments that require continuation history
to survive Redis restarts must enable the AOF/RDB policy above and mount `/data`,
or use an externally managed persistent Redis service. Monitor
`openai_response_history_read_failed`, `openai_response_history_write_failed`,
and `openai_response_history_invalid` events for backend or payload failures.

## Latency Triage

Redis `INFO commandstats` reports `latency_percentiles_usec_*` values in
microseconds. For example `p99=2007` means about 2 ms, not 2 seconds.

Use these checks before attributing app stalls to Redis:

```sh
redis-cli -p 6379 -a "$REDIS_PASSWORD" LATENCY DOCTOR
redis-cli -p 6379 -a "$REDIS_PASSWORD" LATENCY LATEST
redis-cli -p 6379 -a "$REDIS_PASSWORD" SLOWLOG GET 20
redis-cli -p 6379 -a "$REDIS_PASSWORD" INFO persistence
redis-cli -p 6379 -a "$REDIS_PASSWORD" INFO commandstats
redis-cli -p 6379 -a "$REDIS_PASSWORD" INFO clients
```

For immediate mitigation on an existing container that is running with AOF
enabled:

```sh
redis-cli -p 6379 -a "$REDIS_PASSWORD" CONFIG SET appendfsync no
redis-cli -p 6379 -a "$REDIS_PASSWORD" CONFIG SET appendonly no
redis-cli -p 6379 -a "$REDIS_PASSWORD" CONFIG SET save ""
```

Active defrag can help when `mem_fragmentation_ratio` is high, but it is not an
AOF fsync fix. Enable it only after confirming the Redis build supports it:

```sh
redis-cli -p 6379 -a "$REDIS_PASSWORD" CONFIG SET activedefrag yes
redis-cli -p 6379 -a "$REDIS_PASSWORD" CONFIG SET active-defrag-ignore-bytes 50mb
redis-cli -p 6379 -a "$REDIS_PASSWORD" CONFIG SET active-defrag-threshold-lower 10
```

## Normal Expectations

- Each `RuntimeState` Redis backend initializes a fixed set of long-lived
  connection lanes: fast, stream, blocking stream, and admin.
- `connected_clients` should stay near a small fixed number per app instance,
  plus health checks and ad hoc admin clients.
- `total_connections_received` should not grow linearly with request volume.
- Large TIME_WAIT spikes between app and Redis indicate a regression or a
  separate process repeatedly opening Redis connections.

## Emergency Mitigation

1. Disable the retry source first, such as expired Codex/OAuth keys causing a
   retry storm.
2. Restart the app to stop continued connection creation:

   ```sh
   docker compose restart app
   ```

3. On a Linux host, temporarily widen the ephemeral port range and enable safe
   TIME_WAIT reuse:

   ```sh
   sudo sysctl -w net.ipv4.ip_local_port_range="10000 65535"
   sudo sysctl -w net.ipv4.tcp_tw_reuse=1
   ```

4. Do not enable `tcp_tw_recycle`; it is obsolete and unsafe with NAT.

Docker Desktop on macOS runs containers inside a Linux VM. Host-level macOS
`sysctl` changes do not necessarily affect the VM network namespace.

## Checks

Use Redis `INFO clients` and `INFO stats` to inspect:

- `connected_clients`
- `total_connections_received`

Use OS socket tooling on the Redis host or container namespace to inspect
TIME_WAIT counts. Persistent growth after the runtime Redis refactor means a
different code path or process is still opening short-lived Redis connections.

## File Descriptor Limits

Aether's compose files intentionally do not set container `ulimits.nofile`.
Redis connection churn must be fixed in application code, not hidden by larger
file descriptor limits.

For high-concurrency production hosts, set file descriptor policy at the
runtime or service-manager layer instead:

- Docker daemon default ulimit, for example `default-ulimits` in
  `/etc/docker/daemon.json`.
- systemd service limits such as `LimitNOFILE=` for Docker or the process
  supervisor.
- Managed container platform resource settings, when Docker daemon settings are
  not available.

Keep Redis `maxclients` below the effective Redis process `nofile` limit with
room for persistence files, replicas, and admin connections.
