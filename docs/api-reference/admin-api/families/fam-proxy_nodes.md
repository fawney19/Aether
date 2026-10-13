# 代理节点（/api/admin/proxy-nodes）

> 家族：`proxy_nodes_manage` · 权限域：`admin:proxy_nodes`
> 说明：tunnel 节点 CRUD / 心跳 / 升级 / 指标
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/proxy-nodes` | `list_nodes` |
| POST | `/api/admin/proxy-nodes/heartbeat` | `heartbeat_node` |
| POST | `/api/admin/proxy-nodes/install-sessions` | `create_proxy_node_install_session` |
| POST | `/api/admin/proxy-nodes/manual` | `create_manual_node` |
| GET | `/api/admin/proxy-nodes/metrics/fleet` | `list_fleet_metrics` |
| POST | `/api/admin/proxy-nodes/register` | `register_node` |
| POST | `/api/admin/proxy-nodes/test-url` | `test_proxy_url` |
| POST | `/api/admin/proxy-nodes/unregister` | `unregister_node` |
| POST | `/api/admin/proxy-nodes/upgrade` | `batch_upgrade_nodes` |
| POST | `/api/admin/proxy-nodes/upgrade/cancel` | `cancel_upgrade_rollout` |
| POST | `/api/admin/proxy-nodes/upgrade/clear-conflicts` | `clear_upgrade_rollout_conflicts` |
| POST | `/api/admin/proxy-nodes/upgrade/restore-skipped` | `restore_skipped_upgrade_rollout_nodes` |
| DELETE | `/api/admin/proxy-nodes/{node_id}` | `delete_node` |
| GET | `/api/admin/proxy-nodes/{node_id}` | `get_node` |
| PATCH | `/api/admin/proxy-nodes/{node_id}` | `update_manual_node` |
| PUT | `/api/admin/proxy-nodes/{node_id}/config` | `update_node_config` |
| GET | `/api/admin/proxy-nodes/{node_id}/events` | `list_node_events` |
| GET | `/api/admin/proxy-nodes/{node_id}/metrics` | `list_node_metrics` |
| POST | `/api/admin/proxy-nodes/{node_id}/test` | `test_node` |
| POST | `/api/admin/proxy-nodes/{node_id}/upgrade/retry` | `retry_upgrade_rollout_node` |
| POST | `/api/admin/proxy-nodes/{node_id}/upgrade/skip` | `skip_upgrade_rollout_node` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/proxy-nodes?status=online -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/proxy-nodes/manual \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"name":"node-3","ip":"<节点IP>","port":443,"is_manual":true}'
```

```bash
curl -sS -X PUT https://<网关域名>/api/admin/proxy-nodes/{node_id}/config \
  -H "Authorization: Bearer ae-xxx" -H "Content-Type: application/json" \
  -d '{"remote_config":{}}'
```
