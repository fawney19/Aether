#!/bin/bash
# 本地开发启动脚本
clear

# 加载 .env 文件
set -a
source .env
set +a

# 构建 DATABASE_URL
export DATABASE_URL="postgresql://${DB_USER:-postgres}:${DB_PASSWORD}@${DB_HOST:-localhost}:${DB_PORT:-5432}/${DB_NAME:-aether}"
export REDIS_URL=redis://:${REDIS_PASSWORD}@${REDIS_HOST:-localhost}:${REDIS_PORT:-6379}/0

# 开发环境连接池低配（节省内存）
export DB_POOL_SIZE=${DB_POOL_SIZE:-5}
export DB_MAX_OVERFLOW=${DB_MAX_OVERFLOW:-5}
export HTTP_MAX_CONNECTIONS=${HTTP_MAX_CONNECTIONS:-20}
export HTTP_KEEPALIVE_CONNECTIONS=${HTTP_KEEPALIVE_CONNECTIONS:-5}

# 启动 uvicorn（热重载模式，只监视 src 目录）
echo "=> 启动本地开发服务器..."
echo "=> 后端地址: http://localhost:8084"
echo "=> 数据库: ${DATABASE_URL}"
echo ""

uv run uvicorn src.main:app --reload --reload-dir src --port 8084
