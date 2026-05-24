SHELL := /bin/bash

DEV_RUST_LOG := info,executor::candidate_loop=debug,stream::execution=debug
ifeq ($(origin RUST_LOG), command line)
DEV_RUST_LOG := $(RUST_LOG)
endif
export DEV_RUST_LOG

.PHONY: dev dev-backend dev-frontend migration backfill

define DEV_BACKEND_SCRIPT
set -euo pipefail

if [ ! -f .env ]; then
	echo "=> 未找到 .env，请先执行: cp .env.example .env"
	exit 1
fi

set -a
source .env
set +a

dotenv_has_key() {
	local key="$$1"
	grep -Eq "^[[:space:]]*$${key}=" .env
}

lowercase() {
	printf '%s' "$$1" | tr '[:upper:]' '[:lower:]'
}

dev_uses_sqlite_database() {
	local driver
	local url
	driver="$$(lowercase "$${AETHER_DATABASE_DRIVER:-}")"
	url="$${AETHER_DATABASE_URL:-$${DATABASE_URL:-}}"

	[[ "$${driver}" == "sqlite" || "$${url}" == sqlite:* ]]
}

dev_uses_postgres_database() {
	local driver
	local url
	driver="$$(lowercase "$${AETHER_DATABASE_DRIVER:-}")"
	url="$${AETHER_DATABASE_URL:-$${DATABASE_URL:-}}"

	if [[ -z "$${driver}" && -z "$${url}" ]]; then
		return 0
	fi

	[[ "$${driver}" == "postgres" || "$${driver}" == "postgresql" || "$${url}" == postgres:* || "$${url}" == postgresql:* ]]
}

dev_uses_redis_runtime() {
	local backend
	backend="$$(lowercase "$${AETHER_RUNTIME_BACKEND:-}")"

	if [[ "$${backend}" == "memory" ]]; then
		return 1
	fi
	if [[ "$${backend}" == "redis" ]]; then
		return 0
	fi
	if dev_uses_sqlite_database; then
		return 1
	fi

	return 0
}

print_dev_infra_hint() {
	echo "=> 本地开发依赖未就绪。"
	echo "=> 可手动启动 Postgres / Redis:"
	echo "=>   docker compose up -d postgres redis"
}

check_postgres_ready() {
	local host="$$1"
	local port="$$2"

	if command -v pg_isready >/dev/null 2>&1; then
		pg_isready -h "$${host}" -p "$${port}" >/dev/null 2>&1
		return $$?
	fi

	if command -v nc >/dev/null 2>&1; then
		nc -z "$${host}" "$${port}" >/dev/null 2>&1
		return $$?
	fi

	return 0
}

check_redis_ready() {
	local host="$$1"
	local port="$$2"
	local password="$$3"

	if command -v redis-cli >/dev/null 2>&1; then
		REDISCLI_AUTH="$${password}" redis-cli -h "$${host}" -p "$${port}" ping >/dev/null 2>&1
		return $$?
	fi

	if command -v nc >/dev/null 2>&1; then
		nc -z "$${host}" "$${port}" >/dev/null 2>&1
		return $$?
	fi

	return 0
}

is_local_host() {
	case "$$1" in
		localhost|127.0.0.1|::1)
			return 0
			;;
	esac

	return 1
}

