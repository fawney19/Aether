function nonEmptyString(value: string | null | undefined): string | null {
  const trimmed = value?.trim()
  return trimmed ? trimmed : null
}

function formatSecondsFromMsText(value: string): string {
  const milliseconds = Number(value)
  if (!Number.isFinite(milliseconds) || milliseconds <= 0) return '10秒'
  const seconds = milliseconds / 1000
  return Number.isInteger(seconds) ? `${seconds}秒` : `${seconds.toFixed(1)}秒`
}

function unwrapRustErrorMessage(message: string): string {
  const upstreamRequest = message.match(/^UpstreamRequest\("([\s\S]*)"\)$/)
  if (!upstreamRequest) return message
  return upstreamRequest[1]
    .replace(/\\"/g, '"')
    .replace(/\\n/g, '\n')
    .trim()
}

const FAILURE_TYPE_LABELS: Record<string, string> = {
  local_stream_candidate_watchdog_timeout: '本地流式候选首字超时',
  stream_missing_terminal_event: '流式响应缺少结束事件',
  stream_terminal_error: '流式响应结束异常',
  stream_http_error: '流式上游 HTTP 错误',
  stream_missing_terminal_event_after_usage: '流式响应计费后缺少结束事件',
  execution_runtime_unavailable: '执行通道不可用',
  execution_runtime_http_error: '执行通道 HTTP 错误',
  execution_runtime_stream_non_success_status: '上游流式响应返回非成功状态',
  downstream_disconnect: '客户端连接已断开',
  success_failover_pattern: '成功响应触发备用路径',
  retryable_upstream_status: '上游返回可重试错误',
  control_fallback: '调度切换到备用通道',
  upstream_timeout: '上游请求超时',
  upstream_request_timeout: '上游请求超时',
  stream_first_byte_timeout: '流式首字超时',
  insufficient_quota: '额度不足',
  rate_limit_exceeded: '请求频率超限',
  invalid_request_error: '请求参数错误',
  authentication_error: '认证失败',
  permission_error: '权限不足',
  model_not_found: '模型不存在',
  not_found: '资源不存在',
  server_error: '上游服务异常',
  grok_execution_unavailable: 'Grok 执行通道不可用',
  windsurf_native_execution_unavailable: 'Windsurf 原生执行通道不可用',
  kiro_web_search_mcp_unavailable: 'Kiro Web Search MCP 不可用',
  chatgpt_web_image_execution_unavailable: 'ChatGPT Web 图片执行通道不可用',
}

const FAILURE_CODE_LABELS: Record<string, string> = {
  context_length_exceeded: '上下文长度超出限制',
  max_context_length_exceeded: '上下文长度超出限制',
  prompt_too_long: '输入内容过长',
  request_too_large: '请求内容过大',
  model_not_found: '模型不存在',
  not_found: '资源不存在',
  invalid_request_error: '请求参数错误',
  invalid_api_key: 'API Key 无效',
  authentication_error: '认证失败',
  permission_error: '权限不足',
  insufficient_quota: '额度不足',
  rate_limit_exceeded: '请求频率超限',
  content_policy_violation: '内容安全策略拦截',
  server_error: '上游服务异常',
}

function looksInternalErrorType(value: string): boolean {
  return /^[a-z][a-z0-9_]+$/.test(value)
}

function inferInternalFailureTypeLabel(value: string): string | null {
  if (!looksInternalErrorType(value)) return null
  if (value.includes('timeout')) return '请求超时'
  if (value.includes('missing_terminal_event')) return '流式响应缺少结束事件'
  if (value.includes('terminal_error')) return '流式响应结束异常'
  if (value.includes('http_error') || value.includes('non_success_status')) return '上游 HTTP 错误'
  if (value.includes('unavailable')) return '执行通道不可用'
  if (value.includes('disconnect')) return '连接已断开'
  if (value.includes('fallback')) return '触发备用通道'
  return '内部执行错误'
}

export function formatFailureTypeLabel(value: string | null | undefined): string | null {
  const normalized = nonEmptyString(value)
  if (!normalized) return null
  const key = normalized.toLowerCase()
  return FAILURE_TYPE_LABELS[key] ?? inferInternalFailureTypeLabel(normalized) ?? normalized
}

export function formatFailureCodeLabel(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const normalized = nonEmptyString(value)
  if (!normalized) return null
  const key = normalized.toLowerCase()
  return FAILURE_CODE_LABELS[key] ?? FAILURE_TYPE_LABELS[key] ?? inferInternalFailureTypeLabel(normalized)
}

export function resolveFailureReason(input: {
  message?: string | null
  type?: string | null
  code?: unknown
  statusCode?: number | null
}): string | null {
  const codeLabel = formatFailureCodeLabel(input.code)
  if (codeLabel) return codeLabel

  const message = normalizeFailureMessage(input.message, input.statusCode)
  if (message) return message

  return formatFailureTypeLabel(input.type)
}

export function normalizeFailureMessage(message: string | null | undefined, statusCode?: number | null): string | null {
  const normalized = nonEmptyString(message)
  const unwrapped = normalized ? unwrapRustErrorMessage(normalized) : null
  if (!unwrapped) return null

  const firstByteTimeout = unwrapped.match(/provider stream first byte timeout after\s+(\d+)\s*ms/i)
    ?? unwrapped.match(/stream first byte timeout after\s+(\d+)\s*ms/i)
  if (firstByteTimeout) {
    return `请求超时（${formatSecondsFromMsText(firstByteTimeout[1])}）`
  }

  const genericTimeout = unwrapped.match(/(?:request|upstream request|operation)\s+timeout(?:ed)?\s+after\s+(\d+)\s*ms/i)
    ?? unwrapped.match(/timeout(?:ed)?\s+after\s+(\d+)\s*ms/i)
  if (genericTimeout) {
    return `请求超时（${formatSecondsFromMsText(genericTimeout[1])}）`
  }

  if (/stream first byte timeout/i.test(unwrapped)) {
    return '请求超时（等待上游首字超时）'
  }

  if (/execution runtime (stream )?returned non-success status \d+/i.test(unwrapped)) {
    return statusCode != null ? `上游返回非成功状态 ${statusCode}` : '上游返回非成功状态'
  }

  const chineseTimeout = unwrapped.match(/^请求超时[（(]\s*(\d+(?:\.\d+)?)\s*秒\s*[）)]$/)
  if (chineseTimeout) {
    return `请求超时（${chineseTimeout[1]}秒）`
  }

  return unwrapped
}

export function isHttpLikeErrorCode(value: unknown): boolean {
  if (typeof value === 'number') {
    return Number.isInteger(value) && value >= 100 && value <= 599
  }
  if (typeof value === 'string') {
    const trimmed = value.trim()
    return /^\d{3}$/.test(trimmed) && Number(trimmed) >= 100 && Number(trimmed) <= 599
  }
  return false
}
