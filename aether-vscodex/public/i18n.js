(function (root, factory) {
  "use strict";

  const api = factory(root);
  if (typeof module === "object" && module.exports) module.exports = api;
  if (root?.document) root.VscodexI18n = api;
})(typeof window === "object" ? window : undefined, function (root) {
  "use strict";

  const STORAGE_KEY = "aether-vscodex.locale";
  const SUPPORTED = new Set(["zh-CN", "en-US"]);
  const EN = Object.freeze({
    "本地模式": "Local mode",
    "独立模式": "Standalone mode",
    "云端模式": "Cloud mode",
    "控制模式": "Control mode",
    "同步": "Sync",
    "异步": "Async",
    "同步模式跟随 VS Code 当前会话": "Sync mode follows the current VS Code conversation",
    "异步模式可独立管理会话": "Async mode manages conversations independently",
    "正在切换控制模式": "Switching control mode",
    "控制模式已切换": "Control mode switched",
    "控制模式切换失败": "Unable to switch control mode",
    "当前任务或请求完成后才能切换控制模式": "The control mode can be changed after the current task or request finishes",
    "同步模式下会话管理由 VS Code 控制": "VS Code controls conversation navigation in sync mode",
    "当前模式不支持修改会话设置": "The current mode does not support changing conversation settings",
    "本机连接（无需 token）": "Local connection (no token required)",
    "本机模式无需填写；认证模式再填写": "No token is needed locally; enter one only for authenticated mode",
    "访问 token（认证模式）": "Access token (authenticated mode)",
    "粘贴 relay 启动时打印的 token": "Paste the token printed when the relay started",
    "本地连接无需 token": "No token is needed for a local connection",
    "编辑外部文件和联网时始终询问": "Always ask before editing external files or using the network",
    "不限制联网或文件访问": "Allow unrestricted network and file access",
    "查看请求数据": "View request data",
    "查看上下文用量": "View context usage",
    "创建新会话": "New conversation",
    "打开会话历史": "Open conversation history",
    "待处理的 Codex 请求": "Pending Codex requests",
    "当前会话": "Current conversation",
    "当前模型": "Current model",
    "切换模型": "Change model",
    "等待 VS Code 主机": "Waiting for VS Code host",
    "等待连接": "Waiting for connection",
    "对话内容": "Conversation",
    "发送 JSON": "Send JSON",
    "发送后续指令": "Send follow-up",
    "发送消息": "Send message",
    "返回会话列表": "Back to conversations",
    "返回模型强度": "Back to model effort",
    "高级": "Advanced",
    "简洁": "Simple",
    "更多操作": "More actions",
    "更高效": "More efficient",
    "更智能": "More capable",
    "工作目录": "Working directory",
    "工作区": "Workspace",
    "工作区写入": "Workspace write",
    "回到最新消息": "Jump to latest message",
    "正在工作，回到最新消息": "Working, jump to latest message",
    "会话历史": "Conversation history",
    "会话设置": "Conversation settings",
    "仅本次 turn": "This turn only",
    "仅查看文件，不修改工作区": "View files without changing the workspace",
    "仅对可能不安全的操作询问": "Ask only for potentially unsafe actions",
    "拒绝": "Deny",
    "可用会话": "Available conversations",
    "连接设置": "Connection settings",
    "留空使用默认模型": "Leave empty to use the default model",
    "模式": "Mode",
    "模型": "Model",
    "模型与推理强度": "Model and reasoning effort",
    "默认": "Default",
    "启动新 thread": "Start new thread",
    "强度": "Effort",
    "切换模型与推理强度": "Change model and reasoning effort",
    "清除搜索": "Clear search",
    "清空当前输出": "Clear current output",
    "清空对话": "Clear conversation",
    "取消": "Cancel",
    "权限设置": "Permission settings",
    "确认": "Confirm",
    "确认完全访问": "Confirm full access",
    "沙箱": "Sandbox",
    "上下文用量": "Context usage",
    "设置": "Settings",
    "审批策略": "Approval policy",
    "使用 config.toml 中的权限": "Use permissions from config.toml",
    "使用左右方向键调整强度": "Use the left and right arrow keys to adjust effort",
    "授权范围": "Authorization scope",
    "授权与输入": "Approvals and input",
    "刷新会话列表": "Refresh conversations",
    "搜索最近会话": "Search recent conversations",
    "提交后续变更要求": "Ask for follow-up changes",
    "添加工作区上下文": "Add workspace context",
    "添加文件": "Add files",
    "添加文件及更多内容": "Add files and more",
    "添加照片": "Add photos",
    "推理强度": "Reasoning effort",
    "完全访问": "Full access",
    "完全访问允许 Codex 执行命令、访问互联网并编辑工作区之外的文件。": "Full access lets Codex run commands, use the internet, and edit files outside the workspace.",
    "网页搜索": "Web search",
    "未认证": "Unauthenticated",
    "显示 Codex": "Show Codex",
    "修改权限": "Change permissions",
    "需要时询问": "Ask when needed",
    "已附着当前会话": "Attached to current conversation",
    "隐藏面板": "Hide panel",
    "由 Codex 审批": "Let Codex decide",
    "允许": "Allow",
    "允许一次": "Allow once",
    "暂无待处理请求": "No pending requests",
    "暂无用量数据": "No usage data",
    "展开面板": "Expand panel",
    "正在连接": "Connecting",
    "只读": "Read only",
    "中断当前 turn": "Interrupt current turn",
    "重新同步": "Resync",
    "子代理": "Subagent",
    "自定义": "Custom",
    "最近会话": "Recent conversations",
    "Codex 消息": "Codex messages",
    "JSON 响应": "JSON response",
    "语言": "Language",
    "中文": "Chinese",
    "跟随浏览器": "Use browser language",
    "正在连接云端会话": "Connecting to cloud conversation",
    "正在等待云端连接": "Waiting for cloud connection",
    "云端连接已断开": "Cloud connection disconnected",
    "云端连接配置无效": "Invalid cloud connection configuration",
    "云端连接地址必须与当前页面同源": "The cloud connection URL must be same-origin",
    "正在获取新的连接凭证": "Requesting new connection credentials",
    "父页面已断开连接": "Disconnected by the parent page",
    "当前 relay 需要 token": "This relay requires a token",
    "WebSocket 未连接": "WebSocket is not connected",
    "连接中": "Connecting",
    "同步中": "Syncing",
    "已连接": "Connected",
    "认证失败，准备重连": "Authentication failed; preparing to reconnect",
    "准备重连": "Preparing to reconnect",
    "重连中": "Reconnecting",
    "收到无法解析的 relay 消息": "Received an unreadable relay message",
    "等待 relay 连接": "Waiting for relay connection",
    "等待 VS Code 主机连接": "Waiting for VS Code host",
    "VS Code 主机未连接": "VS Code host is disconnected",
    "等待 VS Code 伴随扩展连接": "Waiting for the VS Code companion extension",
    "VS Code 伴随扩展未连接": "VS Code companion extension is disconnected",
    "等待在 VS Code 中打开 Codex 会话": "Open a Codex conversation in VS Code to continue",
    "会话已关闭": "Conversation closed",
    "VS Code 会话已关闭": "VS Code conversation closed",
    "会话操作失败": "Conversation operation failed",
    "当前任务结束或请求处理后才能切换": "You can switch after the current task or request finishes",
    "目标会话没有返回 VS Code 快照，请先在官方 Codex 面板打开它": "The target conversation did not return a VS Code snapshot. Open it in the official Codex panel first.",
    "当前 relay 版本不支持此会话操作，请重启 relay": "This relay version does not support the conversation action. Restart the relay.",
    "正在读取会话…": "Loading conversations...",
    "正在切换会话…": "Switching conversation...",
    "无法读取会话": "Unable to load conversations",
    "没有匹配的会话": "No matching conversations",
    "没有可附加的会话": "No attachable conversations",
    "没有可控制的会话": "No controllable conversations",
    "正在切换": "Switching",
    "未打开": "Not open",
    "当前": "Current",
    "可切换": "Available",
    "会话": "Conversation",
    "当前角色不能创建会话": "Your current role cannot create conversations",
    "正在创建新会话": "Creating a new conversation",
    "无法创建新会话": "Unable to create a new conversation",
    "当前任务仍在运行或等待授权，暂不能切换": "The current task is running or awaiting approval, so it cannot be switched yet",
    "会话切换失败": "Conversation switch failed",
    "正在确认会话": "Confirming conversation",
    "正在加载会话": "Loading conversation",
    "会话已切换": "Conversation switched",
    "正在更新模型设置": "Updating model settings",
    "模型设置已更新": "Model settings updated",
    "无法更新模型设置": "Unable to update model settings",
    "已停止": "Stopped",
    "成功": "Succeeded",
    "无输出": "No output",
    "等待输出…": "Waiting for output...",
    "执行步骤": "Action",
    "正在读取文件": "Reading files",
    "读取完成": "Finished reading",
    "已读取文件运行了命令": "Read files and ran a command",
    "已读取文件": "Read files",
    "编辑了文件": "Edited files",
    "已完成计划": "Completed plan",
    "读取文件失败": "Failed to read files",
    "已停止读取文件": "Stopped reading files",
    "读取文件": "Read files",
    "已运行命令": "Ran command",
    "正在运行命令": "Running command",
    "正在思考": "Thinking",
    "正在制定计划": "Creating a plan",
    "正在编辑文件": "Editing files",
    "正在处理": "Working",
    "已完成思考": "Finished thinking",
    "计划完成": "Plan completed",
    "文件编辑完成": "Finished editing files",
    "工作说明": "Progress update",
    "计划": "Plan",
    "文件变更": "File changes",
    "等待授权": "Waiting for approval",
    "正在生成": "Generating",
    "已中断": "Interrupted",
    "失败": "Failed",
    "已完成": "Completed",
    "正在工作": "Working",
    "正在等待你的回答": "Waiting for your answer",
    "正在搜索网页": "Searching the web",
    "执行失败": "Action failed",
    "处理中": "Working",
    "思考": "Reasoning",
    "编辑文件": "Edit files",
    "思考中": "Thinking",
    "编辑中": "Editing",
    "进行中": "In progress",
    "异常": "Error",
    "未读": "Unread",
    "本地会话": "Local conversation",
    "默认拒绝，请明确允许": "Denied by default; allow explicitly",
    "需要远程确认或输入": "Remote confirmation or input is required",
    "允许运行命令？": "Allow this command?",
    "允许修改文件？": "Allow file changes?",
    "需要扩大权限": "Additional permissions required",
    "Codex 需要你的回答": "Codex needs your answer",
    "需要外部服务确认": "External service confirmation required",
    "Codex 请求确认": "Codex requests confirmation",
    "高风险": "High risk",
    "低风险": "Low risk",
    "需确认": "Confirmation required",
    "请输入": "Enter a response",
    "提交回答": "Submit answer",
    "发送自定义响应": "Send custom response",
    "自定义响应不是有效 JSON": "The custom response is not valid JSON",
    "响应不是有效 JSON": "The response is not valid JSON",
    "远程参与者拒绝": "Denied by remote participant",
    "状态": "Status",
    "命令": "Command",
    "详情": "Details",
    "复制消息": "Copy message",
    "复制命令": "Copy command",
    "复制输出": "Copy output",
    "未知": "Unknown",
    "未知错误": "Unknown error",
    "已附着 VS Code 当前 Codex 会话；输入、输出和授权都回到同一个会话。": "Attached to the current VS Code Codex conversation. Messages, output, and approvals all return to that conversation.",
    "当前为独立 app-server 模式。": "Currently using standalone app-server mode.",
    "已附着现有会话": "Attached to existing conversation",
    "通用 Codex 模型": "General-purpose Codex model",
    "平衡速度与推理": "Balanced speed and reasoning",
    "可用模型": "Available model",
    "极低": "Minimal",
    "轻度": "Low",
    "标准": "Medium",
    "深度": "High",
    "极高": "Extra high",
    "最大": "Maximum",
    "此模型使用默认推理强度": "This model uses its default reasoning effort",
    "返回简洁模型选择": "Return to simple model selection",
    "显示高级模型选项": "Show advanced model options",
    "自定义权限由 config.toml 管理": "Custom permissions are managed by config.toml",
    "正在等待指示": "Waiting for instructions",
    "正在工作": "Working",
    "命令输出": "Command output",
    "工具输出": "Tool output",
    "发送 Steer": "Send steer",
    "会话切换失败，已恢复原会话": "Conversation switch failed; restored the previous conversation",
    "(空消息)": "(empty message)",
    "今天": "Today",
    "昨天": "Yesterday",
    "未完成": "Not completed",
    "步骤": "Step",
    "查看图像": "View image",
    "等待输入": "Waiting for input",
    "读取文件运行命令失败": "Failed to read files and run a command",
    "发送输入": "Send input",
    "工具": "Tool",
    "工具失败": "Tool failed",
    "正在搜索": "Searching",
    "你停止了工作": "You stopped working",
    "关闭子代理": "Close subagent",
    "恢复子代理": "Resume subagent",
    "启动子代理": "Start subagent",
    "搜索": "Search",
    "文件": "File",
    "新会话已在 VS Code 中打开": "The new conversation opened in VS Code",
    "事件窗口已过期，请以当前快照为准": "The event window expired; the current snapshot is authoritative",
    "执行状态未知，请等待主机恢复": "Execution status is unknown; wait for the host to recover",
    "文件已截断": "File truncated",
    "已拒绝": "Denied",
    "已开始工作": "Started working",
    "已添加工作区上下文": "Added workspace context",
    "已添加网页搜索": "Added web search",
    "运行命令": "Run command",
    "整理上下文": "Compacting context",
    "正在切换会话": "Switching conversation",
    "MCP 工具": "MCP tool",
    " · @ 可标记代理": " · @ to mention agents",
  });

  const EN_PATTERNS = Object.freeze([
    [/^用时 1分钟(\d+)秒$/, "Worked for 1m{1}s"],
    [/^用时 (\d+)分(\d+)秒$/, "Worked for {1}m{2}s"],
    [/^用时 1分钟$/, "Worked for 1m"],
    [/^用时 (\d+)分$/, "Worked for {1}m"],
    [/^用时 (\d+)秒$/, "Worked for {1}s"],
    [/^用时 (\d+)毫秒$/, "Worked for {1}ms"],
    [/^用时\s+(.+)$/, "Worked for {1}"],
    [/^已思考 1分钟(\d+)秒$/, "Thought for 1m{1}s"],
    [/^已思考 (\d+)分(\d+)秒$/, "Thought for {1}m{2}s"],
    [/^已思考 (\d+)秒$/, "Thought for {1}s"],
    [/^已思考\s+(.+)$/, "Thought for {1}"],
    [/^退出码\s+(.+)$/, "Exit code {1}"],
    [/^正在读取\s+(.+)$/, "Reading {1}", [1]],
    [/^已读取\s+(.+)$/, "Read {1}", [1]],
    [/^读取失败\s*·\s*(.+)$/, "Failed to read {1}", [1]],
    [/^已停止读取\s+(.+)$/, "Stopped reading {1}", [1]],
    [/^读取\s+(.+)$/, "Read {1}", [1]],
    [/^已读取这些内容\s*·\s*(\d+)\s*个文件(.*)$/, "Read these items · {1} files{2}"],
    [/^已在\s+(.+)\s+内运行\s+(.+)$/, "Ran {2} in {1}", [2]],
    [/^命令运行失败\s*·\s*(.+?)\s*·\s*((?:\d+毫秒|\d+秒|1分钟(?:\d+秒)?|\d+分(?:\d+秒)?))$/, "Command failed · {1} · {2}", [1]],
    [/^命令运行失败\s*·\s*(.+)$/, "Command failed · {1}", [1]],
    // Renderer-owned disclosure labels. Keep the captured command/model text
    // intact; only the surrounding UI words are localized.
    [/^命令\s*·\s*(.+)$/, "Command · {1}", [1]],
    [/^已工具\s*·\s*(.+)$/, "Tool completed · {1}"],
    [/^当前模型\s+(.+?)\s+(极低|轻度|标准|深度|极高|最大)，切换模型$/, "Current model: {1} {2}. Change model", [1]],
    [/^已停止\s*(.+?)\s*·\s*((?:\d+毫秒|\d+秒|1分钟(?:\d+秒)?|\d+分(?:\d+秒)?))$/, "Stopped {1} · {2}", [1]],
    [/^已运行\s*(.+)$/, "Ran {1}", [1]],
    [/^命令运行失败\s*(.*)$/, "Command failed{1}", [1]],
    [/^命令:\s*(.+?)（执行状态未知，请等待主机恢复）$/, "Command: {1} (execution status unknown; wait for the host to recover)", [1]],
    [/^命令:\s*(.+)$/, "Command: {1}", [1]],
    [/^已停止\s*(.+)$/, "Stopped {1}", [1]],
    [/^正在运行\s+(.+)$/, "Running {1}", [1]],
    [/^(.+?)\s*·\s*失败$/, "{1} · Failed"],
    [/^(.+?)\s*·\s*已中断$/, "{1} · Interrupted"],
    [/^(.+)\s+失败$/, "{1} failed"],
    [/^编辑了文件\s*·\s*(.+)$/, "Edited files · {1}"],
    [/^已完成计划\s*·\s*(.+)$/, "Completed plan · {1}"],
    [/^(\d+)\/(\d+)\s*个会话$/, "{1}/{2} conversations"],
    [/^(\d+)\s*个会话$/, "{1} conversations"],
    [/^会话\s+(.+)$/, "Conversation {1}", [1]],
    [/^工作区\s*·\s*(.+)$/, "Workspace · {1}", [1]],
    [/^昨天\s+(.+)$/, "Yesterday {1}", [1]],
    [/^正在切换到「(.+)」…?$/, "Switching to “{1}”...", [1]],
    [/^你在\s+(.+)\s+后停止了$/, "You stopped after {1}"],
    [/^执行失败\s*·\s*(.+)$/, "Action failed · {1}"],
    [/^新会话创建失败：(.+)$/, "Unable to create a new conversation: {1}", [1]],
    [/^模型设置更新失败：(.+)$/, "Unable to update model settings: {1}", [1]],
    [/^(.+)（执行状态未知，请等待主机恢复）$/, "{1} (execution status unknown; wait for the host to recover)", [1]],
    [/^(.+) 完成$/, "{1} completed", [1]],
    [/^请求 #(.+) 已提交$/, "Request #{1} submitted"],
    [/^请求 #(.+) 已发送，等待 VS Code 主机确认$/, "Request #{1} sent; waiting for the VS Code host"],
    [/^无法读取 (.+)$/, "Unable to read {1}", [1]],
    [/^\[图片附件：(.+)\]$/, "[Image attachment: {1}]", [1]],
    [/^(.+) 已开始工作$/, "{1} started working", [1]],
    [/^(.+) 已完成$/, "{1} completed", [1]],
    [/^(.+) 已中断$/, "{1} interrupted", [1]],
    [/^…（文件已截断）$/, "... (file truncated)"],
    [/^当前模型\s+(.+)，切换模型$/, "Current model: {1}. Change model", [1]],
    [/^切换模型（当前\s+(.+?)\s+(极低|轻度|标准|深度|极高|最大)）$/, "Change model (current: {1} {2})", [1]],
    [/^切换模型（当前\s+(.+)）$/, "Change model (current: {1})", [1]],
    [/^修改权限，当前为(.+)$/, "Change permissions. Current: {1}"],
    [/^修改权限（当前：(.+)）$/, "Change permissions (current: {1})"],
    [/^上下文已使用\s*(\d+)%（剩余\s*(\d+)%）$/, "Context used: {1}% ({2}% remaining)"],
    [/^(\d+)%\s*已使用$/, "{1}% used"],
    [/^剩余\s+(.+)\s+tokens$/, "{1} tokens remaining"],
    [/^当前上下文\s+(.+)\s+tokens$/, "Current context: {1} tokens"],
    [/^最近请求\s+(.+)\s+tokens$/, "Latest request: {1} tokens"],
    [/^累计\s+(.+)\s+tokens$/, "Total: {1} tokens"],
    [/^使用\s+(.+)$/, "Using {1}", [1]],
    [/^已用时\s+(.+)$/, "Elapsed: {1}"],
    [/^(\d+)\s*个后台代理(.*)$/, "{1} background agents{2}"],
    [/^(\d+)毫秒$/, "{1}ms"],
    [/^(\d+)秒$/, "{1}s"],
    [/^1分钟(\d+)秒$/, "1m{1}s"],
    [/^(\d+)分(\d+)秒$/, "{1}m{2}s"],
    [/^1分钟(\d+秒)?$/, "1m{1}"],
    [/^(\d+)分(\d+秒)?$/, "{1}m{2}"],
  ]);
  const ZH = Object.freeze(Object.fromEntries(Object.entries(EN).map(([source, translated]) => [translated, source])));

  let currentLocale = "zh-CN";
  let observer = null;
  const textSources = new WeakMap();
  const textRendered = new WeakMap();
  const attributeSources = new WeakMap();
  const attributeRendered = new WeakMap();

  function normalizeLocale(value) {
    const locale = String(value || "").trim().replace("_", "-").toLowerCase();
    return locale.startsWith("zh") ? "zh-CN" : "en-US";
  }

  function embeddedMode() {
    if (root?.AetherVscodexEmbed?.active) return true;
    try { return new URLSearchParams(root?.location?.search || "").get("embed") === "aether"; }
    catch { return false; }
  }

  function interpolate(template, values) {
    return String(template).replace(/\{(\d+)\}/g, (_, index) => values[Number(index)] ?? "");
  }

  function translate(value, locale = currentLocale, depth = 0) {
    const source = String(value ?? "");
    if (!source) return source;
    if (normalizeLocale(locale) === "zh-CN") return ZH[source] || source;
    if (Object.prototype.hasOwnProperty.call(EN, source)) return EN[source];
    for (const [pattern, template, rawIndexes] of EN_PATTERNS) {
      const match = source.match(pattern);
      if (match) {
        const translatedMatch = match.map((part, index) => index === 0
          ? part
          : rawIndexes?.includes(index) ? part
          : depth < 6 ? translate(part, locale, depth + 1) : (EN[part] || part));
        return interpolate(template, translatedMatch);
      }
    }
    return source;
  }

  function shouldSkipTextNode(node) {
    const parent = node?.parentElement;
    return Boolean(parent?.closest?.("code, pre, .message-body, .request-summary, .request-questions, .request-json, .request-command, .diff-output, .terminal-output, .session-option-title, .subagent-name, .subagent-summary-label"));
  }

  function translateTextNode(node) {
    if (!node || shouldSkipTextNode(node)) return;
    const current = node.nodeValue;
    const previousRendered = textRendered.get(node);
    if (!textSources.has(node) || current !== previousRendered) textSources.set(node, current);
    const source = textSources.get(node);
    const leading = source.match(/^\s*/)?.[0] || "";
    const trailing = source.match(/\s*$/)?.[0] || "";
    const core = source.slice(leading.length, source.length - trailing.length);
    if (!core) return;
    const translated = translate(core);
    const rendered = `${leading}${translated}${trailing}`;
    textRendered.set(node, rendered);
    if (rendered !== current) node.nodeValue = rendered;
  }

  function translateAttributes(element) {
    if (!element?.getAttribute || element.closest?.(".message-body, pre, code")) return;
    let sources = attributeSources.get(element);
    let renderedValues = attributeRendered.get(element);
    if (!sources) { sources = new Map(); attributeSources.set(element, sources); }
    if (!renderedValues) { renderedValues = new Map(); attributeRendered.set(element, renderedValues); }
    for (const attribute of ["title", "aria-label", "placeholder", "data-placeholder"]) {
      if (!element.hasAttribute(attribute)) continue;
      const current = element.getAttribute(attribute);
      if (!sources.has(attribute) || current !== renderedValues.get(attribute)) sources.set(attribute, current);
      const source = sources.get(attribute);
      const translated = translate(source);
      renderedValues.set(attribute, translated);
      if (translated !== current) element.setAttribute(attribute, translated);
    }
  }

  function translateTree(node) {
    if (!root?.document || !node) return;
    if (node.nodeType === 3) {
      translateTextNode(node);
      return;
    }
    if (node.nodeType !== 1 && node.nodeType !== 9 && node.nodeType !== 11) return;
    if (node.nodeType === 1) translateAttributes(node);
    const walker = root.document.createTreeWalker(node, root.NodeFilter.SHOW_ELEMENT | root.NodeFilter.SHOW_TEXT);
    for (let current = walker.nextNode(); current; current = walker.nextNode()) {
      if (current.nodeType === 3) translateTextNode(current);
      else translateAttributes(current);
    }
  }

  function applyDocument() {
    if (!root?.document) return;
    root.document.documentElement.lang = currentLocale;
    translateTree(root.document.body);
    const selector = root.document.getElementById("localeSelect");
    if (selector && selector.value !== currentLocale) selector.value = currentLocale;
  }

  function setLocale(value, options = {}) {
    currentLocale = SUPPORTED.has(value) ? value : normalizeLocale(value);
    if (options.persist !== false && root?.localStorage && !embeddedMode()) {
      try { root.localStorage.setItem(STORAGE_KEY, currentLocale); } catch { /* storage may be disabled */ }
    }
    applyDocument();
    if (root?.CustomEvent) root.dispatchEvent?.(new root.CustomEvent("aether-vscodex:locale", { detail: { locale: currentLocale } }));
    return currentLocale;
  }

  function initialLocale() {
    if (embeddedMode()) return normalizeLocale(root?.navigator?.language);
    try {
      const saved = root?.localStorage?.getItem(STORAGE_KEY);
      if (SUPPORTED.has(saved)) return saved;
    } catch { /* storage may be disabled */ }
    return normalizeLocale(root?.navigator?.language);
  }

  function start() {
    if (!root?.document) return;
    currentLocale = initialLocale();
    applyDocument();
    if (typeof root.MutationObserver === "function" && !observer) {
      observer = new root.MutationObserver((records) => {
        if (currentLocale === "zh-CN") return;
        for (const record of records) {
          if (record.type === "characterData") translateTextNode(record.target);
          else if (record.type === "attributes") translateAttributes(record.target);
          else for (const node of record.addedNodes) translateTree(node);
        }
      });
      observer.observe(root.document.documentElement, {
        subtree: true,
        childList: true,
        characterData: true,
        attributes: true,
        attributeFilter: ["title", "aria-label", "placeholder", "data-placeholder"],
      });
    }
  }

  const api = {
    locale: () => currentLocale,
    normalizeLocale,
    setLocale,
    start,
    t: (value) => translate(value),
    translate,
    translateTree,
    messages: { "zh-CN": Object.freeze({}), "en-US": EN },
  };

  if (root?.document) {
    if (root.document.readyState === "loading") root.document.addEventListener("DOMContentLoaded", start, { once: true });
    else start();
  }
  return api;
});
