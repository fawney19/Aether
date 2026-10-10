# Provider 运维（/api/admin/provider-ops）

> 家族：`provider_ops_manage` · 权限域：`admin:provider_ops`
> 说明：连接 / 断开 / 余额 / checkin / 架构列表
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/provider-ops/architectures` | `list_architectures` |
| GET | `/api/admin/provider-ops/architectures/{id}` | `get_architecture` |
| POST | `/api/admin/provider-ops/batch/balance` | `batch_balance` |
| POST | `/api/admin/provider-ops/providers/{id}/actions/{id}` | `execute_provider_action` |
| GET | `/api/admin/provider-ops/providers/{id}/balance` | `get_provider_balance` |
| POST | `/api/admin/provider-ops/providers/{id}/balance` | `refresh_provider_balance` |
| POST | `/api/admin/provider-ops/providers/{id}/checkin` | `provider_checkin` |
| DELETE | `/api/admin/provider-ops/providers/{id}/config` | `delete_provider_config` |
| GET | `/api/admin/provider-ops/providers/{id}/config` | `get_provider_config` |
| PUT | `/api/admin/provider-ops/providers/{id}/config` | `save_provider_config` |
| POST | `/api/admin/provider-ops/providers/{id}/connect` | `connect_provider` |
| POST | `/api/admin/provider-ops/providers/{id}/disconnect` | `disconnect_provider` |
| GET | `/api/admin/provider-ops/providers/{id}/status` | `get_provider_status` |
| POST | `/api/admin/provider-ops/providers/{id}/verify` | `verify_provider` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/provider-ops/architectures -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/provider-ops/providers/{provider_id}/connect \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"type":"new_api","config":{}}'
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/provider-ops/batch/balance -H "Authorization: Bearer ae-xxx"
```
