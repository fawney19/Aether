# 鉴权机制与管理令牌

## 一、三种鉴权方式（代码核实）

| 方式 | 载体 | 适用 | 校验逻辑 |
|---|---|---|---|
| **管理令牌** | `Authorization: Bearer ae-xxxx` | 脚本/API 直调（推荐） | `handlers/proxy/mod.rs::maybe_promote_management_token_admin_principal`：sha256 哈希 → `management_tokens` 表 → 校验 `is_active` / `expires_at` / `allowed_ips` → 用户角色必须可访问管理台 → 解析 `permissions` |
| **Web 会话 JWT** | `Authorization: Bearer <JWT>` + `x-client-device-id` 头 | 前端页面 | `control/auth/resolution.rs::resolve_local_admin_principal`：JWT 由 `JWT_SECRET_KEY`（HS256）签发，payload 需含 `user_id`/`session_id`/`role`；校验 `user_sessions` 表 + 客户端设备 ID |
| **内部信任头** | `x-gateway-trusted-*` 系列 | 网关内部（前端→网关经反代） | `control/auth/credentials.rs::extract_trusted_admin_headers`：`x-gateway-trusted-user-id` / `-role` / `-session-id` / `-management-token-id`，需 `x-gateway-trusted` 标记头 |

## 二、管理令牌（重点）

### 令牌格式

- 前缀：`ae-`（新）；旧版兼容前缀 `ae_`
- 显示前缀：前 10 字符（`token_prefix` 字段）
- 库内只存 `token_hash`（sha256 hex），明文只在创建/重生成时返回一次

### 权限模型

31 个权限域（scope），每个域有 `read` / `write` 权限位。权限 key 格式：`<scope>:<access>`，如 `models:read`、`models:write`、`usage:read`。

全部权限域（`/api/admin/management-tokens/permissions/catalog` 返回同款）：
`adaptive` 自适应调度、`announcements` 公告、`api_keys` API 密钥、`billing` 账单、`endpoints_health` 端点健康、`endpoints_manage` 端点配置、`endpoints_rpm` 端点 RPM、`gemini_files` Gemini 文件、`ldap` LDAP、`management_tokens` 访问令牌（不可分配）、`models` 模型、`modules` 模块管理、`monitoring` 监控、`oauth` OAuth 配置、`payments` 支付、`pool` 号池、`provider_ops` Provider 运维、`provider_oauth` Provider OAuth、`provider_query` Provider 查询、`provider_strategy` Provider 策略、`providers` 供应商与模型、`proxy_nodes` 代理节点、`routing_profiles` 调度分组、`security` 安全、`stats` 统计、`system` 系统、`tasks` 后台任务、`usage` 用量、`users` 用户、`video_tasks` 视频任务、`wallets` 钱包

## 三、管理令牌 API

### GET `/api/admin/management-tokens/permissions/catalog`

返回全部权限域及访问级别（供前端渲染权限选择器）。

### GET `/api/admin/management-tokens`

列出管理令牌。查询参数：`skip`、`limit`。

### POST `/api/admin/management-tokens`

创建管理令牌（HTTP 201）。

请求体：

```json
{
  "name": "ops-bot",
  "description": "运维脚本用",
  "allowed_ips": ["1.2.3.4/32"],
  "permissions": ["models:read", "models:write", "usage:read"],
  "expires_at": "2027-01-01"
}
```

- `name`：必填，1~100 字符
- `description`：可选，≤500 字符
- `allowed_ips`：可选，IP/CIDR 列表（不填则不限）
- `permissions`：可选，不填=全部权限（`admin:management_tokens` 域不可分配）
- `expires_at`：可选，`YYYY-MM-DD` 或 RFC3339

响应：

```json
{
  "message": "Management Token 创建成功",
  "token": "ae-<明文仅此一次>",
  "data": { "id": "...", "name": "ops-bot", "token_prefix": "ae-xxxxxx", "is_active": true, "permissions": [...], "expires_at": "...", "created_at": "..." }
}
```

> ⚠️ `token` 明文只在创建时返回，之后无法再查看；请立即保存。

### GET `/api/admin/management-tokens/{token_id}`

令牌详情（不含明文）。

### PUT `/api/admin/management-tokens/{token_id}`

更新令牌（name/description/allowed_ips/permissions/expires_at）。

### PATCH `/api/admin/management-tokens/{token_id}`

切换启用/停用（toggle status）。

### POST `/api/admin/management-tokens/{token_id}/regenerate`

重生成令牌明文（旧 token 立即失效，返回新 `ae-` 明文）。

### DELETE `/api/admin/management-tokens/{token_id}`

删除令牌。

## 四、Web 登录（JWT 会话）

### POST `/api/auth/login`

请求体：`{"username": "...", "password": "..."}`（或 email）

响应：

```json
{
  "access_token": "<JWT, admin 用户以 admin: 开头>",
  "token_type": "bearer",
  "expires_in": 86400,
  "refresh_token": "<cookie 同时写入>"
}
```

调用 Admin API 时：`Authorization: Bearer <access_token>` + `x-client-device-id: <与登录时一致的设备标识>`。

### POST `/api/auth/refresh` / `/api/auth/logout` / `/api/auth/me`

- `refresh`：用 HttpOnly cookie 中的 refresh_token 换新 access_token（轮换机制，重用旧 refresh 会吊销整个会话）
- `logout`：吊销会话
- `me`：当前用户信息 + 钱包摘要

## 五、权限校验失败时的表现

- 令牌无效/过期/IP 不在白名单 → 请求落入 `admin_proxy` 但无 admin_principal → 返回 401/403
- 权限域不足 → `ManagementTokenPermissionDenied` → 403（响应含 `required_permission` 字段）
- 注意：**只有 GET 请求默认按对应 scope 的 read 权限放行**；写操作（POST/PUT/PATCH/DELETE）要求 `write` 权限，由 `access_for_method` 映射（见 `control/management_token_permissions.rs`）
