# Aether 架构拆分执行计划

这份文档是后续重构执行的唯一追踪面板。

使用规则：

- 开始一个工作项前，先在本文档里把对应任务标记为进行中。
- 合并一个工作项后，立即把对应复选框打勾，并补一行结果说明。
- 子代理只能领取本文档中已经明确写出的任务，不应临时扩散职责。
- 如果发现新依赖或阻塞项，先更新本文档，再继续改代码。

状态约定：

- `未开始`：尚未领取
- `进行中`：已有明确 owner，正在执行
- `已完成`：代码和验证都已落地

## 1. 目标

本轮重构的目标不是“把 crate 数量变多”，而是降低以下三个成本：

- `aether-gateway` 的编译和认知负担
- `aether-data` 的重编译扇出
- `handlers` / `ai_pipeline` 对 gateway 内核模块的强耦合

本轮明确不做：

- 不为了形式上的“微服务化”强拆 crate
- 不优先处理收益很小的微型 crate 合并
- 不在缺少验收标准时直接搬迁大目录

## 2. 当前判断

已确认的事实：

- `aether-gateway` 在 [apps/aether-gateway/src/lib.rs](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/lib.rs) 挂载了大量顶层模块，是当前主要复杂度承载点。
- `aether-data` 在 [crates/aether-data/src/lib.rs](/Users/elky/Desktop/CompleteProjects/Aether/crates/aether-data/src/lib.rs) 同时暴露 `backends`、`postgres`、`redis`、`repository`，是高扇出核心层。
- `handlers` 的共享胶水集中在 [apps/aether-gateway/src/handlers/mod.rs](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/mod.rs)，后续拆分必须先处理这层共享依赖。
- `auth` 和 `hooks` 很薄，但 [apps/aether-gateway/src/cache/mod.rs](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/cache/mod.rs) 已经不是空壳。
- 大量测试位于 [apps/aether-gateway/src/tests](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/tests)，会显著影响测试态编译成本。

## 3. 里程碑总览

- [x] M0 建立基线与护栏
- [x] M1 拆开 `aether-data` 的接口层与实现层
- [x] M2 从 `handlers` 中抽共享 DTO / service facade
- [ ] M3 提取 `admin handlers` 为独立 feature crate
- [ ] M4 收束 `ai_pipeline` 对 gateway 内核的反向依赖
- [ ] M5 提取 `ai_pipeline` 为独立 crate
- [ ] M6 清理残留模块、测试结构和文档

## 4. 执行节奏

建议按以下节奏推进：

1. 先做可验证的边界收缩，再做目录搬迁。
2. 先减少依赖扇出，再减少文件数量。
3. 每个里程碑结束都要保证 `cargo check` 和相关测试仍可运行。
4. 任何跨 crate 拆分都必须先定义对外接口，再移动实现。

## 5. 工作流并行板

这部分用于分派主代理、worker 子代理、explorer 子代理。

| ID | 状态 | owner | 范围 | 前置依赖 | 交付物 |
| --- | --- | --- | --- | --- | --- |
| WS-01 | 已完成 | `explorer` | workspace 只读分析 | 无 | crate 依赖图、gateway 模块耦合矩阵、阻塞关系表 |
| WS-02 | 进行中 | `main` | `crates/aether-data/**` | 建议先吸收 `WS-01` 结果 | 数据契约层与实现层边界定义、迁移顺序 |
| WS-03 | 已完成 | `main` + `worker` | `apps/aether-gateway/src/handlers/**`、少量 `state/control` | `main` 先确认共享 DTO 和 state 归属 | `handlers` 共享面收束、helper/DTO/facade 分类结果 |
| WS-04 | 进行中 | `worker` | `apps/aether-gateway/src/handlers/admin/**` | `WS-03` 收束共享面 | admin 子域重组方案和首批外提候选 |
| WS-05 | 进行中 | `main` + `worker` | `apps/aether-gateway/src/ai_pipeline/**` | `main` 先确认 runtime/control facade | `ai_pipeline` 降耦合清单和替换补丁 |
| WS-06 | 未开始 | `worker` | `apps/aether-gateway/src/tests/**`、`crates/aether-testkit/**` | 无 | 测试迁移清单、编译热点与测试归属矩阵 |
| WS-07 | 未开始 | `explorer` | 微型 crate 策略复核 | `WS-02`、`WS-03`、`WS-05` 稳定后再做 | `aether-cache` / `aether-http` / `aether-wallet` 是否合并的结论 |

推荐启动顺序：

1. 立即启动 `WS-01`、`WS-02`、`WS-06`
2. `main` 给出边界后，启动 `WS-03`、`WS-05`
3. `WS-03` 出结果后，再启动 `WS-04`
4. `WS-07` 最后处理，不阻塞主线

## 6. 详细计划

## M0 建立基线与护栏

当前产物：

- [重构基线](/Users/elky/Desktop/CompleteProjects/Aether/docs/architecture/refactor-baseline.md)
- [`aether-data` 拆分边界设计](/Users/elky/Desktop/CompleteProjects/Aether/docs/architecture/aether-data-seam-design.md)

目标：

- 为后续拆分建立可对比的编译、依赖、测试基线
- 避免“拆完了但没人知道收益和回归”

完成定义：

- 有一份当前依赖关系快照
- 有一组最小验证命令
- 有清晰的阶段性验收门槛

任务：

- [x] 记录当前 workspace crate 依赖图
- [x] 记录 `aether-gateway` 关键模块边界和共享入口
- [x] 选定每阶段必须通过的最小命令集
- [x] 记录当前主要编译热点和测试热点
- [x] 建立重构决策日志区，后续每次边界调整都补充

建议验证：

- `cargo check -p aether-gateway`
- `cargo check -p aether-data`
- `cargo test -p aether-gateway --lib`
- 关键 API / pipeline / state 相关测试子集

当前状态：

- [x] `cargo check -p aether-data`
- [x] `cargo check -p aether-gateway`
- [x] `cargo test -p aether-gateway --lib`

## M1 拆开 `aether-data` 的接口层与实现层

当前产物：

- [`aether-data` 拆分边界设计](/Users/elky/Desktop/CompleteProjects/Aether/docs/architecture/aether-data-seam-design.md)
- [`crates/aether-data-contracts`](/Users/elky/Desktop/CompleteProjects/Aether/crates/aether-data-contracts)

目标：

- 让依赖方尽量依赖 trait / types / query contract
- 把 `sqlx` / `redis` / backend 细节留在实现层

完成定义：

- 新的数据接口层 crate 已建立
- 至少一批上游 crate 不再直接依赖具体 backend 实现
- `aether-data` 不再同时承担“契约层 + 后端实现 + 仓储聚合”三种职责

任务：

- [x] 盘点 `aether-data` 中哪些类型属于稳定契约，哪些属于实现细节
- [x] 新建数据契约层 crate，承载共享 types / traits / query inputs
- [x] 把首批 repository trait / types 迁移到契约层
- [x] 把 `postgres` / `redis` / `backends` 留在实现层
- [x] 迁移 `aether-scheduler-core` 到新的契约层依赖
- [x] 迁移 `aether-video-tasks-core` 到新的契约层依赖
- [x] 迁移 `aether-usage-runtime` 到新的契约层依赖
- [x] 迁移 `aether-billing` 到新的契约层依赖
- [x] 迁移 `aether-model-fetch` 到新的契约层依赖
- [x] 迁移 `aether-provider-transport` 到新的契约层依赖
- [x] 清理旧的重导出，避免继续走实现层捷径

当前状态：

