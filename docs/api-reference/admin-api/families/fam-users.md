# 用户（/api/admin/users）

> 家族：`users_manage` · 权限域：`admin:users`
> 说明：用户 CRUD / 会话 / 用户 API key / 用户组
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/user-groups` | `list_user_groups` |
| POST | `/api/admin/user-groups` | `create_user_group` |
| PUT | `/api/admin/user-groups/default` | `set_default_user_group` |
| DELETE | `/api/admin/user-groups/{id}` | `delete_user_group` |
| PUT | `/api/admin/user-groups/{id}` | `update_user_group` |
| GET | `/api/admin/user-groups/{id}/members` | `list_user_group_members` |
| PUT | `/api/admin/user-groups/{id}/members` | `replace_user_group_members` |
| GET | `/api/admin/users` | `list_users` |
| POST | `/api/admin/users` | `create_user` |
| POST | `/api/admin/users/batch-action` | `batch_action_users` |
| POST | `/api/admin/users/resolve-selection` | `resolve_user_selection` |
| DELETE | `/api/admin/users/{user_id}` | `delete_user` |
| GET | `/api/admin/users/{user_id}` | `get_user` |
| PUT | `/api/admin/users/{user_id}` | `update_user` |
| GET | `/api/admin/users/{user_id}/api-keys` | `list_user_api_keys` |
| POST | `/api/admin/users/{user_id}/api-keys` | `create_user_api_key` |
| DELETE | `/api/admin/users/{user_id}/api-keys/{api_key_id}` | `delete_user_api_key` |
| PUT | `/api/admin/users/{user_id}/api-keys/{api_key_id}` | `update_user_api_key` |
| GET | `/api/admin/users/{user_id}/api-keys/{api_key_id}/full-key` | `reveal_user_api_key` |
| PATCH | `/api/admin/users/{user_id}/api-keys/{api_key_id}/lock` | `lock_user_api_key` |
| GET | `/api/admin/users/{user_id}/billing/entitlements` | `list_user_billing_entitlements` |
| POST | `/api/admin/users/{user_id}/billing/grant-plan` | `grant_user_billing_plan` |
| DELETE | `/api/admin/users/{user_id}/sessions` | `delete_user_sessions` |
| GET | `/api/admin/users/{user_id}/sessions` | `list_user_sessions` |
| DELETE | `/api/admin/users/{user_id}/sessions/{session_id}` | `delete_user_session` |

## 示例

```bash
curl -sS -X POST https://<网关域名>/api/admin/users \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"username":"newbie","password":"<pw>","email":"u@example.com","role":"user","initial_gift_usd":1.0,"unlimited":false,"group_ids":[]}'
```

```bash
curl -sS -X PATCH https://<网关域名>/api/admin/users/{user_id} \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"is_active":false,"role":"user"}'
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/users?limit=20&search=alice -H "Authorization: Bearer ae-xxx"
```
