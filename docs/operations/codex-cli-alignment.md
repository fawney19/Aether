# Codex CLI 通用协议对齐

本次对齐以官方 `openai/codex` 稳定标签 `rust-v0.159.3` 为可发布版本依据，同时核查最新 `main` 的协议实现。稳定标签提交为 `01fc69f4026735edfdf6789820549727a4867b11`，核查的 main 提交为 `444da310e108da16aaeb18fd790b0ac464f08aca`。

网关承接的是客户端与上游之间的协议，不复制 CLI 的本地工具执行、终端界面或个人账户状态。

| 对象 | 当前行为 |
| --- | --- |
| 客户端画像 | 默认版本更新为 0.159.3，现有后台 npm 稳定版本刷新继续生效；UA 按官方格式使用网关公开 OS、版本和架构，无终端时采用官方 `unknown` 标识 |
| OAuth | Codex 模板共享六项官方 scope，包含连接器读取及调用权限，并携带 `originator=codex_cli_rs`；其他 OAuth 类型保持独立模板 |
| 模型目录发现 | UA 与查询参数 `client_version` 使用同一版本；拒绝非法版本，保留现有保护头和运行时账户鉴权 |
| 模型能力 | 从官方公开模型目录提取能力快照，涵盖 `gpt-6-astra`、`gpt-6.1-sol`、六档推理强度及其他模型；账户返回的模型目录继续优先 |
| WebSocket 元数据 | 仅公开模型目录 ETag、轮次状态、实际模型与安全缓冲头；保留公开事件及未知公开字段、事件顺序 |
| 上游账户配额 | `codex.rate_limits` 继续进入账户级熔断与持久化路径，不当作网关用户自己的配额公开 |
| 原生记忆接口 | `POST /v1/memories/trace_summarize` 复用 Responses 权限及调度，执行原生同步操作，保留 traces、output 数组和未来字段，不注入 Responses 的 input/store/include 或流式默认值 |

公开快照来源为 `codex-rs/models-manager/models.json`。未复制提示词或账户套餐可见性；保留官方公开的 `comp_hash` 压缩兼容性标记，以支持 CLI Guardian 的压缩上下文复用；没有引入个人用户标识、已有会话 UUID、Cookie、访问令牌或账户凭据。运行时鉴权和账户字段仍由提供商密钥配置产生。

新增模型能力快照不等于授权访问该模型。可用模型应通过正式管理界面的上游模型查询、全局模型和提供商模型配置，以及密钥模型限制来设置。远端目录及实际账户权限决定上游是否支持模型，不能通过修改 `/models` 列表绕过。

六档推理强度描述 CLI 的选择项。`ultra` 是 CLI 的本地多代理模式：官方 `ModelInfo::resolve_reasoning_effort` 会把普通推理请求转换为有效的 `multi_agent_reasoning_effort`，否则依次采用 `max`、最高非 `ultra` 档位或 `medium`；本地模式仍保持 `ultra`。网关保留模型卡片的相关公开能力，由 CLI 完成该转换，不在原始 Responses 请求中擅自把 `ultra` 等同于 `max`，也不实现客户端的多代理编排。直接发送 `reasoning.effort=ultra` 与 CLI 选择 `ultra` 的请求不同，上游可能拒绝前者。

现场使用校验过发行资产摘要的官方 `0.159.3` CLI，分别对 `gpt-6-astra` 和 `gpt-6.1-sol` 选择 `ultra`，均成功完成；网关使用记录确认请求档位为模型指定的 `xhigh`。两模型的原始 Responses `low`、`medium`、`high`、`xhigh`、`max` 调用也均完成。直接发送原始 `ultra` 时，现场上游候选返回 HTTP 400，按现有重试策略最终返回 503；该结果不作为 CLI 的 `ultra` 模式验收失败。

同一现场的 WebSocket 验收对两模型分别完成两轮 `generate=false` 预热和 `previous_response_id` 续接，接收到了公开完成事件与 Codex metadata；这项检查覆盖连接、元数据和续接，不等同于完整生成、工具调用或所有重连场景。验证使用正式提供商开关，完成后恢复原配置。

记忆接口是 CLI 可选功能对应的协议。是否启动记忆任务仍由 CLI 自己的 feature/config 决定。它要求提供商支持该原生端点；Kiro、Grok、Antigravity 和 Gemini CLI 等私有适配器不能通过格式标签冒充支持。配置自定义路径时须使用 `/memories/{operation}` 等模板；仅描述 Responses 的固定路径会被拒绝。

Apps 文件上传属于 CLI 的 ChatGPT Apps 专用路径，普通自定义 API 提供商不会启动该流程；本次不将其伪装成通用 Responses 路由。`aether-vscodex` 的 app-server UI 协议版本属于独立客户端，不覆盖成 CLI 版本。

验证命令：

```bash
cargo fmt --all -- --check
RUST_MIN_STACK=16777216 cargo test --locked -p aether-ai-formats -p aether-oauth -p aether-model-fetch -p aether-provider-transport -p aether-usage-runtime --lib -- --test-threads=1
RUST_MIN_STACK=16777216 cargo test --locked -p aether-gateway --lib -- --test-threads=1
RUST_MIN_STACK=16777216 cargo test --locked -p aether-gateway --bins
RUST_MIN_STACK=16777216 cargo test --locked -p aether-gateway --test admin_unsigned_identity_headers --test architecture_guard
```

完整网关测试需要可执行的临时 PostgreSQL 工具（`AETHER_INITDB_BIN`、`AETHER_POSTGRES_BIN`、`AETHER_PG_CTL_BIN`）、权限受控的临时目录，以及不允许组写入的测试运行目录。Unix socket、更新元数据与私有运行文件测试会主动拒绝不安全的路径；测试目录应以 `umask 022` 或更严格的权限创建。

原生记忆端到端测试覆盖真实网关的权限、候选调度、执行计划、模型指令、原生 JSON、成功候选状态与明确停止重试策略下的上游错误响应；执行端使用本地测试服务器，实际账户网络可用性须在部署现场单独验证。
