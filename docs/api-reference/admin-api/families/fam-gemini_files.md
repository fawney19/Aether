# Gemini 文件（/api/admin/gemini-files）

> 家族：`gemini_files_manage` · 权限域：`admin:gemini_files`
> 说明：文件映射 / 能力 key / 上传 / 统计
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/gemini-files/capable-keys` | `capable_keys` |
| DELETE | `/api/admin/gemini-files/mappings` | `cleanup_mappings` |
| GET | `/api/admin/gemini-files/mappings` | `list_mappings` |
| DELETE | `/api/admin/gemini-files/mappings/{id}` | `delete_mapping` |
| GET | `/api/admin/gemini-files/stats` | `stats` |
| POST | `/api/admin/gemini-files/upload` | `upload` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/gemini-files/mappings -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/gemini-files/upload \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"file_name":"a.png","mime_type":"image/png","content_base64":"<base64>"}'
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/gemini-files/capable-keys -H "Authorization: Bearer ae-xxx"
```
