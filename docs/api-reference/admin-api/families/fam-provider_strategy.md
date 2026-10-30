# Provider 策略（/api/admin/provider-strategy）

> 家族：`provider_strategy_manage` · 权限域：`admin:provider_strategy`
> 说明：渠道统计 / 配额重置 / 计费更新
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| PUT | `/api/admin/provider-strategy/providers/{id}/billing` | `update_provider_billing` |
| DELETE | `/api/admin/provider-strategy/providers/{id}/quota` | `reset_provider_quota` |
| GET | `/api/admin/provider-strategy/providers/{id}/stats` | `get_provider_stats` |
| GET | `/api/admin/provider-strategy/strategies` | `list_strategies` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/provider-strategy/strategies -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/provider-strategy/providers/{provider_id}/reset-quota -H "Authorization: Bearer ae-xxx"
```
