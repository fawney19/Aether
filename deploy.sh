#!/bin/bash
# 智能部署脚本 - 自动检测依赖/代码/迁移变化
#
# 用法:
#   部署/更新:     ./deploy.sh                    (自动检测所有变化)
#   指定 Hub 版本: ./deploy.sh --hub-tag hub-v0.1.0
#   更新 Hub:      ./deploy.sh --update-hub
#   GitHub 镜像:   ./deploy.sh --mirror https://ghfast.top
#   强制重建:      ./deploy.sh --rebuild-base
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
HASH_FILE=".deps-hash"
CODE_HASH_FILE=".code-hash"
MIGRATION_HASH_FILE=".migration-hash"

# Hub release 配置
GITHUB_REPO="fawney19/Aether"
HUB_TAG_STATE_FILE=".hub-tag"

usage() {
    cat <<'EOF'
Usage: ./deploy.sh [options]

Options:
  --hub-tag <hub-vX.Y.Z>  指定 Hub Release tag（例如 hub-v0.1.0）
  --update-hub            强制刷新 Hub 版本标记（下次构建会重新下载）
  --mirror <url>          GitHub 下载镜像（例如 https://ghfast.top）
  --rebuild-base, -r      仅重建 base 镜像
  --force, -f             强制重建全部（hub/base/app）并重启
  -h, --help              显示帮助
EOF
}

FORCE_REBUILD_ALL=false
REBUILD_BASE_ONLY=false
FORCE_UPDATE_HUB=false
HUB_TAG="${HUB_TAG:-}"
GITHUB_MIRROR="${GITHUB_MIRROR:-}"
RESOLVED_HUB_TAG=""

while [ $# -gt 0 ]; do
    case "$1" in
        --hub-tag)
            if [ $# -lt 2 ]; then
                echo "❌ --hub-tag 需要一个值，例如 hub-v0.1.0"
                exit 1
            fi
            HUB_TAG="$2"
            shift 2
            ;;
        --update-hub)
            FORCE_UPDATE_HUB=true
            shift
            ;;
        --mirror)
            if [ $# -lt 2 ]; then
                echo "ERROR: --mirror needs a URL, e.g. https://ghfast.top"
                exit 1
            fi
            GITHUB_MIRROR="$2"
            shift 2
            ;;
        --rebuild-base|-r)
            REBUILD_BASE_ONLY=true
            shift
            ;;
        --force|-f)
            FORCE_REBUILD_ALL=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "❌ 未知参数: $1"
            usage
            exit 1
            ;;
    esac
done

if [ -n "$HUB_TAG" ]; then
    case "$HUB_TAG" in
        hub-v*) ;;
        *) echo "❌ --hub-tag 格式应为 hub-vX.Y.Z，例如 hub-v0.1.0"; exit 1 ;;
    esac
fi

# 提取 pyproject.toml 中"会影响运行时依赖安装"的最小指纹（与 CI 保持一致）：
# - [build-system] requires / build-backend
# - [project] requires-python / dependencies
# 使用 Python tomllib 解析，不受 TOML 格式变化影响。
pyproject_deps_fingerprint() {
    python3 - <<'PY'
import json, pathlib, tomllib

data = tomllib.loads(pathlib.Path("pyproject.toml").read_text("utf-8"))
project = data.get("project") or {}
build = data.get("build-system") or {}

fingerprint = {
    "requires-python": project.get("requires-python"),
    "dependencies": sorted(project.get("dependencies") or []),
    "build-backend": build.get("build-backend"),
    "build-requires": sorted(build.get("requires") or []),
}

print(json.dumps(fingerprint, sort_keys=True, separators=(",", ":")))
PY
}

# 计算依赖文件的哈希值（包含 Dockerfile.base.local）
calc_deps_hash() {
    {
        cat Dockerfile.base.local 2>/dev/null
        pyproject_deps_fingerprint
        # 前端依赖以 lock 为准（避免仅改 scripts/version 触发 base 重建）
        cat frontend/package-lock.json 2>/dev/null
    } | md5sum | cut -d' ' -f1
}

# 计算代码文件的哈希值（包含 Dockerfile.app.local）
calc_code_hash() {
    {
        cat Dockerfile.app.local 2>/dev/null
        find src -type f -name "*.py" 2>/dev/null | sort | xargs cat 2>/dev/null
        find frontend/src -type f \( -name "*.vue" -o -name "*.ts" -o -name "*.tsx" -o -name "*.js" \) 2>/dev/null | sort | xargs cat 2>/dev/null
    } | md5sum | cut -d' ' -f1
}

# 获取最新 hub release tag
# 支持 GITHUB_TOKEN 环境变量以避免未认证 API 限流（60 次/小时 -> 5000 次/小时）
get_latest_hub_tag() {
    local auth_args=()
    if [ -n "${GITHUB_TOKEN:-}" ]; then
        auth_args=(-H "Authorization: token ${GITHUB_TOKEN}")
    fi
    curl -sL "${auth_args[@]}" "https://api.github.com/repos/$GITHUB_REPO/releases" | \
        python3 -c "
import json, sys
releases = json.load(sys.stdin)
for r in releases:
    tag = r.get('tag_name', '')
    if tag.startswith('hub-v') and not r.get('draft') and not r.get('prerelease'):
        print(tag)
        break
" 2>/dev/null
}

