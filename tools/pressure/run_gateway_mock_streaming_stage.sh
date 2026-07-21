#!/usr/bin/env bash
set -euo pipefail

# Gateway staged mock streaming pressure probe.
#
# Required auth:
#   AETHER_API_KEY_FILE=/path/to/api-key
# or:
#   AUTH_HEADER='Authorization: Bearer <aether-api-key>'
# or:
#   AETHER_API_KEY='<aether-api-key>'
#
# Common settings:
#   PRESSURE_STAGE=S1|S2|S3|S4|S5
#   GATEWAY_BASE_URL=http://127.0.0.1:8084
#   TARGET_URL=http://127.0.0.1:8084/v1/chat/completions
#   METRICS_URL=http://127.0.0.1:8084/_gateway/metrics
#   PRESSURE_MODEL=gpt-5-mini
#   PRESSURE_RESPONSE_MODE=first-body-byte
#   PRESSURE_CARGO_PROFILE=release
#   PRESSURE_FIRST_BODY_HOLD_MS=120000
#   PRESSURE_TIMEOUT_MS=150000
#   PRESSURE_SETTLE_AFTER_MS=180000
#   PRESSURE_WAVES=1
#
# Every request holds the streaming response for about two minutes by default.
# The 180-second settle value is only the maximum post-load drain window; the
# probe exits it early as soon as queues and outboxes are fully drained, and it
# does not extend the lifetime or timeout of an individual request.
# For a sustained soak, run multiple consecutive two-minute waves instead of
# extending one synthetic LLM request to 30 minutes or two hours, for example:
#   PRESSURE_STAGE=S5 PRESSURE_WAVES=8 ./tools/pressure/run_gateway_mock_streaming_stage.sh

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/../.." && pwd)"

PRESSURE_STAGE="${PRESSURE_STAGE:-S1}"
PRESSURE_STAGE="$(printf '%s' "$PRESSURE_STAGE" | tr '[:lower:]' '[:upper:]')"

# Keep the simulated LLM request duration realistic across all stages. The
# client deadline includes a 30-second safety margin for admission, first byte,
# and response-tail draining around the two-minute hold.
default_hold_ms=120000
default_timeout_ms=150000

case "$PRESSURE_STAGE" in
  S1)
    default_requests=1000
    default_concurrency=1000
    default_start_ramp_ms=10000
    default_output=/tmp/aether_gateway_pressure_s1_1k.json
    ;;
  S2)
    default_requests=3000
    default_concurrency=3000
    default_start_ramp_ms=30000
    default_output=/tmp/aether_gateway_pressure_s2_3k.json
    ;;
  S3)
    default_requests=6000
    default_concurrency=6000
    default_start_ramp_ms=60000
    default_output=/tmp/aether_gateway_pressure_s3_6k.json
    ;;
  S4)
    default_requests=10000
    default_concurrency=10000
    default_start_ramp_ms=90000
    default_output=/tmp/aether_gateway_pressure_s4_10k.json
    ;;
  S5)
    default_requests=10000
    default_concurrency=10000
    default_start_ramp_ms=120000
    default_output=/tmp/aether_gateway_pressure_s5_10k_soak.json
    ;;
  *)
    echo "unsupported PRESSURE_STAGE=$PRESSURE_STAGE; expected S1, S2, S3, S4, or S5" >&2
    exit 2
    ;;
esac

