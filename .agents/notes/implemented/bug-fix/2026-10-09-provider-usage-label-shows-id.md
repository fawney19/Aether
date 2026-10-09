# Agent Note: 成本分析“提供商用量”展示提供商名称

Status: implemented

## Problem

成本分析页面（`frontend/src/views/admin/CostAnalysis.vue` → `data-provider-usage`）的“提供商”列直接渲染接口返回的 `label`。而后端 `UsageAnalyticsView::Breakdown` + `group_by=provider` 的分组键是 `provider_id`，同一段 SQL 又把 `label` 写成 `group_id::text`，于是页面显示 `provider-1` 这类内部 ID，管理员无法判断是哪个提供商；同一份数据的 CSV 导出 `label` 列也只有 ID。

不修的话，这个缺陷不会自愈：`id` 必须继续是 `provider_id`（“查看”链接按它跳转用量明细、已登记支出按它匹配），所以只能修 `label` 的取值，而不是换分组键。故障面覆盖两套实现：PostgreSQL 适配器与内存仓储（无数据库部署与单元测试走后者）。

## Decision

`Breakdown` 提供商分组下，行的 `id` 保持 `provider_id` 不变，`label` 解析为提供商名称，解析顺序：

1. 提供商目录 `providers.name`（当前名称，随改名刷新）；
2. 使用记录里的 `provider_name` 快照（历史行、目录已删的提供商）；
3. 原始 `provider_id`（最后的兜底，避免出现空列）。

配套边界，两套实现一致：

- 空白字符串与历史占位值 `unknown` / `unknow` / `pending` 不当作名称展示；
- `provider_id` 为空表示这条记录无法归属，`label` 保持 `null`，由前端沿用既有的“未归属提供商”文案；
- 只有明细分组（`Breakdown`）这么做；时间序列 / Performance / DashboardCharts 仍用 `provider_id` 作标签，`provider_rows` 与 `provider_timeline_rows` 的既有口径不变；
- 内存仓储没有提供商目录，只走第 2、3 步，分组键与标签规则与 PostgreSQL 侧保持一致。

PostgreSQL 侧的落地方式：`grouped` CTE 用 `max(provider_name) AS provider_name` 带出名称快照，最外层 `FROM page LEFT JOIN public.providers AS provider_catalog ON provider_catalog.id = page.group_id`，标签表达式固定在 `ANALYTICS_PROVIDER_LABEL_SQL`；`to_jsonb(page)` 需要额外减去 `provider_name`，否则它会被塞进 `metrics`。

前端不改：`label` 为空时已经回退到“未归属提供商”，`id` 与 `label` 的职责在页面里本来就是分开的。

## Alternatives considered

- **前端用已加载的财务账户列表（`providerFinanceApi.accounts`）把 ID 映射成名称** — 改动最小且不动后端，但只对已登记财务账户的提供商有效，未登记账户的提供商依旧显示 ID；缺陷被藏在展示层，导出的 CSV 仍然只有 ID。否。
- **只取使用记录里的 `provider_name` 快照（与 `provider_rows`、`provider_timeline_rows` 的 `max(provider_name)` 保持一致）** — 无需 JOIN，SQL 更短；但提供商在目录里改名后，历史报表仍显示旧名称，与用量审计聚合页的解析顺序不一致。目录优先、快照兜底。
- **把分组键换成 `provider_name`（照搬用量审计聚合的 `legacy_name` 回退）** — 能让无法归属的历史行按名称分桶，但会改变 `total`、分页口径和“查看”链接语义，并让按 `provider_id` 匹配的已登记支出列错位。否。

## Consequences

- **收益**：页面的“提供商”列与 CSV 的 `label` 列显示可读名称，且与用量审计聚合（`USAGE_RESOLVED_PROVIDER_DISPLAY_NAME_SQL`）保持同一套解析顺序，跨页面观感一致。
- **代价与已知上限**：明细查询多了一次 `providers` 的 LEFT JOIN（发生在 `LIMIT` 之后的 page 上，最多 `limit` 行，按主键关联）；查询结果依赖 `providers` 表可见；提供商被删除后回退到快照名，历史报表不会因目录改名而重写既有数据。
- **重访信号**：如果将来要求“同名历史提供商合并成一行”（即 `legacy_name` 归并），那是分组键变更，需要另开一篇笔记并同步改动 `total` / 分页 / 前端链接。

## Verification

- 内存实现：`cargo test -p aether-data --lib` → `repository::usage::memory::tests::overview_provider_breakdown_labels_rows_with_provider_name`（覆盖名称展示、占位值回退、无法归属三种分支）。
- SQL 片段解析顺序：`cargo test -p aether-data-postgres --lib` → `usage::tests::provider_breakdown_labels_resolve_catalog_name_before_recorded_name_and_id`。
- 展示层：`frontend/src/features/overview/__tests__/costs.spec.ts` 已断言“提供商用量”区块渲染 `Provider One`。
- 取数端到端（真实 SQL 执行）需要 `AETHER_TEST_DATABASE_URL` 的 live 测试环境；本环境无可用 PostgreSQL，未执行 `crates/aether-data/adapters/postgres/src/usage/analytics_tests.rs` 中的 live 用例。
