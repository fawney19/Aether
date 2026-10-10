# Agent Note: 模型/提供商健康监控的 History 状态条改用全窗口分段聚合

Status: implemented

## Problem

健康监控页面（`frontend/src/views/shared/HealthMonitor.vue`）的模型卡片、提供商卡片与“查看详情”抽屉都渲染一条 `History (60pts)` 状态条。后端 `apps/aether-gateway/src/handlers/public/catalog_helpers.rs` 里，这条状态条（`timeline` 与 `timeline_details`）原先是用 `state.list_usage_audits(... limit: Some(per_model_limit), newest_first: true ...)` 取「窗口内最新 N 条事件」后在内存里按 60 段分桶算出来的；而同一张卡片头部的 `total_attempts`/`success_count`（“265 次请求 / 可用率”）来自全窗口聚合 `summarize_usage_breakdown`。两者口径不同源：只要窗口内请求数超过 N，样本就只覆盖最近的时段，更早的分段因为「一个事件都没有」被判成 `unknown`，前端渲染为灰色「无请求」。

实测现象（用户截图）：模型 `deepseek-v4.1-flash` 显示「1 个提供商 / 265 次请求」，但状态条只有零星几段绿色；同一个请求集合在提供商卡片 `codebuddy2api`（1 个模型 / 265 次请求）下却完整得多。原因是提供商卡片的事件上限是 `per_model_event_limit × per_provider_model_limit = 1200 > 265`，恰好拿到了全量样本——它只是**碰巧**对，不是结构性正确。同一张卡片上的“平均 TTFB”也因同样原因对不上（模型 2.21s vs 提供商 1.88s，同一批请求）。

受影响的四条路径：模型卡片 `build_model_health_monitor_payload`、提供商卡片主条 `build_provider_health_payload`、提供商卡片内嵌模型 `model_health_payload_from_row`（最严重：先从提供商级样本分发，再截到 ≤`per_model_event_limit`）、关联抽屉 `related_health_item_payload`。端点卡片 `build_api_format_health_monitor_payload` 不受影响，它本来就走数据库全窗口聚合。

不修不会自愈：样本上限是查询参数（admin 默认 60 / 公开默认 100，前端固定传 100），任何 6 小时内请求数超过该值的对象都会缺历史；而且灰色的 `unknown` 与「真的没有请求」在前端**完全无法区分**。

## Decision

健康监控的 History 状态条一律由数据库侧的全窗口分段聚合产生，**不允许**再从被 `LIMIT` 截断的事件样本分桶。

新增契约（`crates/aether-data/contracts/src/repository/usage/types.rs`）：`UsageHealthTimelineGroupBy { Model, Provider, ApiFormat }`、`UsageHealthTimelineQuery { created_from/until_unix_secs, group_by, group_values, provider_name?, model?, api_format?, segments, exclude_status_codes }`、`StoredUsageHealthTimelineRow`，以及 `UsageReadRepository::aggregate_usage_health_timeline`（带默认实现返回“仓储不可用”，内存部署可安全缺省）。

PostgreSQL 实现（`crates/aether-data/adapters/postgres/src/usage/mod.rs` 的 `build_usage_health_timeline_query`）在 `usage_billing_facts` 视图上分桶：

```sql
GREATEST(0, LEAST(FLOOR(EXTRACT(EPOCH FROM (created_at - TO_TIMESTAMP($from))) / $width)::bigint, $segments - 1))
```

三条必须守住的规则：

1. **同源可对账** —— 数据源、窗口边界（`>= from` / `< until`，右端开区间）、`status NOT IN ('pending','streaming')`、`provider_name NOT IN ('unknown','pending')`、`exclude_status_codes`、成功判定 `status <> 'failed' AND (status_code IS NULL OR status_code < 400) AND error_message IS NULL`，全部与 `summarize_usage_breakdown_raw` 逐字一致。`ApiFormat` 分组额外要求 `api_format IS NOT NULL`，同样对齐 breakdown 的 `filtered_extra_where`。
2. **聚合里永远不出现 `LIMIT`** —— `events` 明细数组仍保留 `LIMIT` 样本（那是给抽屉看的明细列表，允许截断），但 `timeline` / `timeline_details` 的计算不得依赖它。
3. **降级必须可见地退让** —— 聚合返回空（仓储不可用、内存模式、查询失败）时，退回用事件样本分桶（`usage_health_buckets_from_events`），保证卡片不整条变灰；一旦聚合有结果就按分组键精确匹配，不再混用样本。

