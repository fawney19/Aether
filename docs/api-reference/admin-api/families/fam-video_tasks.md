# 视频任务（/api/admin/video-tasks）

> 家族：`video_tasks_manage` · 权限域：`admin:video_tasks`
> 说明：视频生成任务管理
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| GET | `/api/admin/video-tasks` | `list_tasks` |
| GET | `/api/admin/video-tasks/stats` | `stats` |
| POST | `/api/admin/video-tasks/{id}/cancel` | `cancel` |
| GET | `/api/admin/video-tasks/{id}/video` | `video` |

## 示例

```bash
curl -sS -X GET https://<网关域名>/api/admin/video-tasks?status=completed -H "Authorization: Bearer ae-xxx"
```

```bash
curl -sS -X GET https://<网关域名>/api/admin/video-tasks/{task_id}/video -H "Authorization: Bearer ae-xxx"
```