# 解析当前应使用的 Hub release tag（优先使用指定值，否则拉取最新）
resolve_hub_tag() {
    local requested_tag="${1:-}"
    local latest_tag

    if [ -n "$requested_tag" ]; then
        echo "$requested_tag"
        return 0
    fi

    latest_tag="$(get_latest_hub_tag || true)"
    if [ -n "$latest_tag" ]; then
        echo "$latest_tag"
        return 0
    fi

    if [ -f "$HUB_TAG_STATE_FILE" ]; then
        echo "⚠️ 无法查询最新 Hub 版本，回退使用本地记录: $(cat "$HUB_TAG_STATE_FILE")" >&2
        cat "$HUB_TAG_STATE_FILE"
        return 0
    fi

    echo "❌ 无法获取 Hub Release tag，请检查网络或手动指定 --hub-tag" >&2
    exit 1
}

# 确保本次构建的 Hub tag 已解析（默认追踪最新 release，也可通过 --hub-tag 固定版本）
ensure_hub_tag() {
    local requested_tag="${1:-}"
    RESOLVED_HUB_TAG="$(resolve_hub_tag "$requested_tag")"

    if [ -f "$HUB_TAG_STATE_FILE" ] && [ "$(cat "$HUB_TAG_STATE_FILE")" = "$RESOLVED_HUB_TAG" ]; then
        echo ">>> Hub 版本未变化: $RESOLVED_HUB_TAG"
        return 1
    fi

    echo "$RESOLVED_HUB_TAG" > "$HUB_TAG_STATE_FILE"
    echo ">>> 使用 Hub 版本: $RESOLVED_HUB_TAG"
    return 0
}

# 计算迁移文件的哈希值
calc_migration_hash() {
    find alembic/versions -name "*.py" -type f 2>/dev/null | sort | xargs cat 2>/dev/null | md5sum | cut -d' ' -f1
}

