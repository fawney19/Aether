# LDAP（/api/admin/ldap）

> 家族：`ldap_manage` · 权限域：`admin:ldap`
> 说明：LDAP 配置 / 测试
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/ldap/config` | `get_config` |
| PUT | `/api/admin/ldap/config` | `set_config` |
| POST | `/api/admin/ldap/test` | `test_connection` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/ldap/config -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X PUT https://<网关域名>/api/admin/ldap/config \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"server_url":"ldap://<host>","base_dn":"dc=example,dc=com","user_search_filter":"(uid=?)"}'
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/ldap/test \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"username":"test","password":"<pw>"}'
```
