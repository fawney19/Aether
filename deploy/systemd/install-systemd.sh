#!/usr/bin/env bash
set -euo pipefail

SERVICE_NAME="aether-gateway"
SERVICE_USER="${SERVICE_USER:-aether}"
SERVICE_GROUP="${SERVICE_GROUP:-aether}"
INSTALL_ROOT="${INSTALL_ROOT:-/opt/aether}"
CONFIG_DIR="${CONFIG_DIR:-/etc/aether}"
ENV_TARGET="${CONFIG_DIR}/aether-gateway.env"
SYSTEMD_UNIT_PATH="/etc/systemd/system/${SERVICE_NAME}.service"

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"

BIN_SOURCE="${REPO_ROOT}/target/release/aether-gateway"
FRONTEND_SOURCE="${REPO_ROOT}/frontend/dist"
SERVICE_SOURCE="${REPO_ROOT}/deploy/systemd/aether-gateway.service"
ENV_EXAMPLE_SOURCE="${REPO_ROOT}/deploy/systemd/aether-gateway.env.example"
DATA_COMPOSE_SOURCE="${REPO_ROOT}/deploy/docker-compose.data.yml"

ENV_SOURCE=""
RELEASE_ID="${RELEASE_ID:-$(date +%Y%m%d%H%M%S)}"
SKIP_START="false"

