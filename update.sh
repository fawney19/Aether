#!/usr/bin/env bash
# One-click updater for Docker Compose deployments.
#
# This updates the app container image and recreates only the app service. It is
# intentionally not a hot patch of the running Rust process.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

MODE="auto"
COMPOSE_DIR=""
APP_SERVICE="app"
COMPOSE_WAIT_TIMEOUT_SECS=120
COMPOSE_HEALTHCHECK_POLL_INTERVAL_SECS=2
NO_PULL=false
FORCE_RECREATE=false
SHOW_LOGS=false
LOCAL_BUILD=false
PREPARE_ONLY=false
COMPOSE_FILES=()
COMPOSE=()
COMPOSE_ARGS=()

usage() {
    cat <<'EOF'
Usage: ./update.sh [options]

Update Aether Docker Compose deployment in one command.

Options:
  --mode MODE             auto, compose, single-node, or local-build
                          auto uses docker-compose.yml in the current directory
  --compose-dir DIR       deployment directory, default: current directory
  -f, --compose-file FILE compose file path; can be provided multiple times
  --service NAME          app service name, default: app
  --no-pull               skip docker compose pull
  --prepare               pull the latest app image only, do not recreate app
  --force-recreate        force recreate the app container
  --logs                  follow app logs after update
  -h, --help              show help

Examples:
  ./update.sh
  ./update.sh --mode single-node
  ./update.sh --compose-dir /opt/aether/compose
  ./update.sh --mode local-build
EOF
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --mode)
            [[ $# -ge 2 ]] || die "--mode requires a value"
            MODE="$2"
            shift 2
            ;;
        --compose-dir)
            [[ $# -ge 2 ]] || die "--compose-dir requires a value"
            COMPOSE_DIR="$2"
            shift 2
            ;;
        -f|--compose-file)
            [[ $# -ge 2 ]] || die "--compose-file requires a value"
            COMPOSE_FILES+=("$2")
            shift 2
            ;;
        --service)
            [[ $# -ge 2 ]] || die "--service requires a value"
            APP_SERVICE="$2"
            shift 2
            ;;
        --no-pull)
            NO_PULL=true
            shift
            ;;
        --prepare)
            PREPARE_ONLY=true
            shift
            ;;
        --force-recreate)
            FORCE_RECREATE=true
            shift
            ;;
        --logs)
            SHOW_LOGS=true
            shift
            ;;
        --local-build)
            MODE="local-build"
            LOCAL_BUILD=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1"
            ;;
    esac
done

case "$MODE" in
    auto|compose|single-node|local-build)
        ;;
    *)
        die "unsupported mode: ${MODE}; expected auto, compose, single-node, or local-build"
        ;;
esac

if [[ "${MODE}" == "local-build" || "${LOCAL_BUILD}" == "true" ]]; then
    [[ "${PREPARE_ONLY}" != "true" ]] || die "--prepare is only supported for Docker Compose deployments"
    deploy_script="${SCRIPT_DIR}/deploy.sh"
    [[ -f "${deploy_script}" ]] || die "local-build mode requires deploy.sh next to update.sh"
    args=()
    if [[ "${FORCE_RECREATE}" == "true" ]]; then
        args+=(--force)
    fi
    exec bash "${deploy_script}" "${args[@]}"
fi

resolve_compose_cli() {
    if [[ "${#COMPOSE[@]}" -gt 0 ]]; then
        return
    fi

    if docker compose version >/dev/null 2>&1; then
        COMPOSE=(docker compose)
        return
    fi

    if command -v docker-compose >/dev/null 2>&1; then
        COMPOSE=(docker-compose)
        return
    fi

    die "docker compose or docker-compose is required"
}

compose() {
    "${COMPOSE[@]}" "${COMPOSE_ARGS[@]}" "$@"
}

compose_config() {
    compose config "$@"
}

resolve_compose_cli

docker info >/dev/null 2>&1 || die "Docker is not running"

if [[ -z "${COMPOSE_DIR}" ]]; then
    COMPOSE_DIR="$(pwd -P)"
fi
COMPOSE_DIR="$(cd -- "${COMPOSE_DIR}" && pwd -P)"

