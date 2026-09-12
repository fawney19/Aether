# Aether 系统、测试与 CI 瘦身审计

- 审计日期：2026-09-08（Asia/Shanghai）
- 源码基线：`8b766930b`
范围：仓库结构、依赖图、本地构建产物、GitHub Actions 实际日志、测试与发布边界。

本次只新增审计报告，没有修改业务代码、测试或工作流，没有清理文件，也没有访问生产服务器。没有采集生产 RSS、CPU、数据库体积或请求延迟，因此下文不能作为生产内存泄漏或吞吐退化的结论。

## 一、结论

优先减掉的是**重复构建、过重的测试夹具、没有实际用例的构建任务和失效的模块边界**，不是先删功能或减少回归断言。

- 普通 Rust CI 最近 30 条记录中，20 次成功运行的总耗时中位数约 **16 分 33 秒**，范围 **11 分 42 秒～19 分 03 秒**。样本覆盖 2026-09-05～2026-09-08，包含 push 和 PR；没有将失败、取消或未完成运行计入。
- 成功样本中 Gateway 是关键路径：一次耗时 **11 分 39 秒**，另一次 **16 分 46 秒**；既有巨型测试目标编译，也有数分钟测试执行，不能只归咎于缓存或测试数量。
- “Workspace Rest” 虽然排除了 Gateway 测试目标，仍经由 Tunnel 的开发依赖编译完整 Gateway。分 job 没有实现真正的依赖隔离。
- Nightly 文档测试花了 **252 秒**，41 个库实际执行 **0 个 doctest**；发布默认编译 4 个二进制，只上传其中 1 个。
- 本地 `target/` 约 **168GB**，其中 `debug/incremental/` 约 **116GB**。这是构建缓存，不是生产镜像或业务源码体积。

所有改进收益都需要前后对照验证。本文不会把并行 job 的节省简单相加为流水线总时长，也不会承诺尚未测量的提速比例。

## 二、实测基线

### 2.1 CI 运行记录

数据来自 `fawney19/Aether` 的 GitHub Actions API、各 job 的步骤时间戳及原始日志。

| 运行 ID | 类型、源码 | 观察结果 |
| --- | --- | --- |
| `34174603131` | Rust CI，`7113d04f`，成功 | 总耗时 11:55；17 个 job 累计执行 39:19 |
| `34153166516` | Rust CI，`099b810a`，成功 | 总耗时 17:02；17 个 job 累计执行 46:10 |
| `34132961583` | 正式发布，`v0.7.17`，成功 | 总耗时 33:25；最慢为 macOS Intel 构建 |
| `34163371099` | Nightly，`099b810a`，失败 | 总耗时 47:42；检查和编译成功，GHCR 发布 job 在启动阶段失败 |

普通 CI 的总耗时按 `updated_at - run_started_at` 统计，包含调度和收尾；各 job 耗时按自身开始、完成时间统计，累计值不是计费分钟。详细成功样本与本地基线的 Rust CI、Nightly、Release、Cargo profile 和 Vitest 配置没有差异，业务改动会影响测试数量与耗时。

`34174603131` 中各主要测试步骤：

| 任务 | 编译/链接 | 测试执行 | 步骤或 job 耗时 |
| --- | --- | --- | --- |
| Gateway lib | 4:57 | 5,139 个测试，213.807 秒 | Test lib 步骤 514 秒 |
| Gateway bins | 2:27 | 78 个测试，0.270 秒 | Test bins 步骤 151 秒 |
| Workspace Rest | 5:36 | 3,377 个测试，26.357 秒，另有 16 个跳过 | Test 步骤 366 秒 |
| Integration Scenarios | 5:16 | 15 个 bin 内单测及 11 个 E2E 用例，约 8 秒 | Test 步骤 325 秒 |
| Data | 未进一步拆分 | 保留数据库相关保障 | Test 步骤 55 秒，job 88 秒 |

另一轮 `34153166516` 的 Gateway lib 编译 6:22、执行 390.039 秒，bins 编译 3:05、执行 0.329 秒。运行环境和缓存差异明显，不能只用最快一轮估算收益。

