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