- [x] 已创建 `aether-data-contracts`
- [x] 已迁移 `candidate_selection` / `candidates` / `provider_catalog` / `quota` contracts
- [x] `aether-data` 已改为兼容重导出这些 contracts
- [x] `aether-scheduler-core` 已切到新 crate
- [x] `billing` contracts 已迁移到 `aether-data-contracts`
- [x] `aether-billing` 已切到新 crate
- [x] `global_models` contracts 已迁移到 `aether-data-contracts`
- [x] `aether-model-fetch` 已切到新 crate
- [x] `video_tasks` contracts 已迁移到 `aether-data-contracts`
- [x] `aether-video-tasks-core` 已切到新 crate
- [x] `aether-provider-transport` 已切到 contracts 类型与错误；`redis` 锁仍保留在 `aether-data`
- [x] `usage` / `settlement` contracts 已迁移到 `aether-data-contracts`
- [x] `aether-usage-runtime` 已切到 contracts 类型与错误；Redis stream 运行时仍保留在 `aether-data`
- [x] `aether-gateway` 已新增 `aether-data-contracts` 依赖
- [x] gateway 的 `maintenance/runtime` 错误类型，以及 `state/video`、`handlers/public/ai_public`、`data/{candidates,decision_trace}` 已切到 contracts
- [x] gateway 的 `state/catalog`、`state/admin_types`、`state/runtime/{candidate_queries,usage_queries}`、`handlers/public/support/{dashboard_filters,wallet/reads,user_me_catalog,models/responses}`、`handlers/public/system_modules_helpers/system` 已切到 contracts
- [x] gateway 的 `usage/worker` 测试 trait 依赖已切到 contracts；in-memory 实现仍保留在 `aether-data`
- [x] gateway 的 `model_fetch/runtime`、`wallet_runtime/quota` 以及测试文件 `tests/{usage,audit,files/mod}`、`model_fetch/tests` 已继续切到 contracts；in-memory 实现仍保留在 `aether-data`
- [x] gateway 的 `tests/frontdoor.rs`、`tests/async_task.rs`、`tests/video/{mod,data_read,registry_poller,gemini_sync_create}` 已继续切到 contracts；in-memory 实现仍保留在 `aether-data`
- [x] gateway 的 `tests/control/mod.rs` 与 `tests/control/admin/{health_access,endpoints/{routes,quota,keys},video_tasks,provider_query,models/{provider,global}}` 已继续切到 contracts；in-memory 实现仍保留在 `aether-data`
- [x] gateway 的 `tests/control/admin/{pool,stats,usage,provider_ops,gemini_files,providers,system}` 已继续切到 contracts；in-memory 实现仍保留在 `aether-data`
- [x] gateway 的 `tests/ai_execute/{finalize_local,finalize_local_cli/{direct,compact},finalize_local_provider/claude,lifecycle,stream_provider,stream/decision,stream_cli/compact,sync/{chat,claude,gemini}/mod,stream_provider_gemini/mod}` 已继续切到 contracts；`StoredAuthApiKeySnapshot` 与 in-memory 实现仍保留在 `aether-data`
- [x] gateway 的 `handlers/public/support/{user_me_usage,test_connection/shared}` 已切到 contracts；`async_task/{http,query,runtime}` 当前工作树已切到 contracts，`async_task/http/cancel` 漏点也已补齐
- [x] gateway 的 `usage/{http,reporting/{context,mod}}`、`request_candidate_runtime`、`executor/candidate_loop` 以及 `handlers/public/{catalog_helpers,support,support/models/shared}`、`handlers/shared/catalog` 已继续切到 contracts；测试里的 `InMemory*` 实现仍保留在 `aether-data`
- [x] gateway 的 `handlers/mod`、`maintenance/tests`、`data/tests` 与 `tests/video/{gemini_sync_task,openai_sync_create,openai_sync_task,stream}` 已继续切到 contracts；`InMemory*` 与 auth 实现仍保留在 `aether-data`
- [x] gateway 的 `model_fetch/runtime/state` 与 `data/state/integrations` 已继续切到 contracts；`RequestAuditReader`、auth snapshot 与少量 in-memory 测试实现仍保留在 `aether-data`
- [x] `tests/architecture.rs` 已同步收紧新的 ownership 边界：`RequestAuditBundle` 继续归 `aether-data`，`StoredRequestUsageAudit` 改归 `aether-data-contracts`
- [x] gateway 的 `data/candidate_selection` 已切到 contracts；`tests/frontdoor/public_support` 中的 `StoredRequestUsageAudit`、`UsageRepository`、`ProviderCatalogReadRepository` 与 `StoredProviderActiveGlobalModel` 已切到 contracts
- [x] gateway 的 `execution_runtime/{sync/execution,stream/{execution,execution_failures}}`、`maintenance/runtime{,/provider_checkin}` 已切到 contracts；`RequestCandidateStatus` 与 `provider_catalog` 类型不再直连 `aether-data`
- [x] gateway 的 `tests/control/admin/{api_keys,adaptive,oauth,provider_strategy,monitoring}` 已整批切到 contracts；`InMemory*` 实现继续保留在 `aether-data`
- [x] gateway 的 `scheduler/{state,candidate/{mod,runtime,selection}}` 已整簇切到 contracts；`StoredRequestCandidate`、`StoredProviderCatalog{Key,Provider}`、`StoredProviderQuotaSnapshot`、`StoredMinimalCandidateSelectionRow` 不再直连 `aether-data`
- [x] gateway 的 `state/testing`、`data/state/testing/{mod,video_tasks}`、`data/state/runtime` 与 `state/integrations` 已继续切到 contracts；测试装配层不再直连 `usage` / `provider_catalog` / `candidates` / `video_tasks` / `quota`
- [x] gateway 的 `handlers/admin/observability/**`、`handlers/admin/features/{video_tasks,gemini_files}`、`handlers/admin/system/pool/*`、`handlers/admin/endpoint/health_builders/status` 与 `handlers/admin/provider/oauth/quota/shared` 已切到 contracts；gateway 内已不再残留这些已迁移 contract 的实现层旧路径
- [x] `aether-data` 已删除 `billing` / `candidate_selection` / `candidates` / `global_models` / `provider_catalog` / `quota` / `settlement` / `usage` / `video_tasks` 的空壳 `types.rs` wrapper，改为在各自 `mod.rs` 直接挂接 contracts re-export
- [x] `aether-data` 已完成 `wallet/sql.rs` 的显式 backend error 映射，repository SQL 层不再依赖 `From<sqlx::Error>` / `From<redis::RedisError>`
- [x] `aether-data-contracts` 已移除 `backend-errors` feature 及其 `sqlx` / `redis` 可选依赖，纯 contracts crate 默认保持轻量
- [x] `aether-data` 已停止显式启用 `aether-data-contracts/backend-errors`；实现层通过本地 helper 显式映射 backend error
- [x] `aether-gateway` 的 `data/state/auth.rs` 与 `maintenance/runtime/{db_maintenance,audit_cleanup,pending_cleanup,stats_daily,stats_hourly,usage_cleanup,wallet_daily_usage}` 已完成显式 postgres error 映射
- [x] `aether-testkit/src/bin/failure_recovery_baseline.rs` 已补齐显式 postgres error 映射，workspace 再次恢复全绿
- [x] 主线已从“路径迁移”收尾到“边界强制执行”；gateway 中这批已迁移 contracts 的实现层旧路径已清空，只剩架构测试字符串断言与尚未拆出的非-contract 模块路径
- [x] `cargo check -p aether-data-contracts`
- [x] `cargo check -p aether-billing`
- [x] `cargo check -p aether-model-fetch`
- [x] `cargo check -p aether-provider-transport`
- [x] `cargo check -p aether-scheduler-core`
- [x] `cargo check -p aether-usage-runtime`
- [x] `cargo check -p aether-video-tasks-core`
- [x] `cargo check -p aether-data`
- [x] `cargo check -p aether-gateway`
- [x] `cargo test -p aether-provider-transport --no-run`
- [x] `cargo test -p aether-usage-runtime --no-run`

并行说明：

- 上游 crate 的迁移可以分批并行
- trait / facade 的第一次定义必须先完成，不能并行乱改

## M2 从 `handlers` 中抽共享 DTO / service facade

目标：

- 先把 [apps/aether-gateway/src/handlers/mod.rs](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/mod.rs) 这类共享胶水层变薄
- 为后续拆 `admin handlers` 创造条件

完成定义：

- admin/public/shared handler 不再共同依赖一个超大 `handlers/mod.rs`
- 共享 payload、formatter、service facade 有清晰归属

任务：

- [x] 盘点 `handlers/mod.rs` 里的共享能力类别
- [x] 区分 DTO、formatter、request helper、service facade、route helper
- [x] 把纯工具函数迁移到独立 `shared` 模块或 crate 候选目录
- [x] 把面向管理域的 payload 和 facade 下沉到 admin 专属边界
- [x] 清理 `handlers` 对 `state` 暴露的过宽类型依赖
- [x] 为 admin/public/proxy/internal 建立更窄的对外接口

当前状态：

- [x] 新增 [`handlers/admin/shared/support.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/admin/shared/support.rs)、[`handlers/admin/shared/paths.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/admin/shared/paths.rs) 与 [`handlers/admin/shared/payloads.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/admin/shared/payloads.rs)，admin 专属常量 / route helper / payload 已从共享层物理下沉到 admin 边界
- [x] [`handlers/mod.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/mod.rs) 已进一步收缩为纯模块装配；非 admin 侧 usage/stats helper 现通过 [`handlers/shared/usage_stats.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/shared/usage_stats.rs) 暴露
- [x] `public`、`internal`、`proxy` 与 `tests/control/admin` 已显式依赖 `crate::handlers::shared::*`、`crate::handlers::admin::shared::*` 或其真实父模块，不再依赖根 `handlers` 胶水层
- [x] [`handlers/shared/mod.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/shared/mod.rs) 现仅保留真正共享能力；旧的 `handlers/shared/admin_paths.rs` 和 `handlers/shared/admin_support.rs` 已删除，[`handlers/shared/payloads.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/shared/payloads.rs) 仅保留 internal DTO
- [x] [`state/types.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/state/types.rs) 已新增 `GatewayUserSessionView`、`GatewayUserPreferenceView` 与 `GatewayAdminPaymentCallbackView`；`handlers/**` 不再直接依赖原始 `data::state` 记录类型

并行说明：

- 工具函数整理和 payload 分类可以并行
- 最终接口收口需要主线统一裁决

## M3 提取 `admin handlers` 为独立 feature crate

目标：

- 从 gateway 中拆出管理面 API 的主要实现
- 降低 `aether-gateway` 对 admin 代码的直接承载

完成定义：

- admin handlers 已进入独立 crate
- gateway 仅保留路由装配、状态注入、最少量共享协议
- admin crate 不再直接抓取大量 gateway 私有模块

任务：

- [ ] 定义 admin feature crate 的对外接口
- [ ] 明确 admin crate 可访问的状态、service、DTO 边界
- [ ] 迁移 auth/billing/model/provider/system/users 等 admin 子域
- [ ] 将 admin 路由注册与具体实现解耦
- [ ] 回收 gateway 对 admin 私有类型的重导出
- [ ] 修复迁移后测试入口与 test helper

当前状态：

