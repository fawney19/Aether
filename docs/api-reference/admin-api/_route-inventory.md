# Admin API 路由清单（按家族）

> 来源：zbs-develop/Aether @ ba11206cd（v0.7.18+）：`control/route/admin.rs` + `control/route/admin/*.rs` + `control/route/oauth.rs` + `crates/aether-admin` + `handlers/admin/**/routes.rs` 代码提取

> 方法×路径组合 **365** 个；路径去重 305 个；家族 34 个。

## adaptive_manage（6）

- `DELETE` `/api/admin/adaptive/keys/{key_id}/learning`
- `GET` `/api/admin/adaptive/keys`
- `GET` `/api/admin/adaptive/keys/{key_id}/stats`
- `GET` `/api/admin/adaptive/summary`
- `PATCH` `/api/admin/adaptive/keys/{key_id}/limit`
- `PATCH` `/api/admin/adaptive/keys/{key_id}/mode`

## api_keys_manage（7）

- `DELETE` `/api/admin/api-keys/{id}`
- `GET` `/api/admin/api-keys`
- `GET` `/api/admin/api-keys/{id}`
- `PATCH` `/api/admin/api-keys/{id}`
- `POST` `/api/admin/api-keys`
- `POST` `/api/admin/api-keys/{id}/install-sessions`
- `PUT` `/api/admin/api-keys/{id}`

## billing_manage（15）

- `DELETE` `/api/admin/billing/plans/{id}`
- `GET` `/api/admin/billing/collectors`
- `GET` `/api/admin/billing/collectors/{id}`
- `GET` `/api/admin/billing/plans`
- `GET` `/api/admin/billing/presets`
- `GET` `/api/admin/billing/rules`
- `GET` `/api/admin/billing/rules/{id}`
- `PATCH` `/api/admin/billing/plans/{id}/status`
- `POST` `/api/admin/billing/collectors`
- `POST` `/api/admin/billing/plans`
- `POST` `/api/admin/billing/presets/apply`
- `POST` `/api/admin/billing/rules`
- `PUT` `/api/admin/billing/collectors/{id}`
- `PUT` `/api/admin/billing/plans/{id}`
- `PUT` `/api/admin/billing/rules/{id}`

## endpoints_health（9）

- `GET` `/api/admin/endpoints/health/api-formats`
- `GET` `/api/admin/endpoints/health/key/{id}`
- `GET` `/api/admin/endpoints/health/models`
- `GET` `/api/admin/endpoints/health/providers`
- `GET` `/api/admin/endpoints/health/related`
- `GET` `/api/admin/endpoints/health/status`
- `GET` `/api/admin/endpoints/health/summary`
- `PATCH` `/api/admin/endpoints/health/keys`
- `PATCH` `/api/admin/endpoints/health/keys/{id}`

## endpoints_manage（18）

- `DELETE` `/api/admin/endpoints/keys/{id}`
- `DELETE` `/api/admin/endpoints/{id}`
- `GET` `/api/admin/endpoints/defaults/{id}/body-rules`
- `GET` `/api/admin/endpoints/keys/grouped-by-format`
- `GET` `/api/admin/endpoints/keys/{id}/export`
- `GET` `/api/admin/endpoints/keys/{id}/reveal`
- `GET` `/api/admin/endpoints/providers/{id}/endpoints`
- `GET` `/api/admin/endpoints/providers/{id}/keys`
- `GET` `/api/admin/endpoints/{id}`
- `POST` `/api/admin/endpoints/keys/batch-delete`
- `POST` `/api/admin/endpoints/keys/{id}/clear-oauth-invalid`
- `POST` `/api/admin/endpoints/keys/{id}/codex-reset-credit/consume`
- `POST` `/api/admin/endpoints/keys/{id}/reset-cycle-stats`
- `POST` `/api/admin/endpoints/providers/{id}/endpoints`
- `POST` `/api/admin/endpoints/providers/{id}/keys`
- `POST` `/api/admin/endpoints/providers/{id}/refresh-quota`
- `PUT` `/api/admin/endpoints/keys/{id}`
- `PUT` `/api/admin/endpoints/{id}`

## endpoints_rpm（2）

