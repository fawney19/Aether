# 监控（/api/admin/monitoring）

> 家族：`monitoring` · 权限域：`admin:monitoring`
> 说明：审计日志 / 缓存键 / 弹性 / 追踪 / 异常行为
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/monitoring/audit-logs` | `audit_logs` |
| DELETE | `/api/admin/monitoring/cache` | `cache_flush` |
| GET | `/api/admin/monitoring/cache/affinities` | `cache_affinities` |
| GET | `/api/admin/monitoring/cache/affinity/{key_id}` | `cache_affinity` |
| DELETE | `/api/admin/monitoring/cache/affinity/{user_id}/{key_id}/{model}/{api_format}` | `cache_affinity_delete` |
| GET | `/api/admin/monitoring/cache/config` | `cache_config` |
| GET | `/api/admin/monitoring/cache/metrics` | `cache_metrics` |
| DELETE | `/api/admin/monitoring/cache/model-mapping` | `cache_model_mapping_delete` |
| DELETE | `/api/admin/monitoring/cache/model-mapping/provider/{provider_id}/{model_id}` | `cache_model_mapping_delete_provider` |
| GET | `/api/admin/monitoring/cache/model-mapping/stats` | `cache_model_mapping_stats` |
| DELETE | `/api/admin/monitoring/cache/model-mapping/{model_id}` | `cache_model_mapping_delete_model` |
| DELETE | `/api/admin/monitoring/cache/providers/{provider_id}` | `cache_provider_delete` |
| GET | `/api/admin/monitoring/cache/redis-keys` | `cache_redis_keys` |
| DELETE | `/api/admin/monitoring/cache/redis-keys/{key_id}` | `cache_redis_keys_delete` |
| GET | `/api/admin/monitoring/cache/stats` | `cache_stats` |
| DELETE | `/api/admin/monitoring/cache/users/{user_id}` | `cache_users_delete` |
| GET | `/api/admin/monitoring/resilience-status` | `monitoring_resilience` |
| GET | `/api/admin/monitoring/resilience/circuit-history` | `resilience_circuit_history` |
| DELETE | `/api/admin/monitoring/resilience/error-stats` | `monitoring_resilience` |
| GET | `/api/admin/monitoring/suspicious-activities` | `suspicious_activities` |
| GET | `/api/admin/monitoring/system-status` | `user_behavior` |
| GET | `/api/admin/monitoring/trace/stats/provider/{id}` | `trace_provider_stats` |
| GET | `/api/admin/monitoring/trace/stats/provider/{provider_id}` | `trace_provider_stats` |
| GET | `/api/admin/monitoring/trace/{id}` | `trace_request` |
| GET | `/api/admin/monitoring/trace/{request_id}` | `trace_request` |
| GET | `/api/admin/monitoring/user-behavior/{user_id}` | `user_behavior` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/monitoring/audit-logs?days=14&limit=20 -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/monitoring/trace/{request_id} -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/monitoring/cache/stats -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X DELETE https://<网关域名>/api/admin/monitoring/cache -H "Authorization: Bearer ae-xxx"
```
