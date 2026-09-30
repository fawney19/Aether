# endpoints_manage（/api/admin/endpoints_manage）

> 家族：`endpoints_manage` · 权限域：`admin:endpoints_manage`
> 说明：
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/endpoints/defaults/{id}/body-rules` | `default_body_rules` |
| POST | `/api/admin/endpoints/keys/batch-delete` | `batch_delete_keys` |
| GET | `/api/admin/endpoints/keys/grouped-by-format` | `keys_grouped_by_format` |
| DELETE | `/api/admin/endpoints/keys/{id}` | `delete_key` |
| PUT | `/api/admin/endpoints/keys/{id}` | `update_key` |
| POST | `/api/admin/endpoints/keys/{id}/clear-oauth-invalid` | `clear_oauth_invalid` |
| POST | `/api/admin/endpoints/keys/{id}/codex-reset-credit/consume` | `codex_reset_credit_consume` |
| GET | `/api/admin/endpoints/keys/{id}/export` | `export_key` |
| POST | `/api/admin/endpoints/keys/{id}/reset-cycle-stats` | `reset_cycle_stats` |
| GET | `/api/admin/endpoints/keys/{id}/reveal` | `reveal_key` |
| GET | `/api/admin/endpoints/providers/{id}/endpoints` | `list_provider_endpoints` |
| POST | `/api/admin/endpoints/providers/{id}/endpoints` | `create_endpoint` |
| GET | `/api/admin/endpoints/providers/{id}/keys` | `list_provider_keys` |
| POST | `/api/admin/endpoints/providers/{id}/keys` | `create_provider_key` |
| POST | `/api/admin/endpoints/providers/{id}/refresh-quota` | `refresh_quota` |
| DELETE | `/api/admin/endpoints/{id}` | `delete_endpoint` |
| GET | `/api/admin/endpoints/{id}` | `get_endpoint` |
| PUT | `/api/admin/endpoints/{id}` | `update_endpoint` |