ensure_dev_infra() {
	local postgres_host="$${DB_HOST:-localhost}"
	local postgres_port="$${DB_PORT:-5432}"
	local redis_host="$${REDIS_HOST:-localhost}"
	local redis_port="$${REDIS_PORT:-6379}"
	local redis_password="$${REDIS_PASSWORD:-}"
	local need_postgres=false
	local need_redis=false
	local services=()

	if dev_uses_postgres_database; then
		if ! check_postgres_ready "$${postgres_host}" "$${postgres_port}"; then
			if is_local_host "$${postgres_host}"; then
				need_postgres=true
				services+=(postgres)
			else
				echo "=> PostgreSQL 不可用: $${postgres_host}:$${postgres_port}"
				print_dev_infra_hint
				return 1
			fi
		fi
	fi

	if dev_uses_redis_runtime; then
		if ! check_redis_ready "$${redis_host}" "$${redis_port}" "$${redis_password}"; then
			if is_local_host "$${redis_host}"; then
				need_redis=true
				services+=(redis)
			else
				echo "=> Redis 不可用: $${redis_host}:$${redis_port}"
				print_dev_infra_hint
				return 1
			fi
		fi
	fi

	if [ "$${#services[@]}" -eq 0 ]; then
		return 0
	fi

	if ! command -v docker >/dev/null 2>&1; then
		echo "=> 未找到 docker，无法自动启动本地开发依赖。"
		print_dev_infra_hint
		return 1
	fi

	echo "=> 本地开发依赖未就绪，正在启动: docker compose up -d $${services[*]}"
	if ! docker compose up -d "$${services[@]}"; then
		echo "=> docker compose 启动本地开发依赖失败。"
		print_dev_infra_hint
		return 1
	fi

	for _ in {1..100}; do
		local ready=true
		if [ "$${need_postgres}" = "true" ] && ! check_postgres_ready "$${postgres_host}" "$${postgres_port}"; then
			ready=false
		fi
		if [ "$${need_redis}" = "true" ] && ! check_redis_ready "$${redis_host}" "$${redis_port}" "$${redis_password}"; then
			ready=false
		fi
		if [ "$${ready}" = "true" ]; then
			return 0
		fi
		sleep 0.2
	done

	if [ "$${need_postgres}" = "true" ] && ! check_postgres_ready "$${postgres_host}" "$${postgres_port}"; then
		echo "=> PostgreSQL 不可用: $${postgres_host}:$${postgres_port}"
	fi
	if [ "$${need_redis}" = "true" ] && ! check_redis_ready "$${redis_host}" "$${redis_port}" "$${redis_password}"; then
		echo "=> Redis 不可用: $${redis_host}:$${redis_port}"
	fi
	print_dev_infra_hint
	return 1
}

print_startup_failure_hint() {
	local log_file="$$1"

	if [ -n "$${log_file}" ] && [ -f "$${log_file}" ]; then
		if grep -Eq "database schema is behind" "$${log_file}"; then
			echo "=> 检测到数据库 schema 落后，请执行: make migration"
			return
		fi

		if grep -Eq "database backfills are behind" "$${log_file}"; then
			echo "=> 检测到待执行 backfills，请执行: make backfill"
			return
		fi
	fi

	echo "=> 未识别到明确的修复动作，请根据上面的日志继续排查。"
}

wait_for_startup() {
	local pid="$$1"
	local timeout_seconds="$$2"
	local service_name="$$3"
	shift 3

	STARTUP_WAIT_EARLY_EXIT=false

	local attempts=$$((timeout_seconds * 10))
	if [ "$${attempts}" -lt 1 ]; then
		attempts=1
	fi

	for ((i = 0; i < attempts; i++)); do
		if "$$@" >/dev/null 2>&1; then
			return 0
		fi

		if ! kill -0 "$${pid}" >/dev/null 2>&1; then
			STARTUP_WAIT_EARLY_EXIT=true
			echo "=> $${service_name} 启动进程已提前退出，请检查上面的日志。"
			print_startup_failure_hint "$${GATEWAY_LOG_FILE}"
			return 1
		fi

		sleep 0.1
	done

	if "$$@" >/dev/null 2>&1; then
		return 0
	fi

	if ! kill -0 "$${pid}" >/dev/null 2>&1; then
		STARTUP_WAIT_EARLY_EXIT=true
		echo "=> $${service_name} 启动进程已提前退出，请检查上面的日志。"
		print_startup_failure_hint "$${GATEWAY_LOG_FILE}"
		return 1
	fi

	echo "=> $${service_name} 在 $${timeout_seconds}s 内未通过启动检查。"
	echo "=> 如果这是冷编译或存在并发 cargo 构建，可调大启动超时后重试。"
	return 1
}

