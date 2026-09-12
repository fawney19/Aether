# DNS 与出站连接审计（2026-09-08）

## 范围与结论边界

本轮基于当前工作区检查 `apps/`、`crates/` 中的 DNS 查询、IP 地址校验、
客户端构建、代理配置以及 TCP/WebSocket 连接入口，沿调用关系区分供应商请求、
身份认证、任意 URL 下载和隧道转发。不是仅搜索 `WebSocket` 或 `chatgpt.com`。

第一轮 WS 修复不足以说明所有路径已经一致。本轮又发现连接测试的旧地址过滤、
URL 形式 IPv6 误入 DNS、两处 DNS 答案静默截断，以及附件下载只保留首个地址。
这些问题，以及后续复核发现的 SMTP 无界解析和隧道缺少显式远程 DNS 模式，
已在工作区修复。本文不表示生产服务器已经部署，也不保证真实上游的
DNS、TLS、TUN 路由或出口代理一定可用。

## 已修复问题

### 1. 普通供应商出口的策略重复

- 普通 HTTP/SSE、浏览器指纹 HTTP、H2C 已使用供应商解析策略；普通 WS 仍有独立过滤，
  已在第一轮改为使用 `ExecutionSafeDnsResolver`。
- `/v1/test-connection` 的本地快捷路径仍逐项拒绝私网/保留 DNS 答案，导致同一个
  配置好的供应商正式请求能成功、连接测试却失败。本轮删除该重复策略，复用正式
  HTTP 客户端的解析器。
- HTTP、WS 和连接测试统一使用 `validate_execution_upstream_url` 校验供应商 URL。
  域名的 DNS 答案不按地址段过滤，不等于允许在 URL 中直接填写任意私网 IP。
- URL 中的凭据、fragment、私网/保留 IP 字面地址仍被拒绝；HTTP/WS 字面 loopback
  保持正式执行路径已有的兼容策略。禁用重定向、供应商显式代理和 WS 自循环检查保留。

相关文件：

- `apps/aether-gateway/src/execution_runtime/transport.rs`
- `apps/aether-gateway/src/handlers/proxy/websocket/transport.rs`
- `apps/aether-gateway/src/handlers/public/support/test_connection/route.rs`

### 2. IPv6 字面地址被当作域名

`Url::host_str()` 可提供 `[::1]` 形式的主机名，而 `IpAddr::from_str` 和
`lookup_host((host, port))` 的原有调用没有正确消化这个形式。修复前新增测试实际失败，
错误为 `failed to lookup address information: nodename nor servname provided, or not known`。

- 公共解析器现在先识别 IPv4、裸 IPv6 和合法的方括号 IPv6，直接生成 socket 地址。
- 不接受 `[localhost]`、`[127.0.0.1]` 等伪造的方括号主机名。
- relay 的 loopback 判断也使用同一解析函数，防止解析成功后又误判 `[::1]`。
- 隧道 SOCKS 地址编码复用该函数；IPv6 字面地址不再在远程 DNS 模式下被当作域名发送。
- 私网过滤仍由每个调用方的安全策略决定，公共解析器本身不扩大地址权限。

相关文件：`crates/aether-http/src/dns.rs`、
`apps/aether-tunnel/src/egress_proxy.rs`、网关执行传输模块。

### 3. DNS 答案静默截断

Bark 推送和 ChatGPT-Web 图片解析仍直接调用系统 DNS，然后仅取前 32 个答案。
本轮改用共享的 `lookup_host_with_limits`：保留原超时预算，超过 32 个答案直接报错，
不再静默忽略剩余答案。两条路径的私网校验、地址固定及官方来源 Fake-IP 例外不变。

owner gateway 转发的独立实现已取第 33 个答案并拒绝超限，因此不是同类遗漏。

### 4. Grok 附件下载缺少多地址回退

原实现校验全部 DNS 答案后只固定第一个公网地址；首个地址不可连接时，客户端无法
尝试 DNS 返回的其它公网地址。本轮改为把全部经过校验的地址交给客户端，保留双栈和
多地址回退能力。空答案、Fake-IP、私网及公网/私网混合答案仍整体拒绝。

### 5. SMTP DNS 不受连接超时控制

SMTP 发送和连接探测现在都先用共享异步解析器建立 TCP 连接，再把已连接的 blocking
socket 交给原来的 SMTP/TLS 协议实现，不在阻塞任务中重新解析或连接。

