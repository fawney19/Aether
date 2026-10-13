# global_models_manage（/api/admin/global_models_manage）

> 家族：`global_models_manage` · 权限域：`admin:global_models_manage`
> 说明：
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/models/global` | `list_global_models` |
| POST | `/api/admin/models/global` | `create_global_model` |
| POST | `/api/admin/models/global/batch-delete` | `batch_delete_global_models` |
| DELETE | `/api/admin/models/global/{id}` | `delete_global_model` |
| GET | `/api/admin/models/global/{id}` | `get_global_model` |
| PATCH | `/api/admin/models/global/{id}` | `update_global_model` |
| POST | `/api/admin/models/global/{id}/assign-to-providers` | `assign_to_providers` |
| GET | `/api/admin/models/global/{id}/providers` | `global_model_providers` |
| GET | `/api/admin/models/global/{id}/routing` | `routing_preview` |