create_gateway_log_file() {
	local tmp_root="$${TMPDIR:-/tmp}"
	tmp_root="$${tmp_root%/}"

	GATEWAY_LOG_DIR="$$(mktemp -d "$${tmp_root}/aether-dev-startup.XXXXXX")"
	GATEWAY_LOG_FILE="$${GATEWAY_LOG_DIR}/gateway.log"
	: > "$${GATEWAY_LOG_FILE}"
}

cleanup() {
	local status="$${1:-0}"
	trap - INT TERM EXIT

	if [ -n "$${GATEWAY_PID:-}" ]; then
		echo ""
		echo "=> 停止 aether-gateway..."
		kill "$${GATEWAY_PID}" >/dev/null 2>&1 || true
		wait "$${GATEWAY_PID}" >/dev/null 2>&1 || true
	fi

	if [ -n "$${GATEWAY_LOG_FILE:-}" ] && [ -f "$${GATEWAY_LOG_FILE}" ]; then
		rm -f "$${GATEWAY_LOG_FILE}"
	fi

	if [ -n "$${GATEWAY_LOG_DIR:-}" ] && [ -d "$${GATEWAY_LOG_DIR}" ]; then
		rmdir "$${GATEWAY_LOG_DIR}" >/dev/null 2>&1 || true
	fi

	exit "$${status}"
}

trap 'cleanup 130' INT
trap 'cleanup 143' TERM
trap 'cleanup $$?' EXIT

export APP_PORT="$${APP_PORT:-8084}"
export RUST_LOG="$${DEV_RUST_LOG}"
RUST_SERVICE_STARTUP_TIMEOUT_SECONDS="$${RUST_SERVICE_STARTUP_TIMEOUT_SECONDS:-180}"
GATEWAY_STARTUP_TIMEOUT_SECONDS="$${GATEWAY_STARTUP_TIMEOUT_SECONDS:-$${RUST_SERVICE_STARTUP_TIMEOUT_SECONDS}}"
export AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE="$${AETHER_GATEWAY_VIDEO_TASK_TRUTH_SOURCE_MODE:-rust-authoritative}"

if dev_uses_postgres_database; then
	export DATABASE_URL="postgresql://$${DB_USER:-postgres}:$${DB_PASSWORD:-}@$${DB_HOST:-localhost}:$${DB_PORT:-5432}/$${DB_NAME:-aether}"
	if ! dotenv_has_key "AETHER_GATEWAY_DATA_POSTGRES_URL"; then
		export AETHER_GATEWAY_DATA_POSTGRES_URL="$${DATABASE_URL}"
	fi
fi

if dev_uses_redis_runtime; then
	export REDIS_URL="redis://:$${REDIS_PASSWORD:-}@$${REDIS_HOST:-localhost}:$${REDIS_PORT:-6379}/0"
	if ! dotenv_has_key "AETHER_GATEWAY_DATA_REDIS_URL"; then
		export AETHER_GATEWAY_DATA_REDIS_URL="$${REDIS_URL}"
	fi
else
	unset REDIS_URL
	unset AETHER_GATEWAY_DATA_REDIS_URL
fi

if ! dotenv_has_key "AETHER_GATEWAY_DATA_ENCRYPTION_KEY"; then
	export AETHER_GATEWAY_DATA_ENCRYPTION_KEY="$${ENCRYPTION_KEY:-}"
fi

export DB_POOL_SIZE="$${DB_POOL_SIZE:-5}"
export DB_MAX_OVERFLOW="$${DB_MAX_OVERFLOW:-5}"
export HTTP_MAX_CONNECTIONS="$${HTTP_MAX_CONNECTIONS:-20}"
export HTTP_KEEPALIVE_CONNECTIONS="$${HTTP_KEEPALIVE_CONNECTIONS:-5}"

if ! command -v cargo >/dev/null 2>&1; then
	echo "=> 未找到 cargo，无法启动 aether-gateway。请先安装 Rust toolchain。"
	exit 1
fi

if ! command -v curl >/dev/null 2>&1; then
	echo "=> 未找到 curl，无法检查 aether-gateway 健康状态。请先安装 curl。"
	exit 1
fi