网关侧落地（`apps/aether-gateway/src/handlers/public/catalog_helpers.rs`）：删除 `build_model_health_timeline` 与 `build_usage_health_timeline_details`，统一为 `index_usage_health_timeline_rows` + `build_usage_health_timeline_from_buckets`；四条路径各自发起一次聚合（模型卡片 1 次；提供商卡片 1 次按提供商 + 1 次按“该提供商范围内的模型”；关联抽屉按 `query.group_by` 推导维度、并透传 `provider_name`/`model`/`api_format` 范围过滤）。卡片级 `avg_first_byte_ms` 也改由聚合桶的 `first_byte` 求和得出，与头部同源。端点卡片与 `/api/public/health`（health_v2，24 段）不在本次范围内，保持原样。

## Alternatives considered

- **只把 `per_model_limit` 调大（admin 上限 200、public 上限 500）** —— 改动一行，但只是把截断点往后推：繁忙对象在 6 小时内轻松超过 200/500 次，缺陷会以“没那么明显”的形态复现；而且 `events` 明细数组会跟着膨胀。否。
- **复用已有的 `summarize_health_observations`（health_v2，已经支持 Model/Provider/ApiFormat 维度 + 分段）** —— 零新增数据层代码，但它读的是另一张事实表 `usage_analytics_facts_v1`，成功判定是 `status = 'completed'`，与卡片头部（`usage_billing_facts` + 上面那套判定）不同。这会给同一张卡片引入第三套口径，头部数字与状态条依旧无法对账——正是本次要消灭的问题。否。
- **给 `summarize_usage_breakdown` 加一个可选的分段维度，复用同一个查询** —— 概念上最省，但该查询被成本分析、仪表盘、提供商卡片等大量调用方共用，还要处理 `summarize_usage_breakdown_from_daily_aggregates` 日聚合快路径的分段语义，回归面远大于收益。否。最终选择**新增**一条只服务健康监控的聚合方法，对既有调用方零影响。
- **前端把 `unknown` 渲染得更“隐蔽”（例如淡灰、或按窗口外计算）** —— 只是掩盖症状，用户依旧无法判断某段时间到底有没有请求。否。
- **保留样本方案，但把时间条改成只用样本实际覆盖的时间范围** —— 状态条会随时间轴缩放，不同卡片看到的时间跨度不一致，也失去了「统一回溯窗口」的意义。否。

## Consequences

- **收益**：状态条、每段 Tooltip 的计数与延迟指标、卡片头部的请求数/可用率首次真正同源；模型卡片与提供商卡片在“只有唯一提供商、全部请求都走它”时必然画出同一条历史。`FLOOR` 分桶在库内完成，不再受事件条数影响。
- **收益**：原先每个模型都要跑一次 `list_usage_audits`（300+ 列、两个 LEFT JOIN、`LIMIT 100`）只为画时间条，现在最重的部分换成一次按段分组的轻量聚合。提供商卡片的主条进一步**批量聚合**：一次查询拿回全部提供商的 60 段（`group_values` 传全部提供商名），而不是每个提供商各发一次；内嵌模型条仍按“该提供商范围内的模型”逐提供商聚合，因此该卡片的查询数从“每提供商 2 次”变为“每提供商 2 次 + 1 次公共查询”，与改动前持平。
- **代价**：新增一个仓库 trait 方法与两套实现（PostgreSQL `usage_billing_facts`、内存仓储），后续改成功判定或窗口语义时必须三处同步；契约注释里已写明“与 `summarize_usage_breakdown_raw` 逐字对齐”。
- **代价**：降级（聚合不可用）不再静默——`load_usage_health_timeline_buckets` 会打一条 `tracing::warn`（`scope` 区分 model / provider / provider_model / related）。降级会退回被截断的事件样本，历史时段仍会缺，日志是线上唯一线索。
- **已知残留（未修，重访时先看这里）**：
  - `provider_count`（“N 个提供商”）仍取自被 `LIMIT` 截断的事件样本。若某提供商只在窗口早期出现过、最近 N 条里没有它，计数会偏小。`last_event_at` 取的是 `newest_first` 的第一条，因此是准确的。修 `provider_count` 需要按 (模型, 提供商) 再做一次聚合，本次搁置。
  - `UsageHealthTimelineQuery` 没有 `user_id` 字段，而它对齐的 `UsageBreakdownSummaryQuery` 有。当前四个调用方都传 `None`，所以现状等价；**若将来做“按用户维度的健康卡片”，时间线条会静默地与头部统计口径漂移**，加字段时必须两套实现同步。
  - 降级路径与 SQL 口径存在已知差异：`model_health_event_success` 把 `error_message = ''` 判为成功（SQL 判为失败），且事件样本没有 `status NOT IN ('pending','streaming')` / `provider_name NOT IN ('unknown','pending')` 过滤。降级时状态条会比头部更“绿”。未改是因为该判定函数同时服务端点卡片的采样指标，改动面超出本次范围。
  - 前端 `History (60pts)` 是四处硬编码字面量（端点/模型/提供商/关联卡片），不来自数据。当前后端恒返回 60 段所以不冲突，但同仓库已存在 24 段的 health_v2，复用组件前必须先改文案来源。
  - 端点卡片的 `time_range_start/end` 是「窗口内最早/最晚事件时刻」而非窗口边界（`build_public_health_timeline`），与它自己 tooltip 的完整窗口时间不一致。模型/提供商/关联卡片改动前后**都**是窗口边界（被删的 `build_model_health_timeline` 返回的就是 `(since, until)`），本次只是把取值来源从返回值改成直接传参，不是行为变更。
  - 降级路径只在“聚合整体无结果”时启用；聚合成功但某个分组缺失时不会回退（正常数据下不会发生，因为过滤条件与 breakdown 相同）。

