# endpoints_health（/api/admin/endpoints_health）

> 家族：`endpoints_health` · 权限域：`admin:endpoints_health`
> 说明：
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/endpoints/health/api-formats` | `health_api_formats` |
| GET | `/api/admin/endpoints/health/key/{id}` | `key_health` |
| PATCH | `/api/admin/endpoints/health/keys` | `recover_all_keys_health` |
| PATCH | `/api/admin/endpoints/health/keys/{id}` | `recover_key_health` |
| GET | `/api/admin/endpoints/health/models` | `health_models` |
| GET | `/api/admin/endpoints/health/providers` | `health_providers` |
| GET | `/api/admin/endpoints/health/related` | `health_related` |
| GET | `/api/admin/endpoints/health/status` | `health_status` |
| GET | `/api/admin/endpoints/health/summary` | `health_summary` |