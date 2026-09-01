# 后台任务（/api/admin/tasks）

> 家族：`tasks_manage` · 权限域：`admin:tasks`
> 说明：任务列表 / 详情 / 触发 / 取消
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/tasks` | `list_tasks` |
| GET | `/api/admin/tasks/stats` | `stats` |
| POST | `/api/admin/tasks/{id}/cancel` | `cancel` |
| GET | `/api/admin/tasks/{id}/events` | `events` |
| POST | `/api/admin/tasks/{id}/trigger` | `trigger` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/tasks?status=running -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X POST https://<网关域名>/api/admin/tasks/{task_id}/cancel -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/tasks/stats -H "Authorization: Bearer ae-xxx"
```