- `DELETE` `/api/admin/endpoints/rpm/key/{id}`
- `GET` `/api/admin/endpoints/rpm/key/{id}`

## gemini_files_manage（6）

- `DELETE` `/api/admin/gemini-files/mappings`
- `DELETE` `/api/admin/gemini-files/mappings/{id}`
- `GET` `/api/admin/gemini-files/capable-keys`
- `GET` `/api/admin/gemini-files/mappings`
- `GET` `/api/admin/gemini-files/stats`
- `POST` `/api/admin/gemini-files/upload`

## global_models_manage（9）

- `DELETE` `/api/admin/models/global/{id}`
- `GET` `/api/admin/models/global`
- `GET` `/api/admin/models/global/{id}`
- `GET` `/api/admin/models/global/{id}/providers`
- `GET` `/api/admin/models/global/{id}/routing`
- `PATCH` `/api/admin/models/global/{id}`
- `POST` `/api/admin/models/global`
- `POST` `/api/admin/models/global/batch-delete`
- `POST` `/api/admin/models/global/{id}/assign-to-providers`

## ldap_manage（3）

- `GET` `/api/admin/ldap/config`
- `POST` `/api/admin/ldap/test`
- `PUT` `/api/admin/ldap/config`

## management_tokens_manage（8）

- `DELETE` `/api/admin/management-tokens/{id}`
- `GET` `/api/admin/management-tokens`
- `GET` `/api/admin/management-tokens/permissions/catalog`
- `GET` `/api/admin/management-tokens/{id}`
- `PATCH` `/api/admin/management-tokens/{id}/status`
- `POST` `/api/admin/management-tokens`
- `POST` `/api/admin/management-tokens/{id}/regenerate`
- `PUT` `/api/admin/management-tokens/{id}`

## model_catalog_manage（1）

- `GET` `/api/admin/models/catalog`

## model_external_manage（4）

- `DELETE` `/api/admin/models/external/cache`
- `GET` `/api/admin/models/external`
- `GET` `/api/admin/models/external/config`
- `PUT` `/api/admin/models/external/config`

## modules_manage（3）

- `GET` `/api/admin/modules/status`
- `GET` `/api/admin/modules/status/{id}`
- `PUT` `/api/admin/modules/status/{id}/enabled`

## monitoring（26）

- `DELETE` `/api/admin/monitoring/cache`
- `DELETE` `/api/admin/monitoring/cache/affinity/{user_id}/{key_id}/{model}/{api_format}`
- `DELETE` `/api/admin/monitoring/cache/model-mapping`
- `DELETE` `/api/admin/monitoring/cache/model-mapping/provider/{provider_id}/{model_id}`
- `DELETE` `/api/admin/monitoring/cache/model-mapping/{model_id}`
- `DELETE` `/api/admin/monitoring/cache/providers/{provider_id}`
- `DELETE` `/api/admin/monitoring/cache/redis-keys/{key_id}`
- `DELETE` `/api/admin/monitoring/cache/users/{user_id}`
- `DELETE` `/api/admin/monitoring/resilience/error-stats`
- `GET` `/api/admin/monitoring/audit-logs`
- `GET` `/api/admin/monitoring/cache/affinities`
- `GET` `/api/admin/monitoring/cache/affinity/{key_id}`
- `GET` `/api/admin/monitoring/cache/config`
- `GET` `/api/admin/monitoring/cache/metrics`
- `GET` `/api/admin/monitoring/cache/model-mapping/stats`
- `GET` `/api/admin/monitoring/cache/redis-keys`
- `GET` `/api/admin/monitoring/cache/stats`
- `GET` `/api/admin/monitoring/resilience-status`
- `GET` `/api/admin/monitoring/resilience/circuit-history`
- `GET` `/api/admin/monitoring/suspicious-activities`
- `GET` `/api/admin/monitoring/system-status`
- `GET` `/api/admin/monitoring/trace/stats/provider/{id}`
- `GET` `/api/admin/monitoring/trace/stats/provider/{provider_id}`
- `GET` `/api/admin/monitoring/trace/{id}`
- `GET` `/api/admin/monitoring/trace/{request_id}`
- `GET` `/api/admin/monitoring/user-behavior/{user_id}`

## oauth_manage（6）

