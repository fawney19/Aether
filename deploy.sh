#!/bin/bash
# 智能部署脚本 - 自动检测依赖/代码/迁移变化
#
# 用法:
#   部署/更新:    ./deploy.sh  (自动检测所有变化)
#   强制重建:     ./deploy.sh --rebuild-base
#   强制全部重建: ./deploy.sh --force

set -e
cd "$(dirname "$0")"

# 兼容 docker-compose 和 docker compose
if command -v docker-compose &> /dev/null; then
    DC="docker-compose -f docker-compose.build.yml"
else
    DC="docker compose -f docker-compose.build.yml"
fi

# 缓存文件
HASH_FILE=".deps-hash"
CODE_HASH_FILE=".code-hash"
MIGRATION_HASH_FILE=".migration-hash"

# 计算依赖文件的哈希值
calc_deps_hash() {
    cat pyproject.toml frontend/package.json frontend/package-lock.json 2>/dev/null | md5sum | cut -d' ' -f1
}

# 计算代码文件的哈希值
calc_code_hash() {
    find src -type f -name "*.py" 2>/dev/null | sort | xargs cat 2>/dev/null | md5sum | cut -d' ' -f1
    find frontend/src -type f \( -name "*.vue" -o -name "*.ts" -o -name "*.tsx" -o -name "*.js" \) 2>/dev/null | sort | xargs cat 2>/dev/null | md5sum | cut -d' ' -f1
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
    docker build -f Dockerfile.base.local -t aether-base:latest .
    save_deps_hash
}

# 构建应用镜像
build_app() {
    echo ">>> Building app image (code only)..."
    docker build -f Dockerfile.app -t aether-app:latest .
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
if [ "$1" = "--force" ] || [ "$1" = "-f" ]; then
    echo ">>> Force rebuilding everything..."
    build_base
    build_app
    $DC up -d --force-recreate
    sleep 3
    run_migration
    docker image prune -f
    echo ">>> Done!"
    $DC ps
    exit 0
fi

# 强制重建基础镜像
if [ "$1" = "--rebuild-base" ] || [ "$1" = "-r" ]; then
    build_base
    echo ">>> Base image rebuilt. Run ./deploy.sh to deploy."
    exit 0
fi

# 拉取最新代码
echo ">>> Pulling latest code..."
git pull

# 标记是否需要重启
NEED_RESTART=false

# 检查基础镜像是否存在，或依赖是否变化
if ! docker image inspect aether-base:latest >/dev/null 2>&1; then
    echo ">>> Base image not found, building..."
    build_base
    NEED_RESTART=true
elif check_deps_changed; then
    echo ">>> Dependencies changed, rebuilding base image..."
    build_base
    NEED_RESTART=true
else
    echo ">>> Dependencies unchanged."
fi

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

# 只在有变化时重启
if [ "$NEED_RESTART" = true ]; then
    echo ">>> Restarting services..."
    $DC up -d
else
    echo ">>> No changes detected, skipping restart."
fi

# 检查迁移变化
if check_migration_changed; then
    echo ">>> Migration files changed, running database migration..."
    sleep 3
    run_migration
else
    echo ">>> Migration unchanged."
fi

# 清理
docker image prune -f >/dev/null 2>&1 || true

echo ">>> Done!"
$DC ps