- DNS 最长 10 秒；DNS 与 TCP 尝试共用 30 秒总预算。
- 保留全部不超过 32 个答案，不再静默截断为前 16 个；超限直接报错。
- TCP 地址竞争取首个成功连接并释放其它尝试，避免首个黑洞地址吃完整个预算，
  使后续可达地址根本没有机会建连。SMTP/TLS 仅在最终选中的连接上运行。
- 解析失败、空答案、解析超时及总连接超时有明确错误，不把底层 DNS 细节暴露给调用方。
- TLS 继续验证原始 SMTP 主机名；读写超时仍为 30 秒，内网邮件服务器策略不变。

相关文件：`apps/aether-gateway/src/email_delivery.rs`。

### 6. 隧道供应商出口可显式委托代理解析

新增默认关闭的 `upstream_proxy_remote_dns`（CLI `--upstream-proxy-remote-dns`，环境变量
`AETHER_TUNNEL_UPSTREAM_PROXY_REMOTE_DNS`，setup 的 `Proxy Remote DNS` 开关）。

- 默认路径仍本地解析、执行 ACL、固定 IP，`socks5h://` 本身不改变既有安全策略。
- 显式启用后，HTTP CONNECT 或 SOCKS5h 接收原始域名，不再预先查询隧道本机 DNS；
  Host、SNI 和证书校验仍保留原域名。
- 必须配置 HTTP/SOCKS5h 代理；无代理或 `socks5://` 会在启动和客户端构建时拒绝。
- 仍执行端口白名单、URL 校验以及 IP 字面地址/`localhost` 限制；字面 IP 保持固定。
- 远程 DNS 和固定 IP 使用不同连接池键；远程模式解析器明确拒绝本地 DNS 回退。
- 代理 DNS、TCP、CONNECT/SOCKS 和 TLS 握手共同受上游连接超时限制。
- 启用时打印安全提示：**域名目标的最终 IP ACL 由受信任代理负责**。普通 CONNECT/
  SOCKS5 协议无法让隧道校验代理最终连接的 IP，不能宣称远程解析仍保留本地逐 IP 检查。

配置示例及部署边界见 `apps/aether-tunnel/README.md` 的“上游 HTTP 请求”章节。
该文档中 DNS 缓存和连接超时等环境变量误写的 `_SECS` 后缀也已纠正，避免按文档
设置后实际未被程序读取；CLI/TOML 参数名称不变。

## 必须保留的策略差异

| 路径 | DNS / 代理策略 | 本轮处理 |
| --- | --- | --- |
| 普通供应商 HTTP/SSE、浏览器指纹、H2C、WS、连接测试 | 域名答案不按地址段过滤；显式代理优先；供应商客户端不自动使用系统代理环境变量 | 统一遗留分支 |
| 供应商操作类 OAuth、模型获取 | 经执行计划进入供应商运行时；不能与用户登录的身份 OAuth 混为一谈 | 核对调用关系 |
| 身份 OAuth / 管理端 OAuth 探测 | 独立敏感出口；校验目标、固定地址；部分内置官方来源允许窄范围 Fake-IP | 保留，不全局放开 |
| Grok 用户附件、公共视频 URL | 不可信 URL；保留公网限制和固定地址，不能套用供应商域名策略 | Grok 保留全部安全地址 |
| ChatGPT-Web 图片下载与上传 | 普通 URL 严格过滤；可信存储来源有专门 Fake-IP 例外 | 公共有界解析器 |
| 支付出口 | 独立公网校验；固定 Stripe 来源有专门 Fake-IP 例外 | 保留 |
| 系统更新、外部模型目录、Server Chan、Bark | 各自的可信来源例外；自定义目的地不能自动获得同样权限 | Bark 公共有界解析器 |
| gateway owner / internal relay | 独立私网策略、可信 relay 配置和地址固定 | 保留；修复 IPv6 判断 |
| 隧道承载的供应商 HTTP 流量 | 默认本地端口/IP ACL 和固定 IP；显式远程模式委托受信任代理解析及执行域名 IP ACL | 新增默认关闭的远程 DNS 模式 |
| 隧道到 gateway 的控制连接 | 与隧道供应商出口分开；可配置专门出口代理和 IP family | IPv6 / SOCKS 编码复用公共函数 |
| 独立 Responses WS probe | 独立直连诊断程序，不使用供应商代理配置 | 不应当作生产代理路径的等价验证 |

## 仍需注意的实际限制

