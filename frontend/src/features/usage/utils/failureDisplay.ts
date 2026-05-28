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