### 2.2 源码与磁盘

源码只统计 Git 跟踪文件，行数为物理行，包含注释和测试，不等于生产代码行数。

| 项目 | 规模 |
| --- | --- |
| Cargo workspace | 42 个 package |
| Rust 源码 | 1,874 个文件，1,114,527 行 |
| Gateway package 的 Rust 文件 | 1,176 个文件，679,538 行，约占全部 Rust 行数 61% |
| 以 tests/test/testkit 等命名识别的 Rust 测试及支持内容 | 214,086 行；未加上大量内联 `#[cfg(test)]` 模块 |
| Gateway 架构守卫测试 | 13 个文件，14,918 行，208 个 `#[test]` |
| 前端测试 | 208 个文件，31,945 行；Nightly 实跑 1,486 个用例 |
| 本地 `target/debug/incremental/` | 约 116GB |
| 本地 `target/debug/deps/` | 约 49GB |
| 本地 `frontend/node_modules/` / `frontend/dist/` | 约 311MB / 8MB |
| 本地历史 `htmlcov/` / `logs/` | 约 75MB / 121MB，均被 Git 忽略 |

## 三、优先处理：不降低保障的浪费

### P1-1：解除 Tunnel 测试对完整 Gateway 的反向依赖

**证据：** `apps/aether-tunnel/Cargo.toml:48` 将带 `testkit` 的 Gateway 列为 dev-dependency。`.github/workflows/rust-ci.yml:208` 和 `.github/workflows/rust-ci.yml:394` 虽然排除 Gateway 目标，但 Rest 的真实日志仍出现编译 `aether-gateway`。Gateway、Rest、Integration 因此在独立 runner 中重复付出重型构建成本。

**建议：** 将 Tunnel 中需要完整 Gateway 的端到端场景迁到独立集成测试目标；Tunnel 的协议、状态机、配置等单测只依赖轻量契约和测试支持。迁移之后比较测试清单，确保没有丢失端到端场景。

**验收：** 普通 Rest 单测及 Clippy 的依赖闭包不再包含 Gateway；Tunnel 跨端集成场景仍在专门任务执行。此项主要降低累计 runner 工作量，是否缩短总时长取决于 Gateway 关键路径是否也得到优化。

### P1-2：发布只编译真正发布的二进制

**证据：** Cargo metadata 显示 Gateway 有 4 个 bin：服务主程序、backup-restore、两个 WebSocket probe。`.github/workflows/release.yml:227`、`.github/workflows/release.yml:229` 和 `.github/workflows/nightly.yml:296` 未选择具体 bin，但 `.github/workflows/release.yml:236` 只上传主程序。

2026-09-07 的正式发布中，macOS Intel job 耗时 31:24，编译/链接日志耗时 28:52；Nightly 对应 job 耗时 33:45。发布慢不能全算在测试头上。

**建议：** 正式发布与 Nightly 的 `cargo build` / `cross build` 添加 `--bin aether-gateway`；probe 和其他运维程序保留独立检查、测试或按需打包入口。保留当前 release 优化策略，先测减少目标的收益，再讨论 LTO 调整。

**边界：** 没有逐 bin 链接计时，不能声称这会让整个发布缩短四分之三。

### P1-3：取消空 doctest 构建，合并重复的驱动检查

- `.github/workflows/nightly.yml:100` 的 workspace doctest 步骤耗时 252 秒，41 个库合计 0 个用例。建议对确实无 doctest、且不计划承载文档示例的库显式管理 `doctest`，或通过独立清单检查是否存在可执行文档示例后再调度。未来新增示例必须能重新纳入检查，不能永久盲目跳过全部文档测试。
- `crates/aether-data/runtime/Cargo.toml:10` 中 `default = ["postgres"]`，`all-drivers = ["postgres"]`，源码没有单独依赖 `all-drivers` 的条件分支。`.github/workflows/rust-ci.yml:334` 的两个 feature job 实际没有覆盖两个不同数据库驱动。
- `.github/workflows/rust-ci.yml:394` 的 Rest 已执行 Postgres adapter 的测试，`.github/workflows/rust-ci.yml:403` 又运行一次。若保留独立 adapter job，应从 Rest 排除它；否则直接以 Rest 承担这份覆盖。

