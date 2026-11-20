# 供应商（/api/admin/providers）

> 家族：`providers_manage` · 权限域：`admin:providers`
> 说明：供应商 CRUD / 健康 / 池操作（详见 04-providers-endpoints.md）
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/providers` | `list_providers` |
| POST | `/api/admin/providers` | `create_provider` |
| GET | `/api/admin/providers/summary` | `summary_list` |
| DELETE | `/api/admin/providers/{provider_id}` | `delete_provider` |
| PATCH | `/api/admin/providers/{provider_id}` | `update_provider` |
| GET | `/api/admin/providers/{provider_id}/delete-task/{task_id}` | `delete_provider_task` |
| GET | `/api/admin/providers/{provider_id}/health-monitor` | `health_monitor` |
| GET | `/api/admin/providers/{provider_id}/mapping-preview` | `mapping_preview` |
| GET | `/api/admin/providers/{provider_id}/pool-status` | `pool_status` |
| POST | `/api/admin/providers/{provider_id}/pool/clear-cooldown/{key_id}` | `clear_pool_cooldown` |
| POST | `/api/admin/providers/{provider_id}/pool/reset-cost/{key_id}` | `reset_pool_cost` |
| GET | `/api/admin/providers/{provider_id}/summary` | `provider_summary` |

## 示例

```bash
curl -sS https://<网关域名>/api/admin/providers -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/providers \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"name":"new-upstream","provider_type":"openai","billing_type":"monthly_quota","monthly_quota_usd":100,"priority":5}'
```