#!/usr/bin/env bash
set -euo pipefail

# Gateway DB pressure probe.
#
# Required:
#   TARGET_URL=http://127.0.0.1:18080/v1/chat/completions
#   METRICS_URL=http://127.0.0.1:18080/_gateway/metrics
#
# Common optional settings:
#   PRESSURE_REQUESTS=60000
#   PRESSURE_CONCURRENCY=6000
#   PRESSURE_TIMEOUT_MS=120000
#   PRESSURE_SAMPLE_INTERVAL_MS=500
#   PRESSURE_METHOD=POST
#   PRESSURE_BODY='{"model":"...","messages":[...],"stream":true}'
#   PRESSURE_BODY_FILE=/tmp/request.json
#   PRESSURE_RESPONSE_MODE=full
#   PRESSURE_CARGO_PROFILE=release
#   AUTH_HEADER='Authorization: Bearer sk-...'
#   EXTRA_HEADERS=$'Content-Type: application/json\nX-Foo: bar'
#   OUTPUT=/tmp/aether_gateway_pressure_6k.json

TARGET_URL="${TARGET_URL:?TARGET_URL is required}"
METRICS_URL="${METRICS_URL:?METRICS_URL is required}"
PRESSURE_REQUESTS="${PRESSURE_REQUESTS:-60000}"
PRESSURE_CONCURRENCY="${PRESSURE_CONCURRENCY:-6000}"
PRESSURE_TIMEOUT_MS="${PRESSURE_TIMEOUT_MS:-120000}"
PRESSURE_SAMPLE_INTERVAL_MS="${PRESSURE_SAMPLE_INTERVAL_MS:-500}"
PRESSURE_METHOD="${PRESSURE_METHOD:-GET}"
PRESSURE_CARGO_PROFILE="${PRESSURE_CARGO_PROFILE:-release}"
OUTPUT="${OUTPUT:-/tmp/aether_gateway_pressure_6k.json}"

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
  --sample-interval-ms "$PRESSURE_SAMPLE_INTERVAL_MS"
  --method "$PRESSURE_METHOD"
  --output "$OUTPUT"
)

if [[ -n "${AUTH_HEADER:-}" ]]; then
  args+=(--header "$AUTH_HEADER")
fi

if [[ -n "${EXTRA_HEADERS:-}" ]]; then
  while IFS= read -r header; do
    [[ -z "$header" ]] && continue
    args+=(--header "$header")
  done <<< "$EXTRA_HEADERS"
fi

if [[ -n "${PRESSURE_BODY_FILE:-}" ]]; then
  args+=(--body-file "$PRESSURE_BODY_FILE")
elif [[ -n "${PRESSURE_BODY:-}" ]]; then
  args+=(--body "$PRESSURE_BODY")
fi

if [[ -n "${PRESSURE_RESPONSE_MODE:-}" ]]; then
  args+=(--response-mode "$PRESSURE_RESPONSE_MODE")
fi

echo "running gateway pressure probe"
echo "  target:      $TARGET_URL"
echo "  metrics:     $METRICS_URL"
echo "  requests:    $PRESSURE_REQUESTS"
echo "  concurrency: $PRESSURE_CONCURRENCY"
echo "  cargo:       $PRESSURE_CARGO_PROFILE"
echo "  output:      $OUTPUT"

# Use quiet cargo output so sensitive header values (for example Authorization)
# are not echoed back as part of Cargo's `Running ...` command line.
cargo -q "${args[@]}"

echo
echo "pressure report written to $OUTPUT"
