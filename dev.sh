#!/bin/bash
# 本地开发启动脚本
set -euo pipefail
clear >/dev/null 2>&1 || true

usage() {
    cat <<'EOF'
用法:
  ./dev.sh              启动本地 aether-gateway
  ./dev.sh --migrate    执行数据库迁移后退出
  ./dev.sh --apply-backfills
                       执行数据库回填后退出
  ./dev.sh --help       显示帮助
EOF
}

RUN_MIGRATE_ONLY=false
RUN_APPLY_BACKFILLS_ONLY=false

while [ $# -gt 0 ]; do
    case "$1" in
        --migrate)
            RUN_MIGRATE_ONLY=true
            shift
            ;;
        --apply-backfills)
            RUN_APPLY_BACKFILLS_ONLY=true
            shift
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "=> 未知参数: $1"
            usage
            exit 1
            ;;
    esac
done

# 加载 .env 文件
set -a
source .env
set +a

dotenv_has_key() {
    local key="$1"
    rg -q "^[[:space:]]*${key}=" .env
}

print_dev_infra_hint() {
    echo "=> 本地开发依赖未就绪。"
    echo "=> 请先启动 Postgres / Redis:"
    echo "=>   docker compose -f docker-compose.build.yml up -d postgres redis"
}

print_postgres_hint() {
    echo "=> PostgreSQL 未就绪。"
    echo "=> 请先启动 Postgres:"
    echo "=>   docker compose -f docker-compose.build.yml up -d postgres"
}

check_postgres_ready() {
    local host="$1"
    local port="$2"

    if command -v pg_isready >/dev/null 2>&1; then
        pg_isready -h "${host}" -p "${port}" >/dev/null 2>&1
        return $?
    fi

    if command -v nc >/dev/null 2>&1; then
        nc -z "${host}" "${port}" >/dev/null 2>&1
        return $?
    fi

    return 0
}

check_redis_ready() {
    local host="$1"
    local port="$2"
    local password="$3"

    if command -v redis-cli >/dev/null 2>&1; then
        REDISCLI_AUTH="${password}" redis-cli -h "${host}" -p "${port}" ping >/dev/null 2>&1
        return $?
    fi

    if command -v nc >/dev/null 2>&1; then
        nc -z "${host}" "${port}" >/dev/null 2>&1
        return $?
    fi

    return 0
}

preflight_dev_infra() {
    local postgres_host="${DB_HOST:-localhost}"
    local postgres_port="${DB_PORT:-5432}"
    local redis_host="${REDIS_HOST:-localhost}"
    local redis_port="${REDIS_PORT:-6379}"
    local redis_password="${REDIS_PASSWORD:-}"

    if ! check_postgres_ready "${postgres_host}" "${postgres_port}"; then
        echo "=> PostgreSQL 不可用: ${postgres_host}:${postgres_port}"
        print_dev_infra_hint
        return 1
    fi

    if ! check_redis_ready "${redis_host}" "${redis_port}" "${redis_password}"; then
        echo "=> Redis 不可用: ${redis_host}:${redis_port}"
        print_dev_infra_hint
        return 1
    fi

    return 0
}

preflight_postgres_only() {
    local postgres_host="${DB_HOST:-localhost}"
    local postgres_port="${DB_PORT:-5432}"

    if ! check_postgres_ready "${postgres_host}" "${postgres_port}"; then
        echo "=> PostgreSQL 不可用: ${postgres_host}:${postgres_port}"
        print_postgres_hint
        return 1
    fi

    return 0
}

# 构建 DATABASE_URL
export DATABASE_URL="postgresql://${DB_USER:-postgres}:${DB_PASSWORD}@${DB_HOST:-localhost}:${DB_PORT:-5432}/${DB_NAME:-aether}"
export REDIS_URL=redis://:${REDIS_PASSWORD}@${REDIS_HOST:-localhost}:${REDIS_PORT:-6379}/0

if ! dotenv_has_key "AETHER_GATEWAY_DATA_POSTGRES_URL"; then
    export AETHER_GATEWAY_DATA_POSTGRES_URL="${DATABASE_URL}"
