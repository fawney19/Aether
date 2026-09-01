# api_keys_manage（/api/admin/api_keys_manage）

> 家族：`api_keys_manage` · 权限域：`admin:api_keys_manage`
> 说明：
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/api-keys` | `list_api_keys` |
| POST | `/api/admin/api-keys` | `create_api_key` |
| DELETE | `/api/admin/api-keys/{id}` | `delete_api_key` |
| GET | `/api/admin/api-keys/{id}` | `api_key_detail` |
| PATCH | `/api/admin/api-keys/{id}` | `toggle_api_key` |
| PUT | `/api/admin/api-keys/{id}` | `update_api_key` |
| POST | `/api/admin/api-keys/{id}/install-sessions` | `create_api_key_install_session` |