**收益口径：** 在样本中，去掉空 doctest 可省约 4:12 的该 job 时间；feature 与 adapter 重复工作为几十秒量级。它们多数不在普通 CI 关键路径上，不应当作主 CI 总时长的等额收益。

### P1-4：区分集成测试与压测工具的构建

**证据：** `crates/aether-testing/integration/Cargo.toml:1` 所属 package 自动发现 14 个场景 bin；其中 11 个没有测试函数。`.github/workflows/rust-ci.yml:467` 对整个 package 执行 `--bins --tests`，耗时 5:25，实际测试约 8 秒。

这里并没有执行那些压测程序的 `main()` 来验证容量、恢复时间或性能；空测试 harness 的成功不能视为压测成功。

**建议：** 将 E2E 测试与 benchmark/probe 工具分开；把三个工具内的 15 个单测迁入适合的库或独立目标。工具源码继续接受 Clippy/check，真实压测由手动或计划任务运行。必要时使用 `required-features` 门控工具 bin。

**注意：** 仅给 bin 添加 `test = false` 不足以阻止所有额外构建；Cargo 构建 integration test 时还可能自动构建同 package 的普通 bin。应从目标和 package 边界解决，而不是仅换命令拼写。

### P1-5：重做按变更类型路由，同时补覆盖缺口

**证据：** `.github/workflows/rust-ci.yml:9` 将 README、安装脚本、Compose 与 Rust 源码共同触发整条 Rust 流水线；job 内没有进一步区分。当前五份工作流没有 frontend PR 工作流；前端完整检查在 Nightly 执行。

另一个现存缺口：`apps/aether-gateway/tests/admin_unsigned_identity_headers.rs:16` 的普通 integration test 不在 Gateway 的 `--lib`、`--bins` 两条 nextest 命令内；独立 Integration Scenarios job 选择的是另一个 package。Nightly 的 `check --all-targets` 只检查编译，不会替代执行此安全用例。

**建议：**

1. 安装脚本、Compose、发布工作流改动优先运行对应安全 fixtures；纯 README 文档修改不必编译完整 Gateway。
2. Rust 改动执行相关测试；Cargo、工具链、公共契约和测试基础设施变更应保守扩大到完整检查。
3. 前端改动运行自己的类型检查和测试，不必等待 Nightly。
4. 显式加入 Gateway integration test，包括上述身份头安全用例。
5. 保留稳定的最终 `check` 门禁，并验证预期跳过的 job。不要让路径过滤造成 required check 永久 pending，或把失败当成允许跳过。

工具链触发项还应补查 `rust-toolchain.toml`、`.cargo/**`；这些文件目前不在该工作流的路径列表中。

## 四、关键路径：Gateway 测试要减“重量”而非减断言

### P1-6：改造昂贵夹具与不必要的全应用初始化

成功样本中最慢的用例包括：

| 用例 | 时间 | 代码位置 |
| --- | --- | --- |
| 跳过大量 blocked account 的扫描预算 | 22.668 秒 | `apps/aether-gateway/src/dispatch/pool_scheduler.rs:3940` |
| v1 备份兼容及历史密钥尝试 | 14.324 秒 | `apps/aether-gateway/src/backup/executor.rs:1223` |
| 跳过大量 exhausted account | 8.481 秒 | `apps/aether-gateway/src/dispatch/pool_scheduler.rs:3859` |
| 大池 LRU 和动态跳过 | 8.147 秒 | `apps/aether-gateway/src/dispatch/pool_scheduler.rs:4424` |

池调度测试会构造 1,700 个账号，并在 `apps/aether-gateway/src/dispatch/pool_scheduler.rs:4868` 为每个账号调用真实凭据封装。可以将扫描预算、分页、跳过逻辑用轻量 repository/credential fixture 验证，另保留少量真实加解密联调用例和大规模边界场景；不要简单把大池规模缩小到失去原来的回归条件。

