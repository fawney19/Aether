# 其余 Admin 家族（端点级索引）

> 每个家族的**完整端点表（方法/路径/kind）+ 参数说明 + 可复用 curl 示例**都在 [families/](./families/) 目录下按文件组织。
> 本节是快速导航。所有端点统一鉴权：`Authorization: Bearer ae-xxx`（管理令牌）或 Web 会话 JWT。

## 端点级文档导航

| 家族 | 文件 | 端点数 | 权限域 |
|---|---|---|---|
| 系统 | [fam-system.md](./families/fam-system.md) | 10+（configs/export/import/cleanup/purge/update） | `admin:system` |
| 用户 | [fam-users.md](./families/fam-users.md) | 16+（CRUD/会话/API key/用户组） | `admin:users` |
| 计费 | [fam-billing.md](./families/fam-billing.md) | 22（collectors/plans/rules/presets） | `admin:billing` |
| 支付 | [fam-payments.md](./families/fam-payments.md) | 10（orders/callbacks/redeem-codes/epay） | `admin:payments` |
| 钱包 | [fam-wallets.md](./families/fam-wallets.md) | 8（list/detail/ledger/recharge/adjust/refund） | `admin:wallets` |
| 号池 | [fam-pool.md](./families/fam-pool.md) | 6+（overview/presets/keys 批量） | `admin:pool` |
| 代理节点 | [fam-proxy_nodes.md](./families/fam-proxy_nodes.md) | 28（CRUD/register/upgrade/metrics） | `admin:proxy_nodes` |
| 监控 | [fam-monitoring.md](./families/fam-monitoring.md) | 11（audit-logs/cache/trace/resilience） | `admin:monitoring` |
| 统计 | [fam-stats.md](./families/fam-stats.md) | 22（time-series/leaderboard/cost/performance） | `admin:stats` |
| 用量 | [fam-usage.md](./families/fam-usage.md) | 19（records/stats/heatmap/replay） | `admin:usage` |
| IP 安全 | [fam-security.md](./families/fam-security.md) | 14（blacklist/whitelist） | `admin:security` |
| 后台任务 | [fam-tasks.md](./families/fam-tasks.md) | 6（list/detail/events/trigger/cancel） | `admin:tasks` |
| 视频任务 | [fam-video_tasks.md](./families/fam-video_tasks.md) | 4 | `admin:video_tasks` |
| 推荐奖励 | [fam-referrals.md](./families/fam-referrals.md) | 4 | `admin:referrals` |
| LDAP | [fam-ldap.md](./families/fam-ldap.md) | 3 | `admin:ldap` |
| 功能模块 | [fam-modules.md](./families/fam-modules.md) | 3 | `admin:modules` |
| 自适应调度 | [fam-adaptive.md](./families/fam-adaptive.md) | 6 | `admin:adaptive` |
| Gemini 文件 | [fam-gemini_files.md](./families/fam-gemini_files.md) | 6 | `admin:gemini_files` |
| Provider 运维 | [fam-provider_ops.md](./families/fam-provider_ops.md) | 14 | `admin:provider_ops` |
| Provider 查询 | [fam-provider_query.md](./families/fam-provider_query.md) | 3 | `admin:provider_query` |
| Provider 策略 | [fam-provider_strategy.md](./families/fam-provider_strategy.md) | 4 | `admin:provider_strategy` |
| 外部模型源 | [fam-model_external.md](./families/fam-model_external.md) | 4 | `admin:models` |
| OAuth 配置 | [fam-oauth.md](./families/fam-oauth.md) | 6 | `admin:oauth` |
| Provider OAuth | [fam-provider_oauth.md](./families/fam-provider_oauth.md) | 17 | `admin:provider_oauth` |
| 调度分组 | [fam-routing_profiles.md](./families/fam-routing_profiles.md) | 12 | `admin:routing_profiles` |
| 供应商 | [fam-providers.md](./families/fam-providers.md) | 2（详见 04-providers-endpoints.md） | `admin:providers` |

> 已在前几章详述的家族（不在此重复）：管理令牌 [01-auth-and-tokens](./01-auth-and-tokens.md)、API 密钥 [02-api-keys](./02-api-keys.md)、全局模型 [03-models](./03-models.md)、供应商/端点 [04-providers-endpoints](./04-providers-endpoints.md)、用量 [05-usage](./05-usage.md)。
> provider-oauth（cookie 授权/agent-identity 导入）为 handler 级模块，接口路径见路由清单。

## 完整清单

所有端点（方法×路径，365 组合）见 [_route-inventory.md](./_route-inventory.md) 与 [raw-route-inventory.txt](./raw-route-inventory.txt)。
