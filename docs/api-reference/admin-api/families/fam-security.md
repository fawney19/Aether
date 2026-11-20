# IP 安全（/api/admin/security/ip）

> 家族：`security_manage` · 权限域：`admin:security`
> 说明：IP 黑白名单管理
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/security/ip/blacklist` | `blacklist_list` |
| POST | `/api/admin/security/ip/blacklist` | `blacklist_add` |
| GET | `/api/admin/security/ip/blacklist/stats` | `blacklist_stats` |
| DELETE | `/api/admin/security/ip/blacklist/{id}` | `blacklist_remove` |
| GET | `/api/admin/security/ip/whitelist` | `whitelist_list` |
| POST | `/api/admin/security/ip/whitelist` | `whitelist_add` |
| DELETE | `/api/admin/security/ip/whitelist/{id}` | `whitelist_remove` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/security/ip/blacklist -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/security/ip/blacklist \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"ip":"1.2.3.4","reason":"abuse"}'
```

```bash
curl -sS -X DELETE https://<网关域名>/api/admin/security/ip/blacklist/{ip} -H "Authorization: Bearer ae-xxx"
```