`crates/aether-crypto/src/python_fernet.rs:302` 的历史密钥派生包含进程内缓存及 100,000 次 PBKDF2。真实 nextest 用例是分进程执行的，跨用例不能指望共享这份缓存。是否构成主要开销仍需针对性计时；可为不测试历史派生逻辑的夹具选择合法的固定测试密钥或预制密文，历史兼容和真实密码学用例必须保留生产强度。

**不要做：** 为通过 CI 下调生产加密迭代数、删掉备份兼容测试、跨测试共享可变 AppState、将关键安全测试统一 ignore。

### P1-7：把巨大单一测试目标拆成真正独立的边界

`apps/aether-gateway/src/lib.rs:234` 将广泛的内部测试树纳入同一个 lib test binary。单纯把大文件拆成几个 `mod` 文件，不会使它们成为独立 Cargo 编译单元。

建议优先迁出无需访问私有业务状态的架构守卫测试，再逐步将调度策略、协议转换、计费纯函数测试迁到所属 crate；HTTP 行为与跨模块场景放到有明确支持 API 的 integration target。避免为了迁移测试而把所有内部类型公开。

架构守卫现有 208 个测试、约 1.49 万行，很多是源码字符串和依赖规则检查。例如 `apps/aether-gateway/src/tests/architecture/workspace_tiers.rs:3` 按 manifest 字符串判断依赖边界。它们应继续存在，但可进入不依赖 Gateway 的小工具/测试目标；依赖规则优先检查 Cargo metadata，行为正确性继续由行为测试负责。

若采用 nextest 分片，应先复用一次构建产物，再分发执行；直接给 N 个 runner 各自重新编译巨型 Gateway 会放大成本。先衡量编译与执行占比，再确定是否分片。

**特别注意：** 仅将 `--lib` 与 `--bins` 合为一条命令，并不消除普通库与 `cfg(test)` 测试库的两种构建。不能把样本中 2:27 的 bins 编译时间直接记作可全部省掉。

### P1-8：缓存按实际构建方式组织

所有 Rust job 使用同一个 `shared-key`。实测 Gateway、Rest、Integration 恢复了同一份约 380MB 的缓存；日志显示 `cache-workspace-crates: false`。但 Gateway 的 mold `RUSTFLAGS` 只存在于测试 step，见 `.github/workflows/rust-ci.yml:265`，其余任务又使用不同的 Clippy/check/test 和 feature 组合。

Gateway 样本的 Rust sccache 命中率达到 95.48%，仍花了数分钟构建，并存在 76 次标记为 `crate-type` 的不可缓存调用。这说明“再装一个缓存”不是根治；同时也不能据此断言缓存无效。

建议把影响缓存选择的环境提前到 job 级，按 lint/check 与 test、target、toolchain、必要 feature 区分缓存用途；相同构建尽量统一。先测恢复、保存、不可缓存调用与编译时间，不要给每个细碎目标无限新建 cache key，也不要直接缓存完整的巨型 `target/`。

现有 nextest、sccache、Gateway mold 和 CI 的关闭 debug 信息配置已经到位，不列为“尚未实施”的建议。

## 五、源码和依赖的长期瘦身

### P2-1：继续完成已有模块边界，而不是继续增加空壳 crate

Gateway 仍承载约 67.95 万行 Rust。部分现有边界很薄：Gateway execution crate 82 行、control crate 100 行、provider core 91 行、usage core 110 行，而核心实现仍留在应用中。

值得分批治理的集中点：

| 文件 | 物理行数 | 建议拆分依据 |
| --- | --- | --- |
| `apps/aether-gateway/src/execution_runtime/stream/execution.rs:1` | 15,088 | 流状态机、传输适配、计费收尾、对应测试 |
| `crates/aether-data/adapters/postgres/src/usage/mod.rs:1` | 13,833 | 写入、查询、统计聚合、审计存储 |
| `crates/aether-usage/runtime/src/runtime.rs:1` | 12,809 | 状态推进、结算策略、持久化适配、测试 |
| `apps/aether-gateway/src/handlers/admin/request/system/import.rs:1` | 9,490 | 导入校验、版本兼容、执行与回滚 |

