# 数据库速查

> 生产：Postgres 15，容器 `aether-postgres`，`127.0.0.1:15432`，库 `aether`，账号 postgres
> schema 定义源：`crates/aether-data/runtime/schema/logical/*.toml`（8 个文件，本表字段即代码核实结果）
> 旧 Python 版迁移在 `alembic/versions/`（仅残留），Rust 版由 `aether-data/schema` 生成 SQL

## 常用连接

```bash
docker exec aether-postgres psql -U postgres -d aether -P pager=off -c "<SQL>"
```

## 一、身份与鉴权（001_identity.toml）

| 表 | 关键字段 | 说明 |
|---|---|---|
| `users` | id, email, username, password_hash, role, auth_source, is_active, is_deleted, allowed_models/providers/api_formats(+_mode), rate_limit, feature_settings, last_login_at | 用户 |
| `user_groups` / `user_group_members` | name, priority, 各类 allowed_* 限制 / group_id, user_id | 用户组 |
| `api_keys` | id, user_id, key_hash, key_encrypted, name, key_prefix, status, allowed_*, ip_rules, rate_limit, concurrent_limit, is_standalone, is_locked, auto_delete_on_expiry, total_requests/tokens/cost_usd, expires_at, last_used_at | **调用方 key（sk- 前缀）** |
| `management_tokens` | id, user_id, name, token_hash, token_prefix, allowed_ips, permissions, expires_at, last_used_at, usage_count, is_active | **管理令牌（ae- 前缀）** |
| `user_sessions` | id, user_id, client_device_id, refresh_token_hash, prev_refresh_token_hash, last_seen_at, expires_at, revoked_at | Web 会话（JWT） |
| `audit_logs` | event_type, user_id, api_key_id, description, ip_address, request_id, event_metadata, status_code | 审计日志 |
| `announcements` | title, content, type, priority, requires_ack, start/end_time | 公告 |
| `user_preferences` | default_provider_id, theme, language, timezone, usage_alerts | 偏好 |

## 二、渠道目录（002_provider_catalog.toml）★ 运维最常用

| 表 | 关键字段 | 说明 |
|---|---|---|
| `providers` | name, provider_type, billing_type, monthly_quota_usd, enabled, is_active, **priority/provider_priority(越小越优先)**, concurrent_limit, max_retries, proxy(jsonb), request_timeout, **stream_first_byte_timeout**, config(jsonb) | 上游渠道 |
| `provider_endpoints` | provider_id, name, base_url, api_format, api_family, endpoint_kind, enabled, is_active, health_score, weight, **header_rules**, **body_rules**, max_retries, custom_path, config, **format_acceptance_config**, proxy(jsonb) | **端点（规则所在）** |
| `provider_api_keys` | provider_id, name, api_key/encrypted_key, auth_type, capabilities, api_formats, rate_multipliers, allowed_models, expires_at, proxy, learned_rpm_limit, circuit_breaker_by_format, health_by_format, status_snapshot, is_active, rpm_limit | 渠道 key（号池成员） |
| `models` | provider_id, global_model_id, provider_model_name, global_model_name, api_format, enabled, is_active, is_available, pricing, supports_* , provider_model_mappings, config | 渠道模型映射 |
| `global_models` | id, name, display_name, enabled, is_active, default_price_per_request, default_tiered_pricing, supported_capabilities, usage_count, **config(含 model_mappings 正则)** | 全局模型（对外模型名） |
| `api_key_provider_mappings` | api_key_id, provider_id, priority_adjustment, weight_multiplier, is_enabled | key→渠道白名单 |
| `request_candidates` | request_id, candidate_index, retry_index, provider_id, endpoint_id, key_id, status, skip_reason, is_cached, status_code, error_type, error_message, latency_ms, extra_data(proxy 节点) | **路由候选链（排查）** |
| `routing_groups` / `routing_group_bindings` / `routing_group_versions` | name, config_json / group_id, subject_type/id / version, config_json | 调度分组 |
| `pool_member_scores` | pool_kind, member_id, capability, score, hard_state, probe_status, failure_count | 号池评分 |
| `gemini_file_mappings` | file_name, key_id, user_id, mime_type, source_hash, expires_at | Gemini 文件 |
| `video_tasks` | request_id, model, prompt, status, progress, poll, video_urls, webhook | 视频任务 |

## 三、认证配置（003_auth_config.toml）

| 表 | 字段 | 说明 |
|---|---|---|
| `system_configs` | key, value | **全局配置（无全局 body_rules，规则按端点）** |
| `auth_modules` | module_type, enabled, config | 认证模块 |
| `oauth_providers` | provider_type, client_id, client_secret_encrypted, scopes, redirect_uri, attribute_mapping | OAuth 提供商 |
| `ldap_configs` | server_url, bind_dn, base_dn, user_search_filter, is_enabled, is_exclusive | LDAP |
| `user_oauth_links` | user_id, provider_type, provider_user_id | 用户 OAuth 绑定 |

