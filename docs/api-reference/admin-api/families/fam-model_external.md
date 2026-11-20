# 外部模型源（/api/admin/models/external）

> 家族：`model_external_manage` · 权限域：`admin:models`
> 说明：外部模型目录 / 缓存 / 配置
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/models/external` | `external` |
| DELETE | `/api/admin/models/external/cache` | `clear_external_cache` |
| GET | `/api/admin/models/external/config` | `external_config_get` |
| PUT | `/api/admin/models/external/config` | `external_config_set` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/models/external -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/models/external/config -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X DELETE https://<网关域名>/api/admin/models/external/cache -H "Authorization: Bearer ae-xxx"
```
