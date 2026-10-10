# Admin API 总览

> 生产版本：zbsdsb/Aether v0.7.18+（Rust 重写版）
> 管理 API 全部位于 `/api/admin/*`，共 32 个 classified 路由家族 + handler 级模块（oauth/provider-oauth/routing 等），365 个方法×路径组合。
> 完整路由清单见 [_route-inventory.md](./_route-inventory.md) 与 [raw-route-inventory.txt](./raw-route-inventory.txt)。

## 路由家族一览

> 下表 35 行 = 32 个 classified 家族 + handler 级模块（oauth/provider-oauth/user-groups/announcements 等）；每个家族详细端点与示例见 [families/](./families/)。

| 家族 (route_family) | 权限域 (scope) | 路径前缀 | 端点数 | 说明 |
|---|---|---|---|---|
| `api_keys_manage` | `admin:api_keys` | `/api/admin/api-keys` | 7 | 独立余额 Key 管理 |
| `global_models_manage` | `admin:models` | `/api/admin/models/global` | 9 | 全局模型 CRUD |
| `model_catalog_manage` | `admin:models` | `/api/admin/models/catalog` | 1 | 模型目录 |
| `model_external_manage` | `admin:models` | `/api/admin/models/external` | 4 | 外部模型源 |
| `providers_manage` | `admin:providers` | `/api/admin/providers` | 11 | 供应商 CRUD |
| `provider_models_manage` | `admin:providers` | `/api/admin/providers/{id}/models` | 9 | 渠道模型映射 |
| `endpoints_manage` | `admin:endpoints_manage` | `/api/admin/endpoints` | 18 | 端点配置（含 body_rules） |
| `endpoints_health` | `admin:endpoints_health` | `/api/admin/endpoints/health` | 9 | 端点健康 |
| `endpoints_rpm` | `admin:endpoints_rpm` | `/api/admin/endpoints/rpm` | 2 | 端点 RPM |
| `management_tokens_manage` | `admin:management_tokens` | `/api/admin/management-tokens` | 8 | **管理令牌**（本机直调用） |
| `usage_manage` | `admin:usage` | `/api/admin/usage` | 11 | 用量查询 |
| `stats_manage` | `admin:stats` | `/api/admin/stats` | 11 | 统计报表 |
| `monitoring` | `admin:monitoring` | `/api/admin/monitoring` | 6 | 监控/审计/缓存 |
| `system_manage` | `admin:system` | `/api/admin/system` | 42 | 系统配置/备份/清理/升级 |
| `users_manage` | `admin:users` | `/api/admin/users` | 25 | 用户管理（含 user-groups） |
| `billing_manage` | `admin:billing` | `/api/admin/billing` | 15 | 计费规则/计划 |
| `payments_manage` | `admin:payments` | `/api/admin/payments` | 13 | 支付/兑换码 |
| `wallets_manage` | `admin:wallets` | `/api/admin/wallets` | 11 | 钱包 |
| `pool_manage` | `admin:pool` | `/api/admin/pool` | 10 | 号池 |
| `proxy_nodes_manage` | `admin:proxy_nodes` | `/api/admin/proxy-nodes` | 21 | 代理节点（tunnel） |
| `security_manage` | `admin:security` | `/api/admin/security/ip` | 7 | IP 黑白名单 |
| `provider_ops_manage` | `admin:provider_ops` | `/api/admin/provider-ops` | 14 | Provider 运维 |
| `provider_query_manage` | `admin:provider_query` | `/api/admin/provider-query` | 3 | Provider 测试 |
| `provider_strategy_manage` | `admin:provider_strategy` | `/api/admin/provider-strategy` | 4 | Provider 策略 |
| `provider_oauth_manage` | `admin:provider_oauth` | `/api/admin/provider-oauth` | — | Provider OAuth |
| `adaptive_manage` | `admin:adaptive` | `/api/admin/adaptive` | 6 | 自适应调度 |
| `gemini_files_manage` | `admin:gemini_files` | `/api/admin/gemini-files` | 6 | Gemini 文件 |
| `video_tasks_manage` | `admin:video_tasks` | `/api/admin/video-tasks` | 5 | 视频任务 |
| `tasks_manage` | `admin:tasks` | `/api/admin/tasks` | 6 | 后台任务 |
| `referrals_manage` | `admin:referrals` | `/api/admin/referrals` | 4 | 推荐奖励 |
| `ldap_manage` | `admin:ldap` | `/api/admin/ldap` | 3 | LDAP |
| `modules_manage` | `admin:modules` | `/api/admin/modules` | 3 | 功能模块 |
| `routing_profiles` | `admin:routing_profiles` | `/api/admin/routing` | 9 | 调度分组 |
| `oauth_manage` | `admin:oauth` | `/api/admin/oauth` | 6 | OAuth 提供商配置 |
| `provider_oauth_manage` | `admin:provider_oauth` | `/api/admin/provider-oauth` | 8 | 渠道 OAuth（cookie 授权等） |
| `user_groups_manage` | `admin:users` | `/api/admin/user-groups` | 4 | 用户组（在 users 模块内） |
| `announcements` | `admin:announcements` | `/api/admin/announcements` | — | 公告（handler 级） |

> 权限域 31 个，与 `/api/admin/management-tokens/permissions/catalog` 返回的 scope 一一对应。
> 家族表为 32 个 classified 家族；`user-groups`、`provider-oauth`、`announcements` 等 handler 分发族见各章节。

## 调用约定

- **Base URL**：`https://<网关域名>`（生产）；`http://127.0.0.1:8084`（容器内直连）
- **鉴权头**：`Authorization: Bearer <token>`，token 有两种：
  1. **管理令牌（推荐脚本使用）**：`ae-` 开头，见 [01-auth-and-tokens](./01-auth-and-tokens.md)
  2. **Web 登录 JWT**：`admin:` 前缀标记 + `x-client-device-id` 头（前端专用）
- **内容类型**：`Content-Type: application/json`
- **幂等**：PUT 全量更新；PATCH 部分更新（如 `models/global/{id}`）

## 通用错误格式

所有本地错误均为 OpenAI 风格：

```json
{
  "error": {
    "type": "http_error",
    "message": "具体错误信息"
  }
}
```

业务校验失败常见 HTTP 400 `{"detail": "..."}`；未授权 401；无权限 403；未找到 404；服务端异常 500。
每条响应带 `x-aether-trace-id` 头，可用 `_gateway/audit/*` 追踪。

## 快速上手（管理令牌直调）

```bash
# 1. 创建管理令牌（需 Web 登录态或现有管理令牌）
curl -sS https://<网关域名>/api/admin/management-tokens \
  -H "Authorization: Bearer <你的令牌>" -H "Content-Type: application/json" \
  -d '{"name":"ops","permissions":["models:read","models:write"],"expires_at":"2027-01-01"}'
# → {"message":"Management Token 创建成功","token":"ae-xxxx...","data":{...}}

# 2. 用新令牌查全局模型
curl -sS https://<网关域名>/api/admin/models/global \
  -H "Authorization: Bearer ae-xxxx..."

# 3. 查最近用量
curl -sS "https://<网关域名>/api/admin/usage/records?limit=20" \
  -H "Authorization: Bearer ae-xxxx..."
```
