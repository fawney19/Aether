# 系统（/api/admin/system）

> 家族：`system_manage` · 权限域：`admin:system`
> 说明：配置 / 备份 / 清理 / 升级 / 导入导出
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/system/api-formats` | `api_formats` |
| POST | `/api/admin/system/apply-update` | `apply_update` |
| GET | `/api/admin/system/aws-regions` | `aws_regions` |
| POST | `/api/admin/system/backups/s3/run` | `s3_backup_run` |
| GET | `/api/admin/system/check-update` | `check_update` |
| POST | `/api/admin/system/cleanup` | `cleanup` |
| GET | `/api/admin/system/cleanup/runs` | `cleanup_runs` |
| POST | `/api/admin/system/cleanup/usage/manual` | `cleanup_usage_manual` |
| GET | `/api/admin/system/cleanup/usage/preview` | `cleanup_usage_preview` |
| GET | `/api/admin/system/config/export` | `config_export` |
| POST | `/api/admin/system/config/import` | `config_import` |
| GET | `/api/admin/system/configs` | `configs_list` |
| DELETE | `/api/admin/system/configs/{key}` | `config_delete` |
| GET | `/api/admin/system/configs/{key}` | `config_get` |
| PUT | `/api/admin/system/configs/{key}` | `config_set` |
| GET | `/api/admin/system/data/export` | `data_export` |
| POST | `/api/admin/system/data/import` | `data_import` |
| GET | `/api/admin/system/email/templates` | `email_templates_list` |
| GET | `/api/admin/system/email/templates/{id}` | `email_template_get` |
| PUT | `/api/admin/system/email/templates/{id}` | `email_template_set` |
| POST | `/api/admin/system/email/templates/{id}/preview` | `email_template_preview` |
| POST | `/api/admin/system/email/templates/{id}/reset` | `email_template_reset` |
| POST | `/api/admin/system/important-notification/test` | `important_notification_test` |
| POST | `/api/admin/system/prepare-update` | `prepare_update` |
| POST | `/api/admin/system/purge/audit-logs` | `purge_audit_logs` |
| POST | `/api/admin/system/purge/config` | `purge_config` |
| POST | `/api/admin/system/purge/request-bodies` | `purge_request_bodies` |
| POST | `/api/admin/system/purge/request-bodies/task` | `purge_request_bodies_task` |
| POST | `/api/admin/system/purge/stats` | `purge_stats` |
| POST | `/api/admin/system/purge/usage` | `purge_usage` |
| POST | `/api/admin/system/purge/users` | `purge_users` |
| GET | `/api/admin/system/releases` | `releases` |
| POST | `/api/admin/system/rollback` | `rollback` |
| GET | `/api/admin/system/settings` | `settings_get` |
| PUT | `/api/admin/system/settings` | `settings_set` |
| POST | `/api/admin/system/smtp/test` | `smtp_test` |
| GET | `/api/admin/system/stats` | `stats` |
| GET | `/api/admin/system/update-capability` | `update_capability` |
| GET | `/api/admin/system/update-history` | `update_history` |
| GET | `/api/admin/system/update-status` | `update_status` |
| GET | `/api/admin/system/users/export` | `users_export` |
| POST | `/api/admin/system/users/import` | `users_import` |
| GET | `/api/admin/system/version` | `version` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/system/version -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/system/config/export -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X PUT https://<网关域名>/api/admin/system/configs/{key} \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"value":{}}'
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/system/cleanup/usage/preview -H "Authorization: Bearer ae-xxx"
```
