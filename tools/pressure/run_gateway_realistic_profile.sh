#!/usr/bin/env bash
set -euo pipefail

# Gateway realistic profile pressure probe.
#
# Profiles:
#   realistic-stream: full-body streaming; use with mock upstream chunks/delay/payload set to realistic values.
#   tps: no artificial hold; measures completed request throughput through auth + DB/Redis + usage/counter paths.
#
# Both profiles default PRESSURE_TIMEOUT_MS to 150000: about two minutes for a
# typical LLM request plus a 30-second safety margin for admission and draining.
# The standard TPS baseline is 30000 requests at 600 concurrency, a 29000 ms
# ramp, and a 180000 ms post-load drain window. It uses one API key so the
# result measures the production hot-key path. AETHER_API_KEY_LIST_FILE is an
# optional multi-tenant fan-out extension, not the standard TPS baseline.
#
# Suggested mock upstream for realistic-stream:
#   cargo run --release -p aether-integration-tests --bin mock_openai_upstream -- \
#     --bind 127.0.0.1:18181 --chunks 80 --first-byte-delay-ms 150 --chunk-delay-ms 50 --payload-bytes 128
#
# Suggested mock upstream for tps:
#   cargo run --release -p aether-integration-tests --bin mock_openai_upstream -- \
#     --bind 127.0.0.1:18181 --chunks 8 --first-byte-delay-ms 20 --chunk-delay-ms 5 --payload-bytes 64
#
# Required auth:
#   AETHER_API_KEY_FILE=/path/to/api-key
# or:
#   AUTH_HEADER='Authorization: Bearer <aether-api-key>'
# or:
#   AETHER_API_KEY='<aether-api-key>'
#
# Optional multi-tenant fan-out auth:
#   AETHER_API_KEY_LIST_FILE=/path/to/api-key-list

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/../.." && pwd)"

PROFILE="${PRESSURE_PROFILE:-${1:-realistic-stream}}"
PROFILE="$(printf '%s' "$PROFILE" | tr '[:upper:]' '[:lower:]')"

default_timeout_ms=150000

case "$PROFILE" in
  realistic-stream)
    default_requests=1000
    default_concurrency=1000
    default_start_ramp_ms=10000
    default_response_mode=full
    default_settle_after_ms=10000
    default_output=/tmp/aether_gateway_realistic_stream_1k.json
    ;;
  tps)
    default_requests=30000
    default_concurrency=600
    default_start_ramp_ms=29000
    default_response_mode=full
    default_settle_after_ms=180000
    default_output=/tmp/aether_gateway_tps_30k_c600_1krps.json
    ;;
  *)
    echo "unsupported PRESSURE_PROFILE=$PROFILE; expected realistic-stream or tps" >&2
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
PRESSURE_SETTLE_AFTER_MS="${PRESSURE_SETTLE_AFTER_MS:-$default_settle_after_ms}"
PRESSURE_START_RAMP_MS="${PRESSURE_START_RAMP_MS:-$default_start_ramp_ms}"
PRESSURE_FIRST_BODY_HOLD_MS="${PRESSURE_FIRST_BODY_HOLD_MS:-0}"
PRESSURE_METHOD="${PRESSURE_METHOD:-POST}"
PRESSURE_RESPONSE_MODE="${PRESSURE_RESPONSE_MODE:-$default_response_mode}"
PRESSURE_CARGO_PROFILE="${PRESSURE_CARGO_PROFILE:-release}"
PRESSURE_MODEL="${PRESSURE_MODEL:-gpt-5-mini}"
OUTPUT="${OUTPUT:-$default_output}"
api_key_file="${AETHER_API_KEY_FILE:-${API_KEY_FILE:-${PRESSURE_API_KEY_FILE:-}}}"
api_key_list_file="${AETHER_API_KEY_LIST_FILE:-${API_KEY_LIST_FILE:-${PRESSURE_API_KEY_LIST_FILE:-}}}"
PRESSURE_BODY_FILE="${PRESSURE_BODY_FILE:-/tmp/aether-pressure-${PROFILE}-request.json}"

if [[ -z "${AUTH_HEADER:-}" ]]; then
  if [[ -n "$api_key_list_file" && -s "$api_key_list_file" ]]; then
    :
  elif [[ -n "$api_key_file" && -s "$api_key_file" ]]; then
    :
  elif [[ -n "${AETHER_API_KEY:-}" ]]; then
    AUTH_HEADER="Authorization: Bearer ${AETHER_API_KEY}"
  elif [[ -n "${API_KEY:-}" ]]; then
    AUTH_HEADER="Authorization: Bearer ${API_KEY}"
  else
    echo "missing auth: set AETHER_API_KEY_FILE, AUTH_HEADER, or AETHER_API_KEY before running gateway realistic pressure" >&2
    exit 2
  fi
