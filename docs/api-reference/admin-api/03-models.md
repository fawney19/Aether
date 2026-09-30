# 全局模型管理（/api/admin/models/global）

> 家族：`global_models_manage`（权限域 `admin:models`）
> 代码位置：`apps/aether-gateway/src/handlers/admin/model/global_models/`

全局模型（global model）是网关对外暴露的"模型名"，可绑定多个渠道模型（provider models）做自动路由与故障转移。

## 端点清单

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/admin/models/global` | 列表（分页/搜索/过滤） |
| POST | `/api/admin/models/global` | 创建 |
| GET | `/api/admin/models/global/{id}` | 详情（含渠道数与价格区间） |
| PATCH | `/api/admin/models/global/{id}` | **部分更新（config 整体替换）** |
| DELETE | `/api/admin/models/global/{id}` | 删除 |
| POST | `/api/admin/models/global/batch-delete` | 批量删除 |
| POST | `/api/admin/models/global/{id}/providers` | 关联/分配渠道（assign_to_providers） |
| GET | `/api/admin/models/global/{id}/routing` | 路由预览 |
| GET | `/api/admin/models/global/{id}/providers` | 已绑定的渠道模型列表 |

## GET 列表

```
GET /api/admin/models/global?skip=0&limit=100&is_active=true&search=deepseek
```

参数：`skip`（默认 0）、`limit`（默认 100，≤1000）、`is_active`、`search`（名称模糊）。

响应：

```json
{
  "models": [
    {
      "id": "global-<模型名>",
      "name": "<模型名>",
      "display_name": "DeepSeek V4 Flash",
      "is_active": true,
      "default_price_per_request": null,
      "default_tiered_pricing": null,
      "supported_capabilities": ["chat", "responses", "extended_thinking"],
      "config": { "model_mappings": ["deepseek-v4.*"] },
      "provider_count": 4,
      "active_provider_count": 3,
      "usage_count": 12345,
      "created_at": "2026-07-01T00:00:00Z",
      "updated_at": "2026-08-15T10:00:00Z"
    }
  ],
  "total": 80
}
```

## GET 详情

```
GET /api/admin/models/global/{id}
```

比列表项多 `total_models`（渠道模型数）、`total_providers`、`price_range`（各渠道价格区间）。

## POST 创建

```json
{
  "name": "gpt-5.6-luna",
  "display_name": "GPT-5.6 Luna",
  "default_price_per_request": 0.001,
  "default_tiered_pricing": { "tiers": [{"up_to": null, "input_price_per_1m": 0.3, "output_price_per_1m": 1.2}] },
  "supported_capabilities": ["chat", "responses"],
  "config": { "model_mappings": ["gpt5\\.6", "gpt-5\\.6"] },
  "is_active": true
}
```

- `name`、`display_name` 必填
- `config.model_mappings`：入站模型名正则兜底映射（如停用某模型后旧名仍可路由）

## PATCH 更新（重点）

**部分更新**：只传要改的字段；但 `config` 字段是**整体替换**（不是 merge）。

```json
PATCH /api/admin/models/global/global-<模型名>
{
  "display_name": "DeepSeek V4 Flash (new)",
  "config": { "model_mappings": ["deepseek-v4.*", "deepseek-v4.1.*"] }
}
```

> ⚠️ 更新 `config` 时务必带上全部想要的键，否则原配置丢失。

## POST 分配渠道（assign_to_providers）

```
POST /api/admin/models/global/{id}/providers
```

```json
{
  "provider_ids": ["<provider-uuid-1>", "<provider-uuid-2>"],
  "mappings": { "<provider-uuid-1>": { "provider_model_name": "<模型名>" } }
}
```

## POST 批量删除

```
POST /api/admin/models/global/batch-delete
{"ids": ["id-1", "id-2"]}
```

## 运维提示（来自 skill 经验）

- **停用模型的旧名兜底**：设 `is_active: false` 后，入站旧模型名靠 `config.model_mappings` 正则继续路由（候选 SQL 有 `COALESCE(gm.is_active,1)=1` 过滤，精确匹配落空后走 mapping 正则）。
- 测试 key 可直接用 `POST /api/admin/api-keys` 创建（见 02-api-keys.md）。
