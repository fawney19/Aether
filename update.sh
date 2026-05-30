#!/usr/bin/env bash
# One-click updater for Docker Compose deployments.
#
# This updates the app container image and recreates only the app service. It is
# intentionally not a hot patch of the running Rust process.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"

MODE="auto"
COMPOSE_DIR=""
PROJECT_NAME=""
APP_SERVICE="app"
COMPOSE_WAIT_TIMEOUT_SECS=120
COMPOSE_HEALTHCHECK_POLL_INTERVAL_SECS=2
NO_PULL=false
FORCE_RECREATE=false
SHOW_LOGS=false
LOCAL_BUILD=false
PREPARE_ONLY=false
APPLY_PREPARED=false
BACKUP_ENABLED=true
BACKUP_DIR=""
POSTGRES_SERVICE="postgres"
POSTGRES_USER="postgres"
POSTGRES_DB="aether"
ROLLBACK_ENABLED=true
DRY_RUN=false
PREV_IMAGE_ID=""
PREV_IMAGE_REF=""
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
  --project-name NAME     docker compose project name (-p); auto-detected if omitted
  -f, --compose-file FILE compose file path; can be provided multiple times
  --service NAME          app service name, default: app
  --no-pull               skip docker compose pull
  --prepare               pull the latest app image only, do not recreate app
  --apply-prepared        recreate app using the already pulled image
  --force-recreate        force recreate the app container
  --logs                  follow app logs after update
  --no-backup             skip the pre-update postgres pg_dump
  --backup-dir DIR        backup output directory, default: <compose-dir>/backups
  --postgres-service NAME postgres service name, default: postgres
  --postgres-user NAME    postgres user, default: postgres
  --postgres-db NAME      postgres database name, default: aether
  --no-rollback           skip auto rollback to the previous image on failure
  --dry-run               validate compose config and print images, do not pull or recreate
  -h, --help              show help

