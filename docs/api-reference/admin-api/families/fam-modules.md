# 功能模块（/api/admin/modules）

> 家族：`modules_manage` · 权限域：`admin:modules`
> 说明：模块状态 / 启停
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/modules/status` | `status_list` |
| GET | `/api/admin/modules/status/{id}` | `status_detail` |
| PUT | `/api/admin/modules/status/{id}/enabled` | `set_enabled` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/modules/status -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/modules/{module}/set-enabled \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"enabled":true}'
```
