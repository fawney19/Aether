# Provider OAuth（/api/admin/provider-oauth）

> 家族：`provider_oauth_manage` · 权限域：`admin:provider_oauth`
> 说明：渠道 OAuth 授权（cookie/agent-identity/device）
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| POST | `/api/admin/provider-oauth/keys/{key_id}/complete` | `complete_key_oauth` |
| POST | `/api/admin/provider-oauth/keys/{key_id}/refresh` | `refresh_key_oauth` |
| POST | `/api/admin/provider-oauth/keys/{key_id}/start` | `start_key_oauth` |
| POST | `/api/admin/provider-oauth/providers/{provider_id}/agent-identity-import/tasks` | `start_agent_identity_import_task` |
| GET | `/api/admin/provider-oauth/providers/{provider_id}/agent-identity-import/tasks/{id}` | `get_agent_identity_import_task_status` |
| POST | `/api/admin/provider-oauth/providers/{provider_id}/batch-import` | `batch_import_oauth` |
| POST | `/api/admin/provider-oauth/providers/{provider_id}/batch-import/tasks` | `start_batch_import_oauth_task` |
| GET | `/api/admin/provider-oauth/providers/{provider_id}/batch-import/tasks/{id}` | `get_batch_import_task_status` |
| POST | `/api/admin/provider-oauth/providers/{provider_id}/complete` | `complete_provider_oauth` |
| POST | `/api/admin/provider-oauth/providers/{provider_id}/cookie-authorize` | `cookie_authorize` |
| POST | `/api/admin/provider-oauth/providers/{provider_id}/cookie-authorize/tasks` | `start_cookie_authorize_task` |
| GET | `/api/admin/provider-oauth/providers/{provider_id}/cookie-authorize/tasks/{id}` | `get_cookie_authorize_task_status` |
| POST | `/api/admin/provider-oauth/providers/{provider_id}/device-authorize` | `device_authorize` |
| POST | `/api/admin/provider-oauth/providers/{provider_id}/device-poll` | `device_poll` |
| POST | `/api/admin/provider-oauth/providers/{provider_id}/import-refresh-token` | `import_refresh_token` |
| POST | `/api/admin/provider-oauth/providers/{provider_id}/start` | `start_provider_oauth` |
| GET | `/api/admin/provider-oauth/supported-types` | `supported_types` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/provider-oauth/supported-types -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/provider-oauth/keys/{key_id}/start \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"provider_type":"claude"}'
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/provider-oauth/providers/{provider_id}/cookie-authorize/tasks \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"callback_url":"https://..."}'
```