## 四、代理节点（004_proxy_nodes.toml）

| 表 | 字段 | 说明 |
|---|---|---|
| `proxy_nodes` | name, ip, port, region, status, last_heartbeat_at, active_connections, tunnel_mode, tunnel_connected, estimated_max_concurrency, proxy_url, proxy_username/password, config_version, hardware_info | **tunnel 节点** |
| `proxy_node_events` | node_id, event_type, detail | 节点事件 |
| `proxy_node_metrics_1m/1h` | node_id, bucket_start, samples, active_connections_*, heartbeat_rtt_ms_*, ws_* | 节点指标桶 |

## 五、钱包与计费（005_wallet_billing.toml）

| 表 | 字段 | 说明 |
|---|---|---|
| `wallets` | user_id, api_key_id, balance, gift_balance, limit_mode, currency, status, total_* | **钱包（key 余额）** |
| `wallet_transactions` | wallet_id, category, reason_code, amount, balance_*, link_type/id, operator_id | 交易流水 |
| `wallet_daily_usage_ledgers` | wallet_id, billing_date, total_cost_usd, total_requests, tokens | 日账 |
| `payment_orders` / `payment_callbacks` | order_no, amount_usd, status / callback_key, signature_valid, payload | 支付 |
| `payment_gateway_configs` | provider, endpoint_url, merchant_key_encrypted, channels_json | 支付渠道 |
| `billing_plans` / `user_plan_entitlements` | price, duration, entitlements_json / status, entitlements_snapshot | 套餐 |
| `redeem_code_batches` / `redeem_codes` | amount_usd, status / code_hash, code_prefix, redeemed_by | 兑换码 |
| `refund_requests` | refund_no, amount_usd, status, reason, payout_* | 退款 |
| `user_referrals` / `referral_rewards` | inviter/invitee, source_json / reward_type, amount_usd, status | 推荐 |

## 六、用量（006_usage.toml）★ 排查主入口

| 表 | 关键字段 | 说明 |
|---|---|---|
| `usage` | request_id, user_id, api_key_id, provider_name, model, target_model, api_format, endpoint_kind, has_format_conversion, is_stream, tokens/cost 系列, **status_code, status, error_message, error_category**, **response_time_ms, first_byte_time_ms**, request_headers/body, provider_request_*, response_*, client_response_*, **candidate_index, route_family, route_kind, execution_path**, wallet_balance_* | 每请求一条记录 |
| `usage_body_blobs` | request_id, body_field(request_body/provider_request_body/response_body/client_response_body), **payload_gzip(bytea gzip)** | 请求/响应体（大字段） |
| `usage_http_audits` | request_id, request_headers, provider_request_headers, response_headers, **client_response_headers(含 x-proxy-timing)**, body_refs, body_capture_mode | 响应头审计 |
| `usage_routing_snapshots` | request_id, candidate_id, candidate_index, planner_kind, route_family, execution_path, selected_provider/endpoint/key | 路由快照 |
| `usage_counter_deltas` | request_id, kind, target_id, *delta 系列 | 计数增量 |
| `usage_settlement_snapshots` | request_id, billing_status, wallet_*, provider_monthly_used_usd, pricing_*, billing_* | 结算快照 |

## 七、统计（007_stats.toml）

`stats_hourly` / `stats_daily`（+ `_model` / `_provider` / `_api_key` / `_error` 变体）、`stats_summary`、`stats_hourly_user` / `_user_model`、`stats_user_daily*`、`user_model_usage_counts`
字段模式：total_requests/success/error、input/output/cache tokens、total_cost/actual_total_cost、response_time 分位（p50/p90/p99）、settled_*（结算口径）、is_complete

## 八、后台任务（008_background_tasks.toml）

`background_task_runs`（id, task_type, status, params, result, error, started/finished_at）、`background_task_events`

## 常见排查 SQL

```sql
-- 最近 2 小时请求（最常用）
select created_at,request_id,provider_name,model,api_format,provider_endpoint_kind,
       status_code,status,error_category,left(error_message,200) as error,response_time_ms
from usage where created_at > now() - interval '2 hours' order by created_at desc limit 30;

-- 候选路由详情（走了节点还是直连）
select request_id,provider_name,endpoint_id,extra_data
from request_candidates where request_id='<id>' order by candidate_index;

-- 端点规则查看
select p.name,e.id,e.api_format,e.endpoint_kind,e.base_url,e.is_active,e.enabled,e.body_rules
from providers p join provider_endpoints e on e.provider_id=p.id where p.name='<渠道名>';

-- 解压请求体（usage_body_blobs，gzip bytea）
select encode(payload_gzip,'base64') from usage_body_blobs
where request_id='<id>' and body_field='request_body';
-- 本地 python: base64.b64decode → gzip.decompress → json

-- 查管理令牌（截断显示，勿泄明文）
select id,name,token_prefix,is_active,allowed_ips,permissions,expires_at
from management_tokens order by created_at desc;
```