GATEWAY_BASE_URL="${GATEWAY_BASE_URL:-http://127.0.0.1:8084}"
TARGET_URL="${TARGET_URL:-${GATEWAY_BASE_URL%/}/v1/chat/completions}"
METRICS_URL="${METRICS_URL:-${GATEWAY_BASE_URL%/}/_gateway/metrics}"
PRESSURE_REQUESTS="${PRESSURE_REQUESTS:-$default_requests}"
PRESSURE_CONCURRENCY="${PRESSURE_CONCURRENCY:-$default_concurrency}"
PRESSURE_TIMEOUT_MS="${PRESSURE_TIMEOUT_MS:-$default_timeout_ms}"
PRESSURE_CONNECT_TIMEOUT_MS="${PRESSURE_CONNECT_TIMEOUT_MS:-30000}"
PRESSURE_SAMPLE_INTERVAL_MS="${PRESSURE_SAMPLE_INTERVAL_MS:-500}"
PRESSURE_SETTLE_AFTER_MS="${PRESSURE_SETTLE_AFTER_MS:-180000}"
PRESSURE_START_RAMP_MS="${PRESSURE_START_RAMP_MS:-$default_start_ramp_ms}"
PRESSURE_FIRST_BODY_HOLD_MS="${PRESSURE_FIRST_BODY_HOLD_MS:-$default_hold_ms}"
PRESSURE_METHOD="${PRESSURE_METHOD:-POST}"
PRESSURE_RESPONSE_MODE="${PRESSURE_RESPONSE_MODE:-first-body-byte}"
PRESSURE_CARGO_PROFILE="${PRESSURE_CARGO_PROFILE:-release}"
PRESSURE_MODEL="${PRESSURE_MODEL:-gpt-5-mini}"
PRESSURE_WAVES="${PRESSURE_WAVES:-1}"
OUTPUT="${OUTPUT:-$default_output}"
api_key_file="${AETHER_API_KEY_FILE:-${API_KEY_FILE:-${PRESSURE_API_KEY_FILE:-}}}"
stage_lower="$(printf '%s' "$PRESSURE_STAGE" | tr '[:upper:]' '[:lower:]')"
PRESSURE_BODY_FILE="${PRESSURE_BODY_FILE:-/tmp/aether-pressure-${stage_lower}-mock-streaming-request.json}"

if ! [[ "$PRESSURE_WAVES" =~ ^[1-9][0-9]*$ ]]; then
  echo "PRESSURE_WAVES must be a positive integer, got $PRESSURE_WAVES" >&2
  exit 2
fi

if [[ -z "${AUTH_HEADER:-}" ]]; then
  if [[ -n "$api_key_file" && -s "$api_key_file" ]]; then
    :
  elif [[ -n "${AETHER_API_KEY:-}" ]]; then
    AUTH_HEADER="Authorization: Bearer ${AETHER_API_KEY}"
  elif [[ -n "${API_KEY:-}" ]]; then
    AUTH_HEADER="Authorization: Bearer ${API_KEY}"
  else
    echo "missing auth: set AETHER_API_KEY_FILE, AUTH_HEADER, or AETHER_API_KEY before running gateway staged pressure" >&2
    exit 2
  fi
fi

if [[ -z "${PRESSURE_BODY:-}" && ! -s "$PRESSURE_BODY_FILE" ]]; then
  cat >"$PRESSURE_BODY_FILE" <<JSON
{"model":"${PRESSURE_MODEL}","messages":[{"role":"user","content":"ping"}],"stream":true}
JSON
fi

args=(run)
case "$PRESSURE_CARGO_PROFILE" in
  release)
    args+=(--release)
    ;;
  debug)
    ;;
  *)
    echo "unsupported PRESSURE_CARGO_PROFILE=$PRESSURE_CARGO_PROFILE; expected release or debug" >&2
    exit 2
    ;;
esac

args+=(
  -p aether-loadtools --bin gateway_pressure_probe --
  --url "$TARGET_URL"
  --metrics-url "$METRICS_URL"
  --requests "$PRESSURE_REQUESTS"
  --concurrency "$PRESSURE_CONCURRENCY"
  --timeout-ms "$PRESSURE_TIMEOUT_MS"
  --connect-timeout-ms "$PRESSURE_CONNECT_TIMEOUT_MS"
  --sample-interval-ms "$PRESSURE_SAMPLE_INTERVAL_MS"
  --settle-after-ms "$PRESSURE_SETTLE_AFTER_MS"
  --start-ramp-ms "$PRESSURE_START_RAMP_MS"
  --first-body-hold-ms "$PRESSURE_FIRST_BODY_HOLD_MS"
  --method "$PRESSURE_METHOD"
  --response-mode "$PRESSURE_RESPONSE_MODE"
)

if [[ -n "$api_key_file" && -s "$api_key_file" ]]; then
  args+=(--api-key-file "$api_key_file")
else
  args+=(--header "$AUTH_HEADER")
fi

if [[ -n "${EXTRA_HEADERS:-}" ]]; then
  while IFS= read -r header; do
    [[ -z "$header" ]] && continue
    args+=(--header "$header")
  done <<< "$EXTRA_HEADERS"
else
  args+=(--header "Content-Type: application/json")
fi

if [[ -n "${PRESSURE_CLIENT_SHARDS:-}" ]]; then
  args+=(--client-shards "$PRESSURE_CLIENT_SHARDS")
