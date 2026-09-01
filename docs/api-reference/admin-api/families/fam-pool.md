# 号池（/api/admin/pool）

> 家族：`pool_manage` · 权限域：`admin:pool`
> 说明：渠道 key 批量导入/更新/动作/调度预设/概览
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/pool/overview` | `overview` |
| GET | `/api/admin/pool/scheduling-presets` | `scheduling_presets` |
| GET | `/api/admin/pool/{provider_id}/keys` | `list_keys` |
| POST | `/api/admin/pool/{provider_id}/keys/batch-action` | `batch_action_keys` |
| GET | `/api/admin/pool/{provider_id}/keys/batch-delete-task/{task_id}` | `batch_delete_task_status` |
| POST | `/api/admin/pool/{provider_id}/keys/batch-import` | `batch_import_keys` |
| PATCH | `/api/admin/pool/{provider_id}/keys/batch-update` | `batch_update_keys` |
| POST | `/api/admin/pool/{provider_id}/keys/cleanup-banned` | `cleanup_banned_keys` |
| POST | `/api/admin/pool/{provider_id}/keys/resolve-selection` | `resolve_selection` |
| GET | `/api/admin/pool/{provider_id}/scores` | `scores` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/pool/overview -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/pool/{provider_id}/keys/batch-import \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"keys":["sk-xxx","sk-yyy"]}'
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/pool/scheduling-presets -H "Authorization: Bearer ae-xxx"
```
