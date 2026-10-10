# 统计（/api/admin/stats）

> 家族：`stats_manage` · 权限域：`admin:stats`
> 说明：时序 / 对比 / 成本 / 排行 / 性能
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/stats/comparison` | `comparison` |
| GET | `/api/admin/stats/cost/forecast` | `cost_forecast` |
| GET | `/api/admin/stats/cost/savings` | `cost_savings` |
| GET | `/api/admin/stats/errors/distribution` | `error_distribution` |
| GET | `/api/admin/stats/leaderboard/api-keys` | `leaderboard_api_keys` |
| GET | `/api/admin/stats/leaderboard/models` | `leaderboard_models` |
| GET | `/api/admin/stats/leaderboard/users` | `leaderboard_users` |
| GET | `/api/admin/stats/performance/percentiles` | `performance_percentiles` |
| GET | `/api/admin/stats/performance/providers` | `provider_performance` |
| GET | `/api/admin/stats/providers/quota-usage` | `provider_quota_usage` |
| GET | `/api/admin/stats/time-series` | `time_series` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/stats/time-series?range=24h -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/stats/leaderboard/models?limit=10 -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/stats/cost/forecast?days=30 -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/stats/performance/percentiles -H "Authorization: Bearer ae-xxx"
```
