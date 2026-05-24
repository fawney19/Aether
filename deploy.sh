#!/bin/bash
# 智能部署脚本 - 自动检测代码变化并重建
#
# 用法:
#   部署/更新:     ./deploy.sh
#   强制全部重建:  ./deploy.sh --force

set -euo pipefail
cd "$(dirname "$0")"

LOCAL_APP_IMAGE="${LOCAL_APP_IMAGE:-aether-app:latest}"
export LOCAL_APP_IMAGE

detect_build_version() {
    if command -v git >/dev/null 2>&1; then
        local version
        if version=$(git describe --tags --always --dirty 2>/dev/null); then
            if [ -n "$version" ]; then
                printf '%s\n' "$version"
                return 0
            fi
        fi
    fi

    printf 'local-%s\n' "$(date -u +%Y%m%d%H%M%S)"
}

AETHER_BUILD_VERSION="${AETHER_BUILD_VERSION:-$(detect_build_version)}"
export AETHER_BUILD_VERSION

# 兼容 docker-compose 和 docker compose
if command -v docker-compose &> /dev/null; then
    DC=(docker-compose -f docker-compose.yml -f docker-compose.local.yml)
    USE_LEGACY_COMPOSE=true
else
    DC=(docker compose -f docker-compose.yml -f docker-compose.local.yml)
    USE_LEGACY_COMPOSE=false
fi

compose_up() {
    if [ "$USE_LEGACY_COMPOSE" = true ]; then
        "${DC[@]}" up -d --no-build "$@"
    else
        "${DC[@]}" up -d --no-build --pull missing "$@"
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

Environment:
  LOCAL_APP_IMAGE          本地构建镜像名，默认 aether-app:latest
  AETHER_BUILD_VERSION     应用显示版本，默认 git describe --tags --always --dirty
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

require_file() {
    if [ ! -f "$1" ]; then
        echo "Required file not found: $1"
        exit 1
    fi
}

hash_stream() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum | cut -d' ' -f1
    else
        shasum -a 256 | cut -d' ' -f1
    fi
}

emit_file_for_hash() {
    local path="$1"
    [ -f "$path" ] || return 0
    printf '\n>>> %s\n' "$path"
    cat "$path"
}

emit_tree_for_hash() {
    local root="$1"
    [ -d "$root" ] || return 0
    find "$root" -type f \
        ! -path '*/node_modules/*' \
        ! -path '*/target/*' \
        ! -path '*/dist/*' \
        ! -path '*/.mypy_cache/*' \
        ! -path '*/.vite/*' \
        2>/dev/null | sort | while IFS= read -r path; do
        emit_file_for_hash "$path"
    done
}

# 计算代码文件的哈希值
calc_code_hash() {
    {
        printf '\n>>> AETHER_BUILD_VERSION\n%s\n' "$AETHER_BUILD_VERSION"

        for file in \
            Dockerfile.app.local \
            docker-compose.yml \
            docker-compose.local.yml \
            .dockerignore \
            Cargo.toml \
            Cargo.lock \
            rust-toolchain.toml \
            frontend/package.json \
            frontend/package-lock.json \
            frontend/index.html \
            frontend/vite.config.ts \
            frontend/tsconfig.json \
            frontend/tsconfig.app.json \
            frontend/tsconfig.node.json \
            frontend/postcss.config.js \
            frontend/tailwind.config.js; do
            emit_file_for_hash "$file"
        done

        for dir in frontend/src frontend/public apps crates; do
            emit_tree_for_hash "$dir"
        done
    } | hash_stream
}

# 检查代码是否变化
check_code_changed() {
    local current_hash
    current_hash=$(calc_code_hash)
    if [ -f "$CODE_HASH_FILE" ]; then
        local saved_hash
        saved_hash=$(cat "$CODE_HASH_FILE")
        if [ "$current_hash" = "$saved_hash" ]; then
            return 1
        fi
    fi
    return 0
}

save_code_hash() { calc_code_hash > "$CODE_HASH_FILE"; }

# 构建应用镜像
build_app() {
    require_file Dockerfile.app.local
    echo ">>> Building app image: $LOCAL_APP_IMAGE"
    echo ">>> Build version: $AETHER_BUILD_VERSION"
    DOCKER_BUILDKIT="${DOCKER_BUILDKIT:-1}" docker build \
        --pull=false \
        --build-arg "AETHER_BUILD_VERSION=$AETHER_BUILD_VERSION" \
        -f Dockerfile.app.local \
        -t "$LOCAL_APP_IMAGE" \
        .
    save_code_hash
}

# 强制全部重建
if [ "$FORCE_REBUILD_ALL" = true ]; then
    echo ">>> Force rebuilding everything..."
    build_app
    compose_up --force-recreate
    docker image prune -f
    echo ">>> Done!"
    "${DC[@]}" ps
    exit 0
fi

# 标记是否需要重启
NEED_RESTART=false

# 检查代码是否变化
if ! docker image inspect "$LOCAL_APP_IMAGE" >/dev/null 2>&1; then
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
if [ -z "$("${DC[@]}" ps -q 2>/dev/null)" ]; then
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
echo ">>> Note: empty databases auto-bootstrap on first start."
echo ">>> Note: docker compose now defaults to auto-running pending migrations/backfills on app startup."
echo ">>> Note: set AETHER_GATEWAY_AUTO_PREPARE_DATABASE=false if you want to keep manual rollout."
"${DC[@]}" ps