- `DELETE` `/api/admin/oauth/providers/{provider_type}`
- `GET` `/api/admin/oauth/providers`
- `GET` `/api/admin/oauth/providers/{provider_type}`
- `GET` `/api/admin/oauth/supported-types`
- `POST` `/api/admin/oauth/providers/{provider_type}/test`
- `PUT` `/api/admin/oauth/providers/{provider_type}`

## payments_manage（19）

- `GET` `/api/admin/payments/callbacks`
- `GET` `/api/admin/payments/gateways/epay`
- `GET` `/api/admin/payments/gateways/{provider}`
- `GET` `/api/admin/payments/orders`
- `GET` `/api/admin/payments/orders/{gateway_provider}`
- `GET` `/api/admin/payments/redeem-codes/batches`
- `GET` `/api/admin/payments/redeem-codes/batches/{gateway_provider}`
- `GET` `/api/admin/payments/redeem-codes/batches/{gateway_provider}/codes`
- `POST` `/api/admin/payments/gateways/epay/test`
- `POST` `/api/admin/payments/gateways/{provider}/test`
- `POST` `/api/admin/payments/orders/{gateway_provider}/credit`
- `POST` `/api/admin/payments/orders/{gateway_provider}/expire`
- `POST` `/api/admin/payments/orders/{gateway_provider}/fail`
- `POST` `/api/admin/payments/redeem-codes/batches`
- `POST` `/api/admin/payments/redeem-codes/batches/{gateway_provider}/delete`
- `POST` `/api/admin/payments/redeem-codes/batches/{gateway_provider}/disable`
- `POST` `/api/admin/payments/redeem-codes/codes/{gateway_provider}/disable`
- `PUT` `/api/admin/payments/gateways/epay`
- `PUT` `/api/admin/payments/gateways/{provider}`

## pool_manage（10）

- `GET` `/api/admin/pool/overview`
- `GET` `/api/admin/pool/scheduling-presets`
- `GET` `/api/admin/pool/{provider_id}/keys`
- `GET` `/api/admin/pool/{provider_id}/keys/batch-delete-task/{task_id}`
- `GET` `/api/admin/pool/{provider_id}/scores`
- `PATCH` `/api/admin/pool/{provider_id}/keys/batch-update`
- `POST` `/api/admin/pool/{provider_id}/keys/batch-action`
- `POST` `/api/admin/pool/{provider_id}/keys/batch-import`
- `POST` `/api/admin/pool/{provider_id}/keys/cleanup-banned`
- `POST` `/api/admin/pool/{provider_id}/keys/resolve-selection`

## provider_models_manage（9）

- `DELETE` `/api/admin/providers/{provider_id}/models/{model_id}`
- `GET` `/api/admin/providers/{id}/available-source-models`
- `GET` `/api/admin/providers/{provider_id}/models`
- `GET` `/api/admin/providers/{provider_id}/models/{model_id}`
- `PATCH` `/api/admin/providers/{provider_id}/models/{model_id}`
- `POST` `/api/admin/providers/{id}/assign-global-models`
- `POST` `/api/admin/providers/{id}/import-from-upstream`
- `POST` `/api/admin/providers/{provider_id}/models`
- `POST` `/api/admin/providers/{provider_id}/models/batch`

## provider_oauth_manage（17）

- `GET` `/api/admin/provider-oauth/providers/{provider_id}/agent-identity-import/tasks/{id}`
- `GET` `/api/admin/provider-oauth/providers/{provider_id}/batch-import/tasks/{id}`
- `GET` `/api/admin/provider-oauth/providers/{provider_id}/cookie-authorize/tasks/{id}`
- `GET` `/api/admin/provider-oauth/supported-types`
- `POST` `/api/admin/provider-oauth/keys/{key_id}/complete`
- `POST` `/api/admin/provider-oauth/keys/{key_id}/refresh`
- `POST` `/api/admin/provider-oauth/keys/{key_id}/start`
- `POST` `/api/admin/provider-oauth/providers/{provider_id}/agent-identity-import/tasks`
- `POST` `/api/admin/provider-oauth/providers/{provider_id}/batch-import`
- `POST` `/api/admin/provider-oauth/providers/{provider_id}/batch-import/tasks`
- `POST` `/api/admin/provider-oauth/providers/{provider_id}/complete`
- `POST` `/api/admin/provider-oauth/providers/{provider_id}/cookie-authorize`
- `POST` `/api/admin/provider-oauth/providers/{provider_id}/cookie-authorize/tasks`
- `POST` `/api/admin/provider-oauth/providers/{provider_id}/device-authorize`
- `POST` `/api/admin/provider-oauth/providers/{provider_id}/device-poll`
- `POST` `/api/admin/provider-oauth/providers/{provider_id}/import-refresh-token`
- `POST` `/api/admin/provider-oauth/providers/{provider_id}/start`