if [ -z "$${RUSTC_WRAPPER:-}" ] && command -v sccache >/dev/null 2>&1; then
	export RUSTC_WRAPPER="$$(command -v sccache)"
	echo "=> 启用 Rust 编译缓存: $${RUSTC_WRAPPER}"
fi

if ! ensure_dev_infra; then
	exit 1
fi

GATEWAY_PID=""
GATEWAY_LOG_DIR=""
GATEWAY_LOG_FILE=""
STARTUP_WAIT_EARLY_EXIT=false
create_gateway_log_file

echo "=> 启动 aether-gateway (Rust frontdoor: 0.0.0.0:$${APP_PORT})..."
echo "=> 日志过滤: $${RUST_LOG}"
echo "=> 执行命令: cargo run -p aether-gateway -- --app-port $${APP_PORT}"
cargo run -p aether-gateway -- --app-port "$${APP_PORT}" > >(
	tee -a "$${GATEWAY_LOG_FILE}"
) 2>&1 &
GATEWAY_PID=$$!

if ! wait_for_startup "$${GATEWAY_PID}" "$${GATEWAY_STARTUP_TIMEOUT_SECONDS}" "aether-gateway" curl -sf "http://127.0.0.1:$${APP_PORT}/_gateway/health"; then
	if [ "$${STARTUP_WAIT_EARLY_EXIT}" = "true" ]; then
		GATEWAY_PID=""
	fi
	exit 1
fi

if wait "$${GATEWAY_PID}"; then
	gateway_exit_code=0
else
	gateway_exit_code=$$?
fi

GATEWAY_PID=""

if [ "$${gateway_exit_code}" -ne 130 ] && [ "$${gateway_exit_code}" -ne 143 ]; then
	echo "=> aether-gateway 运行失败并已退出，请检查上面的日志。"
	print_startup_failure_hint "$${GATEWAY_LOG_FILE}"
fi

exit "$${gateway_exit_code}"
endef
export DEV_BACKEND_SCRIPT

define DEV_SCRIPT
set -euo pipefail

backend_pid=""
frontend_pid=""

cleanup() {
	local status="$${1:-0}"
	trap - INT TERM EXIT

	if [ -n "$${backend_pid}" ] || [ -n "$${frontend_pid}" ]; then
		echo ""
		echo "=> 停止本地开发服务..."
		if [ -n "$${backend_pid}" ]; then
			kill "$${backend_pid}" >/dev/null 2>&1 || true
			wait "$${backend_pid}" >/dev/null 2>&1 || true
		fi
		if [ -n "$${frontend_pid}" ]; then
			kill "$${frontend_pid}" >/dev/null 2>&1 || true
			wait "$${frontend_pid}" >/dev/null 2>&1 || true
		fi
	fi

	exit "$${status}"
}

wait_for_backend_ready() {
	while :; do
		if curl -sf "http://127.0.0.1:$${APP_PORT}/_gateway/health" >/dev/null 2>&1; then
			return 0
		fi

		if ! kill -0 "$${backend_pid}" >/dev/null 2>&1; then
			if wait "$${backend_pid}"; then
				status=0
			else
				status=$$?
			fi
			if [ "$${status}" -ne 0 ]; then
				echo "=> 后端进程已退出 (status $${status})"
			else
				echo "=> 后端进程已退出"
			fi
			backend_pid=""
			cleanup "$${status}"
		fi

		sleep 0.2
	done
}

trap 'cleanup 130' INT
trap 'cleanup 143' TERM
trap 'cleanup $$?' EXIT

if [ -f .env ]; then
	set -a
	source .env
	set +a
fi
export APP_PORT="$${APP_PORT:-8084}"

echo "=> 启动后端: RUST_LOG=$${DEV_RUST_LOG} cargo run -p aether-gateway -- --app-port $${APP_PORT:-8084}"
/bin/bash -euo pipefail -c "$$DEV_BACKEND_SCRIPT" &
backend_pid=$$!

echo "=> 等待后端健康检查: http://127.0.0.1:$${APP_PORT}/_gateway/health"
wait_for_backend_ready