usage() {
    cat <<'EOF'
Usage: sudo deploy/systemd/install-systemd.sh [options]

Options:
  --env-file PATH      Copy PATH to /etc/aether/aether-gateway.env before install
  --release-id ID      Release identifier under /opt/aether/releases/ (default: timestamp)
  --bin-source PATH    Built aether-gateway binary (default: target/release/aether-gateway)
  --frontend-source PATH
                      Built frontend directory (default: frontend/dist)
  --skip-start         Install files and unit, but do not restart the service
  -h, --help           Show this help
EOF
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

info() {
    echo ">>> $*"
}

warn() {
    echo "WARNING: $*" >&2
}

require_root() {
    if [[ "${EUID}" -ne 0 ]]; then
        die "run as root"
    fi
}

require_linux_systemd() {
    [[ "$(uname -s)" == "Linux" ]] || die "systemd deployment is only supported on Linux"
    command -v systemctl >/dev/null 2>&1 || die "systemctl not found"
}

trim_whitespace() {
    local value="$1"
    value="${value#"${value%%[![:space:]]*}"}"
    value="${value%"${value##*[![:space:]]}"}"
    printf '%s' "${value}"
}

strip_optional_quotes() {
    local value="$1"
    if [[ ${#value} -ge 2 ]]; then
        if [[ "${value:0:1}" == "\"" && "${value: -1}" == "\"" ]]; then
            value="${value:1:${#value}-2}"
        elif [[ "${value:0:1}" == "'" && "${value: -1}" == "'" ]]; then
            value="${value:1:${#value}-2}"
        fi
    fi
    printf '%s' "${value}"
}

is_placeholder_value() {
    local value="$1"
    case "${value}" in
        *change-me*|*change-this*|*your_secure_password_here*|*your_redis_password_here*)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

find_nologin_shell() {
    if [[ -x /usr/sbin/nologin ]]; then
        echo "/usr/sbin/nologin"
    elif [[ -x /sbin/nologin ]]; then
        echo "/sbin/nologin"
    else
        echo "/bin/false"
    fi
}

ensure_service_account() {
    if ! getent group "${SERVICE_GROUP}" >/dev/null 2>&1; then
        info "creating group ${SERVICE_GROUP}"
        groupadd --system "${SERVICE_GROUP}"
    fi

    if ! id -u "${SERVICE_USER}" >/dev/null 2>&1; then
        info "creating user ${SERVICE_USER}"
        useradd \
            --system \
            --gid "${SERVICE_GROUP}" \
            --home-dir "${INSTALL_ROOT}" \
            --shell "$(find_nologin_shell)" \
            "${SERVICE_USER}"
    fi
}

ensure_sources_exist() {
    [[ -x "${BIN_SOURCE}" ]] || die "binary not found or not executable: ${BIN_SOURCE}. Run: cargo build --release -p aether-gateway"
    [[ -d "${FRONTEND_SOURCE}" ]] || die "frontend dist not found: ${FRONTEND_SOURCE}. Run: (cd frontend && npm ci && npm run build)"
    [[ -f "${SERVICE_SOURCE}" ]] || die "service unit template not found: ${SERVICE_SOURCE}"
    [[ -f "${ENV_EXAMPLE_SOURCE}" ]] || die "env example not found: ${ENV_EXAMPLE_SOURCE}"
    [[ -f "${DATA_COMPOSE_SOURCE}" ]] || die "data compose file not found: ${DATA_COMPOSE_SOURCE}"
    if [[ -n "${ENV_SOURCE}" ]]; then
        [[ -f "${ENV_SOURCE}" ]] || die "env file not found: ${ENV_SOURCE}"
    fi
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --env-file)
                [[ $# -ge 2 ]] || die "--env-file requires a path"
                ENV_SOURCE="$2"
                shift 2
                ;;
            --release-id)
                [[ $# -ge 2 ]] || die "--release-id requires a value"
                RELEASE_ID="$2"
                shift 2
                ;;
            --bin-source)
                [[ $# -ge 2 ]] || die "--bin-source requires a path"
                BIN_SOURCE="$2"
                shift 2
                ;;
            --frontend-source)
                [[ $# -ge 2 ]] || die "--frontend-source requires a path"
                FRONTEND_SOURCE="$2"
                shift 2
                ;;
            --skip-start)
                SKIP_START="true"
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
}

install_env_file() {
    install -d -m 0750 "${CONFIG_DIR}"

    if [[ -n "${ENV_SOURCE}" ]]; then
        info "installing env file to ${ENV_TARGET}"
        install -m 0600 "${ENV_SOURCE}" "${ENV_TARGET}"
        return
    fi

    if [[ ! -f "${ENV_TARGET}" ]]; then
        info "creating env template at ${ENV_TARGET}"
        install -m 0600 "${ENV_EXAMPLE_SOURCE}" "${ENV_TARGET}"
        cat >&2 <<EOF

The env file did not exist, so a template was created:
  ${ENV_TARGET}

Edit it, then rerun:
  sudo deploy/systemd/install-systemd.sh --env-file ${ENV_TARGET}
EOF
        exit 1
    fi
}

validate_env_file() {
    local env_file="$1"
    local raw_line=""
    local line=""
    local key=""
    local value=""
    local line_no=0
    local topology="single-node"
    local node_role="all"
    local db_password=""
    local redis_password=""
    local database_url=""
    local redis_url=""
    local jwt_secret_key=""
    local encryption_key=""
    local payment_callback_secret=""
    local video_task_store_path=""
    local static_dir=""

    [[ -f "${env_file}" ]] || die "env file not found: ${env_file}"

    info "validating env file ${env_file}"
    while IFS= read -r raw_line || [[ -n "${raw_line}" ]]; do
        line_no=$((line_no + 1))
        line="${raw_line%$'\r'}"
        line="$(trim_whitespace "${line}")"

        [[ -z "${line}" ]] && continue
        [[ "${line:0:1}" == "#" ]] && continue

        [[ "${line}" == export\ * ]] && die "env file ${env_file}:${line_no} must not use 'export'"
        [[ "${line}" == *'${'* ]] && die "env file ${env_file}:${line_no} must not use variable expansion"
        [[ "${line}" == *'$('* ]] && die "env file ${env_file}:${line_no} must not use command substitution"
        [[ "${line}" == *'`'* ]] && die "env file ${env_file}:${line_no} must not use command substitution"
        [[ "${line}" =~ ^[A-Za-z_][A-Za-z0-9_]*= ]] || die "env file ${env_file}:${line_no} must be KEY=VALUE"

        key="${line%%=*}"
        value="${line#*=}"
        value="$(strip_optional_quotes "${value}")"

        case "${key}" in
            AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY)
                topology="${value}"
                ;;
            AETHER_GATEWAY_NODE_ROLE)
                node_role="${value}"
                ;;
            DATABASE_URL|AETHER_GATEWAY_DATA_POSTGRES_URL)
                [[ -n "${value}" ]] && database_url="${value}"
                ;;
            REDIS_URL|AETHER_GATEWAY_DATA_REDIS_URL)
                [[ -n "${value}" ]] && redis_url="${value}"
                ;;
            DB_PASSWORD)
                db_password="${value}"
                ;;
            REDIS_PASSWORD)
                redis_password="${value}"
                ;;
            JWT_SECRET_KEY)
                jwt_secret_key="${value}"
                ;;
            ENCRYPTION_KEY|AETHER_GATEWAY_DATA_ENCRYPTION_KEY)
                [[ -n "${value}" ]] && encryption_key="${value}"
                ;;
            PAYMENT_CALLBACK_SECRET)
                payment_callback_secret="${value}"
                ;;
            AETHER_GATEWAY_VIDEO_TASK_STORE_PATH)
                video_task_store_path="${value}"
                ;;
            AETHER_GATEWAY_STATIC_DIR)
                static_dir="${value}"
                ;;
        esac
    done < "${env_file}"

    case "${topology}" in
        single-node|multi-node)
            ;;
        *)
            die "AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY must be single-node or multi-node"
            ;;
    esac

    case "${node_role}" in
        all|frontdoor|background)
            ;;
        *)
            die "AETHER_GATEWAY_NODE_ROLE must be all, frontdoor, or background"
            ;;
    esac

    [[ -n "${jwt_secret_key}" ]] || die "JWT_SECRET_KEY is required"
    [[ -n "${encryption_key}" ]] || die "ENCRYPTION_KEY or AETHER_GATEWAY_DATA_ENCRYPTION_KEY is required"

    is_placeholder_value "${jwt_secret_key}" && die "JWT_SECRET_KEY still uses the example placeholder"
    is_placeholder_value "${encryption_key}" && die "ENCRYPTION_KEY still uses the example placeholder"
    if [[ -n "${database_url}" ]] && is_placeholder_value "${database_url}"; then
        die "DATABASE_URL still uses the example placeholder"
    fi
    if [[ -n "${redis_url}" ]] && is_placeholder_value "${redis_url}"; then
        die "REDIS_URL still uses the example placeholder"
    fi

    if [[ "${topology}" == "multi-node" ]]; then
        [[ "${node_role}" != "all" ]] || die "multi-node deployment requires AETHER_GATEWAY_NODE_ROLE=frontdoor or background"
        [[ -n "${database_url}" ]] || die "multi-node deployment requires DATABASE_URL or AETHER_GATEWAY_DATA_POSTGRES_URL"
        [[ -n "${redis_url}" ]] || die "multi-node deployment requires REDIS_URL or AETHER_GATEWAY_DATA_REDIS_URL"
        [[ -z "${video_task_store_path}" ]] || die "multi-node deployment must not set AETHER_GATEWAY_VIDEO_TASK_STORE_PATH"
    else
        if [[ "${node_role}" != "all" ]]; then
            warn "single-node deployment usually uses AETHER_GATEWAY_NODE_ROLE=all; split roles are only useful for cluster drills"
        fi
        if [[ -z "${database_url}" || -z "${redis_url}" ]]; then
            warn "single-node env is running in minimal mode without full Postgres/Redis persistence"
        fi
    fi

    if [[ -z "${payment_callback_secret}" ]]; then
        warn "PAYMENT_CALLBACK_SECRET is unset; public payment callback routes will stay disabled"
    elif is_placeholder_value "${payment_callback_secret}"; then
        warn "PAYMENT_CALLBACK_SECRET still uses the example placeholder"
    fi

    if is_placeholder_value "${db_password}"; then
        warn "DB_PASSWORD still uses the example placeholder"
    fi
    if is_placeholder_value "${redis_password}"; then
        warn "REDIS_PASSWORD still uses the example placeholder"
    fi

    if [[ -n "${static_dir}" && "${static_dir}" != "${INSTALL_ROOT}/current/frontend" ]]; then
        warn "AETHER_GATEWAY_STATIC_DIR points to ${static_dir}; install script still publishes frontend to ${INSTALL_ROOT}/current/frontend"
    fi
}

