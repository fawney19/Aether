#!/usr/bin/env bash
set -euo pipefail

PRESSURE_STAGE="${PRESSURE_STAGE:-S1}"
exec "$(dirname "$0")/run_gateway_mock_streaming_stage.sh"