这些数字包含内联测试，不是生产实现行数。目标应是缩小依赖闭包、变更影响面和测试目标，而非追求拆出更多文件。

`apps/aether-gateway/src/state/app.rs:376` 的 AppState 有 91 个字段，其中 28 个字段名包含 cache；`crates/aether-data/contracts/src/repository/usage/types.rs:1984` 的 UpsertUsageRecord 有 67 个字段。这反映了测试构造和模块依赖面较宽，但不能据此直接判定运行时占用过大。可以引入按职责的窄上下文和统一 fixture builder，避免每个测试复制完整对象。

### P2-2：从基础契约中剥离重型格式实现

`crates/aether-data/contracts/Cargo.toml:10` 依赖整个 `aether-ai-formats`；后者约 8.23 万行 Rust。contracts 中实际使用包括格式权限、别名与少量 usage 元数据策略，见 `crates/aether-data/contracts/src/repository/auth.rs:295`。

建议把稳定的格式标识、权限和小型元数据契约下沉到现有基础契约层，完整 request/response/stream 转换留在 formats。避免为了一个格式权限判断，让数据库契约持续依赖整个转换实现。这比任意合并 crate 更有价值。

### P2-3：依赖体积优化必须保留兼容能力

`cargo tree --offline --locked` 确认 Gateway 同时包含：

- 主请求链路的 reqwest 0.12 与 `object_store` 引入的 reqwest 0.13。
- rustls 的 ring 和 aws-lc 路径，以及 wreq 的 boring2 路径。

这些是进一步分析构建和二进制体积的候选，不是已证实可直接删除的依赖。`aws-lc-rs` 还有直接密码学用途，wreq 承担专门传输能力；移除前必须核查调用和握手、指纹、代理兼容测试。

先获取 release 的 Cargo timings 和二进制符号/section 体积，再评估版本统一或可选能力 profile；不要仅根据依赖名字或锁文件重复条目盲目替换。

## 六、前端测试与构建

### P1/P2：测试环境按需要加载

Nightly 前端测试实测 106.01 秒，208 个文件、1,486 个用例全部通过。Vitest 报告 environment 146.40 秒、import 51.29 秒、tests 72.78 秒；这些分项含并行累计时间，不能相加当作墙钟时间。

`frontend/vitest.config.ts:10` 为全部测试使用 jsdom；静态扫描有 126/208 个测试文件未直接出现常见 DOM 操作标记，但这并不证明它们的传递依赖不需要 DOM。`frontend/src/tests/vitest.setup.ts:61` 还在每个用例前加载 i18n 并重设语言。

建议通过独立 Vitest project 或显式环境标记，将已确认的纯函数/解析器/数据转换测试放到 node 环境，DOM 组件保留 jsdom；按测试类别拆分 setup，不要全局取消隔离。先迁一组、验证测试数与结果，再扩展。

### P1：同一 web 项目重复构建

`.github/workflows/release.yml:84` 和 `.github/workflows/deploy-pages.yml:58` 已单独构建 VSCodex web；随后 frontend 的 `prebuild` 又调用 `frontend/scripts/sync-vscodex.mjs:64` 无条件重建一次。

建议分开“安装/构建嵌入 web”与“复制已构建产物”，在同一个任务内只构建一次，消费明确来源的 artifact。不要仅通过 `dist` 存在就认定源码和产物一致。Nightly 前端这里没有相同的预先重复 build，不应错误地宣称所有流水线都重复。

### P2：低风险依赖清理候选

当前前端源码未发现 `three` 的模块导入，但 package 声明了 `three` 和 `@types/three`；本地二者合计约 36MB。确认无动态/外部消费者后可移除并更新锁文件，验证 type-check、测试和构建。

这主要减少安装和维护成本；不能保证生产 bundle 同样减少 36MB。现有 chart、pinyin、Stripe 均发现使用，不能一并判作无用依赖。