install_release() {
    local release_dir="${INSTALL_ROOT}/releases/${RELEASE_ID}"
    local current_link="${INSTALL_ROOT}/current"

    info "installing release ${RELEASE_ID} into ${release_dir}"
    install -d -m 0755 "${INSTALL_ROOT}" "${INSTALL_ROOT}/releases" "${INSTALL_ROOT}/shared"
    rm -rf "${release_dir}"
    install -d -m 0755 "${release_dir}/bin" "${release_dir}/frontend"
    install -m 0755 "${BIN_SOURCE}" "${release_dir}/bin/aether-gateway"
    cp -R "${FRONTEND_SOURCE}/." "${release_dir}/frontend/"
    chmod -R u=rwX,go=rX "${release_dir}"
    chown -R root:root "${release_dir}"
    ln -sfn "${release_dir}" "${current_link}"

    install -m 0644 "${DATA_COMPOSE_SOURCE}" "${INSTALL_ROOT}/shared/docker-compose.data.yml"
    install -d -o "${SERVICE_USER}" -g "${SERVICE_GROUP}" -m 0750 /var/lib/aether
}

install_systemd_unit() {
    local rendered_unit

    info "installing systemd unit to ${SYSTEMD_UNIT_PATH}"
    rendered_unit="$(mktemp)"
    sed \
        -e "s|__SERVICE_USER__|${SERVICE_USER}|g" \
        -e "s|__SERVICE_GROUP__|${SERVICE_GROUP}|g" \
        -e "s|__INSTALL_ROOT__|${INSTALL_ROOT}|g" \
        -e "s|__ENV_TARGET__|${ENV_TARGET}|g" \
        "${SERVICE_SOURCE}" > "${rendered_unit}"
    install -m 0644 "${rendered_unit}" "${SYSTEMD_UNIT_PATH}"
    rm -f "${rendered_unit}"
    systemctl daemon-reload
    systemctl enable "${SERVICE_NAME}" >/dev/null
}