# 检查依赖是否变化
check_deps_changed() {
    local current_hash=$(calc_deps_hash)
    if [ -f "$HASH_FILE" ]; then
        local saved_hash=$(cat "$HASH_FILE")
        if [ "$current_hash" = "$saved_hash" ]; then
            return 1
        fi
    fi
    return 0
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



# 检查迁移是否变化
check_migration_changed() {
    local current_hash=$(calc_migration_hash)
    if [ -f "$MIGRATION_HASH_FILE" ]; then
        local saved_hash=$(cat "$MIGRATION_HASH_FILE")
        if [ "$current_hash" = "$saved_hash" ]; then
            return 1
        fi
    fi
    return 0
}

# 保存哈希
save_deps_hash() { calc_deps_hash > "$HASH_FILE"; }
save_code_hash() { calc_code_hash > "$CODE_HASH_FILE"; }
save_migration_hash() { calc_migration_hash > "$MIGRATION_HASH_FILE"; }

# 构建基础镜像
build_base() {
    echo ">>> Building base image (dependencies)..."
    docker build --pull=false -f Dockerfile.base.local -t aether-base:latest .
    save_deps_hash
}


# 生成版本文件
generate_version_file() {
    # 从 git 获取版本号
    local version
    version=$(git describe --tags --always 2>/dev/null | sed 's/^v//')
    if [ -z "$version" ]; then
        version="unknown"
    fi
    echo ">>> Generating version file: $version"
    cat > src/_version.py << EOF
# Auto-generated by deploy.sh - do not edit
__version__ = '$version'
__version_tuple__ = tuple(int(x) for x in '$version'.split('-')[0].split('.') if x.isdigit())
version = __version__
version_tuple = __version_tuple__
EOF
}

# 构建应用镜像
build_app() {
    echo ">>> Building app image (code only)..."
    if [ -z "${RESOLVED_HUB_TAG:-}" ]; then
        echo ">>> RESOLVED_HUB_TAG 为空，无法构建 app 镜像"
        exit 1
    fi
    echo ">>> Build args: HUB_TAG=$RESOLVED_HUB_TAG"
    generate_version_file
    local token_args=()
    if [ -n "${GITHUB_TOKEN:-}" ]; then
        token_args=(--build-arg "GITHUB_TOKEN=${GITHUB_TOKEN}")
    fi
    local mirror_args=()
    if [ -n "${GITHUB_MIRROR:-}" ]; then
        mirror_args=(--build-arg "GITHUB_MIRROR=${GITHUB_MIRROR}")
    fi
    docker build --pull=false \
        --build-arg HUB_RELEASE_REPO="$GITHUB_REPO" \
        --build-arg HUB_TAG="$RESOLVED_HUB_TAG" \
        "${token_args[@]}" \
        "${mirror_args[@]}" \
        -f Dockerfile.app.local \
        -t aether-app:latest .
    save_code_hash
}

# 运行数据库迁移
run_migration() {
    echo ">>> Running database migration..."

    # 尝试运行 upgrade head，捕获错误
    UPGRADE_OUTPUT=$($DC exec -T app alembic upgrade head 2>&1) && {
        echo "$UPGRADE_OUTPUT"
        save_migration_hash
        return 0
    }

    # 检查是否是因为找不到旧版本（基线重置场景）
    if echo "$UPGRADE_OUTPUT" | grep -q "Can't locate revision"; then
        echo ">>> Detected baseline reset: old revision not found in migrations"
        echo ">>> Clearing old version and stamping to new baseline..."

        # 先清除旧的版本记录，再 stamp 到新基线
        $DC exec -T app python -c "
from sqlalchemy import create_engine, text
import os
engine = create_engine(os.environ['DATABASE_URL'])
with engine.connect() as conn:
    conn.execute(text('DELETE FROM alembic_version'))
    conn.commit()
print('Old version cleared')
"
        # 获取最新的迁移版本（匹配 revision_id (head) 格式）
        LATEST_VERSION=$($DC exec -T app alembic heads 2>/dev/null | grep -oE '^[0-9a-zA-Z_]+' | head -1)
        if [ -n "$LATEST_VERSION" ]; then
            $DC exec -T app alembic stamp "$LATEST_VERSION"
            echo ">>> Database stamped to $LATEST_VERSION"
            save_migration_hash
        else
            echo ">>> ERROR: Could not determine latest migration version"
            exit 1
        fi
    else
        # 其他错误，直接输出并退出
        echo "$UPGRADE_OUTPUT"
        exit 1
    fi
}

# 强制全部重建
if [ "$FORCE_REBUILD_ALL" = true ]; then
    echo ">>> Force rebuilding everything..."
    if [ "$FORCE_UPDATE_HUB" = true ]; then
        rm -f "$HUB_TAG_STATE_FILE"
    fi
    ensure_hub_tag "$HUB_TAG" || true
    build_base
    build_app
    compose_up --force-recreate
    sleep 3
    run_migration
    docker image prune -f
    echo ">>> Done!"
    $DC ps
    exit 0
fi

# 强制重建基础镜像
if [ "$REBUILD_BASE_ONLY" = true ]; then
    build_base
    echo ">>> Base image rebuilt. Run ./deploy.sh to deploy."
    exit 0
fi

# 更新 Hub 版本标记
if [ "$FORCE_UPDATE_HUB" = true ]; then
    rm -f "$HUB_TAG_STATE_FILE"
    ensure_hub_tag "$HUB_TAG" || true
    echo ">>> Hub tag updated: $RESOLVED_HUB_TAG"
    echo ">>> Run ./deploy.sh to build app image with the new Hub release."
    exit 0
fi

# 标记是否需要重启
NEED_RESTART=false
BASE_REBUILT=false
HUB_UPDATED=false

# 检查基础镜像是否存在，或依赖是否变化
if ! docker image inspect aether-base:latest >/dev/null 2>&1; then
    echo ">>> Base image not found, building..."
    build_base
    BASE_REBUILT=true
    NEED_RESTART=true
elif check_deps_changed; then
    echo ">>> Dependencies changed, rebuilding base image..."
    build_base
    BASE_REBUILT=true
    NEED_RESTART=true
else
    echo ">>> Dependencies unchanged."
fi

# 解析/检查 Hub 版本（构建时由 Dockerfile 从 GitHub Release 下载）
if ensure_hub_tag "$HUB_TAG"; then
    HUB_UPDATED=true
    NEED_RESTART=true
else
    echo ">>> Hub version unchanged."
fi

# 检查代码或迁移是否变化，或者 base 重建了（app 依赖 base）
# 注意：迁移文件打包在镜像中，所以迁移变化也需要重建 app 镜像
MIGRATION_CHANGED=false
if check_migration_changed; then
    MIGRATION_CHANGED=true
fi

if ! docker image inspect aether-app:latest >/dev/null 2>&1; then
    echo ">>> App image not found, building..."
    build_app
    NEED_RESTART=true
elif [ "$BASE_REBUILT" = true ]; then
    echo ">>> Base image rebuilt, rebuilding app image..."
    build_app
    NEED_RESTART=true
elif [ "$HUB_UPDATED" = true ]; then
    echo ">>> Hub version updated, rebuilding app image..."
    build_app
    NEED_RESTART=true
elif check_code_changed; then
    echo ">>> Code changed, rebuilding app image..."
    build_app
    NEED_RESTART=true
elif [ "$MIGRATION_CHANGED" = true ]; then
    echo ">>> Migration files changed, rebuilding app image..."
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

# 检查迁移变化（如果前面已经检测到变化并重建了镜像，这里直接运行迁移）
if [ "$MIGRATION_CHANGED" = true ]; then
    echo ">>> Running database migration..."
    sleep 3
    run_migration
else
    echo ">>> Migration unchanged."
fi

# 清理
docker image prune -f >/dev/null 2>&1 || true

echo ">>> Done!"
$DC ps
