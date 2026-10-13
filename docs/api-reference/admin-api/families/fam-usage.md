# 用量（/api/admin/usage）

> 家族：`usage_manage` · 权限域：`admin:usage`
> 说明：请求记录 / 统计 / 热力图 / 重放（详见 05）
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/usage/active` | `active` |
| GET | `/api/admin/usage/aggregation/stats` | `aggregation_stats` |
| GET | `/api/admin/usage/cache-affinity/hit-analysis` | `cache_affinity_hit_analysis` |
| GET | `/api/admin/usage/cache-affinity/interval-timeline` | `cache_affinity_interval_timeline` |
| GET | `/api/admin/usage/cache-affinity/ttl-analysis` | `cache_affinity_ttl_analysis` |
| GET | `/api/admin/usage/heatmap` | `heatmap` |
| GET | `/api/admin/usage/records` | `records` |
| GET | `/api/admin/usage/stats` | `stats` |
| GET | `/api/admin/usage/{request_id}` | `detail` |
| GET | `/api/admin/usage/{request_id}/curl` | `curl` |
| POST | `/api/admin/usage/{request_id}/replay` | `replay` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/usage/records?limit=20&status=error -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/usage/stats?range=24h -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/usage/{request_id}/replay -H "Authorization: Bearer ae-xxx"
```
