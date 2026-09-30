# endpoints_rpm（/api/admin/endpoints_rpm）

> 家族：`endpoints_rpm` · 权限域：`admin:endpoints_rpm`
> 说明：
> 鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点表

| 方法 | 路径 | kind |
|---|---|---|
| DELETE | `/api/admin/endpoints/rpm/key/{id}` | `reset_key_rpm` |
| GET | `/api/admin/endpoints/rpm/key/{id}` | `key_rpm` |