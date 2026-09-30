# 计费（/api/admin/billing）

> 家族：`billing_manage` · 权限域：`admin:billing`
> 说明：维度采集器 / 套餐 / 计费规则 / 预设
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/billing/collectors` | `list_collectors` |
| POST | `/api/admin/billing/collectors` | `create_collector` |
| GET | `/api/admin/billing/collectors/{id}` | `get_collector` |
| PUT | `/api/admin/billing/collectors/{id}` | `update_collector` |
| GET | `/api/admin/billing/plans` | `list_plans` |
| POST | `/api/admin/billing/plans` | `create_plan` |
| DELETE | `/api/admin/billing/plans/{id}` | `delete_plan` |
| PUT | `/api/admin/billing/plans/{id}` | `update_plan` |
| PATCH | `/api/admin/billing/plans/{id}/status` | `set_plan_status` |
| GET | `/api/admin/billing/presets` | `list_presets` |
| POST | `/api/admin/billing/presets/apply` | `apply_preset` |
| GET | `/api/admin/billing/rules` | `list_rules` |
| POST | `/api/admin/billing/rules` | `create_rule` |
| GET | `/api/admin/billing/rules/{id}` | `get_rule` |
| PUT | `/api/admin/billing/rules/{id}` | `update_rule` |

## 示例

```bash
curl -sS -X POST https://<网关域名>/api/admin/billing/rules \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"name":"deepseek-input","task_type":"chat","expression":"input_tokens * 0.3 / 1000000","variables":{},"dimension_mappings":{},"is_enabled":true}'
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/billing/presets/apply \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"preset_id":"<id>"}'
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/billing/collectors -H "Authorization: Bearer ae-xxx"
```