1. **Fake-IP 只是地址，不提供路由。** 取消供应商 DNS 地址过滤后，进程所在网络仍必须
   能通过对应的 TUN/透明代理处理 Fake-IP；否则会变成 TCP 超时，而不是过滤报错。
2. **隧道代理不等于网关直连代理。** 默认仍先本地解析并通过 ACL；只有显式启用
   `upstream_proxy_remote_dns` 才委托代理解析供应商域名。代理端点自身的域名仍需
   本地 DNS；本地解析完全不可用时，代理 URL 应使用可达 IP。
3. 隧道默认 `AETHER_TUNNEL_ALLOW_PRIVATE_TARGETS=false` 仍会拒绝 Fake-IP。
   该开关是扩大内网访问权限，不是建议普遍启用的 DNS 修复；应优先让隧道主机得到
   可路由的真实 DNS 答案，或配置受信任代理的显式远程 DNS 模式。
4. 更新客户端支持自己的代理环境变量和 `NO_PROXY`；不能把这一点推广到供应商请求。
   网关供应商 SOCKS 配置会归一化为远程 DNS 语义，但其它明确区分 SOCKS5/SOCKS5h
   的独立工具仍遵循各自配置。
5. SMTP 等辅助服务不是供应商解析器的调用方。SMTP 已修复 DNS 超时和答案截断，
   但不自动继承供应商出口代理。系统解析器由 Tokio 阻塞池承载，异步超时会停止等待，
   不等于操作系统正在执行的 DNS 调用能够被强制终止。
6. 本轮没有使用生产凭据、发送真实模型请求、修改系统 DNS、关闭 TUN 或重启服务器。
   生产验证仍需在实际容器/进程的网络命名空间内进行。

## 回归验证

- 公共 DNS：合法/非法 IPv6 主机形式、端口、零超时、答案上限、超限拒绝。
- 供应商 DNS：Fake-IP 保留，HTTP 与 wreq 解析结果一致；relay 私网过滤不变。
- WS：普通与浏览器指纹客户端，经本机 HTTP、SOCKS5、SOCKS5h mock 代理，使用
  `provider-dns.invalid` 完成真实 WS upgrade；SOCKS mock 断言收到域名而非本地解析 IP。
  这是本地明文 WS 的代理路径测试，不代替生产 WSS 的 TLS/SNI 检查。
- 连接测试：供应商 URL 校验一致，无预解析构建请求，重定向不转发凭据。
- 附件与辅助出口：多地址保留、私网/混合答案拒绝及官方 Fake-IP 例外回归。
- 隧道：IPv6 SOCKS 编码、地址 ACL、缓存策略隔离、默认固定 IP 代理连接；新增远程
  DNS 模式的 HTTP CONNECT/SOCKS5h 域名握手、Host/SNI 主机名、禁止本地解析回退、
  连接池隔离、代理/TLS 握手超时，以及 CLI/TOML/TUI 配置验证和持久化。
- SMTP：DNS 超时、空答案/错误信息、前 16 个地址不可用时使用第 17 个地址，以及
  本机 mock SMTP 探测和邮件投递；不发送真实邮件。多地址测试先复现串行建连超时，
  改成有界地址竞争后通过。

最终重新执行结果：**809 项测试通过，0 失败**。

- 网关：594 项，覆盖完整 WS 模块、执行传输、Grok、ChatGPT-Web 图片、连接测试，
  DNS/Fake-IP/解析地址校验，以及 SMTP 发送、探测和相关配置回归。
- 隧道：197 项，全量单元/本机集成测试。
- `aether-http`：18 项，包含先失败、后修复通过的方括号 IPv6 回归。
- 单独执行的 13 项 SMTP 回归及前轮测试均为上述集合的子集，不重复计入总数。
- Rust 格式检查与 `git diff --check` 通过。

涉及监听器的测试在获准的沙箱外绑定本机回环端口；最初沙箱内的端口权限失败不作为
功能失败，也未通过跳过测试来规避。所有 Cargo 测试使用 `--offline`，代理上游是
本机 mock，不使用生产凭据。

复现命令：

```bash
cargo test -p aether-gateway --lib --offline -- --quiet \
  handlers::proxy::websocket:: execution_runtime::transport::tests \
  execution_runtime::grok::tests execution_runtime::chatgpt_web_image::tests \
  test_connection bark_push::tests server_chan_push::tests \
  dns fake_ip benchmarking resolved_addrs email_delivery smtp
cargo test -p aether-tunnel --offline -- --quiet
cargo test -p aether-http --offline
```