## provider_ops_manage（14）

- `DELETE` `/api/admin/provider-ops/providers/{id}/config`
- `GET` `/api/admin/provider-ops/architectures`
- `GET` `/api/admin/provider-ops/architectures/{id}`
- `GET` `/api/admin/provider-ops/providers/{id}/balance`
- `GET` `/api/admin/provider-ops/providers/{id}/config`
- `GET` `/api/admin/provider-ops/providers/{id}/status`
- `POST` `/api/admin/provider-ops/batch/balance`
- `POST` `/api/admin/provider-ops/providers/{id}/actions/{id}`
- `POST` `/api/admin/provider-ops/providers/{id}/balance`
- `POST` `/api/admin/provider-ops/providers/{id}/checkin`
- `POST` `/api/admin/provider-ops/providers/{id}/connect`
- `POST` `/api/admin/provider-ops/providers/{id}/disconnect`
- `POST` `/api/admin/provider-ops/providers/{id}/verify`
- `PUT` `/api/admin/provider-ops/providers/{id}/config`

## provider_query_manage（3）

- `POST` `/api/admin/provider-query/models`
- `POST` `/api/admin/provider-query/test-model`
- `POST` `/api/admin/provider-query/test-model-failover`

## provider_strategy_manage（4）

- `DELETE` `/api/admin/provider-strategy/providers/{id}/quota`
- `GET` `/api/admin/provider-strategy/providers/{id}/stats`
- `GET` `/api/admin/provider-strategy/strategies`
- `PUT` `/api/admin/provider-strategy/providers/{id}/billing`

## providers_manage（12）

- `DELETE` `/api/admin/providers/{provider_id}`
- `GET` `/api/admin/providers`
- `GET` `/api/admin/providers/summary`
- `GET` `/api/admin/providers/{provider_id}/delete-task/{task_id}`
- `GET` `/api/admin/providers/{provider_id}/health-monitor`
- `GET` `/api/admin/providers/{provider_id}/mapping-preview`
- `GET` `/api/admin/providers/{provider_id}/pool-status`
- `GET` `/api/admin/providers/{provider_id}/summary`
- `PATCH` `/api/admin/providers/{provider_id}`
- `POST` `/api/admin/providers`
- `POST` `/api/admin/providers/{provider_id}/pool/clear-cooldown/{key_id}`
- `POST` `/api/admin/providers/{provider_id}/pool/reset-cost/{key_id}`

## proxy_nodes_manage（21）

- `DELETE` `/api/admin/proxy-nodes/{node_id}`
- `GET` `/api/admin/proxy-nodes`
- `GET` `/api/admin/proxy-nodes/metrics/fleet`
- `GET` `/api/admin/proxy-nodes/{node_id}`
- `GET` `/api/admin/proxy-nodes/{node_id}/events`
- `GET` `/api/admin/proxy-nodes/{node_id}/metrics`
- `PATCH` `/api/admin/proxy-nodes/{node_id}`
- `POST` `/api/admin/proxy-nodes/heartbeat`
- `POST` `/api/admin/proxy-nodes/install-sessions`
- `POST` `/api/admin/proxy-nodes/manual`
- `POST` `/api/admin/proxy-nodes/register`
- `POST` `/api/admin/proxy-nodes/test-url`
- `POST` `/api/admin/proxy-nodes/unregister`
- `POST` `/api/admin/proxy-nodes/upgrade`
- `POST` `/api/admin/proxy-nodes/upgrade/cancel`
- `POST` `/api/admin/proxy-nodes/upgrade/clear-conflicts`
- `POST` `/api/admin/proxy-nodes/upgrade/restore-skipped`
- `POST` `/api/admin/proxy-nodes/{node_id}/test`
- `POST` `/api/admin/proxy-nodes/{node_id}/upgrade/retry`
- `POST` `/api/admin/proxy-nodes/{node_id}/upgrade/skip`
- `PUT` `/api/admin/proxy-nodes/{node_id}/config`

