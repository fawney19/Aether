# Aether - 数据库迁移说明

## 当前版本

- **Revision ID**: `aether_baseline`
- **创建日期**: 2025-12-06
- **状态**: 全新基线

## 迁移历史

所有历史增量迁移已清理，当前以完整 schema 作为新起点。

## 核心数据库结构

### 用户系统
- **users**: 用户账户管理
- **api_keys**: API 密钥管理
- **wallets**: 统一钱包账户（充值余额/赠款余额/无限制模式）
- **user_preferences**: 用户偏好设置

### Provider 三层架构
- **providers**: LLM 提供商配置
- **provider_endpoints**: Provider 的 API 端点配置
- **provider_api_keys**: Endpoint 的具体 API 密钥
- **api_key_provider_mappings**: 用户 API Key 到 Provider 的映射关系

### 模型系统
- **global_models**: 统一模型定义（GlobalModel）
- **models**: Provider 的模型实现和价格配置
- **model_mappings**: 统一的别名与降级映射表

### 监控和追踪
- **usage**: API 使用记录
- **request_candidates**: 请求候选记录
- **provider_usage_tracking**: Provider 使用统计
- **audit_logs**: 系统审计日志

### 系统功能
- **announcements**: 系统公告
- **announcement_reads**: 公告阅读记录
- **system_configs**: 系统配置

## 从旧数据库迁移

如需从旧数据库迁移数据，请使用迁移脚本：

```bash
# 设置环境变量
export OLD_DATABASE_URL="postgresql://user:pass@old-host:5432/old_db"
export NEW_DATABASE_URL="postgresql://user:pass@new-host:5432/aether"

# 干运行（查看迁移量）
python scripts/migrate_data.py --dry-run

# 执行迁移
python scripts/migrate_data.py

# 只迁移特定表
python scripts/migrate_data.py --tables users,providers,api_keys

# 跳过大表
python scripts/migrate_data.py --skip usage,audit_logs
```

## 新数据库初始化

```bash
# 1. 运行迁移创建表结构
DATABASE_URL="postgresql://user:pass@host:5432/aether" uv run alembic upgrade head

# 2. 初始化管理员账户
python -m src.database.init_db
```

## 未来迁移

基于 `aether_baseline` 创建增量迁移：

```bash
# 修改模型后，生成新的迁移
DATABASE_URL="..." uv run alembic revision --autogenerate -m "描述变更"

# 应用迁移
DATABASE_URL="..." uv run alembic upgrade head
```