- [x] [`handlers/admin/mod.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/admin/mod.rs) 已收缩为纯模块装配；admin 子域边界现在直接以 `auth`、`billing`、`endpoint`、`features`、`model`、`observability`、`provider`、`system`、`users` 暴露
- [x] `handlers/proxy/local`、`handlers/internal/gateway_helpers`、`maintenance/runtime`、`handlers/shared/usage_stats`、`handlers/public/support/user_me` 与 `tests/control/admin/*` 已改为显式依赖对应 admin 子域模块，不再通过单一 facade 转运
- [x] 旧的 `handlers/admin/facade.rs` 已删除；新的护栏要求根 `admin/mod.rs` 不再保留 facade seam
- [x] `provider` 子域已将 provider 专属 `support` / `paths` / `payloads` 下沉到 [`handlers/admin/provider/shared`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/admin/provider/shared/mod.rs)，`handlers/admin/shared` 只保留非 provider admin DTO、路径与通用 helper
- [x] `model` 子域已将 global-model 专属 `paths` / `payloads` 下沉到 [`handlers/admin/model/shared`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/admin/model/shared/mod.rs)，`handlers/admin/shared` 不再拥有 model 专属路由 helper 或 payload
- [x] `system` 子域已将 management-token、system config / email template、oauth provider config 专属 `paths` / `payloads` 下沉到 [`handlers/admin/system/shared`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/admin/system/shared/mod.rs)
- [x] `observability` 根模块已收缩为三个本地响应入口；跨子域 stats helper 已改由 [`handlers/admin/observability/stats`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/admin/observability/stats/mod.rs) 直接承载，`usage` 与 [`handlers/shared/usage_stats.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/shared/usage_stats.rs) 不再经由根 `observability/mod.rs` 取用内部 helper
- [x] `monitoring` 根模块已收缩为本地入口与常量边界；`routes`、`activity`、`cache`、`resilience`、`trace` 已改为直接依赖各自真实子模块，不再经由 [`handlers/admin/observability/monitoring/mod.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/admin/observability/monitoring/mod.rs) 转运
- [x] `monitoring` 的 cache 配置与 redis 提示面已下沉到 [`handlers/admin/observability/monitoring/cache_config.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/admin/observability/monitoring/cache_config.rs)；[`monitoring/mod.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/admin/observability/monitoring/mod.rs) 不再持有 cache 子域常量
- [x] `monitoring/common.rs` 已删除，原有职责已拆到 [`responses.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/admin/observability/monitoring/responses.rs)、[`cache_types.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/admin/observability/monitoring/cache_types.rs)、[`usage_helpers.rs`](/Users/elky/Desktop/CompleteProjects/Aether/apps/aether-gateway/src/handlers/admin/observability/monitoring/usage_helpers.rs) 与 `activity` / `resilience` 子域自身
- [ ] observability 等大子域内部仍有较厚的 query helper / DTO / service 面，距离独立 crate 仍需继续收窄归属

并行说明：

- admin 子域可以按目录并行搬迁
- 路由装配层和共享状态边界必须先冻结

## M4 收束 `ai_pipeline` 对 gateway 内核的反向依赖

目标：

- 在提取 crate 之前，先减少 `ai_pipeline` 对 `control`、`execution_runtime`、`provider_transport`、`executor`、`scheduler`、`data` 等内部模块的直接依赖

完成定义：

- `ai_pipeline` 的外部依赖主要收敛到少数 facade / contract 模块
- 不再到处直接引用 gateway 私有实现细节

任务：

- [x] 统计 `ai_pipeline` 当前直接依赖的 gateway 模块清单
- [x] 定义 pipeline 所需的 control facade
- [x] 定义 pipeline 所需的 execution facade
- [x] 定义 pipeline 所需的 scheduler facade
- [x] 定义 pipeline 所需的 data / auth snapshot facade
- [ ] 用 facade 替代直接模块引用
- [ ] 整理 finalize / planner / runtime 三大子层的边界

并行说明：

- facade 设计由主线负责
- 不同依赖面的替换可以分给不同子代理并行推进

## M5 提取 `ai_pipeline` 为独立 crate

目标：

- 在边界收束完成后，把 pipeline 迁出 gateway 主 crate

完成定义：

- `aether-ai-pipeline` 或等价 crate 已建立
- gateway 通过稳定接口调用 pipeline
- pipeline 不再依赖 gateway 内部散落模块

任务：

- [x] 建立 pipeline crate 和最小公开 API
- [ ] 迁移 `contracts`
- [ ] 迁移 `conversion`
- [ ] 迁移 `adaptation`
- [ ] 迁移 `planner`
- [ ] 迁移 `runtime`
- [ ] 迁移 `finalize`
- [ ] 修复测试、feature gate、可见性和重导出
- [x] 把 `plan_kinds` / `report_kinds` / `actions` 常量搬到新 crate，gateway 只保留 thin re-export
- [x] 把 `planner/route.rs` 的执行 runtime plan 解析逻辑搬到 pipeline crate，使 gateway 只保留 facade 接口
- [x] 把 `planner/specialized/files.rs` 的 pure spec / resolver 迁到 pipeline crate
- [x] 把 `planner/specialized/video.rs` 的 pure spec / resolver 迁到 pipeline crate
- [x] 把 `planner/standard/openai/cli` 的 pure spec / resolver 迁到 pipeline crate
- [ ] 把 `conversion/error.rs` 中的 `LocalCoreSyncErrorKind` / `build_core_error_body_for_client_format` 等 pure helpers 迁到 pipeline crate，并让 gateway `conversion/mod.rs` 只做 thin re-export
- [ ] 把 `planner/common.rs` 的纯请求体解析逻辑迁到 pipeline crate，并让 gateway 仅保留 header 判定与 thin adapter

并行说明：

- 迁移顺序应从 contracts / conversion 开始
- planner、runtime、finalize 可以在边界稳定后分批并行

## M6 清理残留模块、测试结构和文档

目标：

- 回收重构过渡期遗留噪声
- 保证新的边界被文档和测试固定住

完成定义：

- 旧重导出、临时 facade、过渡模块已清理
- 文档和测试结构与新的 crate 边界一致

任务：

- [ ] 清理无效重导出和兼容层
- [ ] 复核 `auth` / `hooks` 是否继续保留
- [ ] 复核 gateway 内部 `cache`、`state`、`query` 的边界
- [ ] 调整测试目录和测试依赖归属
- [ ] 更新 architecture / deploy / contributor 文档
- [ ] 输出重构总结和后续 backlog

## 7. 子代理任务板

使用规则：

- 只有“写入范围明确”的任务才分给 worker 子代理。
- 只有“回答具体代码问题”的任务才分给 explorer 子代理。
- 主代理负责边界设计、最终整合、冲突裁决和验收。

推荐初始拆分：

| ID | 状态 | owner | 范围 | 前置依赖 | 交付物 |
| --- | --- | --- | --- | --- | --- |
| SA-01 | [ ] | `explorer` | `crates/aether-data` | 无 | 契约层 / 实现层拆分建议，列出可迁移类型和 trait |
| SA-02 | [x] | `main` + `worker` | `apps/aether-gateway/src/handlers` | 无 | `handlers/mod.rs` 共享能力分类清单 |
| SA-03 | [ ] | `explorer` | `apps/aether-gateway/src/ai_pipeline` | 无 | `ai_pipeline` 对 gateway 内部模块依赖矩阵 |
| SA-04 | [x] | `worker` | 新数据契约层 crate | M1 设计冻结 | 建 crate、迁移首批 types / traits |
| SA-05 | [x] | `worker` | `crates/aether-scheduler-core` | M1 基础契约就位 | 切换到新数据契约依赖 |
| SA-06 | [x] | `worker` | `crates/aether-usage-runtime` | M1 基础契约就位 | 切换到新数据契约依赖 |
| SA-07 | [ ] | `worker` | admin billing / auth 子域 | M2 边界冻结 | admin 子域迁移补丁 |
| SA-08 | [ ] | `worker` | pipeline facade 替换 | M4 facade 设计完成 | 分模块替换直接依赖 |

子代理追加模板：

| ID | 状态 | owner | 范围 | 前置依赖 | 交付物 |
| --- | --- | --- | --- | --- | --- |
| SA-XX | [ ] | `explorer/worker` | 待填写 | 待填写 | 待填写 |

## 8. 不可并行项

以下事项不应并行推进：

- `aether-data` 契约层的第一次边界定义
原因：一旦不同子代理各自定义接口，后续会产生重复 contract 和命名漂移。

- `admin handlers` crate 的公开接口设计
原因：路由装配、状态访问、共享 DTO 一旦不统一，后续迁移会反复返工。

- `ai_pipeline` facade 的第一版设计
原因：这一步决定后续 planner/runtime/finalize 的迁移方向，必须先统一协议面。

## 9. 验收清单

每完成一个里程碑，至少确认以下事项：

- [ ] `cargo check` 覆盖受影响 crate
- [ ] 受影响测试子集通过
- [ ] 没有新增反向依赖或循环依赖
- [ ] 文档中的复选框和实际代码状态一致
- [ ] 如果新增 facade / contract，已写清归属和生命周期

## 10. 决策日志

按时间倒序追加：

- [x] 2026-04-06：M2 第一刀采用“先迁 import 到 `handlers::shared` / 真实父模块，再删除 `pub(crate) use shared::*`”的兼容优先策略，避免在非绿态下盲拆 `handlers`
- [x] 2026-04-06：M4 第一刀采用“先在 `planner` 内落 `gateway_facade`，再把 scheduler / data-auth snapshot / provider transport / request-candidate runtime 的直连调用整批切到 facade”的单点收口策略，优先压低 `ai_pipeline` 对 gateway 内核散点依赖
- [x] 2026-04-06：M2 第二刀采用“先落 `handlers/admin/shared`，再把 admin 树整体切到 `crate::handlers::admin::shared::*`，最后删除 `handlers/shared` 中 admin 残留”的物理边界收缩策略
- [x] 2026-04-06：M2 第三刀采用“将跨 admin/public 的 usage/stats helper 收口到 `handlers/shared/usage_stats.rs`，根 `handlers/mod.rs` 不再承担 facade 转运”的根模块纯化策略
- [x] 2026-04-06：M3 第二轮采用“移除根 `admin/facade.rs`，转为直接暴露顶层 admin 子域模块”的边界显式化策略，优先压平对子域真实归属的隐藏层
- [x] 2026-04-06：M2 第四刀采用“在 `state/types.rs` 引入 handler-facing view model，由 `state/runtime` 完成原始记录类型与 app 视图之间的转换”的边界隔离策略
- [x] 2026-04-06：M3 第一刀采用“先抽 `handlers/admin/facade.rs` 冻结 admin 对外入口，再清空 `handlers/admin/mod.rs` 根 re-export”的边界冻结策略，为后续子域外提保留单一接缝
- [x] 2026-04-06：`aether-usage-runtime` 采用“usage/settlement contract 前移、Redis stream 运行时后留”的迁移路径
- [x] 2026-04-06：`aether-provider-transport` 采用“contracts 类型与错误前移、`redis` 锁后留”的折中迁移路径
- [x] 2026-04-06：`global_models` 与 `video_tasks` 均判定为纯 contract 文件，直接整文件迁入 `aether-data-contracts`
- [x] 2026-04-06：第二个消费者优先选择 `aether-billing`，不先硬拆 `DataLayerError`，以控制回归面
- [x] 2026-04-06：第一刀选择 `candidate_selection` / `candidates` / `provider_catalog` / `quota`，并先切 `aether-scheduler-core`
- [x] 2026-04-06：确认执行顺序为“先拆 `aether-data`，再收束 `handlers`，后处理 `ai_pipeline` crate 化”
- [x] 2026-04-06：M0 基线文档已建立，WS-02 第一版边界设计已输出

## 11. 执行记录

按时间顺序追加：

- [x] 2026-04-06：初始化执行计划文档，并加入子代理并行工作流
- [x] 2026-04-06：执行 `cargo check -p aether-data` 与 `cargo check -p aether-gateway`，当前构建基线为绿色
- [x] 2026-04-06：新增 `docs/architecture/refactor-baseline.md` 与 `docs/architecture/aether-data-seam-design.md`
- [x] 2026-04-06：新增 `crates/aether-data-contracts`，承接首批 repository contracts 与 `DataLayerError`
- [x] 2026-04-06：`aether-data` 已切为首批 contracts 的兼容重导出层
- [x] 2026-04-06：`aether-scheduler-core` 已切换到 `aether-data-contracts`
- [x] 2026-04-06：验证通过 `cargo check -p aether-data-contracts`、`cargo check -p aether-scheduler-core`、`cargo check -p aether-data`、`cargo check -p aether-gateway`
- [x] 2026-04-06：新增 `billing` contracts 到 `aether-data-contracts`，`aether-data` 保留兼容重导出
- [x] 2026-04-06：`aether-billing` 已切换到 `aether-data-contracts`，且源码中不再残留 `aether_data::` 导入
- [x] 2026-04-06：验证通过 `cargo check -p aether-billing`、`cargo check -p aether-data`、`cargo check -p aether-gateway`
- [x] 2026-04-06：新增 `global_models` contracts 到 `aether-data-contracts`，`aether-model-fetch` 已切换到新 crate
- [x] 2026-04-06：新增 `video_tasks` contracts 到 `aether-data-contracts`，`aether-video-tasks-core` 已切换到新 crate
- [x] 2026-04-06：验证通过 `cargo check -p aether-model-fetch`、`cargo check -p aether-video-tasks-core`、`cargo check -p aether-data`、`cargo check -p aether-gateway`
- [x] 2026-04-06：`aether-provider-transport` 已切换到 contracts 中的 `provider_catalog` / `video_tasks` types 与 `DataLayerError`
- [x] 2026-04-06：`aether-provider-transport` 只保留 `aether-data::redis` 作为实现层依赖；`snapshot.rs` 测试也已去除 `aether-data` in-memory repository 依赖
- [x] 2026-04-06：验证通过 `cargo check -p aether-provider-transport`、`cargo test -p aether-provider-transport --no-run`、`cargo check -p aether-gateway`
- [x] 2026-04-06：新增 `usage` / `settlement` contracts 到 `aether-data-contracts`，`aether-usage-runtime` 已切换到 contracts 中的类型与 `DataLayerError`
- [x] 2026-04-06：`aether-usage-runtime` 仅保留 `aether-data::redis` 作为实现层依赖
- [x] 2026-04-06：验证通过 `cargo check -p aether-usage-runtime`、`cargo test -p aether-usage-runtime --no-run`、`cargo check -p aether-gateway`
- [x] 2026-04-06：`aether-gateway` 已新增 `aether-data-contracts` 依赖，并将 `maintenance/runtime`、`state/video`、`handlers/public/ai_public`、`data/{candidates,decision_trace}` 切到 contracts
- [x] 2026-04-06：验证通过 `cargo check -p aether-gateway`
- [x] 2026-04-06：更新 `tests/architecture.rs` 的 ownership 断言到 `aether-data-contracts` 边界，并验证通过 `cargo test -p aether-gateway tests::architecture::`
- [x] 2026-04-06：验证通过 `cargo test -p aether-gateway --lib`
- [x] 2026-04-06：`aether-gateway` 的 `state/catalog`、`state/admin_types`、`handlers/public/support/{dashboard_filters,wallet/reads}`、`handlers/public/system_modules_helpers/system` 已切到 contracts
- [x] 2026-04-06：再次验证通过 `cargo check -p aether-gateway` 与 `cargo test -p aether-gateway --lib`
- [x] 2026-04-06：并行推进 gateway 收缩；主线已迁移 `handlers/public/support/{user_me_catalog,models/responses}`、`state/runtime/{candidate_queries,usage_queries}` 与 `usage/worker` 的 contracts 依赖
- [x] 2026-04-06：子代理已完成 `maintenance/runtime/{stats_daily,stats_hourly,wallet_daily_usage,pending_cleanup,config}` 的 `DataLayerError` 收缩，并已人工核实合并
- [x] 2026-04-06：并行收口后再次验证通过 `cargo check -p aether-gateway` 与 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：主线已迁移 `model_fetch/runtime`、`tests/{usage,audit,files/mod}` 到 contracts，并保持 `aether_data` 仅承载 in-memory 实现
- [x] 2026-04-06：子代理已迁移 `model_fetch/tests` 与 `wallet_runtime/quota` 的 contracts 依赖，并验证通过 `cargo check -p aether-gateway`
- [x] 2026-04-06：再次验证通过 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：主线已迁移 `tests/async_task.rs` 与 `tests/video/{mod,data_read,registry_poller,gemini_sync_create}` 到 contracts，并保持 `aether_data` 仅承载 in-memory 实现
- [x] 2026-04-06：子代理已迁移 `tests/frontdoor.rs` 的 contracts 依赖，并验证通过 `cargo check -p aether-gateway`
- [x] 2026-04-06：再次验证通过 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：主线已迁移 `tests/control/admin/{health_access,endpoints/{routes,quota,keys},video_tasks}` 到 contracts，并保持 `aether_data` 仅承载 in-memory 实现
- [x] 2026-04-06：子代理已迁移 `tests/control/mod.rs` 与 `tests/control/admin/{provider_query,models/{provider,global}}` 的 contracts 依赖；过程中修复了 `InMemoryWalletRepository` 的重复导入
- [x] 2026-04-06：再次验证通过 `cargo check -p aether-gateway` 与 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：主线已迁移 `tests/control/admin/{pool,stats,usage,provider_ops,gemini_files}` 到 contracts，并保持 `aether_data` 仅承载 in-memory 实现
- [x] 2026-04-06：子代理已迁移 `tests/control/admin/{providers,system}` 的 contracts 依赖；过程中补齐了 `providers.rs` 里一个遗留的 `AdminProviderModelListQuery` 路径
- [x] 2026-04-06：再次验证通过 `cargo check -p aether-gateway` 与 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：主线已迁移 `tests/ai_execute/{finalize_local,finalize_local_cli/{direct,compact},finalize_local_provider/claude,lifecycle,stream_provider,stream/decision,stream_cli/compact,sync/{chat,claude,gemini}/mod,stream_provider_gemini/mod}` 到 contracts；`StoredAuthApiKeySnapshot` 与 in-memory 实现继续保留在 `aether-data`
- [x] 2026-04-06：修正 `tests/ai_execute/{stream_provider,stream/decision}` 中两个错误的 `aether_data_contracts::repository::auth` 导入，避免指向不存在的 contracts 模块
- [x] 2026-04-06：再次验证通过 `cargo check -p aether-gateway` 与 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：主线已迁移 `handlers/public/support/{user_me_usage,test_connection/shared}` 到 contracts，并补齐 `async_task/http/cancel` 的 `video_tasks` contracts 漏点；`async_task/{http,query,runtime}` 在当前工作树中已切到 contracts
- [x] 2026-04-06：快速模式下已验证通过 `cargo check -p aether-gateway`；本轮未重复执行 `cargo test -p aether-gateway --lib`
- [x] 2026-04-06：主线已迁移 `usage/{http,reporting/{context,mod}}`、`request_candidate_runtime` 与 `executor/candidate_loop` 到 contracts；测试里的 `InMemory*` 实现继续保留在 `aether-data`
- [x] 2026-04-06：主线已迁移 `handlers/public/{catalog_helpers,support,support/models/shared}` 与 `handlers/shared/catalog` 到 contracts
- [x] 2026-04-06：两批连续收口后均验证通过 `cargo check -p aether-gateway`；本轮未重复执行 `cargo test -p aether-gateway --lib`
- [x] 2026-04-06：主线已迁移 `handlers/mod`、`maintenance/tests`、`data/tests` 与 `tests/video/{gemini_sync_task,openai_sync_create,openai_sync_task,stream}` 到 contracts；`InMemory*` 与 auth 实现继续保留在 `aether-data`
- [x] 2026-04-06：主线已迁移 `model_fetch/runtime/state` 与 `data/state/integrations` 到 contracts；`RequestAuditReader`、auth snapshot 与少量 in-memory 测试实现暂时继续保留在 `aether-data`
- [x] 2026-04-06：修正 `tests::architecture::gateway_request_audit_bundle_type_is_owned_by_aether_data` 的 ownership 断言，使 `usage/http.rs` 对 `StoredRequestUsageAudit` 的依赖改为指向 `aether-data-contracts`
- [x] 2026-04-06：再次验证通过 `cargo test -p aether-gateway tests::architecture::gateway_request_audit_bundle_type_is_owned_by_aether_data -- --exact`
- [x] 2026-04-06：再次验证通过 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：主线已迁移 `data/candidate_selection`，并收口 `tests/frontdoor/public_support` 中剩余的 `usage` / `provider_catalog` / `global_models` contracts 依赖
- [x] 2026-04-06：再次验证通过 `cargo check -p aether-gateway`，以及 `tests::frontdoor::public_support::{gateway_handles_users_me_available_models_locally_without_proxying_upstream,gateway_returns_service_unavailable_for_users_me_available_models_without_provider_catalog}`
- [x] 2026-04-06：主线已整批迁移 `execution_runtime/{sync/execution,stream/{execution,execution_failures}}` 与 `maintenance/runtime{,/provider_checkin}`，清除其中残留的 `RequestCandidateStatus` 和 `provider_catalog` 旧路径
- [x] 2026-04-06：主线已整批迁移 `tests/control/admin/{api_keys,adaptive,oauth,provider_strategy,monitoring}` 的 contracts 依赖；`InMemory*` 实现继续保留在 `aether-data`
- [x] 2026-04-06：再次验证通过 `cargo check -p aether-gateway`
- [x] 2026-04-06：再次验证通过 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：主线已整簇迁移 `scheduler/{state,candidate/{mod,runtime,selection}}` 的 contracts 依赖，清除其中残留的 `candidate_selection` / `candidates` / `provider_catalog` / `quota` 旧路径
- [x] 2026-04-06：再次验证通过 `cargo check -p aether-gateway`
- [x] 2026-04-06：再次验证通过 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：主线已整批迁移 `state/testing`、`data/state/testing/{mod,video_tasks}`、`data/state/runtime` 与 `state/integrations` 的 contracts 依赖，清除测试装配层中残留的 `usage` / `provider_catalog` / `candidates` / `video_tasks` / `quota` 旧路径
- [x] 2026-04-06：主线已整批迁移 `handlers/admin/observability/**` 的 contracts 依赖，并保持 `auth` / `users` / `InMemory*` 留在 `aether-data`
- [x] 2026-04-06：主线已收尾迁移 `handlers/admin/{endpoint/health_builders/status,system/pool/*,provider/oauth/quota/shared,features/{video_tasks,gemini_files}}` 的 contracts 依赖，gateway 全局残留已压到仅剩架构测试字符串断言
- [x] 2026-04-06：再次验证通过 `cargo check -p aether-gateway`
- [x] 2026-04-06：再次验证通过 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：全 workspace 的旧 contract 路径扫描已收敛到仅剩 `tests/architecture.rs` 中的一条字符串断言
- [x] 2026-04-06：`aether-data` 已删除 9 个空壳 `types.rs` wrapper，并把 `billing` / `candidate_selection` / `candidates` / `global_models` / `provider_catalog` / `quota` / `settlement` / `usage` / `video_tasks` 的 contracts re-export 内联到各自 `mod.rs`
- [x] 2026-04-06：再次验证通过 `cargo check -p aether-data`
- [x] 2026-04-06：再次验证通过 `cargo check -p aether-gateway`
- [x] 2026-04-06：wrapper 删除后再次验证通过 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：主线已整批迁移 `data/state/{mod,integrations}`、`ai_pipeline/planner/{passthrough/provider,candidate_affinity}` 与 `handlers/admin/model/*` 的 contracts 依赖，并补齐 `handlers/admin/{endpoint/health_builders/endpoints,features/gemini_files/upload,system/pool/batch_routes}` 的 `provider_catalog` 旧路径
- [x] 2026-04-06：主线已整批迁移 `data/{candidates,decision_trace}` 的 `#[cfg(test)]` 支持块、`scheduler/candidate/tests/*`、`tests/proxy` 与 `tests/ai_execute/{sync/cli,stream_cli/direct,finalize_local_provider/gemini,finalize_local_cli/cross_format}` 的 contracts 依赖
- [x] 2026-04-06：gateway 内已迁移 contracts 的旧类型路径扫描已清空；剩余 `aether_data::repository::*` 仅保留 `InMemory*` 测试实现模块路径与 `tests/architecture.rs` 的字符串断言
- [x] 2026-04-06：再次验证通过 `cargo check -p aether-gateway` 与 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：`aether-data` 中 `billing` / `candidate_selection` / `candidates` / `global_models` / `provider_catalog` / `quota` / `settlement` / `usage` / `video_tasks` 的 contracts re-export 已从对外公开降为 `pub(crate)`，外部消费者改为只从 `aether-data-contracts` 获取 trait/type
- [x] 2026-04-06：修复 `aether-testkit` 中陈旧的 `AppState::new(upstream_base_url)` 调用，恢复 `cargo check --workspace` 绿灯
- [x] 2026-04-06：`DataLayerError` 已从携带 `sqlx::Error` / `redis::RedisError` 实例改为字符串载荷，同时保留 `From<sqlx::Error>` / `From<redis::RedisError>` 适配，先切断错误枚举对具体库类型的 API 暴露
- [x] 2026-04-06：再次验证通过 `cargo check --workspace` 与 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：`aether-data-contracts` 已新增 `backend-errors` feature；默认编译不再携带 `sqlx` / `redis` backend error adapter，只有 `aether-data` 显式启用该 feature
- [x] 2026-04-06：验证 `cargo tree -e features -p aether-scheduler-core -i aether-data-contracts` 显示纯 consumer 仅使用 `aether-data-contracts/default`
- [x] 2026-04-06：再次验证通过 `cargo check -p aether-data-contracts`、`cargo check --workspace` 与 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：`aether-data/src/error.rs` 已新增本地 `postgres_error` / `redis_error` helper 与 `SqlxResultExt` / `RedisResultExt`，用于在实现层显式映射 backend error，不再依赖隐式 `From`
- [x] 2026-04-06：主线已收口 `aether-data` 基础设施层的显式错误映射，覆盖 `redis/{client,kv,lock,stream}`、`postgres/{tx,lease}` 与 `backends/postgres`
- [x] 2026-04-06：并行收口 repository SQL 层的显式错误映射；主线已完成 `global_models`、`usage`、`billing`、`quota`、`candidate_selection`，子代理已完成 `video_tasks`
- [x] 2026-04-06：上述收口后再次验证通过 `cargo check -p aether-data`、`cargo check --workspace` 与 `cargo test -p aether-gateway --lib`
- [x] 2026-04-06：继续并行收口第二批 repository SQL 显式错误映射；主线已完成 `announcements`、`oauth_providers`、`auth_modules`、`users`，子代理已完成 `shadow_results`
- [x] 2026-04-06：第二批 repository SQL 收口后再次验证通过 `cargo check -p aether-data`、`cargo check --workspace` 与 `cargo test -p aether-gateway --lib`
- [x] 2026-04-06：继续并行收口第三批 repository SQL 显式错误映射；主线已完成 `provider_catalog` 与 `candidates`，子代理已完成 `management_tokens` 与 `gemini_file_mappings`
- [x] 2026-04-06：第三批 repository SQL 收口后再次验证通过 `cargo check -p aether-data`、`cargo check --workspace` 与 `cargo test -p aether-gateway --lib`
- [x] 2026-04-06：继续并行收口第四批 repository SQL 显式错误映射；主线已完成 `auth`，子代理已完成 `settlement` 与 `proxy_nodes`
- [x] 2026-04-06：第四批 repository SQL 收口后再次验证通过 `cargo check -p aether-data`、`cargo check --workspace` 与 `cargo test -p aether-gateway --lib`
- [x] 2026-04-06：主线已收口 `aether-data/src/repository/wallet/sql.rs` 的显式 backend error 映射，并补齐 `backends/postgres.rs` 中一处残留的 `updated_at_unix_secs` 解码漏点
- [x] 2026-04-06：`aether-data-contracts` 已移除 `backend-errors` feature 与 `sqlx` / `redis` 可选依赖；`aether-data` 也已停止启用该 feature，边界从“过渡期兼容”收紧为“显式映射”
- [x] 2026-04-06：`apps/aether-gateway/src/data/state/auth.rs` 已完成本地 `postgres_error` / `SqlxResultExt` / `row_get` 收口，不再依赖 `From<sqlx::Error> for DataLayerError`
- [x] 2026-04-06：`apps/aether-gateway/src/maintenance/runtime/{db_maintenance,audit_cleanup,pending_cleanup,stats_daily,stats_hourly,usage_cleanup,wallet_daily_usage}` 已完成显式 postgres error 映射
- [x] 2026-04-06：`crates/aether-testkit/src/bin/failure_recovery_baseline.rs` 已补齐显式 postgres error 映射；最终验证通过 `cargo check -p aether-data`、`cargo check -p aether-gateway`、`cargo check --workspace` 与 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：M2 第一轮已完成 `handlers` 共享胶水收束：新增 `handlers/shared/admin_support.rs`，将原先根模块里的共享常量 / struct 下沉到 `shared`
- [x] 2026-04-06：`public`、`admin`、`internal`、`proxy` 与 `tests/control/admin` 已改为显式依赖 `crate::handlers::shared::*` 或真实父模块；`handlers/mod.rs` 已删除 `pub(crate) use shared::*`
- [x] 2026-04-06：M2 第一轮最终验证通过 `cargo check -p aether-gateway` 与 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：M2 第二轮已完成 `handlers/admin/shared/{support,paths,payloads}.rs` 接线，admin 树已切到 `crate::handlers::admin::shared::*`；`handlers/shared` 只保留真正共享能力
- [x] 2026-04-06：M2 第二轮已删除 `handlers/shared/admin_paths.rs` 与 `handlers/shared/admin_support.rs`，并将 `handlers/shared/payloads.rs` 收窄为 internal DTO
- [x] 2026-04-06：M2 第二轮最终验证通过 `cargo check -p aether-gateway` 与 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：M2 第三轮已新增 `handlers/shared/usage_stats.rs`，将 public 侧 `user_me` / `wallet` 对 usage/stats helper 的依赖从根 `handlers` 下沉到 `handlers/shared`
- [x] 2026-04-06：M2 第三轮已清空 `handlers/mod.rs` 对 admin stats helper 的 re-export，并同步更新架构断言
- [x] 2026-04-06：M2 第三轮最终验证通过 `cargo check -p aether-gateway` 与 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：M2 第四轮已在 `state/types.rs` 引入 `GatewayUserSessionView`、`GatewayUserPreferenceView` 与 `GatewayAdminPaymentCallbackView`，并将 `state/runtime/{auth/sessions,user_preferences,billing/finance_queries}` 收口为 handler-facing 视图返回
- [x] 2026-04-06：M2 第四轮已迁移 `handlers/public/support/{auth_session,user_me_shared,user_me_sessions,user_me_preferences}`、`handlers/admin/{users/sessions,billing/payments/shared}` 到新的 app-level state view，不再直连原始 `data::state` 记录
- [x] 2026-04-06：M2 第四轮已新增架构断言，禁止 `handlers/**` 再次引用 `StoredUserSessionRecord`、`StoredUserPreferenceRecord` 与 `AdminPaymentCallbackRecord`
- [x] 2026-04-06：M2 第四轮最终验证通过 `cargo check -p aether-gateway` 与 `cargo test -p aether-gateway --lib`，结果 `1214 passed; 0 failed`
- [x] 2026-04-06：M3 第一轮已新增 `handlers/admin/facade.rs`，承接 admin 对外入口；`handlers/admin/mod.rs` 已清空根 re-export，收缩为纯模块装配
- [x] 2026-04-06：M3 第一轮已迁移 `handlers/proxy/local.rs`、`handlers/internal/mod.rs`、`handlers/shared/usage_stats.rs`、`handlers/public/support/user_me.rs`、`maintenance/runtime*` 与 `tests/control/admin/*` 到 `crate::handlers::admin::facade::*`
- [x] 2026-04-06：M3 第一轮已新增架构断言，要求 `handlers/admin/mod.rs` 保持纯模块装配且 `handlers/admin/facade.rs` 继续拥有 admin 对外入口
- [x] 2026-04-06：M3 第二轮已删除 `handlers/admin/facade.rs`，并将 `handlers/proxy/local.rs`、`handlers/internal/mod.rs`、`handlers/shared/usage_stats.rs`、`handlers/public/support/user_me.rs`、`maintenance/runtime*` 与 `tests/control/admin/*` 切到显式 admin 子域模块
- [x] 2026-04-06：M3 第二轮已更新架构断言，要求 `handlers/admin/mod.rs` 保持纯模块装配且不再保留 facade seam
- [x] 2026-04-06：M3 第二轮最终验证通过 `cargo check -p aether-gateway` 与 `cargo test -p aether-gateway --lib`，结果 `1217 passed; 0 failed`
- [x] 2026-04-06：M3 第三轮已将 provider 专属 `support` / `paths` / `payloads` 从 `handlers/admin/shared` 下沉到 `handlers/admin/provider/shared`，并完成 `endpoint`、`model`、`provider/**` 跨域调用点的 import 收口
- [x] 2026-04-06：M3 第三轮已新增架构断言，要求 `handlers/admin/shared/{paths,payloads}.rs` 不再拥有 provider 路由 helper 或 provider payload，所有权固定到 `handlers/admin/provider/shared/*`
- [x] 2026-04-06：M3 第四轮已新增 `handlers/admin/model/shared/{paths,payloads}.rs`，并将 `global_models/routes.rs` 与 `model/write.rs` 切到 model 子域自有 shared surface
- [x] 2026-04-06：M3 第四轮已新增架构断言，要求 `handlers/admin/shared/{paths,payloads}.rs` 不再拥有 global-model 路由 helper 或 payload，所有权固定到 `handlers/admin/model/shared/*`
- [x] 2026-04-06：M3 第五轮已新增 `handlers/admin/system/shared/{paths,payloads}.rs`，并将 `system/core/{management_tokens_routes,oauth_routes,system_routes}.rs` 与 `auth/oauth_config.rs` 切到 system 子域自有 shared surface
- [x] 2026-04-06：M3 第五轮已新增架构断言，要求 `handlers/admin/shared/{paths,payloads}.rs` 不再拥有 management token、system config/email template 或 oauth provider config 路由 helper / payload，所有权固定到 `handlers/admin/system/shared/*`
- [x] 2026-04-06：M3 第六轮已将 `handlers/admin/observability/mod.rs` 收缩为三个本地响应入口；`usage/**` 与 `handlers/shared/usage_stats.rs` 已改为直接依赖 `handlers/admin/observability/stats/*`，不再经由根 `observability` re-export
- [x] 2026-04-06：M3 第六轮已新增架构断言，要求 `handlers/admin/observability/mod.rs` 不再重导出 stats helper 或 monitoring route 类型，shared usage stats facade 必须直接依赖 `observability::stats`
- [x] 2026-04-06：M3 第七轮已将 `handlers/admin/observability/monitoring/mod.rs` 从“模块总线”收缩为入口与常量边界；`routes.rs`、`cache.rs`、`activity.rs`、`resilience.rs`、`trace.rs` 与测试已改为直接依赖 `common / route_filters / routes / cache_*` 等真实子模块
- [x] 2026-04-06：M3 第七轮已新增架构断言，要求 `handlers/admin/observability/monitoring/mod.rs` 不再保留 `use self::{activity,cache,resilience,trace}` 这类 glue imports，也不再重导出 `routes`
- [x] 2026-04-06：M3 第八轮已新增 `handlers/admin/observability/monitoring/cache_config.rs`，并将 cache 子域的 redis 提示、TTL、动态预留参数和分类定义从 `monitoring/mod.rs` 下沉到 cache 专属配置面
- [x] 2026-04-06：M3 第八轮已更新架构断言，要求 `handlers/admin/observability/monitoring/mod.rs` 不再保留任何 `const ADMIN_MONITORING_*` cache 常量，只保留入口装配与子模块注册
- [x] 2026-04-06：M3 第九轮已删除 `handlers/admin/observability/monitoring/common.rs`，并将其原有职责拆分到 `responses.rs`、`cache_types.rs`、`usage_helpers.rs`，同时把 user-behavior path 解析与 resilience snapshot ownership 下沉回各自子域
- [x] 2026-04-06：M3 第九轮已更新架构断言，要求 `handlers/admin/observability/monitoring/mod.rs` 不再登记 `mod common;`，且 `monitoring/common.rs` 文件必须保持删除状态
- [x] 2026-04-06：M3 第十轮已新增 `handlers/admin/observability/monitoring/cache_mutations.rs`，并将 `cache users/affinity/provider/model-mapping/redis-keys` 删除与清理入口整体从 `cache.rs` 外迁；`routes.rs` 现已按读写边界分别依赖 `cache` 与 `cache_mutations`
- [x] 2026-04-06：M3 第十轮已新增架构断言，要求 `monitoring/cache.rs` 不再持有 delete/mutation entrypoint，所有 cache 清理入口固定归属 `monitoring/cache_mutations.rs`
- [x] 2026-04-06：M3 第十一轮通过并行子代理完成 `monitoring` 读侧继续拆分：新增 `cache_affinity_reads.rs` 承接 affinity list/detail，新增 `cache_model_mapping.rs` 承接 model-mapping / redis-category 读接口；`routes.rs` 现已按 `cache` / `cache_affinity_reads` / `cache_model_mapping` / `cache_mutations` 四层接线
- [x] 2026-04-06：M3 第十一轮已收紧 `handlers/admin/system/mod.rs` 与 `handlers/admin/endpoint/mod.rs`，移除 `system` 对 `auth/model/provider` 的跨域 re-export，以及 `endpoint` 对 `extractors/health_builders/payloads` 的本地 helper 中转，根模块回到纯装配职责
- [x] 2026-04-06：M3 第十一轮并行收口了 `provider` 与 `model` 子域边界：`provider/crud/routes.rs` 已改为直连 `delete_task/pool/summary/write`，`provider/mod.rs` 只保留必要对外 seam；`model` 已新增 `payloads.rs` 承接 helper/payload 逻辑，`model/mod.rs` 收缩为模块注册与接口 re-export
- [x] 2026-04-06：M3 第十二轮已收紧 `handlers/admin/observability/usage/mod.rs`，删除其对 `analytics/helpers` 的大规模 helper re-export；`analytics_routes.rs`、`summary_routes.rs`、`detail_routes.rs` 与 `replay.rs` 现已直接依赖 `analytics` / `helpers` / `replay` 的真实边界
- [x] 2026-04-06：M3 第十二轮已新增架构断言，要求 `handlers/admin/observability/usage/mod.rs` 保持纯装配层，不再回退为 helper 中转站
- [x] 2026-04-06：M3 第十三轮已将 `handlers/admin/observability/stats/helpers.rs` 实体化为 stats 类型与公共算法的 owner，并将 `analytics_routes.rs`、`cost_routes.rs`、`leaderboard_routes.rs`、`provider_quota_routes.rs` 改为直接依赖 `helpers/range/responses/timeseries/leaderboard` 的真实边界
- [x] 2026-04-06：M3 第十三轮已将 `handlers/admin/observability/stats/mod.rs` 收缩为纯装配层加极小对外 seam，并新增架构断言防止其回退为厚 glue 模块
- [x] 2026-04-06：M3 第十三轮已新增架构断言 `admin_stats_root_stays_thin`，要求 `handlers/admin/observability/stats/mod.rs` 只登记子模块、不要再 `use self::` 导入 helpers/range/responses/timeseries，以及确保 `analytics_routes.rs` / `cost_routes.rs` / `leaderboard_routes.rs` / `provider_quota_routes.rs` 直接依赖对应 helpers，而非从根模块一股脑导出
- [x] 2026-04-07：M3 第十四轮已将 `handlers/admin/provider/mod.rs` 从内部 helper export hub 收缩为 route seam，并显式开放 `oauth/ops/pool/write` 子域；`maintenance/runtime*`、`handlers/internal/*`、`handlers/admin/system/pool/*`、`handlers/admin/endpoint/keys.rs` 与 `provider/strategy/builders.rs` 已改为直连 `provider::{oauth,ops,pool,write}`。新增 `admin_provider_root_stays_thin` 架构断言，禁止根模块继续转运 quota refresh、pool runtime、write builder 与 internal control error helper
- [x] 2026-04-07：M3 第十五轮已将 `handlers/admin/endpoint/keys.rs` 的 provider key 管理逻辑整体迁入 `handlers/admin/provider/endpoint_keys.rs`，旧入口现退化为 thin wrapper；`endpoint` 子域不再实际拥有 key reveal/export/create/update/refresh/batch 逻辑，只保留路由装配。架构护栏同步要求 `endpoint/keys.rs` 只委托 `provider::endpoint_keys`
- [x] 2026-04-07：M3 第十六轮已将 `handlers/admin/system/pool/**` 五个实现文件整体迁入 `handlers/admin/provider/pool_admin/**`，旧 `system/pool/mod.rs` 现退化为 thin wrapper；`system` 子域不再实际拥有 pool overview/list/resolve-selection/batch-import/batch-action/cleanup 逻辑，只保留入口装配。架构护栏同步要求 `system/pool/mod.rs` 只委托 `provider::pool_admin`
- [x] 2026-04-07：M3 第十七轮已将 `handlers/admin/endpoint` 下的 provider-owned endpoint CRUD 整簇迁入 `handlers/admin/provider/endpoints_admin/**`；旧 `endpoint/routes.rs` 现退化为 thin wrapper，`endpoint` 子域只继续拥有 `health/rpm/keys` 入口。架构护栏同步要求 `endpoint/routes.rs` 只委托 `provider::endpoints_admin`
- [x] 2026-04-07：M3 第二十七轮已将 `handlers/admin/provider/ops` 的 admin action helper 所有权固定在 `handlers/admin/provider/ops/providers/*`，`handlers/admin/provider/ops/mod.rs` 不再重新导出 `admin_provider_ops_local_action_response`；同时已收紧 `handlers/admin/provider/oauth/mod.rs`，禁止 alias re-export `self::quota`, `self::refresh`, `self::state`，oauth 子树内部改为直连真实子模块
- [x] 2026-04-07：M3 第十八轮已将 `handlers/admin/system/core/oauth_routes.rs` 的 auth-owned oauth config 路由整体迁入 `handlers/admin/auth/oauth_routes.rs`，并将 `AdminOAuthProviderUpsertRequest` 与 oauth path parser 一并下沉到 `handlers/admin/auth/oauth_config.rs`；旧 `system/core/oauth_routes.rs` 现退化为 thin wrapper，`system/shared/payloads.rs` 已删除。架构护栏同步要求 oauth config route / DTO / path helper 均归 `auth` 子域拥有
- [x] 2026-04-07：M3 第十九轮已将 `handlers/admin/system/core/model_routes.rs` 的 model-owned catalog/external-cache 路由整体迁入 `handlers/admin/model/catalog_routes.rs`；旧 `system/core/model_routes.rs` 现退化为 thin wrapper，`system/core` 不再保留 model catalog data-unavailable helper。架构护栏同步要求该 route seam 归 `model` 子域拥有
- [x] 2026-04-07：M3 第二十轮已将 `handlers/admin/system/core/{management_tokens_routes,modules_routes}.rs` 的 system-owned route owner 迁入 `handlers/admin/system/{management_tokens,modules}.rs`；旧 `system/core/*` 入口现退化为 thin wrapper，`handlers/admin/system/mod.rs` 直接暴露 `management_tokens/modules` seam，不再把这两条 route owner 藏在 `core` 内部
- [x] 2026-04-07：M3 第二十一轮已将 admin module DTO / path parser / runtime / validation / status payload owner 从 `handlers/public/system_modules_helpers/modules.rs` 下沉到 `handlers/admin/system/shared/modules.rs`；`handlers/public/system_modules_helpers.rs` 与 `handlers/public/mod.rs` 不再 re-export 这批 admin helper，public 侧仅保留 public auth module 状态能力。架构护栏同步要求 admin module helper 不得回流到 `handlers/public/system_modules_helpers*`
- [x] 2026-04-07：M3 第二十二轮已将 `handlers/admin/system/core/system_routes.rs` 对 system-owned settings / configs / export helper 的依赖切到 `handlers/admin/system/shared/{settings,configs}.rs`；`handlers/public/system_modules_helpers.rs` 与 `handlers/public/mod.rs` 不再对外暴露 `current_aether_version`、system settings/config CRUD、config export/users export 或 `serialize_admin_system_users_export_wallet`
- [x] 2026-04-07：M3 第二十三轮已将 `system_config_*`、`module_available_from_env` 与 admin email template 底层读取/渲染原语上提到 `handlers/shared/{system_config_values,email_templatd'w'qes}.rs`，并将 admin 邮件模板 route helper 明确下沉到 `handlers/admin/system/shared/email_templates.rs`；`handlers/public/system_modules_helpers/system.rs` 现退化为 shared helper re-export，不再持有 ownership。架构护栏同步要求 shared owner 固定在 `handlers/shared/*`，且 `handlers/admin/system/shared/system.rs` 被 `email_templates.rs` 取代
- [x] 2026-04-07：M3 第二十四轮已将 `build_proxy_error_response` 从 `handlers/admin/auth/oauth_config.rs` 上提到 `handlers/admin/shared/proxy_errors.rs`；`system/core/system_routes.rs`、`system/adaptive/routes.rs`、`features/video_tasks/routes.rs`、`provider/strategy/shared.rs` 与 `auth/oauth_routes.rs` 均改为直接依赖 `handlers/admin/shared`。架构护栏同步禁止该错误 builder 回流到 `auth` 或被 `provider/system` 根入口继续借用
- [x] 2026-04-07：M3 第二十五轮已将 provider-model 的 payload/write builder owner 从 `handlers/admin/model/{payloads,write}.rs` 下沉回 `handlers/admin/provider/models/{payloads,write}.rs`；`provider/models/*` 路由文件现全部直连本地 owner，`handlers/admin/model/mod.rs` 不再导出 `build_admin_provider_model_*`、`admin_provider_model_name_exists`、`build_admin_import_provider_models_payload`、`build_admin_batch_assign_global_models_payload` 等 provider-model helper。架构护栏同步要求 `provider/models` 根模块登记本地 `payloads/write`，并禁止 route files 再从 `admin/model` 借 builder
- [x] 2026-04-07：M3 第二十六轮已删除 `handlers/admin/system/core/{management_tokens_routes,model_routes,modules_routes,oauth_routes}.rs` 与 `handlers/admin/endpoint/{keys,routes}.rs` 这批失去价值的 thin wrapper；`system/core/mod.rs` 与 `endpoint/mod.rs` 现直接 dispatch 到 `system/auth/model/provider` 的真实 owner。同步删除 `handlers/admin/system/pool/mod.rs`，`handlers/admin/system/mod.rs` 直接暴露 `provider::pool_admin` seam。架构护栏升级为要求这些 wrapper 文件不存在，并要求根模块直接依赖真实 owner
- [x] 2026-04-07：M3 第二十八轮已将 `normalize_provider_billing_type` 与 `parse_optional_rfc3339_unix_secs` 的 owner 从 `handlers/admin/provider/write/mod.rs` 上提到 `handlers/admin/provider/shared/support.rs`；`handlers/admin/provider/strategy/builders.rs` 与 `handlers/admin/provider/write/provider.rs` 现统一直连 `provider::shared`，不再让 `strategy` 借用 `write` 子域的 billing/time normalizer
- [x] 2026-04-07：M3 第二十九轮已将 `handlers/admin/system/shared/mod.rs` 从兼容 re-export hub 收缩为纯子模块边界；`system/auth` 子树当前已显式依赖 `shared::{configs,email_templates,modules,paths,settings}` 的真实 owner，根模块不再继续转运 `configs::*`、`email_templates::*`、`modules::*`、`paths::*` 与 `settings::*`
- [x] 2026-04-07：M3 第三十轮已清空 `handlers/admin/provider/shared/mod.rs` 的兼容 wildcard re-export；`provider` 子树当前已显式依赖 `shared::{paths,payloads,support}` 的真实 owner，根模块不再继续转运 `paths::*`、`payloads::*` 与 `support::*`
- [x] 2026-04-07：M3 第三十一轮已将 `handlers/admin/provider/query/shared.rs` 拆为 `query/payload.rs` 与 `query/response.rs` 两个局部 owner；`query/models.rs` 与 `query/routes.rs` 当前分别直连所需的 payload extractor 与 response helper，不再依赖 generic `query::shared`
- [x] 2026-04-07：M3 第三十二轮已将 `handlers/admin/provider/strategy/shared.rs` 收缩并重命名为 `strategy/responses.rs`；`routes.rs` 只从 `responses.rs` 获取 data-unavailable 响应，`builders.rs` 的 provider-not-found 响应已本地化，`strategy` 子域不再保留 generic `shared` 入口
- [x] 2026-04-07：M3 第三十三轮已将 `handlers/admin/provider/crud/shared.rs` 收缩并重命名为 `crud/responses.rs`；`crud/routes.rs` 当前直接依赖局部 response owner，`crud/mod.rs` 不再保留 `use shared::*` 形式的局部 facade
- [x] 2026-04-07：M3 第三十四轮已将 `handlers/admin/provider/pool.rs` 收束为显式 `pool::{config,runtime}` 边界；`pool/config.rs` 持有 pool config 解析，`pool/runtime.rs` 持有 Redis runtime 读取与 pool status/mutation helper，`pool_admin/{read_routes,batch_routes}.rs` 与 `crud/routes.rs` 已改为直连这些真实 owner
- [x] 2026-04-07：M3 第三十五轮已将 `handlers/admin/provider/write/mod.rs` 从 helper hub 收束为显式 `write::{keys,normalize,provider,reveal}` 边界；`endpoint_keys.rs` 与 `crud/routes.rs` 当前分别直连 `write::keys/reveal` 和 `write::provider`，`keys.rs` / `provider.rs` 也已停止通过 `write/mod.rs` 中转 shared normalize helper
- [x] 2026-04-07：M3 第三十六轮已将 `handlers/admin/provider/oauth/dispatch/mod.rs` 的 audit attachment helper 下沉到 `dispatch/helpers.rs`；`dispatch/mod.rs` 当前只负责 route-kind 分发和薄调度，不再同时持有本地共享 helper 实现
- [x] 2026-04-07：M3 第三十七轮已将 `handlers/admin/provider/ops/providers/mod.rs` 从 DTO/常量/helper hub 收束为薄装配层；新增 `ops/providers/support.rs` 持有 request DTO 与 runtime-only message/敏感字段常量，`routes.rs` 改为显式直连 `actions/config/support/verify`，`maintenance/runtime*` 也已改成从 `providers::actions` 直接调用 `admin_provider_ops_local_action_response`
- [x] 2026-04-07：M3 第三十八轮已清理 `handlers/admin/provider/oauth/mod.rs` 根 re-export，只保留 dispatch entry seam；`handlers/internal/{mod,gateway_helpers,gateway}.rs` 现直接通过 `oauth::refresh::build_internal_control_error_response`，`handlers/admin/provider/endpoint_keys.rs` 直接从 `oauth::quota::*` 读取 quota helper，架构断言禁止该仓库重新转运 quota/refresh helper
- [x] 2026-04-07：M3 第三十九轮已将 `handlers/admin/provider/pool_admin/mod.rs` 从本地 parser/const hub 收束为薄装配层；新增 `pool_admin/support.rs` 持有常量、query/path parser、route matcher、error response 与 `AdminPoolResolveSelectionRequest`，`read_routes.rs` / `batch_routes.rs` 已改为显式直连 `support`
- [x] 2026-04-07：M3 第四十轮已将 `handlers/admin/provider/oauth/quota/mod.rs` 从 quota helper re-export hub 收束为显式边界；`endpoint_keys.rs`、`oauth/refresh.rs` 与 `dispatch/{complete,refresh}.rs` 已改为直连 `quota::{antigravity,codex,kiro,shared}` 的真实 owner，`antigravity/codex/kiro.rs` 也已显式从 `shared.rs` 引入公共 quota helper
- [x] 2026-04-07：M3 第四十一轮已将 `handlers/admin/provider/endpoints_admin/read_routes.rs` 拆为 `list.rs`、`detail.rs` 与 `defaults.rs` 三个真实 route owner；`endpoints_admin/mod.rs` 当前直接 dispatch 这三条读路径，不再保留 generic read bus
- [x] 2026-04-07：M3 第四十二轮已将 `handlers/admin/provider/endpoints_admin/write_routes.rs` 拆为 `create.rs`、`update.rs` 与 `delete.rs` 三个真实 route owner；`endpoints_admin/mod.rs` 当前直接 dispatch 六个显式 route owner，并新增架构断言禁止 `read_routes/write_routes` bus 回流
- [x] 2026-04-07：M3 第四十三轮已将 `handlers/admin/provider/endpoint_keys.rs` 从 keys/quota CRUD 总线收束为 `endpoint_keys/{reads,mutations,quota}.rs` 三个真实 owner；根模块当前只做显式 dispatch，不再直接持有 grouped-by-format、reveal/export、key CRUD、batch delete、clear-oauth-invalid 与 refresh-quota 路由实现
- [x] 2026-04-06：M4 第一轮已完成 `ai_pipeline/planner/gateway_facade.rs` 落地，统一承接 auth snapshot、provider transport、scheduler candidate 选择、request-candidate 持久化与 unused-candidate 标记
- [x] 2026-04-06：M4 第一轮通过并行子代理完成 `planner/{candidate_affinity,standard/family/**,passthrough/provider/family/**,specialized/{files,video},standard/openai/{chat,cli}/**}` 对 `gateway_facade` 的首批切换
- [x] 2026-04-06：M4 第二轮已补齐 `planner/{decision/control_plan,standard/{normalize,matrix},passthrough/provider/request}` 的 facade 类型边界；最终验证通过 `cargo check -p aether-gateway` 与 `cargo test -p aether-gateway --lib`，结果 `1228 passed; 0 failed`
- [x] 2026-04-06：M4 第三轮已新增 `ai_pipeline/{control_facade,execution_facade}.rs`，并通过并行子代理把 `contracts/control_payloads.rs`、`planner/**`、`finalize/**` 中对 `control` / `headers` / `execution_runtime` 的直接依赖收束到 facade；同时新增架构断言，禁止这些子树回退为直连 gateway 内核
- [x] 2026-04-06：M4 第四轮已新增 `ai_pipeline/provider_transport_facade.rs`，并通过并行子代理与主线收口把 `conversion/registry.rs`、`runtime/**`、`planner/**` 中对 `provider_transport` 的直接依赖压到 facade；同时新增架构断言，禁止 `ai_pipeline/{planner,runtime,conversion}` 回退为直连 `provider_transport`
- [x] 2026-04-06：M4 第五轮已拆除 `planner/gateway_facade.rs`，改为 `auth_snapshot_facade.rs`、`transport_facade.rs`、`scheduler_facade.rs`、`candidate_runtime_facade.rs`、`executor_facade.rs` 五个细粒度边界；并新增架构断言，禁止旧 seam 回归
- [x] 2026-04-06：M5 第一轮已建立 `crates/aether-ai-pipeline`，首批迁出 `contracts/{actions,plan_kinds,report_kinds}` 与 `planner/route.rs` 的纯所有权；gateway 侧 `ai_pipeline/contracts/mod.rs` 与 `planner/route.rs` 现已退化为 thin re-export / thin adapter，并新增架构断言防止旧 owner 回流
- [x] 2026-04-06：M5 第二轮已将 `ai_pipeline/conversion/error.rs` 的 `LocalCoreSyncErrorKind`、`build_core_error_body_for_client_format` 等 pure helpers 迁入 `crates/aether-ai-pipeline/src/conversion/error.rs`；gateway `ai_pipeline/conversion/mod.rs` 已退化为 thin re-export，并新增架构断言防止旧 owner 回流
- [x] 2026-04-06：M5 第三轮已将 `planner/common.rs` 的纯请求体解析逻辑迁入 `crates/aether-ai-pipeline/src/planner/common.rs`；gateway `ai_pipeline/planner/common.rs` 仅保留 `is_json_request` 判定与 thin adapter，并新增架构断言防止 parser 实现回流
- [x] 2026-04-06：M5 第四轮已将 `ai_pipeline/conversion/request/**` 的纯请求转换逻辑迁入 `crates/aether-ai-pipeline/src/conversion/request`；gateway `ai_pipeline/conversion/request/mod.rs` 退化为 thin re-export 并新增架构断言防止旧模块复活
- [x] 2026-04-06：M5 第五轮已将 `ai_pipeline/conversion/response/**` 的纯响应转换逻辑迁入 `crates/aether-ai-pipeline/src/conversion/response`；gateway `ai_pipeline/conversion/response/mod.rs` 仅保留 pipeline re-export 并新增架构断言防止旧模块复活
- [x] 2026-04-06：M5 第六轮已将 `planner/standard/openai/chat/mod.rs` 中的纯 `stop/max_tokens/reasoning/number-field` helpers 迁入 `crates/aether-ai-pipeline/src/planner/openai.rs`；gateway `planner/standard/openai/mod.rs` 已退化为 thin re-export，并新增架构断言防止 helper 实现回流
- [x] 2026-04-06：M5 第七轮已将 `planner/standard/matrix.rs` 中的请求体/normalize 逻辑拆分：canonical conversion 与 openai-chat payload builder owner 移入 `crates/aether-ai-pipeline/src/planner/standard/matrix.rs`，并通过 `crates/aether-ai-pipeline/src/planner/matrix.rs` 暴露 facade；gateway 继续处理 body-rule 与 upstream URL，并新增架构断言防止 gateway 再占用 conversion helper
- [x] 2026-04-06：M5 第七轮已将 `planner/standard/normalize.rs` 中的纯 request-body 构建逻辑拆到 `crates/aether-ai-pipeline/src/planner/standard/normalize.rs`，gateway `planner/standard/normalize.rs` 只保留对新 helper 的调用并在本地调用 `apply_local_body_rules`
- [x] 2026-04-06：M5 第八轮已将 `planner/standard/family.rs` 的纯 `LocalStandardSourceFamily/Mode/Spec` owner 与 `planner/standard/{claude,gemini}/{chat,cli}.rs` 的 pure spec resolver 迁入 `crates/aether-ai-pipeline`；gateway 已删除对应 `{chat,cli}.rs` 文件并让 `{claude,gemini}/mod.rs` 直接委托 pipeline crate，同时新增架构断言防止 spec 构造逻辑回流
- [x] 2026-04-06：M5 第九轮已将 `planner/passthrough/provider.rs` 的纯 `LocalSameFormatProviderFamily/Spec` owner 与 `resolve_{sync,stream}_spec` 迁入 `crates/aether-ai-pipeline/src/planner/passthrough/provider.rs`；gateway `planner/passthrough/provider/{family/types,plans}.rs` 已退化为 thin re-export，并新增架构断言防止 same-format spec 构造逻辑回流
- [x] 2026-04-06：M5 第十轮已将 `planner/specialized/{files,video}.rs` 的 pure spec owner 与 `resolve_*_spec` 迁入 `crates/aether-ai-pipeline/src/planner/specialized`；gateway 对应文件只保留 runtime/input/candidate/materialization/payload 逻辑，并新增架构断言防止 specialized spec 构造逻辑回流
- [x] 2026-04-06：M5 第十一轮已将 `planner/standard/openai/cli` 的纯 `LocalOpenAiCliSpec` owner 与 `resolve_{sync,stream}_spec` 迁入 `crates/aether-ai-pipeline/src/planner/standard/openai_cli.rs`；gateway `standard/openai/cli/{decision,plans}.rs` 已退化为 type re-export 与 thin delegation，并新增架构断言防止 openai-cli spec 构造逻辑回流
- [x] 2026-04-06：M5 第十二轮已为 `finalize/standard/**/sync` 补齐架构护栏，要求 `openai/claude/gemini` 的历史 response converter owner 不再滞留在细粒度 sync 文件中；后续清理完成后，gateway 只保留 wrapper + aggregate，并通过 `crate::ai_pipeline::conversion::response` 在 `sync/mod.rs` 或外层 `mod.rs` 暴露这些符号
- [x] 2026-04-06：M5 第十三轮已将 `finalize` 的 SSE、canonical stream 类型，以及 `provider/client` emitters 迁入 `crates/aether-ai-pipeline/src/finalize/**`；gateway 现只保留 envelope 编排与 `GatewayError` 映射，同时新增 `ai_pipeline_finalize_stream_engine_is_owned_by_pipeline_crate` 护栏，防止 openai/claude/gemini stream 文件继续宣称 provider/client struct owner
- [x] 2026-04-06：M5 第十四轮已将 `finalize/internal/stream_rewrite.rs` 的 rewrite-mode 判定矩阵迁入 `crates/aether-ai-pipeline/src/finalize/stream_rewrite.rs`；gateway 现仅根据 pipeline 返回的 mode materialize `Standard` / `EnvelopeUnwrap` / `KiroToClaudeCli` state，并新增 `ai_pipeline_finalize_stream_rewrite_matrix_is_owned_by_pipeline_crate` 护栏防止格式矩阵与 envelope 判定回流
- [x] 2026-04-06：M5 第十五轮已将 `finalize` 的 standard cross-format sync aggregate / convert / product builder 迁入 `crates/aether-ai-pipeline/src/finalize/sync_products.rs`；gateway `finalize/standard/mod.rs` 已退化为 thin re-export，`internal/sync_finalize.rs` 只保留 envelope unwrap 与 response/report render，并新增 `ai_pipeline_finalize_standard_sync_products_are_owned_by_pipeline_crate` 护栏防止 pure orchestration 回流
- [x] 2026-04-06：M5 第十六轮已将 `openai/claude/gemini` 的 same-format stream aggregators 与 `parse_stream_json_events` owner 并入 `crates/aether-ai-pipeline/src/finalize/sync_products.rs`；gateway `standard/{openai,claude,gemini}/sync/*.rs` 仅保留 same-format success render，`finalize/common.rs` 不再承载 stream parsing helper，并继续沿用 `ai_pipeline_finalize_standard_sync_products_are_owned_by_pipeline_crate` 护栏锁定所有权
- [x] 2026-04-06：M5 第十七轮已将 `finalize/internal/sync_finalize.rs` 里 `standard cross-format` 的 `chat/cli × stream/sync` 四路 normalized payload 判定与 product 生成统一迁入 `crates/aether-ai-pipeline/src/finalize/sync_products.rs::maybe_build_standard_cross_format_sync_product_from_normalized_payload`；gateway 现只保留最终 response/report render，并新增护栏禁止四个旧 helper 回流
- [x] 2026-04-07：M5 第十八轮已将 `same-format / same-family` 的 normalized payload raw-body 选择逻辑继续迁入 `crates/aether-ai-pipeline/src/finalize/sync_products.rs`，新增 `maybe_build_standard_same_format_sync_body_from_normalized_payload` 与 `maybe_build_openai_cli_same_family_sync_body_from_normalized_payload`；gateway `standard/{openai,claude,gemini}/sync/*.rs` 现只保留 envelope unwrap 与 success render，并新增护栏禁止 `openai_cli` 的三段本地 helper 回流
- [x] 2026-04-07：M5 第十九轮已删除 `standard/openai/sync/cli.rs` 中面向 `antigravity:v1internal` 的冗余 cross-format 特判；由于 `finalize/internal/sync_finalize.rs` 入口已先执行 `maybe_normalize_provider_private_sync_report_payload`，generic cross-format 路径即可接管。护栏同步禁止 `maybe_build_local_openai_cli_antigravity_cross_format_*`、`unwrap_cli_conversion_response_value` 与 `is_antigravity_v1internal_envelope` 回流
- [x] 2026-04-07：M5 第二十轮已在 `crates/aether-ai-pipeline/src/finalize/sync_products.rs` 新增 `maybe_build_openai_chat_cross_format_sync_product_from_normalized_payload` 与 `maybe_build_openai_cli_cross_format_sync_product_from_normalized_payload`，将 `openai chat/cli` cross-format wrapper 里 `provider_api_format -> aggregate -> convert` 的 product selection 纯逻辑整体迁入 pipeline crate；gateway `standard/openai/sync/{chat,cli}.rs` 现只保留 `local_finalize_allows_envelope`、provider-body unwrap 与 response/report render，并通过扩展现有 `ai_pipeline_finalize_standard_sync_products_are_owned_by_pipeline_crate` 护栏禁止 `sync_*_response_conversion_kind`、本地 `match provider_api_format.as_str()` 与 `convert_*_to_openai_*` 分支选择回流
- [x] 2026-04-07：M5 第二十一轮已将 `finalize/internal/sync_finalize.rs` 里标准 sync finalize 的 ordered dispatch 矩阵迁入 `crates/aether-ai-pipeline/src/finalize/sync_products.rs::maybe_build_standard_sync_finalize_product_from_normalized_payload`，并新增 `StandardSyncFinalizeNormalizedProduct` 枚举统一 same-format/same-family success body 与 cross-format conversion product；gateway 入口现收敛为 `normalize -> delegate -> unwrap/render` 三段式，护栏同步禁止 `sync_finalize.rs` 继续直接串联 `maybe_build_local_*` wrapper 链
- [x] 2026-04-07：M5 第二十二轮已删除 `apps/aether-gateway/src/ai_pipeline/finalize/standard/{openai,claude,gemini}/sync/{chat,cli}.rs` 六个失去运行时价值的 thin wrapper 文件；对应 `sync/mod.rs` 与上层 `mod.rs` 已移除 `maybe_build_local_*` re-export，只保留 aggregator / converter seam。架构护栏同步改为要求这些文件不再存在，并禁止各层 `mod.rs` 保留死 wrapper 导出
- [x] 2026-04-07：M5 第二十三轮已进一步删除 `apps/aether-gateway/src/ai_pipeline/finalize/standard/{openai,claude,gemini}/sync/mod.rs` 三个空壳模块，并从 `standard/{openai,claude,gemini}/mod.rs` 与根 `standard/mod.rs` 移除对应 `pub(super) mod sync` / wildcard re-export；`internal/sync_finalize.rs` 改为直接从 `aether-ai-pipeline::finalize::sync_products` 暴露 stream aggregators。护栏同步要求这些 `sync/mod.rs` 不再存在，并保留 `conversion::response` 侧最小导出 seam
- [x] 2026-04-07：M5 第二十四轮已将 `finalize/standard/stream_core` 的 provider/client parser-emitter matrix 与 `StreamingStandardFormatMatrix` 迁入 `crates/aether-ai-pipeline/src/finalize/standard/stream_core/format_matrix.rs`；gateway `stream_core/mod.rs` 现只保留 thin re-export，`orchestrator.rs` 只负责 envelope unwrap 与 matrix delegation。架构护栏同步禁止 gateway 继续本地持有 `ProviderStreamParser` / `ClientStreamEmitter` / format string matrix