fi

if [[ -n "${PRESSURE_POOL_MAX_IDLE_PER_HOST:-}" ]]; then
  args+=(--pool-max-idle-per-host "$PRESSURE_POOL_MAX_IDLE_PER_HOST")
fi

if [[ -n "${PRESSURE_WARMUP_CONNECTIONS:-}" ]]; then
  args+=(--warmup-connections "$PRESSURE_WARMUP_CONNECTIONS")
fi

if [[ -n "${PRESSURE_WARMUP_URL:-}" ]]; then
  args+=(--warmup-url "$PRESSURE_WARMUP_URL")
fi

if [[ "${PRESSURE_HTTP1_ONLY:-false}" == "true" ]]; then
  args+=(--http1-only)
fi

if [[ "${PRESSURE_HTTP2_PRIOR_KNOWLEDGE:-false}" == "true" ]]; then
  args+=(--http2-prior-knowledge)
fi

if [[ -n "${PRESSURE_BODY:-}" ]]; then
  args+=(--body "$PRESSURE_BODY")
else
  args+=(--body-file "$PRESSURE_BODY_FILE")
fi

if [[ "${PRESSURE_PREFLIGHT:-true}" == "true" ]]; then
  preflight_args=(
    --stage "$PRESSURE_STAGE"
    --gateway-base-url "$GATEWAY_BASE_URL"
    --target-url "$TARGET_URL"
    --metrics-url "$METRICS_URL"
  )
  if [[ -n "$api_key_file" && -s "$api_key_file" ]]; then
    preflight_args+=(--api-key-file "$api_key_file")
  fi
  "$script_dir/check_gateway_stage_preflight.js" \
    "${preflight_args[@]}"
fi

echo "running $PRESSURE_STAGE gateway mock streaming pressure probe"
echo "  target:        $TARGET_URL"
echo "  metrics:       $METRICS_URL"
echo "  requests:      $PRESSURE_REQUESTS"
echo "  concurrency:   $PRESSURE_CONCURRENCY"
echo "  hold ms:       $PRESSURE_FIRST_BODY_HOLD_MS"
echo "  ramp ms:       $PRESSURE_START_RAMP_MS"
echo "  settle ms:     $PRESSURE_SETTLE_AFTER_MS"
echo "  response mode: $PRESSURE_RESPONSE_MODE"
echo "  cargo:         $PRESSURE_CARGO_PROFILE"
echo "  waves:         $PRESSURE_WAVES"
echo "  output:        $OUTPUT"

wave_output_path() {
  local wave="$1"
  if ((PRESSURE_WAVES == 1)); then
    printf '%s\n' "$OUTPUT"
  elif [[ "$OUTPUT" == *.json ]]; then
    printf '%s.wave-%02d.json\n' "${OUTPUT%.json}" "$wave"
  else
    printf '%s.wave-%02d\n' "$OUTPUT" "$wave"
  fi
}

for ((wave = 1; wave <= PRESSURE_WAVES; wave += 1)); do
  current_output="$(wave_output_path "$wave")"
  metrics_before="${current_output%.json}.metrics.before.prom"
  metrics_after="${current_output%.json}.metrics.after.prom"

  echo
  echo "starting $PRESSURE_STAGE wave $wave/$PRESSURE_WAVES"
  echo "  report: $current_output"

  if [[ "${PRESSURE_CAPTURE_METRICS_SNAPSHOTS:-true}" == "true" ]]; then
    curl -fsS "$METRICS_URL" >"$metrics_before" || true
  fi

  # Use quiet cargo output so sensitive header values are not echoed back as
  # part of Cargo's `Running ...` command line.
  (cd "$repo_root" && cargo -q "${args[@]}" --output "$current_output")

  if [[ "${PRESSURE_CAPTURE_METRICS_SNAPSHOTS:-true}" == "true" ]]; then
    curl -fsS "$METRICS_URL" >"$metrics_after" || true
    echo "metrics snapshots written to:"
    echo "  before: $metrics_before"
    echo "  after:  $metrics_after"
  fi

  echo "$PRESSURE_STAGE wave $wave/$PRESSURE_WAVES report written to $current_output"

  if [[ "${PRESSURE_CHECK_REPORT:-true}" == "true" ]]; then
    "$script_dir/check_gateway_stage_report.js" --stage "$PRESSURE_STAGE" "$current_output"
  fi
done