Examples:
  ./update.sh
  ./update.sh --mode single-node
  ./update.sh --prepare
  ./update.sh --apply-prepared
  ./update.sh --compose-dir /opt/aether/compose
  ./update.sh --mode local-build
  ./update.sh --dry-run
  ./update.sh --no-backup --no-rollback
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
        --project-name)
            [[ $# -ge 2 ]] || die "--project-name requires a value"
            PROJECT_NAME="$2"
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
        --apply-prepared)
            NO_PULL=true
            APPLY_PREPARED=true
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
        --no-backup)
            BACKUP_ENABLED=false
            shift
            ;;
        --backup-dir)
            [[ $# -ge 2 ]] || die "--backup-dir requires a value"
            BACKUP_DIR="$2"
            shift 2
            ;;
        --postgres-service)
            [[ $# -ge 2 ]] || die "--postgres-service requires a value"
            POSTGRES_SERVICE="$2"
            shift 2
            ;;
        --postgres-user)
            [[ $# -ge 2 ]] || die "--postgres-user requires a value"
            POSTGRES_USER="$2"
            shift 2
            ;;
        --postgres-db)
            [[ $# -ge 2 ]] || die "--postgres-db requires a value"
            POSTGRES_DB="$2"
            shift 2
            ;;
        --no-rollback)
            ROLLBACK_ENABLED=false
            shift
            ;;
        --dry-run)
            DRY_RUN=true
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
    [[ "${APPLY_PREPARED}" != "true" ]] || die "--apply-prepared is only supported for Docker Compose deployments"
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
if [[ -n "${PROJECT_NAME}" ]]; then
    COMPOSE_ARGS+=(-p "${PROJECT_NAME}")
fi
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
    COMPOSE_PROGRESS=plain BUILDKIT_PROGRESS=plain "${COMPOSE[@]}" "${COMPOSE_ARGS[@]}" pull "${APP_SERVICE}"
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

backup_postgres() {
    if ! grep -qx "${POSTGRES_SERVICE}" <<< "${services}"; then
        echo ">>> WARNING: postgres service '${POSTGRES_SERVICE}' not found in compose; skipping backup." >&2
        echo ">>> Use --postgres-service to set the correct name or --no-backup to silence this." >&2
        return 0
    fi

    [[ -n "${BACKUP_DIR}" ]] || BACKUP_DIR="${COMPOSE_DIR}/backups"
    mkdir -p "${BACKUP_DIR}" || die "cannot create backup dir: ${BACKUP_DIR}"

    local timestamp
    timestamp="$(date +%Y%m%d-%H%M%S)"
    local backup_file="${BACKUP_DIR}/aether-${POSTGRES_DB}-${timestamp}.sql.gz"

    echo ">>> Backing up ${POSTGRES_DB} from ${POSTGRES_SERVICE} to ${backup_file}..."
    if compose exec -T "${POSTGRES_SERVICE}" pg_dump -U "${POSTGRES_USER}" "${POSTGRES_DB}" \
        | gzip > "${backup_file}"; then
        local size
        size="$(du -h "${backup_file}" 2>/dev/null | cut -f1)"
        echo ">>> Backup complete (${size:-unknown}): ${backup_file}"
    else
        rm -f "${backup_file}"
        die "pg_dump failed; aborting update. Use --no-backup to skip backups."
    fi
}

record_prev_image() {
    local container_id
    container_id="$(compose ps -q "${APP_SERVICE}" 2>/dev/null | head -n 1)"
    if [[ -z "${container_id}" ]]; then
        echo ">>> No running ${APP_SERVICE} container found; rollback will be unavailable."
        return 0
    fi
    PREV_IMAGE_ID="$(docker inspect --format='{{.Image}}' "${container_id}" 2>/dev/null || true)"
    PREV_IMAGE_REF="$(docker inspect --format='{{.Config.Image}}' "${container_id}" 2>/dev/null || true)"
    if [[ -n "${PREV_IMAGE_ID}" && -n "${PREV_IMAGE_REF}" ]]; then
        echo ">>> Recorded prev image for rollback: ${PREV_IMAGE_REF} (${PREV_IMAGE_ID})"
    else
        PREV_IMAGE_ID=""
        PREV_IMAGE_REF=""
        echo ">>> Failed to read prev image; rollback will be unavailable."
    fi
}

rollback_to_prev_image() {
    if [[ -z "${PREV_IMAGE_ID}" || -z "${PREV_IMAGE_REF}" ]]; then
        echo ">>> No prev image recorded; cannot rollback." >&2
        return 1
    fi

    echo ">>> Rolling back: re-tagging ${PREV_IMAGE_ID} as ${PREV_IMAGE_REF}..."
    if ! docker tag "${PREV_IMAGE_ID}" "${PREV_IMAGE_REF}"; then
        echo ">>> docker tag failed; rollback aborted." >&2
        return 1
    fi
    if compose_up_app false; then
        if ! wait_healthy; then
            echo ">>> Rollback recreate completed, but ${APP_SERVICE} did not become healthy." >&2
            return 1
        fi
        echo ">>> Rollback complete. ${APP_SERVICE} is running on the previous image."
        return 0
    fi
    echo ">>> Rollback recreate failed; manual intervention required." >&2
    return 1
}

if [[ "${DRY_RUN}" == "true" ]]; then
    echo ">>> Dry-run: validating compose config..."
    compose config >/dev/null || die "compose config validation failed"
    echo ">>> Compose config OK."
    echo ">>> Services:"
    echo "    ${services//$'\n'/$'\n    '}"
    echo ">>> Images that would be referenced:"
    compose config --images 2>/dev/null | sort -u | sed 's/^/    /'
    if [[ "${BACKUP_ENABLED}" != "true" ]]; then
        echo ">>> Backup target: (skipped via --no-backup)"
    elif grep -qx "${POSTGRES_SERVICE}" <<< "${services}"; then
        echo ">>> Backup target: ${BACKUP_DIR:-${COMPOSE_DIR}/backups}/aether-${POSTGRES_DB}-<timestamp>.sql.gz"
    else
        echo ">>> Backup target: (skipped; postgres service '${POSTGRES_SERVICE}' not found)"
    fi
    echo ">>> Rollback: $([[ "${ROLLBACK_ENABLED}" == "true" ]] && echo "enabled" || echo "disabled via --no-rollback")"
    echo ">>> Done (dry-run; nothing changed)."
    exit 0
fi

if [[ "${PREPARE_ONLY}" == "true" ]]; then
    echo ">>> Preparing update by pulling latest image for ${APP_SERVICE}..."
    compose_pull_app
    echo ">>> Done."
    echo ">>> Note: image is downloaded; ${APP_SERVICE} is still running on the current image."
    echo ">>> Next: rerun the same command with --apply-prepared instead of --prepare for a quick cutover."
    exit 0
fi

if [[ "${BACKUP_ENABLED}" == "true" ]]; then
    backup_postgres
fi

if [[ "${ROLLBACK_ENABLED}" == "true" ]]; then
    record_prev_image
fi

if [[ "${NO_PULL}" != "true" ]]; then
    echo ">>> Pulling latest image for ${APP_SERVICE}..."
    compose_pull_app
elif [[ "${APPLY_PREPARED}" == "true" ]]; then
    echo ">>> Applying previously prepared image for ${APP_SERVICE}; skipping pull before recreate."
else
    echo ">>> Skipping image pull for ${APP_SERVICE} (--no-pull)."
fi

echo ">>> Recreating ${APP_SERVICE}..."
if ! compose_up_app true; then
    echo ">>> Compose up with --wait failed; falling back to simple recreate..."
    if ! compose_up_app false || ! wait_healthy; then
        if [[ "${ROLLBACK_ENABLED}" == "true" ]]; then
            echo ">>> Update failed; attempting rollback to previous image..."
            if rollback_to_prev_image; then
                die "update failed but rollback succeeded; service is on the previous image"
            fi
        fi
        die "update failed and rollback was unavailable; manual intervention required"
    fi
fi

echo ">>> Current services:"
compose ps

echo ">>> Done."

if [[ "${SHOW_LOGS}" == "true" ]]; then
    compose logs -f "${APP_SERVICE}"
fi
