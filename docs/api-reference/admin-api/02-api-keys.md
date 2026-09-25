# API 密钥管理（/api/admin/api-keys）

> 家族：`api_keys_manage`（权限域 `admin:api_keys`）
> 代码位置：`apps/aether-gateway/src/handlers/admin/auth/api_keys/`

管理**独立余额 Key**（standalone API key）——每个 key 自带钱包、限流与模型/渠道白名单，是给下游用户/客户端发调用凭证的入口。

## 端点清单

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/admin/api-keys` | 分页列表 |
| POST | `/api/admin/api-keys` | 创建（返回明文 key） |
| GET | `/api/admin/api-keys/{id}` | 详情（`?include_key=true` 可解密回显明文，需审计） |
| PUT | `/api/admin/api-keys/{id}` | 全量更新 |
| PATCH | `/api/admin/api-keys/{id}` | 启用/停用切换 |
| DELETE | `/api/admin/api-keys/{id}` | 删除 |
| POST | `/api/admin/api-keys/{id}/install-sessions` | 生成安装会话（供客户端一键配置） |

## GET 列表

```
GET /api/admin/api-keys?skip=0&limit=20&is_active=true
```

响应：

```json
{
  "api_keys": [
    {
      "id": "uuid",
      "user_id": "uuid",
      "name": "my-key",
      "key_display": "sk-****abcd",
      "is_active": true,
      "is_standalone": true,
      "total_requests": 120,
      "total_tokens": 3500000,
      "total_cost_usd": 0.42,
      "rate_limit": 60,
      "concurrent_limit": 5,
      "allowed_providers": ["<渠道名>"],
      "allowed_api_formats": ["openai:responses"],
      "allowed_models": ["<模型名>"],
      "ip_rules": ["1.2.3.4/32"],
      "last_used_at": "2026-08-16T12:00:00Z",
      "expires_at": null,
      "created_at": "2026-08-01T08:00:00Z",
      "updated_at": "2026-08-16T12:00:00Z",
      "auto_delete_on_expiry": false,
      "feature_settings": null,
      "wallet": { "balance": 5.5, "gift_balance": 0, "limit_mode": "balance", "status": "active" }
    }
  ],
  "total": 35,
  "limit": 20,
  "skip": 0
}
```

## POST 创建

```
POST /api/admin/api-keys
```

请求体（全部字段可选，除下述规则）：

```json
{
  "name": "client-a",
  "allowed_providers": ["<渠道名>"],
  "allowed_api_formats": ["openai:responses", "openai:chat"],
  "allowed_models": ["<模型名>"],
  "ip_rules": ["1.2.3.4/32"],
  "rate_limit": 60,
  "concurrent_limit": 5,
  "initial_balance_usd": 10.0,
  "unlimited_balance": false,
  "expires_at": "2027-01-01",
  "auto_delete_on_expiry": false,
  "feature_settings": {}
}
```

字段规则（代码核实）：
- `name` 空则用默认名；`allowed_providers`/`allowed_models`/`ip_rules` 为字符串数组
- `allowed_api_formats` 合法值：`openai:chat` / `openai:responses` / `claude:messages` / `gemini:*` 等
- `rate_limit` ≥ 0；`concurrent_limit` ≥ 1（可选）
- 余额二选一：`initial_balance_usd` > 0，或 `unlimited_balance: true`；两者都缺省时默认 **unlimited**
- `expires_at` 格式 `YYYY-MM-DD`（当日 23:59:59）或 RFC3339
- `expire_days` 字段**已废弃**（请求会 400，请用 `expires_at`）
- `auto_delete_on_expiry: true` 必须先给 `expires_at`

响应（HTTP 200，**明文只此一次**）：

```json
{
  "id": "uuid",
  "key": "sk-xxxxxxxxxxxx",
  "name": "client-a",
  "key_display": "sk-****",
  "is_standalone": true,
  "is_active": true,
  "rate_limit": 60,
  "concurrent_limit": 5,
  "allowed_providers": ["<渠道名>"],
  "allowed_api_formats": ["openai:responses"],
  "allowed_models": ["<模型名>"],
  "expires_at": "2027-01-01T00:00:00Z",
  "auto_delete_on_expiry": false,
  "feature_settings": null,
  "wallet": { "balance": 10.0, "gift_balance": 0, "limit_mode": "balance", "status": "active" },
  "message": "独立余额Key创建成功，请妥善保存完整密钥，后续将无法查看"
}
```

> ⚠️ `key` 明文只在创建响应中出现一次，库内仅存加密与哈希。

## GET 详情 / 回显明文

```
GET /api/admin/api-keys/{id}
GET /api/admin/api-keys/{id}?include_key=true   # 解密返回完整明文（触发审计事件 admin_standalone_api_key_revealed）
```

## PUT 更新（全量）

与创建同字段；`ip_rules` 等列表字段支持显式置空（`[]`）。未提供的字段按默认处理。

## PATCH 切换

```
PATCH /api/admin/api-keys/{id}
{"is_active": false}   # 停用；true 恢复
```

## DELETE 删除

```
DELETE /api/admin/api-keys/{id}
```

删除会同步清理钱包与关联引用（prune）。

## 常见错误

- 400 `{"detail":"输入验证失败"}`：字段非法（如 `initial_balance_usd` ≤ 0、`expire_days` 已废弃）
- 400 `{"detail":"仅支持查看独立密钥"}`：对非 standalone key 调详情
- 404：key 不存在
- 解密失败：500 `{"detail":"解密密钥失败"}`
