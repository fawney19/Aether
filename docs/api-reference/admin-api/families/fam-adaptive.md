# 自适应调度（/api/admin/adaptive）

> 家族：`adaptive_manage` · 权限域：`admin:adaptive`
> 说明：自适应 key 限额 / 学习 / 开关
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/adaptive/keys` | `list_keys` |
| DELETE | `/api/admin/adaptive/keys/{key_id}/learning` | `reset_learning` |
| PATCH | `/api/admin/adaptive/keys/{key_id}/limit` | `set_limit` |
| PATCH | `/api/admin/adaptive/keys/{key_id}/mode` | `toggle_mode` |
| GET | `/api/admin/adaptive/keys/{key_id}/stats` | `get_stats` |
| GET | `/api/admin/adaptive/summary` | `summary` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/adaptive/summary -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X PATCH https://<网关域名>/api/admin/adaptive/keys/{key_id}/limit \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"limit":100}'
```

```bash
curl -sS -X PATCH https://<网关域名>/api/admin/adaptive/keys/{key_id}/mode \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"enabled":true,"fixed_limit":50}'
```