MarkdownViewer 使用完整 `highlight.js`，而 CodeHighlight 已按语言导入，可统一按需高亮策略；现有前端产物约 8MB，优先级低于 Rust 重复构建和测试初始化。

## 七、本地构建产物治理

当前最值得清理的是 116GB 的 `target/debug/incremental/`，其次是 49GB 的 `target/debug/deps/`。全量 `cargo clean` 虽能回收空间，也会迫使下次重建全部依赖。

建议先确认没有进行中的 Cargo/rustc 任务，对长期未使用的增量会话、历史目标产物做定期清理；稳定本地 profile、工具链和 `RUSTFLAGS`，避免频繁产生不同构建组合。保留仍在使用的依赖缓存，不要每次构建前清空 target。

`htmlcov/` 属于历史 Python 覆盖率产物，可在确认不再需要后清理，但仅几十 MB，不是主要收益。不要误删仍有效的 Python 安装/Compose 安全测试，也不要把日志、备份或数据目录当构建缓存删除。

生产 Dockerfile 已使用预构建二进制、前端产物和 distroless runtime，并通过 `.dockerignore` 排除 target、node_modules 等开发内容；不建议把“换更小基础镜像”列为当前第一优先级。生产镜像实际体积还需单独测量。

## 八、建议实施顺序与验收

| 批次 | 改造 | 验收标准 |
| --- | --- | --- |
| A：小改动去空转 | 发布指定主 bin；管理空 doctest；消除重复 adapter 与等价 feature 任务；嵌入 web 只构建一次 | 保留现有有效用例与工件；对照任务耗时和测试清单 |
| B：加快反馈并补漏 | 变更路由、稳定 gate、前端 PR 检查、Gateway integration 安全用例 | 纯文档/脚本不编译全栈；公共变更仍完整检查；安全用例实际执行 |
| C：解除编译耦合 | Tunnel 跨端测试迁移；工具与 E2E 分离；架构守卫轻量化 | Rest 依赖闭包无 Gateway；无用 bin 不参与 E2E 构建；守卫持续有效 |
| D：减少测试初始化 | 重型夹具拆层；纯前端测试切 node；窄上下文和统一 builder | 慢用例时间降低，测试数与断言目的不减少，无新增污染或 flaky |
| E：结构与依赖治理 | 真实职责迁入现有 crate；基础格式契约下沉；审慎统一依赖 | 普通修改触发的重编译面缩小，性能和兼容性基线不回退 |

每批先使用相同 SHA、runner 类型、toolchain 和 feature 集合做对照，至少区分冷缓存/热缓存与 job 执行/流水线总耗时。持续保存编译 timings、nextest 测试清单和结果、慢测试列表、缓存统计、frontend environment 时间、发布工件体积。

首要结果指标：普通 PR 更快得到正确反馈、累计重复构建下降、测试保障不退化。不要把删除测试数量、增大并发数或缩短单个非关键任务当作最终目标。

## 九、复查入口

本次没有重新运行全量构建或全量测试，使用了真实 CI 日志、离线 Cargo metadata/tree 和只读源码统计。临时 API JSON、job 日志与依赖树保存在本机 `/tmp/aether-slim-audit/`，未纳入 Git；临时目录可能被系统清理，运行 ID 可用于再次取证。

可重复的只读命令：

```sh
cargo metadata --offline --locked --no-deps --format-version 1
cargo tree --offline --locked -p aether-gateway -e normal -d
gh api 'repos/fawney19/Aether/actions/runs/34174603131/jobs?per_page=100'
gh api 'repos/fawney19/Aether/actions/jobs/101901417322/logs'
gh api 'repos/fawney19/Aether/actions/jobs/101869558414/logs'
du -h -d 1 target/debug
```

Cargo 关于默认构建目标及 integration test 自动构建 bin 的语义，另对照本机 Rust 1.95.0 随附的 Cargo `cargo-build`、`cargo-test`、`cargo-targets` 官方文档。并行测试时间、缓存命中率和源码体积均按各自定义解释，未将其混用为生产性能结论。
