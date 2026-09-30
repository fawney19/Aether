# 供应商与端点管理（/api/admin/providers、/api/admin/endpoints）

> 家族：`providers_manage`（`admin:providers`）、`endpoints_manage`（`admin:endpoints_manage`）
> 代码位置：`apps/aether-gateway/src/handlers/admin/provider/`
> **这是日常运维改动最多的区域**（body_rules/header_rules/模型映射/启停端点），改前务必按 [07-ops 章节](../ops/playbook.md) 的"备份与回滚"流程备份。

## 一、供应商（providers）

### 端点清单

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/admin/providers` | 供应商列表（list_providers） |
| POST | `/api/admin/providers` | 创建供应商 |
| PATCH | `/api/admin/providers/{provider_id}` | 部分更新 |
| DELETE | `/api/admin/providers/{provider_id}` | 删除（异步任务） |
| GET | `/api/admin/providers/summary` | 汇总 |
| GET | `/api/admin/providers/{provider_id}/summary` | 单供应商汇总 |
| GET | `/api/admin/providers/{provider_id}/health-monitor` | 健康监控 |
| GET | `/api/admin/providers/{provider_id}/mapping-preview` | 映射预览 |
| GET | `/api/admin/providers/{provider_id}/pool-status` | 号池状态 |
| GET | `/api/admin/providers/{provider_id}/delete-task/{task_id}` | 删除任务状态 |
| POST | `/api/admin/providers/{provider_id}/pool/clear-cooldown/{key_id}` | 清除冷却 |
| POST | `/api/admin/providers/{provider_id}/pool/reset-cost/{key_id}` | 重置成本 |

供应商关键字段（来自 `providers` 表）：
`name`、`provider_type`、`billing_type`、`monthly_quota_usd`、`enabled`、`is_active`、`priority`/`provider_priority`（越小越优先）、`concurrent_limit`、`max_retries`、`proxy`（jsonb，节点配置）、`request_timeout`、`stream_first_byte_timeout`、`config`（jsonb，含模型映射正则等）。

### 创建示例

```json
POST /api/admin/providers
{
  "name": "<渠道名>",
  "provider_type": "openai",
  "billing_type": "monthly_quota",
  "monthly_quota_usd": 100,
  "priority": 1,
  "proxy": { "node_name": "<节点>" }
}
```

## 二、端点（provider_endpoints，body_rules 所在）

### 端点清单

| 方法 | 路径 | 说明 |
|---|---|---|
| GET/POST | `/api/admin/endpoints/` | 端点列表 / 创建（provider_id + api_format + base_url） |
| GET/PUT/DELETE | `/api/admin/endpoints/{id}` | 详情 / 更新 / 删除 |
| GET | `/api/admin/endpoints/defaults/` | 默认端点模板 |
| GET | `/api/admin/endpoints/health/` 系列 | 健康状态（api-formats/models/providers/related/status/summary/key） |
| PATCH | `/api/admin/endpoints/health/keys` | 恢复 key 健康 |
| CRUD | `/api/admin/endpoints/keys/` | 端点密钥（渠道 key）管理 |
| POST | `/api/admin/endpoints/keys/batch-delete` | 批量删除密钥 |
| GET | `/api/admin/endpoints/keys/grouped-by-format` | 按格式分组 |
| CRUD | `/api/admin/endpoints/providers/` | 某供应商的端点 |
| CRUD | `/api/admin/endpoints/rpm/` | 端点 RPM 限流 |
| GET/DELETE | `/api/admin/endpoints/rpm/key/` | 单 key RPM |

### 创建端点

```json
POST /api/admin/endpoints/
{
  "provider_id": "<provider-uuid>",
  "api_format": "openai:responses",
  "base_url": "https://upstream.example.com/v1",
  "custom_path": null,
  "header_rules": null,
  "body_rules": [
    {"path": "tools[*]", "action": "drop", "condition": {"op": "eq", "path": "$item.name", "value": "automation_update"}}
  ],
  "max_retries": 2,
  "config": {},
  "proxy": null,
  "format_acceptance_config": {}
}
```

- `api_format`：`openai:chat` / `openai:responses` / `claude:messages` / `gemini:*` / `jina:*` / `doubao:*` / `aliyun:*` 等
- `body_rules` / `header_rules`：请求改写规则，完整语法见 [body-rules 章节](../ops/body-rules.md)
- `proxy`：`{"node_name": "..."}` 指定出站节点；不填=直连
- `format_acceptance_config`：格式接受配置（哪些客户端格式接受）

### 更新端点（改 body_rules 的标准操作）

```json
PATCH /api/admin/endpoints/{id}
{
  "body_rules": [ ...新规则... ]
}
```

> ⚠️ 更新规则前先 `GET /api/admin/endpoints/{id}` 拿到现有 body_rules/header_rules，**全量带上**再提交，避免覆盖。

### 查询端点配置（含规则）

```
GET /api/admin/endpoints/providers/{provider_id}
GET /api/admin/endpoints/{id}
```

也可以直接查库（见数据库章节）：

```sql
select p.name, e.id, e.api_format, e.endpoint_kind, e.base_url, e.is_active, e.enabled, e.body_rules
from providers p join provider_endpoints e on e.provider_id = p.id
where p.name = '<渠道名>';
```

## 三、常用运维动作速查

| 目标 | 操作 |
|---|---|
| 停用坏端点（如死链反代） | `PATCH /api/admin/endpoints/{id}` `{"enabled": false}` |
| 给端点加删除规则 | 更新 `body_rules`（见 body-rules 章节实战例 1/2/3） |
| 停用全局模型让旧名走 mapping 正则 | `PATCH /api/admin/models/global/{id}` `{"is_active": false}` + `config.model_mappings` |
| 创建测试 key | `POST /api/admin/api-keys`（见 02-api-keys.md） |
| 查端点健康 | `GET /api/admin/endpoints/health/status`、`/api/admin/endpoints/health/summary` |

## 四、Provider 运维/查询/策略（简表）

| 路径前缀 | 说明 |
|---|---|
| `/api/admin/provider-ops/*` | 连接/断开/批量余额/checkin/架构列表（anyrouter/cubence/new_api/sub2api 等） |
| `/api/admin/provider-query/*` | 渠道模型查询、测试模型（test-model / test-model-failover） |
| `/api/admin/provider-strategy/*` | 渠道统计、重置配额、更新计费 |
| `/api/admin/provider-oauth/*` | Provider OAuth（cookie 授权任务、agent-identity 导入） |
| `/api/admin/adaptive/*` | 自适应调度（key 列表/统计/重置学习/限额） |
| `/api/admin/pool/*` | 号池（key 批量导入/操作/调度预设/概览） |
