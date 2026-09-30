# management_tokens_manage（/api/admin/management_tokens_manage）

> 家族：`management_tokens_manage` · 权限域：`admin:management_tokens_manage`
> 说明：
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/management-tokens` | `list_tokens` |
| POST | `/api/admin/management-tokens` | `create_token` |
| GET | `/api/admin/management-tokens/permissions/catalog` | `permissions_catalog` |
| DELETE | `/api/admin/management-tokens/{id}` | `delete_token` |
| GET | `/api/admin/management-tokens/{id}` | `get_token` |
| PUT | `/api/admin/management-tokens/{id}` | `update_token` |
| POST | `/api/admin/management-tokens/{id}/regenerate` | `regenerate_token` |
| PATCH | `/api/admin/management-tokens/{id}/status` | `toggle_status` |