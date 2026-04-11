#!/bin/bash
# 智能部署脚本 - 自动检测代码变化并重建
#
# 用法:
#   部署/更新:     ./deploy.sh
#   强制全部重建:  ./deploy.sh --force

set -euo pipefail
cd "$(dirname "$0")"

# 兼容 docker-compose 和 docker compose
if command -v docker-compose &> /dev/null; then
    DC="docker-compose -f docker-compose.build.yml"
    USE_LEGACY_COMPOSE=true
else
    DC="docker compose -f docker-compose.build.yml"
    USE_LEGACY_COMPOSE=false
fi

compose_up() {
    if [ "$USE_LEGACY_COMPOSE" = true ]; then
        $DC up -d --no-build "$@"
    else
        $DC up -d --no-build --pull never "$@"
    fi
}

# 缓存文件
CODE_HASH_FILE=".code-hash"

usage() {
    cat <<'EOF'
Usage: ./deploy.sh [options]

Options:
  --force, -f             强制重建并重启
  -h, --help              显示帮助
EOF
}

FORCE_REBUILD_ALL=false

while [ $# -gt 0 ]; do
    case "$1" in
        --force|-f)
            FORCE_REBUILD_ALL=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1"
            usage
            exit 1
            ;;
    esac
done

# 计算代码文件的哈希值
calc_code_hash() {
    {
        cat Dockerfile.app.local 2>/dev/null
        cat Cargo.toml Cargo.lock 2>/dev/null
        find frontend/src -type f \( -name "*.vue" -o -name "*.ts" -o -name "*.tsx" -o -name "*.js" \) 2>/dev/null | sort | xargs cat 2>/dev/null
        find apps -type f \( -name "*.rs" -o -name "Cargo.toml" \) 2>/dev/null | sort | xargs cat 2>/dev/null
        find crates -type f \( -name "*.rs" -o -name "*.sql" -o -name "Cargo.toml" \) 2>/dev/null | sort | xargs cat 2>/dev/null
    } | md5sum | cut -d' ' -f1
}

# 检查代码是否变化
check_code_changed() {
    local current_hash=$(calc_code_hash)
    if [ -f "$CODE_HASH_FILE" ]; then
        local saved_hash=$(cat "$CODE_HASH_FILE")
        if [ "$current_hash" = "$saved_hash" ]; then
            return 1
        fi
    fi
    return 0
}

save_code_hash() { calc_code_hash > "$CODE_HASH_FILE"; }

# 构建应用镜像
build_app() {
    echo ">>> Building app image (rust gateway + frontend)..."
    docker build --pull=false -f Dockerfile.app.local -t aether-app:latest .
    save_code_hash
}

# 强制全部重建
if [ "$FORCE_REBUILD_ALL" = true ]; then
    echo ">>> Force rebuilding everything..."
    build_app
    compose_up --force-recreate
    docker image prune -f
    echo ">>> Done!"
    $DC ps
    exit 0
fi

# 标记是否需要重启
NEED_RESTART=false

# 检查代码是否变化
if ! docker image inspect aether-app:latest >/dev/null 2>&1; then
    echo ">>> App image not found, building..."
    build_app
    NEED_RESTART=true
elif check_code_changed; then
    echo ">>> Code changed, rebuilding app image..."
    build_app
    NEED_RESTART=true
else
    echo ">>> Code unchanged."
fi

# 检查容器是否在运行
CONTAINERS_RUNNING=true
if [ -z "$($DC ps -q 2>/dev/null)" ]; then
    CONTAINERS_RUNNING=false
fi

# 有变化时重启，或容器未运行时启动
if [ "$NEED_RESTART" = true ]; then
    echo ">>> Restarting services..."
    compose_up
elif [ "$CONTAINERS_RUNNING" = false ]; then
    echo ">>> Containers not running, starting services..."
    compose_up
else
    echo ">>> No changes detected, skipping restart."
fi

# 清理
docker image prune -f >/dev/null 2>&1 || true

echo ">>> Done!"
$DC ps
