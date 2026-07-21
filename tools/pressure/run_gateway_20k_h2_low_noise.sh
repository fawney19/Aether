#!/usr/bin/env bash
set -euo pipefail

# Reproducible 20k client-concurrency profile for the local cleartext gateway.
#
# The gateway's current axum/hyper HTTP/2 server advertises 200 concurrent
# streams per connection. 128 independently warmed reqwest client shards give
# capacity for 25,600 streams while keeping the load-generator connection
# count far below 20,000. The 60-second ramp stays below the 120-second client
# hold, so all 20,000 client requests can overlap before the first one drains.
#
# This wrapper intentionally reuses the S5 report contract and does not change
# its acceptance thresholds. Inspect load.max_in_flight_requests in the report
# alongside gateway_requests_max_in_flight and the mock upstream max-in-flight
# metric before claiming that 20k reached every hop.
#
# Pair it with a deterministic two-minute mock stream; a short upstream stream
# plus a client-only hold does not prove two-minute gateway concurrency:
#   cargo run --release -p aether-integration-tests --bin mock_openai_upstream -- \
#     --bind 127.0.0.1:18181 --chunks 121 --first-byte-delay-ms 150 \
#     --chunk-delay-ms 1000 --payload-bytes 32 --seed 20260721 --assume-stream

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

export GATEWAY_BASE_URL="${GATEWAY_BASE_URL:-http://127.0.0.1:8084}"
export PRESSURE_STAGE="${PRESSURE_STAGE:-S5}"
export PRESSURE_REQUESTS="${PRESSURE_REQUESTS:-20000}"
export PRESSURE_CONCURRENCY="${PRESSURE_CONCURRENCY:-20000}"
export PRESSURE_CLIENT_SHARDS="${PRESSURE_CLIENT_SHARDS:-128}"
export PRESSURE_POOL_MAX_IDLE_PER_HOST="${PRESSURE_POOL_MAX_IDLE_PER_HOST:-1}"
export PRESSURE_WARMUP_CONNECTIONS="${PRESSURE_WARMUP_CONNECTIONS:-128}"
export PRESSURE_WARMUP_URL="${PRESSURE_WARMUP_URL:-${GATEWAY_BASE_URL%/}/_gateway/health}"
export PRESSURE_HTTP2_PRIOR_KNOWLEDGE="${PRESSURE_HTTP2_PRIOR_KNOWLEDGE:-true}"
export PRESSURE_START_RAMP_MS="${PRESSURE_START_RAMP_MS:-60000}"
export PRESSURE_FIRST_BODY_HOLD_MS="${PRESSURE_FIRST_BODY_HOLD_MS:-120000}"
export PRESSURE_TIMEOUT_MS="${PRESSURE_TIMEOUT_MS:-150000}"
export OUTPUT="${OUTPUT:-/tmp/aether_gateway_pressure_20k_h2_low_noise.json}"

exec "$script_dir/run_gateway_mock_streaming_stage.sh"
