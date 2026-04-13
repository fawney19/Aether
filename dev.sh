#!/bin/bash
# 本地开发启动脚本
set -euo pipefail
clear >/dev/null 2>&1 || true

usage() {
    cat <<'EOF'
用法:
  ./dev.sh              启动本地 aether-gateway
  ./dev.sh --migrate    执行数据库迁移后退出
  ./dev.sh --help       显示帮助
EOF
}

RUN_MIGRATE_ONLY=false

while [ $# -gt 0 ]; do
    case "$1" in
        --migrate)
            RUN_MIGRATE_ONLY=true
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
STARTUP_WAIT_EARLY_EXIT=false

cleanup() {
    if [ -n "${GATEWAY_PID}" ]; then
        echo ""
        echo "=> 停止 aether-gateway..."
        kill "${GATEWAY_PID}" >/dev/null 2>&1 || true
        wait "${GATEWAY_PID}" >/dev/null 2>&1 || true
    fi
}

trap cleanup EXIT

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
            echo "=> 如果日志提示数据库 schema 落后，请先执行: ./dev.sh --migrate"
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
        echo "=> 如果日志提示数据库 schema 落后，请先执行: ./dev.sh --migrate"
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
RUST_SERVICE_STARTUP_TIMEOUT_SECONDS=${RUST_SERVICE_STARTUP_TIMEOUT_SECONDS:-120}
GATEWAY_STARTUP_TIMEOUT_SECONDS=${GATEWAY_STARTUP_TIMEOUT_SECONDS:-${RUST_SERVICE_STARTUP_TIMEOUT_SECONDS}}

if ! command -v cargo >/dev/null 2>&1; then
    echo "=> 未找到 cargo，无法启动 aether-gateway。请先安装 Rust toolchain。"
    exit 1
fi

export AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE=${AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE:-rust-authoritative}

if [ "${RUN_MIGRATE_ONLY}" = "true" ]; then
    if ! preflight_postgres_only; then
        exit 1
    fi

    echo "=> 执行 aether-gateway 数据库迁移..."
    cargo run -q -p aether-gateway -- --migrate
    exit 0
fi

if ! preflight_dev_infra; then
    exit 1
fi

GATEWAY_ARGS=(--app-port "${APP_PORT}")
echo "=> 启动 aether-gateway (Rust frontdoor: 0.0.0.0:${APP_PORT})..."
echo "=> 日志过滤: ${RUST_LOG} (需要更详细日志可用: RUST_LOG=aether_gateway=debug ./dev.sh)"
cargo run -q -p aether-gateway -- "${GATEWAY_ARGS[@]}" &
GATEWAY_PID=$!

if ! wait_for_startup "${GATEWAY_PID}" "${GATEWAY_STARTUP_TIMEOUT_SECONDS}" "aether-gateway" curl -sf "http://127.0.0.1:${APP_PORT}/_gateway/health"; then
    if [ "${STARTUP_WAIT_EARLY_EXIT}" = "true" ]; then
        GATEWAY_PID=""
    fi
    exit 1
fi

wait "${GATEWAY_PID}"