## Verification

- **真库端到端对账（最有价值的一条）**：`live_usage_health_timeline_totals_match_breakdown`。往 `public.usage` 写 265 条本次 run 专属 request_id 的记录，用窗口函数把 `created_at` 等距铺满 6 小时，然后断言：① 头部 `summarize_usage_breakdown` 看到 265 条；② **时间线条各分段 `request_count` 之和 == 头部 `request_count`**；③ 成功数同样逐字相等；④ ≥59/60 段有数据；⑤ 构造的失败请求确实体现在状态条上。结束删除本次 run 的行。
- **真库 SQL 语义**：`live_usage_health_timeline_buckets_by_window_offset`（会话级临时表 + 同名临时视图替换 `usage_billing_facts`，不写业务表）。逐条断言分段归属（offset 0/599→段 0、600→段 1、1500→段 2、3599→段 5）、窗口开闭区间（`-1` 与正好 `=until` 都被排除）、`499`/`pending`/`unknown` 提供商/空串 `error_message` 的过滤结果与耗时聚合。
- 两条真库用例的运行方式：`AETHER_TEST_DATABASE_URL=... cargo nextest run -p aether-data-postgres --run-ignored ignored-only -E 'test(live_usage_health_timeline)'` → 2 passed。容器重启前该库不可达（宿主机名解析到代理地址、PostgreSQL 协议无响应），因此这两条断言是在库恢复后才首次执行的。
- 内存仓储全窗口覆盖：`cargo test -p aether-data --lib` → `repository::usage::memory::tests::usage_health_timeline_covers_full_window_regardless_of_sample_size`（265 次请求铺满 6 小时窗口，断言 ≥59 段有数据、总数 265、提供商维度与模型维度看到同一批请求）。
- SQL 结构与“不得出现 LIMIT”回归门：`cargo test -p aether-data-postgres --lib` → `usage_health_timeline_sql_buckets_full_window_without_sample_limit`、`usage_health_timeline_sql_excludes_null_api_format_for_api_format_dimension`、`usage_health_timeline_query_rejects_invalid_windows`。
- 网关分段映射与降级：`cargo test -p aether-gateway --lib catalog_helpers::tests`（5 passed）→ `usage_health_timeline_paints_every_segment_covered_by_aggregation`、`usage_health_timeline_falls_back_to_event_sample_when_aggregation_missing`。
- 架构守卫（含“usage SQL 片段仍为 29 个 .sql”“handler 不内联 SQL”）：`cargo test -p aether-gateway --test architecture_guard` → 208 passed。
- 全量（`cargo nextest run -p aether-data -p aether-data-postgres -p aether-gateway --no-fail-fast`）：6205 passed / 3 failed / 50 skipped（skip 里包含上面两条 `#[ignore]` 真库用例与仓库既有的 live 用例）。3 个失败项（`control::auth::resolution::tests::due_antigravity_bearer_refresh_observes_cross_node_allowlist_revocation`、`strong_system_config_read_bypasses_app_and_data_caches`、`tests::video::xai::xai_video_native_and_compatibility_http_lifecycle_postgres`）需要测试进程自行拉起临时 PostgreSQL，本容器没有对应的服务端二进制，与本次改动无关。`cargo check --workspace --all-targets` 与 `cargo fmt --all --check` 均干净。
- **未验证**：前端渲染（本次未改前端，`timeline`/`timeline_details` 的 JSON 字段与长度均未变）。
