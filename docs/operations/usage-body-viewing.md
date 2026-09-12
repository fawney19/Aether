# 请求正文查看与性能边界

## 按需读取

请求详情的轻量读取仍使用 `GET /api/admin/usage/{id}?include_bodies=false`，返回正文可用性与记录概要，不解析正文。

查看正文时，管理界面使用 `GET /api/admin/usage/{id}?include_bodies=true&body_field={field}&body_format=raw`。`body_field` 仅允许以下值：

- `request_body`：客户端请求体。
- `provider_request_body`：提供商请求体。
- `response_body`：提供商响应体。
- `client_response_body`：客户端响应体。

网页正文读取不再经过服务器 JSON 解码链路。数据库中的 `payload_gzip` 原样返回，历史压缩列同样直传；历史内联 JSON 返回 JSON 字节。接口仍经过管理权限校验、正文捕获状态检查、引用归属校验及审计，不加载用户名称和提供商名称等无关详情。

二进制响应的 `X-Aether-Body-Encoding` 为 `gzip` 或 `json`，另含 `X-Aether-Usage-Id`、`X-Aether-Body-Field`，前端验证记录与字段匹配。使用 `application/octet-stream`、`Content-Encoding: identity`，避免 HTTP 中间件重复压缩或浏览器提前解压；`Cache-Control: no-store, no-transform` 避免缓存敏感正文及代理改写。跨域允许凭证时显式暴露上述协议头。错误通过 HTTP 状态及 `X-Aether-Body-Error` 返回，不要求主线程解析二进制错误响应。

未指定 `body_format=raw` 的服务端 JSON 接口保持原有行为，但网页不再调用它加载正文；前端详情 API 默认也只取概要。非法字段、空字段、raw 模式缺少字段或同时设置 `include_bodies=false` 返回 HTTP 400。

前端按请求和正文字段保留 Worker 句柄，最多缓存两份已解析正文，并按解压字节总量 64 MiB 预算淘汰最久未访问的 Worker。该预算不是浏览器进程内存上限：解析对象、字符串及加载中的数据仍有额外开销。切换正文来源或标签会取消未完成的下载与解压；关闭抽屉、切换记录或组件卸载会终止全部 Worker。迟到结果不显示、不入缓存。请求从进行中变为完成或失败时，正文缓存失效，并重新读取当前选中的正文。

## 渲染与资源控制

- 仅挂载当前标签的内容，不再后台渲染隐藏标签。
- JSON 与对话均连续虚拟滚动，不需要点击上一页或下一页。接近底部时自动读取后续内容，向上滚动可重新查看先前内容。
- 折叠节点不遍历其子节点；点击括号展开节点时完整展开该子树，无需逐层点击。JSON 内部按 50 个显示片段批量读取，视口与预读区域最多挂载 4 批（200 个片段）。离屏内容用高度占位，缓存仅保留视口附近最多 6 批，不随滚动积累 DOM 或正文副本。
- JSON 不再有“显示更多”或“继续显示”按钮：长字符串和长键名在 Worker 中分段转义，随滚动自动显示全部字符。纯文本及非 JSON 响应同样自动衔接完整内容。分段保持 Unicode 字符完整，续段不重复显示 JSON 行号。
- 虚拟显示范围不改变正文长度；复制按钮仍复制完整内容。JSON 顺序读取复用遍历游标，避免每次滚动都从正文起点重新遍历。
- 压缩数据以 transferable ArrayBuffer 交给 Worker；解压、UTF-8 解码、JSON 解析、JSON 遍历及对话解析均在 Worker 中进行，完整对象不返回页面主线程。
- 对话内部按每批最多 10 个顶层块预览并自动衔接，限制嵌套块和文本传输量，长内容明确提示并支持增加预览。完整复制在用户点击后由 Worker 生成，不受虚拟显示范围影响。
- Worker 解压时逐块检查 64 MiB 上限，损坏 gzip、无效 UTF-8 和无效 JSON 给出明确错误。后台任务 30 秒超时会终止 Worker；不支持 Worker 或原生 gzip 解压的浏览器提示升级，不回退到主线程解析。
- 服务端最多 4 个后台解码任务的保护仍用于复制 cURL、请求重放等实际内部调用，不是网页解压的兼容回退。存储读取实现复用，不维护两套数据库读取逻辑。
- 数据库读取先按存储字节过滤：压缩正文最大 65 MiB，未压缩正文最大 64 MiB，超限不将完整载荷读入网关；浏览器还会独立校验解压后的大小。

## 错误定位

二进制接口的 `X-Aether-Body-Error` 和 JSON 接口的 `body_load_error_codes` 使用以下存储错误码；浏览器解压失败也映射为相同的明确提示：

| 错误码 | 含义 |
| --- | --- |
| `too_large` | 解压后正文超过 64 MiB 的安全读取上限。 |
| `decode_failed` | 正文解压或 JSON 解析失败。 |
| `missing` | 记录标记正文可用，但未能解析到实际存储内容。 |
| `storage_unavailable` | 其他存储读取错误；内部连接信息不会返回给页面。 |

前端分别显示请求超时、网络失败、HTTP 错误及上述存储错误。`too_large` 和 `decode_failed` 不提供无意义的重复重试；其他错误可手动重试。正文请求仍沿用现有 API 超时配置。

完整记录模式与已有记录不做静默截断；64 MiB 解压安全上限没有放宽。因此，超出上限的历史正文仍不能在线预览，但页面会明确说明限制，不再仅显示笼统的加载失败。

前后端需要一同更新。前端收到缺少二进制协议头、记录不匹配或字段不匹配的响应时会拒绝显示，并提示检查前后端版本；不会自动退回耗资源的完整 JSON 正文接口。构建前端时必须同时发布生成的 Worker JavaScript 资源。

## 针对性验证

```sh
cd frontend
npm run test:run -- src/features/usage src/api/__tests__/dashboard-body-loading.spec.ts
npm run type-check
```

```sh
cargo test -p aether-data-postgres usage_body_decode --lib --offline
cargo test -p aether-gateway admin_usage_detail --lib --offline
cargo test -p aether-gateway raw_body --lib --offline
cargo test -p aether-gateway body_load_errors_expose_safe_codes --lib --offline
```