## referrals_manage（4）

- `GET` `/api/admin/referral-rewards`
- `GET` `/api/admin/referrals`
- `POST` `/api/admin/referral-rewards/{id}/retry`
- `POST` `/api/admin/referral-rewards/{id}/void`

## routing_profiles_manage（12）

- `DELETE` `/api/admin/routing/bindings/{id}`
- `DELETE` `/api/admin/routing/groups/{id}`
- `GET` `/api/admin/routing/bindings`
- `GET` `/api/admin/routing/groups`
- `GET` `/api/admin/routing/groups/{id}`
- `GET` `/api/admin/routing/groups/{id}/versions`
- `PATCH` `/api/admin/routing/bindings/{id}`
- `PATCH` `/api/admin/routing/groups/{id}`
- `POST` `/api/admin/routing/bindings`
- `POST` `/api/admin/routing/groups`
- `POST` `/api/admin/routing/groups/{id}/dry-run`
- `POST` `/api/admin/routing/groups/{id}/publish`

## security_manage（7）

- `DELETE` `/api/admin/security/ip/blacklist/{id}`
- `DELETE` `/api/admin/security/ip/whitelist/{id}`
- `GET` `/api/admin/security/ip/blacklist`
- `GET` `/api/admin/security/ip/blacklist/stats`
- `GET` `/api/admin/security/ip/whitelist`
- `POST` `/api/admin/security/ip/blacklist`
- `POST` `/api/admin/security/ip/whitelist`

## stats_manage（11）

- `GET` `/api/admin/stats/comparison`
- `GET` `/api/admin/stats/cost/forecast`
- `GET` `/api/admin/stats/cost/savings`
- `GET` `/api/admin/stats/errors/distribution`
- `GET` `/api/admin/stats/leaderboard/api-keys`
- `GET` `/api/admin/stats/leaderboard/models`
- `GET` `/api/admin/stats/leaderboard/users`
- `GET` `/api/admin/stats/performance/percentiles`
- `GET` `/api/admin/stats/performance/providers`
- `GET` `/api/admin/stats/providers/quota-usage`
- `GET` `/api/admin/stats/time-series`

## system_manage（43）

- `DELETE` `/api/admin/system/configs/{key}`
- `GET` `/api/admin/system/api-formats`
- `GET` `/api/admin/system/aws-regions`
- `GET` `/api/admin/system/check-update`
- `GET` `/api/admin/system/cleanup/runs`
- `GET` `/api/admin/system/cleanup/usage/preview`
- `GET` `/api/admin/system/config/export`
- `GET` `/api/admin/system/configs`
- `GET` `/api/admin/system/configs/{key}`
- `GET` `/api/admin/system/data/export`
- `GET` `/api/admin/system/email/templates`
- `GET` `/api/admin/system/email/templates/{id}`
- `GET` `/api/admin/system/releases`
- `GET` `/api/admin/system/settings`
- `GET` `/api/admin/system/stats`
- `GET` `/api/admin/system/update-capability`
- `GET` `/api/admin/system/update-history`
- `GET` `/api/admin/system/update-status`
- `GET` `/api/admin/system/users/export`
- `GET` `/api/admin/system/version`
- `POST` `/api/admin/system/apply-update`
- `POST` `/api/admin/system/backups/s3/run`
- `POST` `/api/admin/system/cleanup`
- `POST` `/api/admin/system/cleanup/usage/manual`
- `POST` `/api/admin/system/config/import`
- `POST` `/api/admin/system/data/import`
- `POST` `/api/admin/system/email/templates/{id}/preview`
- `POST` `/api/admin/system/email/templates/{id}/reset`
- `POST` `/api/admin/system/important-notification/test`
- `POST` `/api/admin/system/prepare-update`
- `POST` `/api/admin/system/purge/audit-logs`
- `POST` `/api/admin/system/purge/config`
- `POST` `/api/admin/system/purge/request-bodies`
- `POST` `/api/admin/system/purge/request-bodies/task`
- `POST` `/api/admin/system/purge/stats`
- `POST` `/api/admin/system/purge/usage`
- `POST` `/api/admin/system/purge/users`
- `POST` `/api/admin/system/rollback`
- `POST` `/api/admin/system/smtp/test`
- `POST` `/api/admin/system/users/import`
- `PUT` `/api/admin/system/configs/{key}`
- `PUT` `/api/admin/system/email/templates/{id}`
- `PUT` `/api/admin/system/settings`