fi
if ! dotenv_has_key "AETHER_GATEWAY_DATA_REDIS_URL"; then
    export AETHER_GATEWAY_DATA_REDIS_URL="${REDIS_URL}"
fi
if ! dotenv_has_key "AETHER_GATEWAY_DATA_ENCRYPTION_KEY"; then
    export AETHER_GATEWAY_DATA_ENCRYPTION_KEY="${ENCRYPTION_KEY:-}"
fi

# 开发环境连接池低配（节省内存）
export DB_POOL_SIZE=${DB_POOL_SIZE:-5}
export DB_MAX_OVERFLOW=${DB_MAX_OVERFLOW:-5}
export HTTP_MAX_CONNECTIONS=${HTTP_MAX_CONNECTIONS:-20}
export HTTP_KEEPALIVE_CONNECTIONS=${HTTP_KEEPALIVE_CONNECTIONS:-5}

GATEWAY_PID=""
GATEWAY_LOG_DIR=""
GATEWAY_LOG_FILE=""
STARTUP_WAIT_EARLY_EXIT=false

cleanup() {
    if [ -n "${GATEWAY_PID}" ]; then
        echo ""
        echo "=> 停止 aether-gateway..."
        kill "${GATEWAY_PID}" >/dev/null 2>&1 || true
        wait "${GATEWAY_PID}" >/dev/null 2>&1 || true
    fi
    if [ -n "${GATEWAY_LOG_FILE}" ] && [ -f "${GATEWAY_LOG_FILE}" ]; then
        rm -f "${GATEWAY_LOG_FILE}"
    fi
    if [ -n "${GATEWAY_LOG_DIR}" ] && [ -d "${GATEWAY_LOG_DIR}" ]; then
        rmdir "${GATEWAY_LOG_DIR}" >/dev/null 2>&1 || true
    fi
}

trap cleanup EXIT

create_gateway_log_file() {
    local tmp_root="${TMPDIR:-/tmp}"
    tmp_root="${tmp_root%/}"

    GATEWAY_LOG_DIR="$(mktemp -d "${tmp_root}/aether-dev-startup.XXXXXX")"
    GATEWAY_LOG_FILE="${GATEWAY_LOG_DIR}/gateway.log"
    : > "${GATEWAY_LOG_FILE}"
}

print_startup_failure_hint() {
    local log_file="$1"

    if [ -n "${log_file}" ] && [ -f "${log_file}" ]; then
        if rg --text -q "database schema is behind|run \`aether-gateway --migrate\` before starting the service" "${log_file}"; then
            echo "=> 检测到数据库 schema 落后，请先执行: ./dev.sh --migrate"
            return
        fi

        if rg --text -q "database backfills are behind|run \`aether-gateway --apply-backfills\` before starting the service" "${log_file}"; then
            echo "=> 检测到待执行 backfills，请执行: ./dev.sh --apply-backfills"
            return
        fi
    fi

    echo "=> 未识别到明确的修复动作，请根据上面的日志继续排查。"
}

wait_for_startup() {
    local pid="$1"
    local timeout_seconds="$2"
    local service_name="$3"
    shift 3

    STARTUP_WAIT_EARLY_EXIT=false

    local attempts=$((timeout_seconds * 10))
    if [ "${attempts}" -lt 1 ]; then
        attempts=1
    fi

    for ((i = 0; i < attempts; i++)); do
        if "$@" >/dev/null 2>&1; then
            return 0
        fi

        if ! kill -0 "${pid}" >/dev/null 2>&1; then
            STARTUP_WAIT_EARLY_EXIT=true
            echo "=> ${service_name} 启动进程已提前退出，请检查上面的日志。"
            print_startup_failure_hint "${GATEWAY_LOG_FILE}"
            return 1
        fi

        sleep 0.1
    done

    if "$@" >/dev/null 2>&1; then
        return 0
    fi

    if ! kill -0 "${pid}" >/dev/null 2>&1; then
        STARTUP_WAIT_EARLY_EXIT=true
        echo "=> ${service_name} 启动进程已提前退出，请检查上面的日志。"
        print_startup_failure_hint "${GATEWAY_LOG_FILE}"
        return 1
    fi

    echo "=> ${service_name} 在 ${timeout_seconds}s 内未通过启动检查。"
    echo "=> 如果这是冷编译或存在并发 cargo 构建，可调大 *_STARTUP_TIMEOUT_SECONDS 后重试。"
    return 1
}