restart_service_if_requested() {
    if [[ "${SKIP_START}" == "true" ]]; then
        info "skipping service restart"
        return
    fi

    info "restarting ${SERVICE_NAME}"
    systemctl restart "${SERVICE_NAME}"
}

print_next_steps() {
    local gateway_port
    gateway_port="$(awk -F= '/^[[:space:]]*APP_PORT=/{print $2}' "${ENV_TARGET}" | tail -n1 | tr -d '[:space:]')"
    gateway_port="${gateway_port:-8084}"

    cat <<EOF

Install complete.

Gateway service:
  sudo systemctl status ${SERVICE_NAME} --no-pager
  sudo journalctl -u ${SERVICE_NAME} -n 100 --no-pager
  sudo journalctl -u ${SERVICE_NAME} -f

Data services (Docker only for Postgres/Redis):
  docker compose --env-file ${ENV_TARGET} -f ${INSTALL_ROOT}/shared/docker-compose.data.yml up -d
  docker compose --env-file ${ENV_TARGET} -f ${INSTALL_ROOT}/shared/docker-compose.data.yml ps

Health checks:
  curl -fsS http://127.0.0.1:${gateway_port}/_gateway/health
  curl -fsS http://127.0.0.1:${gateway_port}/readyz

Database:
  empty database: first service start auto-bootstraps to the current baseline
  later schema upgrades: ${INSTALL_ROOT}/current/bin/aether-gateway --migrate

Current release:
  ${INSTALL_ROOT}/current
EOF
}

parse_args "$@"
require_root
require_linux_systemd
ensure_sources_exist
ensure_service_account
install_env_file
validate_env_file "${ENV_TARGET}"
install_release
install_systemd_unit
restart_service_if_requested
print_next_steps
