# 历史权限空值升级修复

## 原因

旧版 API Key、用户、用户组及上游 Key 允许把 JSON 字面量 `null`、字符串 `"null"`（忽略大小写和首尾空白）以及空字符串作为未设置的权限。严格权限读取启用后，这些值会触发 `contains JSON null; use SQL NULL for an unset policy` 等错误，影响 API Key 鉴权、列表和用户、用户组读取。管理令牌的 IP 限制和权限也曾接受 JSON 字面量 `null`，但不接受字符串空值；严格校验同样会使这些旧令牌失效。

增量迁移 `20260908000000_normalize_legacy_policy_nulls.sql` 将这些已知的旧版空值转换成 SQL `NULL`，不修改既有迁移及其校验和。新安装和已有数据库升级均通过同一迁移机制执行。

## 覆盖范围

| 表 | 字段 |
| --- | --- |
| `api_keys` | `allowed_providers`、`allowed_api_formats`、`allowed_models`、`ip_rules` |
| `users` | `allowed_providers`、`allowed_api_formats`、`allowed_models` |
| `user_groups` | `allowed_providers`、`allowed_api_formats`、`allowed_models` |
| `provider_api_keys` | `api_formats`、`allowed_models` |
| `management_tokens` | `allowed_ips`、`permissions`（仅 JSON 字面量 `null`） |

共 5 张表、14 个字段，同时兼容 `json` 和 `jsonb` 列。迁移可重复执行，且只更新命中旧版空值的字段：

- 保留 SQL `NULL`、空数组 `[]`、正常名单、字符串化的名单和单字符串策略。
- 保留 `specific`、`deny_all`、`inherit` 等权限模式，不修改用户组成员关系。
- 管理令牌只转换 JSON 字面量 `null`，恢复旧版既有的未设置 IP 限制或 `legacy_full` 权限语义；字符串 `"null"`、空字符串、空数组和其他非法权限不转换，避免把原本无效的令牌权限变成旧版全权限。
- 不清理数组内部的 `null`、空白元素、数字、对象或其他异常权限；它们仍由严格读取逻辑拒绝，不能借迁移变成无限制访问。
- 不修改其他 JSON 字段，例如 `metadata` 内的 JSON `null`。

## 升级方式

先备份数据库，部署包含此迁移的新网关二进制或镜像，并保留原来的数据库连接配置与密钥。仅重启不包含此迁移的旧版本不会修复数据。

默认 `AETHER_GATEWAY_DATABASE_MODE=auto` 会在启动时执行挂起迁移，再进入正常服务。使用 `verify-only` 的部署需在相同数据库连接配置下先执行新版本的准备命令，再重启服务：

```sh
aether-gateway db prepare
```

迁移只更新上述权限列，不删除业务记录、不替换用户或 Key、不重建数据库。不要把空数组或任意异常 JSON 统一改成 SQL `NULL`，也不要通过关闭严格权限校验绕过问题。

升级后可以只读检查迁移记录：

```sql
SELECT version, description, success
FROM public._sqlx_migrations
WHERE version = 20260908000000;
```

该记录应存在且 `success = true`。若仍有权限解码错误，核对实际报错实例连接的数据库，以及是否有旧进程或外部工具继续写入旧格式；不要将其他类型的权限错误直接作为空值清除。
