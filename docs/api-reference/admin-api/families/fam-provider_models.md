# provider_models_manage（/api/admin/provider_models_manage）

> 家族：`provider_models_manage` · 权限域：`admin:provider_models_manage`
> 说明：
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| POST | `/api/admin/providers/{id}/assign-global-models` | `assign_global_models` |
| GET | `/api/admin/providers/{id}/available-source-models` | `available_source_models` |
| POST | `/api/admin/providers/{id}/import-from-upstream` | `import_from_upstream` |
| GET | `/api/admin/providers/{provider_id}/models` | `list_provider_models` |
| POST | `/api/admin/providers/{provider_id}/models` | `create_provider_model` |
| POST | `/api/admin/providers/{provider_id}/models/batch` | `batch_create_provider_models` |
| DELETE | `/api/admin/providers/{provider_id}/models/{model_id}` | `delete_provider_model` |
| GET | `/api/admin/providers/{provider_id}/models/{model_id}` | `get_provider_model` |
| PATCH | `/api/admin/providers/{provider_id}/models/{model_id}` | `update_provider_model` |