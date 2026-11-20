# 调度分组（/api/admin/routing）

> 家族：`routing_profiles_manage` · 权限域：`admin:routing_profiles`
> 说明：调度分组与绑定
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/routing/bindings` | `list_bindings` |
| POST | `/api/admin/routing/bindings` | `create_binding` |
| DELETE | `/api/admin/routing/bindings/{id}` | `delete_binding` |
| PATCH | `/api/admin/routing/bindings/{id}` | `update_binding` |
| GET | `/api/admin/routing/groups` | `list_groups` |
| POST | `/api/admin/routing/groups` | `create_group` |
| DELETE | `/api/admin/routing/groups/{id}` | `delete_group` |
| GET | `/api/admin/routing/groups/{id}` | `get_group` |
| PATCH | `/api/admin/routing/groups/{id}` | `update_group` |
| POST | `/api/admin/routing/groups/{id}/dry-run` | `dry_run_group` |
| POST | `/api/admin/routing/groups/{id}/publish` | `publish_group` |
| GET | `/api/admin/routing/groups/{id}/versions` | `list_group_versions` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/routing/groups -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/routing/groups \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"name":"high-priority","description":"","enabled":true,"config_json":{}}'
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/routing/bindings \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"group_id":"<id>","subject_type":"user","subject_id":"<user_id>","is_default":true}'
```