## tasks_manage（5）

- `GET` `/api/admin/tasks`
- `GET` `/api/admin/tasks/stats`
- `GET` `/api/admin/tasks/{id}/events`
- `POST` `/api/admin/tasks/{id}/cancel`
- `POST` `/api/admin/tasks/{id}/trigger`

## usage_manage（11）

- `GET` `/api/admin/usage/active`
- `GET` `/api/admin/usage/aggregation/stats`
- `GET` `/api/admin/usage/cache-affinity/hit-analysis`
- `GET` `/api/admin/usage/cache-affinity/interval-timeline`
- `GET` `/api/admin/usage/cache-affinity/ttl-analysis`
- `GET` `/api/admin/usage/heatmap`
- `GET` `/api/admin/usage/records`
- `GET` `/api/admin/usage/stats`
- `GET` `/api/admin/usage/{request_id}`
- `GET` `/api/admin/usage/{request_id}/curl`
- `POST` `/api/admin/usage/{request_id}/replay`

## users_manage（25）

- `DELETE` `/api/admin/user-groups/{id}`
- `DELETE` `/api/admin/users/{user_id}`
- `DELETE` `/api/admin/users/{user_id}/api-keys/{api_key_id}`
- `DELETE` `/api/admin/users/{user_id}/sessions`
- `DELETE` `/api/admin/users/{user_id}/sessions/{session_id}`
- `GET` `/api/admin/user-groups`
- `GET` `/api/admin/user-groups/{id}/members`
- `GET` `/api/admin/users`
- `GET` `/api/admin/users/{user_id}`
- `GET` `/api/admin/users/{user_id}/api-keys`
- `GET` `/api/admin/users/{user_id}/api-keys/{api_key_id}/full-key`
- `GET` `/api/admin/users/{user_id}/billing/entitlements`
- `GET` `/api/admin/users/{user_id}/sessions`
- `PATCH` `/api/admin/users/{user_id}/api-keys/{api_key_id}/lock`
- `POST` `/api/admin/user-groups`
- `POST` `/api/admin/users`
- `POST` `/api/admin/users/batch-action`
- `POST` `/api/admin/users/resolve-selection`
- `POST` `/api/admin/users/{user_id}/api-keys`
- `POST` `/api/admin/users/{user_id}/billing/grant-plan`
- `PUT` `/api/admin/user-groups/default`
- `PUT` `/api/admin/user-groups/{id}`
- `PUT` `/api/admin/user-groups/{id}/members`
- `PUT` `/api/admin/users/{user_id}`
- `PUT` `/api/admin/users/{user_id}/api-keys/{api_key_id}`

## video_tasks_manage（4）

- `GET` `/api/admin/video-tasks`
- `GET` `/api/admin/video-tasks/stats`
- `GET` `/api/admin/video-tasks/{id}/video`
- `POST` `/api/admin/video-tasks/{id}/cancel`

## wallets_manage（11）

- `GET` `/api/admin/wallets`
- `GET` `/api/admin/wallets/ledger`
- `GET` `/api/admin/wallets/refund-requests`
- `GET` `/api/admin/wallets/{id}`
- `GET` `/api/admin/wallets/{id}/transactions`
- `GET` `/api/admin/wallets/{wallet_id}/refunds`
- `POST` `/api/admin/wallets/{id}/adjust`
- `POST` `/api/admin/wallets/{id}/recharge`
- `POST` `/api/admin/wallets/{wallet_id}/refunds/{refund_id}/complete`
- `POST` `/api/admin/wallets/{wallet_id}/refunds/{refund_id}/fail`
- `POST` `/api/admin/wallets/{wallet_id}/refunds/{refund_id}/process`
