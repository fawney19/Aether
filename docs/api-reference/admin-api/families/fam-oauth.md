# OAuth 配置（/api/admin/oauth）

> 家族：`oauth_manage` · 权限域：`admin:oauth`
> 说明：OAuth 提供商配置
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/oauth/providers` | `list_providers` |
| DELETE | `/api/admin/oauth/providers/{provider_type}` | `delete_provider` |
| GET | `/api/admin/oauth/providers/{provider_type}` | `get_provider` |
| PUT | `/api/admin/oauth/providers/{provider_type}` | `upsert_provider` |
| POST | `/api/admin/oauth/providers/{provider_type}/test` | `test_provider` |
| GET | `/api/admin/oauth/supported-types` | `supported_types` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/oauth/providers -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/oauth/supported-types -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X PUT https://<网关域名>/api/admin/oauth/providers/{provider_type} \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"client_id":"<id>","scopes":["openid"],"redirect_uri":"https://..."}'
```