# 本地开发默认约定：
# - Rust aether-gateway 绑定 APP_PORT，作为唯一公开入口
# - ./dev.sh 不再启动 Python 宿主；本地默认只验证 Rust-owned 路径
export APP_PORT=${APP_PORT:-8084}
export RUST_LOG=${RUST_LOG:-aether_gateway=info}
RUST_SERVICE_STARTUP_TIMEOUT_SECONDS=${RUST_SERVICE_STARTUP_TIMEOUT_SECONDS:-180}
GATEWAY_STARTUP_TIMEOUT_SECONDS=${GATEWAY_STARTUP_TIMEOUT_SECONDS:-${RUST_SERVICE_STARTUP_TIMEOUT_SECONDS}}

if ! command -v cargo >/dev/null 2>&1; then
    echo "=> 未找到 cargo，无法启动 aether-gateway。请先安装 Rust toolchain。"
    exit 1
fi

export AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE=${AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE:-rust-authoritative}

if [ "${RUN_MIGRATE_ONLY}" = "true" ] && [ "${RUN_APPLY_BACKFILLS_ONLY}" = "true" ]; then
    if ! preflight_postgres_only; then
        exit 1
    fi

    echo "=> 执行 aether-gateway 数据库迁移..."
    cargo run -q -p aether-gateway -- --migrate
    echo "=> 执行 aether-gateway 数据库回填..."
    cargo run -q -p aether-gateway -- --apply-backfills
    exit 0
fi

if [ "${RUN_MIGRATE_ONLY}" = "true" ]; then
    if ! preflight_postgres_only; then
        exit 1
    fi

    echo "=> 执行 aether-gateway 数据库迁移..."
    cargo run -q -p aether-gateway -- --migrate
    exit 0
fi

if [ "${RUN_APPLY_BACKFILLS_ONLY}" = "true" ]; then
    if ! preflight_postgres_only; then
        exit 1
    fi

    echo "=> 执行 aether-gateway 数据库回填..."
    cargo run -q -p aether-gateway -- --apply-backfills
    exit 0
fi

if ! preflight_dev_infra; then
    exit 1
fi

GATEWAY_ARGS=(--app-port "${APP_PORT}")
create_gateway_log_file
echo "=> 启动 aether-gateway (Rust frontdoor: 0.0.0.0:${APP_PORT})..."
echo "=> 日志过滤: ${RUST_LOG} (需要更详细日志可用: RUST_LOG=aether_gateway=debug ./dev.sh)"
echo "=> 执行命令: cargo run -p aether-gateway -- ${GATEWAY_ARGS[*]}"
cargo run -p aether-gateway -- "${GATEWAY_ARGS[@]}" > >(tee -a "${GATEWAY_LOG_FILE}") 2>&1 &
GATEWAY_PID=$!

if ! wait_for_startup "${GATEWAY_PID}" "${GATEWAY_STARTUP_TIMEOUT_SECONDS}" "aether-gateway" curl -sf "http://127.0.0.1:${APP_PORT}/_gateway/health"; then
    if [ "${STARTUP_WAIT_EARLY_EXIT}" = "true" ]; then
        GATEWAY_PID=""
    fi
    exit 1
fi

if wait "${GATEWAY_PID}"; then
    exit 0
else
    gateway_exit_code=$?
fi

if [ "${gateway_exit_code}" -ne 130 ] && [ "${gateway_exit_code}" -ne 143 ]; then
    echo "=> aether-gateway 运行失败并已退出，请检查上面的日志。"
    print_startup_failure_hint "${GATEWAY_LOG_FILE}"
fi
exit "${gateway_exit_code}"