echo "=> 启动前端: cd frontend && npm run dev"
( cd frontend && exec npm run dev ) &
frontend_pid=$$!

while :; do
	if ! kill -0 "$${backend_pid}" >/dev/null 2>&1; then
		if wait "$${backend_pid}"; then
			status=0
		else
			status=$$?
		fi
		if [ "$${status}" -ne 0 ]; then
			echo "=> 后端进程已退出 (status $${status})"
		else
			echo "=> 后端进程已退出"
		fi
		backend_pid=""
		cleanup "$${status}"
	fi

	if ! kill -0 "$${frontend_pid}" >/dev/null 2>&1; then
		if wait "$${frontend_pid}"; then
			status=0
		else
			status=$$?
		fi
		if [ "$${status}" -ne 0 ]; then
			echo "=> 前端进程已退出 (status $${status})"
		else
			echo "=> 前端进程已退出"
		fi
		frontend_pid=""
		cleanup "$${status}"
	fi

	sleep 1
done
endef
export DEV_SCRIPT

define DB_TASK_SCRIPT
set -euo pipefail

if [ -z "$${DB_TASK_FLAG:-}" ] || [ -z "$${DB_TASK_LABEL:-}" ]; then
	echo "=> 内部错误: DB_TASK_FLAG / DB_TASK_LABEL 未设置"
	exit 1
fi

if [ ! -f .env ]; then
	echo "=> 未找到 .env，请先执行: cp .env.example .env"
	exit 1
fi

set -a
source .env
set +a

dotenv_has_key() {
	local key="$$1"
	grep -Eq "^[[:space:]]*$${key}=" .env
}

lowercase() {
	printf '%s' "$$1" | tr '[:upper:]' '[:lower:]'
}

uses_postgres_database() {
	local driver
	local url
	driver="$$(lowercase "$${AETHER_DATABASE_DRIVER:-}")"
	url="$${AETHER_DATABASE_URL:-$${DATABASE_URL:-}}"

	if [[ -z "$${driver}" && -z "$${url}" ]]; then
		return 0
	fi

	[[ "$${driver}" == "postgres" || "$${driver}" == "postgresql" || "$${url}" == postgres:* || "$${url}" == postgresql:* ]]
}

if uses_postgres_database; then
	export DATABASE_URL="postgresql://$${DB_USER:-postgres}:$${DB_PASSWORD:-}@$${DB_HOST:-localhost}:$${DB_PORT:-5432}/$${DB_NAME:-aether}"
	if ! dotenv_has_key "AETHER_GATEWAY_DATA_POSTGRES_URL"; then
		export AETHER_GATEWAY_DATA_POSTGRES_URL="$${DATABASE_URL}"
	fi
fi

if ! dotenv_has_key "AETHER_GATEWAY_DATA_ENCRYPTION_KEY"; then
	export AETHER_GATEWAY_DATA_ENCRYPTION_KEY="$${ENCRYPTION_KEY:-}"
fi

if ! command -v cargo >/dev/null 2>&1; then
	echo "=> 未找到 cargo，无法执行 $${DB_TASK_LABEL}。请先安装 Rust toolchain。"
	exit 1
fi

echo "=> 执行 $${DB_TASK_LABEL}: cargo run -p aether-gateway -- $${DB_TASK_FLAG}"
exec cargo run -p aether-gateway -- "$${DB_TASK_FLAG}"
endef
export DB_TASK_SCRIPT

dev:
	@$(SHELL) -euo pipefail -c "$$DEV_SCRIPT"

dev-backend:
	@$(SHELL) -euo pipefail -c "$$DEV_BACKEND_SCRIPT"

dev-frontend:
	@cd frontend && npm run dev

migration:
	@DB_TASK_FLAG=--migrate DB_TASK_LABEL="数据库迁移" $(SHELL) -euo pipefail -c "$$DB_TASK_SCRIPT"

backfill:
	@DB_TASK_FLAG=--apply-backfills DB_TASK_LABEL="数据库 backfill" $(SHELL) -euo pipefail -c "$$DB_TASK_SCRIPT"