resolve_compose_file() {
    local filename="$1"
    if [[ "${filename}" = /* ]]; then
        printf '%s\n' "${filename}"
    else
        printf '%s\n' "${COMPOSE_DIR}/${filename}"
    fi
}

resolve_default_compose_files() {
    case "${MODE}" in
        compose)
            COMPOSE_FILES=("docker-compose.yml")
            ;;
        single-node)
            if [[ -f "${COMPOSE_DIR}/docker-compose.single-node.yml" ]]; then
                COMPOSE_FILES=("docker-compose.single-node.yml")
            else
                COMPOSE_FILES=("docker-compose.yml")
            fi
            ;;
        auto)
            if [[ -f "${COMPOSE_DIR}/docker-compose.yml" ]]; then
                COMPOSE_FILES=("docker-compose.yml")
            elif [[ -f "${COMPOSE_DIR}/docker-compose.single-node.yml" ]]; then
                COMPOSE_FILES=("docker-compose.single-node.yml")
            else
                die "no docker-compose.yml or docker-compose.single-node.yml found in ${COMPOSE_DIR}"
            fi
            ;;
    esac
}

if [[ "${#COMPOSE_FILES[@]}" -eq 0 ]]; then
    resolve_default_compose_files
fi

COMPOSE_ARGS+=(--project-directory "${COMPOSE_DIR}")
for file in "${COMPOSE_FILES[@]}"; do
    resolved_file="$(resolve_compose_file "${file}")"
    [[ -f "${resolved_file}" ]] || die "compose file not found: ${resolved_file}"
    COMPOSE_ARGS+=(-f "${resolved_file}")
done

services="$(compose_config --services)"
if ! grep -qx "${APP_SERVICE}" <<< "${services}"; then
    die "service '${APP_SERVICE}' not found in compose config"
fi

echo ">>> Compose directory: ${COMPOSE_DIR}"
echo ">>> App service: ${APP_SERVICE}"

compose_pull_app() {
    compose pull "${APP_SERVICE}"
}

compose_up_app() {
    local wait_for_health="${1:-false}"
    local -a up_args=(up -d)

    if [[ "${FORCE_RECREATE}" == "true" ]]; then
        up_args+=(--force-recreate)
    fi
    if [[ "${wait_for_health}" == "true" ]]; then
        up_args+=(--wait --wait-timeout "${COMPOSE_WAIT_TIMEOUT_SECS}")
    fi

    up_args+=("${APP_SERVICE}")
    compose "${up_args[@]}"
}

wait_healthy() {
    local timeout="${1:-${COMPOSE_WAIT_TIMEOUT_SECS}}"
    local elapsed=0
    echo ">>> Waiting for ${APP_SERVICE} to become healthy (timeout ${timeout}s)..."
    while (( elapsed < timeout )); do
        local container_id
        local state
        container_id="$(compose ps -q "${APP_SERVICE}" 2>/dev/null | head -n 1)"
        if [[ -z "${container_id}" ]]; then
            sleep "${COMPOSE_HEALTHCHECK_POLL_INTERVAL_SECS}"
            elapsed=$(( elapsed + COMPOSE_HEALTHCHECK_POLL_INTERVAL_SECS ))
            continue
        fi
        state="$(docker inspect --format='{{.State.Health.Status}}' \
            "${container_id}" 2>/dev/null || true)"
        if [[ "${state}" == "healthy" ]]; then
            echo ">>> Container is healthy."
            return 0
        fi
        sleep "${COMPOSE_HEALTHCHECK_POLL_INTERVAL_SECS}"
        elapsed=$(( elapsed + COMPOSE_HEALTHCHECK_POLL_INTERVAL_SECS ))
    done
    echo ">>> WARNING: health check timed out after ${timeout}s."
    return 1
}

if [[ "${PREPARE_ONLY}" == "true" ]]; then
    echo ">>> Preparing update by pulling latest image for ${APP_SERVICE}..."
    compose_pull_app
    echo ">>> Done."
    echo ">>> Note: image is downloaded. Recreate ${APP_SERVICE} to apply the update."
    exit 0
fi

if [[ "${NO_PULL}" != "true" ]]; then
    echo ">>> Pulling latest image for ${APP_SERVICE}..."
    compose_pull_app
fi

echo ">>> Recreating ${APP_SERVICE}..."
compose_up_app true || {
    echo ">>> Compose up with --wait failed; falling back to simple recreate..."
    compose_up_app false
    wait_healthy || true
}

echo ">>> Current services:"
compose ps

echo ">>> Done."

if [[ "${SHOW_LOGS}" == "true" ]]; then
    compose logs -f "${APP_SERVICE}"
fi