fi

if [[ -z "${PRESSURE_BODY:-}" && ! -s "$PRESSURE_BODY_FILE" ]]; then
  cat >"$PRESSURE_BODY_FILE" <<JSON
{"model":"${PRESSURE_MODEL}","messages":[{"role":"system","content":"You are a concise assistant."},{"role":"user","content":"Write a practical deployment checklist for a high-concurrency API gateway. Include authentication, billing, observability, rollout, rollback, and incident handling."}],"stream":true}
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
  --output "$OUTPUT"
)

case "$(printf '%s' "$PRESSURE_RESPONSE_MODE" | tr '[:upper:]' '[:lower:]')" in
  full|full-body|full_body|body)
    args+=(--require-sse-done)
    ;;
esac

if [[ -n "$api_key_list_file" && -s "$api_key_list_file" ]]; then
  args+=(--api-key-list-file "$api_key_list_file")
elif [[ -n "$api_key_file" && -s "$api_key_file" ]]; then
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

metrics_before="${OUTPUT%.json}.metrics.before.prom"
metrics_after="${OUTPUT%.json}.metrics.after.prom"

if [[ "${PRESSURE_PREFLIGHT:-true}" == "true" ]]; then
  preflight_args=(
    --stage "$PROFILE"
    --gateway-base-url "$GATEWAY_BASE_URL"
    --target-url "$TARGET_URL"
    --metrics-url "$METRICS_URL"
  )
  if [[ -n "$api_key_list_file" && -s "$api_key_list_file" ]]; then
    preflight_args+=(--api-key-list-file "$api_key_list_file")
  elif [[ -n "$api_key_file" && -s "$api_key_file" ]]; then
    preflight_args+=(--api-key-file "$api_key_file")
  fi
  "$script_dir/check_gateway_stage_preflight.js" "${preflight_args[@]}"
fi

echo "running $PROFILE gateway realistic pressure probe"
echo "  target:        $TARGET_URL"
echo "  metrics:       $METRICS_URL"
echo "  requests:      $PRESSURE_REQUESTS"
echo "  concurrency:   $PRESSURE_CONCURRENCY"
echo "  response mode: $PRESSURE_RESPONSE_MODE"
echo "  hold ms:       $PRESSURE_FIRST_BODY_HOLD_MS"
echo "  ramp ms:       $PRESSURE_START_RAMP_MS"
echo "  settle ms:     $PRESSURE_SETTLE_AFTER_MS"
echo "  cargo:         $PRESSURE_CARGO_PROFILE"
echo "  output:        $OUTPUT"

if [[ "${PRESSURE_CAPTURE_METRICS_SNAPSHOTS:-true}" == "true" ]]; then
  curl -fsS "$METRICS_URL" >"$metrics_before" || true
fi

(cd "$repo_root" && cargo -q "${args[@]}")

if [[ "${PRESSURE_CAPTURE_METRICS_SNAPSHOTS:-true}" == "true" ]]; then
  curl -fsS "$METRICS_URL" >"$metrics_after" || true
  echo "metrics snapshots written to:"
  echo "  before: $metrics_before"
  echo "  after:  $metrics_after"
fi

echo
echo "$PROFILE pressure report written to $OUTPUT"

if [[ "${PRESSURE_CHECK_REPORT:-true}" == "true" ]]; then
  check_args=(--stage "$PROFILE")
  if [[ -n "${PRESSURE_MIN_THROUGHPUT_RPS:-}" ]]; then
    check_args+=(--min-throughput-rps "$PRESSURE_MIN_THROUGHPUT_RPS")
  fi
  if [[ -n "${PRESSURE_MAX_HEADERS_P95_MS:-}" ]]; then
    check_args+=(--max-headers-p95-ms "$PRESSURE_MAX_HEADERS_P95_MS")
  fi
  if [[ -n "${PRESSURE_MAX_FIRST_BODY_P95_MS:-}" ]]; then
    check_args+=(--max-first-body-p95-ms "$PRESSURE_MAX_FIRST_BODY_P95_MS")
  fi
  if [[ -n "${PRESSURE_MAX_P95_MS:-}" ]]; then
    check_args+=(--max-p95-ms "$PRESSURE_MAX_P95_MS")
  fi
  if [[ -n "${PRESSURE_MAX_P99_MS:-}" ]]; then
    check_args+=(--max-p99-ms "$PRESSURE_MAX_P99_MS")
  fi
  if [[ -n "${PRESSURE_MAX_FIRST_BODY_HOLD_MS:-}" ]]; then
    check_args+=(--max-first-body-hold-ms "$PRESSURE_MAX_FIRST_BODY_HOLD_MS")
  fi
  "$script_dir/check_gateway_stage_report.js" "${check_args[@]}" "$OUTPUT"
fi
