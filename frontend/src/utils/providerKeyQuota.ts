import type {
  ProviderKeyStatusSnapshot,
  QuotaStatusSnapshot,
  QuotaWindowSnapshot,
} from '@/api/endpoints/types/statusSnapshot'
import type { UpstreamMetadata } from '@/api/endpoints/types/provider'

export interface ProviderKeyQuotaCarrier {
  account_quota?: string | null
  status_snapshot?: ProviderKeyStatusSnapshot | null
  upstream_metadata?: UpstreamMetadata | null
}

function normalizeText(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const text = value.trim()
  return text || null
}

function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0
  if (value < 0) return 0
  if (value > 100) return 100
  return value
}

function formatPercent(value: number): string {
  return `${clampPercent(value).toFixed(1)}%`
}

function getQuotaSnapshot(
  input: ProviderKeyQuotaCarrier,
): QuotaStatusSnapshot | null {
  return input.status_snapshot?.quota ?? null
}

function getQuotaProviderType(
  quota: QuotaStatusSnapshot | null | undefined,
  fallbackProviderType?: string | null,
): string {
  const snapshotProviderType = normalizeText(quota?.provider_type)?.toLowerCase()
  if (snapshotProviderType) return snapshotProviderType
  return normalizeText(fallbackProviderType)?.toLowerCase() || ''
}

function getQuotaWindows(
  quota: QuotaStatusSnapshot | null | undefined,
): QuotaWindowSnapshot[] {
  return Array.isArray(quota?.windows) ? quota.windows : []
}

function getQuotaWindowRemainingPercent(
  window: QuotaWindowSnapshot | null | undefined,
): number | null {
  if (!window) return null
  if (typeof window.remaining_ratio === 'number') {
    return clampPercent(window.remaining_ratio * 100)
  }
  if (typeof window.used_ratio === 'number') {
    return clampPercent((1 - window.used_ratio) * 100)
  }
  if (typeof window.limit_value === 'number' && window.limit_value > 0) {
    if (typeof window.remaining_value === 'number') {
      return clampPercent((window.remaining_value / window.limit_value) * 100)
    }
    if (typeof window.used_value === 'number') {
      return clampPercent((1 - (window.used_value / window.limit_value)) * 100)
    }
  }
  return null
}

function getQuotaWindow(
  quota: QuotaStatusSnapshot | null | undefined,
  code: string,
): QuotaWindowSnapshot | null {
  const normalizedCode = code.trim().toLowerCase()
  return getQuotaWindows(quota).find(window => normalizeText(window.code)?.toLowerCase() === normalizedCode) ?? null
}

function finiteNumber(value: unknown): number | null {
  if (typeof value === 'number') return Number.isFinite(value) ? value : null
  if (typeof value === 'string') {
    const parsed = Number(value.trim())
    return Number.isFinite(parsed) ? parsed : null
  }
  return null
}

function firstFiniteNumber(...values: unknown[]): number | null {
  for (const value of values) {
    const parsed = finiteNumber(value)
    if (parsed != null) return parsed
  }
  return null
}

function objectValue(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null
}

function positiveNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : null
}

function windsurfCooldownHasPositiveReset(quota: QuotaStatusSnapshot): boolean {
  const rateLimit = quota.rate_limit
  if (rateLimit && typeof rateLimit === 'object') {
    const retryAfterMs = positiveNumber(rateLimit.retry_after_ms) ?? positiveNumber(rateLimit.retryAfterMs)
    if (retryAfterMs != null) return true
  }

  const rateLimitWindow = getQuotaWindow(quota, 'rate_limit')
  return (
    positiveNumber(rateLimitWindow?.reset_seconds) != null
    || positiveNumber(rateLimitWindow?.reset_at) != null
  )
}

function getQuotaWindowsByScope(
  quota: QuotaStatusSnapshot | null | undefined,
  scope: string,
): QuotaWindowSnapshot[] {
  const normalizedScope = scope.trim().toLowerCase()
  return getQuotaWindows(quota).filter(window => normalizeText(window.scope)?.toLowerCase() === normalizedScope)
}

function formatQuotaValue(value: number | null | undefined): string {
  const normalized = Number(value)
  if (!Number.isFinite(normalized)) return '0'
  const rounded = Math.round(normalized)
  if (Math.abs(normalized - rounded) < 1e-6) {
    return String(rounded)
  }
  return normalized.toFixed(1)
}

function getQuotaWindowValueText(window: QuotaWindowSnapshot | null | undefined): string | null {
  if (!window || typeof window.limit_value !== 'number' || window.limit_value <= 0) return null
  if (typeof window.remaining_value === 'number') {
    return `${formatQuotaValue(window.remaining_value)}/${formatQuotaValue(window.limit_value)}`
  }
  if (typeof window.used_value === 'number') {
    return `${formatQuotaValue(Math.max(window.limit_value - window.used_value, 0))}/${formatQuotaValue(window.limit_value)}`
  }
  return null
}

function getGeminiCliCreditsTextFromQuota(quota: QuotaStatusSnapshot | null | undefined): string | null {
  const credits = quota?.credits
  if (!credits) return null
  const remaining = firstFiniteNumber(credits.remaining, credits.balance)
  if (remaining != null) return `AI Credits 剩余 ${formatQuotaValue(remaining)}`
  if (credits.unlimited === true) return 'AI Credits 不限量'
  if (credits.has_credits === true) return 'AI Credits 可用'
  if (credits.has_credits === false) return 'AI Credits 已用尽'
  return null
}

export function getGeminiCliAccountCreditsText(
  input: ProviderKeyQuotaCarrier,
  fallbackProviderType?: string | null,
): string | null {
  const quota = getQuotaSnapshot(input)
  if (getQuotaProviderType(quota, fallbackProviderType) !== 'gemini_cli') return null

  const quotaText = getGeminiCliCreditsTextFromQuota(quota)
  if (quotaText) return quotaText

  const metadata = input.upstream_metadata?.gemini_cli
  const credits = objectValue(metadata?.credits)
  const paidTier = objectValue(metadata?.paidTier)
  const currentTier = objectValue(metadata?.currentTier)
  const remaining = firstFiniteNumber(
    credits?.remaining,
    credits?.remainingCredits,
    credits?.available,
    credits?.availableCredits,
    credits?.balance,
    paidTier?.availableCredits,
    paidTier?.remainingCredits,
    currentTier?.availableCredits,
    currentTier?.remainingCredits,
  )
  if (remaining != null) return `AI Credits 剩余 ${formatQuotaValue(remaining)}`

  const hasCredits = typeof credits?.has_credits === 'boolean'
    ? credits.has_credits
    : typeof paidTier?.hasCredits === 'boolean'
      ? paidTier.hasCredits
      : typeof currentTier?.hasCredits === 'boolean'
        ? currentTier.hasCredits
        : null
  if (hasCredits === true) return 'AI Credits 可用'
  if (hasCredits === false) return 'AI Credits 已用尽'

  const unlimited = credits?.unlimited === true || paidTier?.unlimited === true || currentTier?.unlimited === true
  return unlimited ? 'AI Credits 不限量' : null
}

const GROK_QUOTA_MODE_LABELS: Record<string, string> = {
  quota_auto: 'Auto',
  auto: 'Auto',
  quota_fast: 'Fast',
  fast: 'Fast',
  quota_expert: 'Expert',
  expert: 'Expert',
  quota_heavy: 'Heavy',
  heavy: 'Heavy',
  quota_grok_4_3: 'Grok 4.3',
  'grok-420-computer-use-sa': 'Grok 4.3',
}

function getGrokQuotaWindowLabel(window: QuotaWindowSnapshot): string {
  const rawCode = normalizeText(window.code)?.replace(/^model:/i, '') || ''
  const rawLabel = normalizeText(window.label) || normalizeText(window.model) || rawCode
  const normalized = (rawLabel || rawCode).trim().toLowerCase()
  return GROK_QUOTA_MODE_LABELS[normalized] || GROK_QUOTA_MODE_LABELS[rawCode.toLowerCase()] || rawLabel || rawCode || '模式'
}

function getCodexQuotaText(quota: QuotaStatusSnapshot): string | null {
  const parts: string[] = []
  for (const [label, code] of [
    ['周', 'weekly'],
    ['5H', '5h'],
    ['Spark5H', 'spark_5h'],
    ['Spark周', 'spark_weekly'],
  ] as const) {
    const remainingPercent = getQuotaWindowRemainingPercent(getQuotaWindow(quota, code))
    if (remainingPercent == null) continue
    parts.push(`${label}剩余 ${formatPercent(remainingPercent)}`)
  }
  if (parts.length > 0) return parts.join(' | ')

  if (quota.credits?.has_credits === true && typeof quota.credits.balance === 'number') {
    return `积分 ${quota.credits.balance.toFixed(2)}`
  }
  if (quota.credits?.has_credits === true) return '有积分'
  if (quota.credits?.has_credits === false) return '无可用积分'

  return normalizeText(quota.label)
}

function getKiroQuotaText(quota: QuotaStatusSnapshot): string | null {
  const code = normalizeText(quota.code)?.toLowerCase()
  if (code === 'banned') {
    return normalizeText(quota.label) || '账号已封禁'
  }

  const window = getQuotaWindow(quota, 'usage') ?? getQuotaWindowsByScope(quota, 'account')[0] ?? null
  const remainingPercent = getQuotaWindowRemainingPercent(window)
  if (typeof window?.remaining_value === 'number' && typeof window.limit_value === 'number' && window.limit_value > 0 && window.remaining_value <= 0) {
    return `剩余 ${formatQuotaValue(window.remaining_value)}/${formatQuotaValue(window.limit_value)}`
  }
  if (remainingPercent != null) {
    if (typeof window?.used_value === 'number' && typeof window.limit_value === 'number' && window.limit_value > 0) {
      return `剩余 ${formatPercent(remainingPercent)} (${formatQuotaValue(window.used_value)}/${formatQuotaValue(window.limit_value)})`
    }
    return `剩余 ${formatPercent(remainingPercent)}`
  }

  if (typeof window?.remaining_value === 'number' && typeof window.limit_value === 'number' && window.limit_value > 0) {
    return `剩余 ${formatQuotaValue(window.remaining_value)}/${formatQuotaValue(window.limit_value)}`
  }

  return normalizeText(quota.label)
}

function getGrokQuotaText(quota: QuotaStatusSnapshot): string | null {
  const code = normalizeText(quota.code)?.toLowerCase()
  if (code === 'banned') {
    return normalizeText(quota.label) || '账号已封禁'
  }
  if (code === 'forbidden') {
    return normalizeText(quota.label) || '访问受限'
  }

  const modelWindows = getQuotaWindowsByScope(quota, 'model')
  const modelParts = modelWindows
    .map((window) => {
      const remainingPercent = getQuotaWindowRemainingPercent(window)
      if (remainingPercent == null) return null
      const valueText = getQuotaWindowValueText(window)
      return `${getGrokQuotaWindowLabel(window)}剩余 ${formatPercent(remainingPercent)}${valueText ? ` (${valueText})` : ''}`
    })
    .filter((value): value is string => value != null)

  if (modelParts.length > 0) return modelParts.join(' | ')

  const window = getQuotaWindow(quota, 'usage') ?? getQuotaWindowsByScope(quota, 'account')[0] ?? null
  const remainingPercent = getQuotaWindowRemainingPercent(window)
  if (typeof window?.remaining_value === 'number' && typeof window.limit_value === 'number' && window.limit_value > 0 && window.remaining_value <= 0) {
    return `剩余 ${formatQuotaValue(window.remaining_value)}/${formatQuotaValue(window.limit_value)}`
  }
  if (remainingPercent != null) {
    const valueText = getQuotaWindowValueText(window)
    if (valueText) {
      return `剩余 ${formatPercent(remainingPercent)} (${valueText})`
    }
    return `剩余 ${formatPercent(remainingPercent)}`
  }

  if (typeof window?.remaining_value === 'number' && typeof window.limit_value === 'number' && window.limit_value > 0) {
    return `剩余 ${formatQuotaValue(window.remaining_value)}/${formatQuotaValue(window.limit_value)}`
  }

  return normalizeText(quota.label)
}

function getWindsurfQuotaText(quota: QuotaStatusSnapshot): string | null {
  const code = normalizeText(quota.code)?.toLowerCase()
  if (code === 'banned' || code === 'forbidden' || code === 'quarantined') {
    return normalizeText(quota.label) || '账号不可用'
  }
  if (code === 'cooldown' && windsurfCooldownHasPositiveReset(quota)) {
    return normalizeText(quota.label) || '冷却中'
  }
  if (code === 'rate_limited' || code === 'rate_limit') {
    return normalizeText(quota.label) || '速率受限'
  }
  if (code === 'exhausted') {
    return normalizeText(quota.label) || '额度已耗尽'
  }

  const parts: string[] = []
  const dailyRemaining = getQuotaWindowRemainingPercent(getQuotaWindow(quota, 'daily'))
  const weeklyRemaining = getQuotaWindowRemainingPercent(getQuotaWindow(quota, 'weekly'))
  if (dailyRemaining != null) parts.push(`日剩余 ${formatPercent(dailyRemaining)}`)
  if (weeklyRemaining != null) parts.push(`周剩余 ${formatPercent(weeklyRemaining)}`)

  for (const [label, code] of [
    ['Prompt', 'prompt'],
    ['Flex', 'flex'],
  ] as const) {
    const window = getQuotaWindow(quota, code)
    if (!window) continue
    if (typeof window.remaining_value === 'number' && typeof window.limit_value === 'number' && window.limit_value > 0) {
      parts.push(`${label} 剩余 ${formatQuotaValue(window.remaining_value)}/${formatQuotaValue(window.limit_value)}`)
      continue
    }
    if (typeof window.used_value === 'number' && typeof window.limit_value === 'number' && window.limit_value > 0) {
      parts.push(`${label} 剩余 ${formatQuotaValue(Math.max(window.limit_value - window.used_value, 0))}/${formatQuotaValue(window.limit_value)}`)
      continue
    }
    const remainingPercent = getQuotaWindowRemainingPercent(window)
    if (remainingPercent != null) {
      parts.push(`${label} 剩余 ${formatPercent(remainingPercent)}`)
    }
  }

  if (typeof quota.allowed_models_count === 'number') {
    parts.push(`可用模型 ${quota.allowed_models_count} 个`)
  }

  if (parts.length > 0) return parts.join(' | ')

  if (code === 'cooldown') {
    return normalizeText(quota.label) || '冷却中'
  }

  return normalizeText(quota.label)
}

function getAntigravityQuotaText(quota: QuotaStatusSnapshot): string | null {
  const code = normalizeText(quota.code)?.toLowerCase()
  if (code === 'forbidden') {
    return normalizeText(quota.label) || '访问受限'
  }

  const remainingList = getQuotaWindowsByScope(quota, 'model')
    .map(getQuotaWindowRemainingPercent)
    .filter((value): value is number => value != null)

  if (remainingList.length === 0) return normalizeText(quota.label)

  const minimumRemaining = Math.min(...remainingList)
  if (remainingList.length === 1) {
    return `剩余 ${formatPercent(minimumRemaining)}`
  }
  return `最低剩余 ${formatPercent(minimumRemaining)} (${remainingList.length} 模型)`
}

function getGeminiCliQuotaText(quota: QuotaStatusSnapshot): string | null {
  const creditsText = getGeminiCliCreditsTextFromQuota(quota)
  const modelWindows = getQuotaWindowsByScope(quota, 'model')
  const activeCoolingModels = modelWindows
    .filter((window) => {
      if (window.is_exhausted === true) return true
      if (typeof window.used_ratio === 'number') return window.used_ratio >= 1.0 - 1e-6
      return false
    })
    .filter((window) => {
      if (typeof window.reset_at !== 'number') return true
      return window.reset_at > Math.floor(Date.now() / 1000)
    })
    .map((window) => normalizeText(window.label) || normalizeText(window.model) || '模型')

  if (activeCoolingModels.length === 1) {
    return `${activeCoolingModels[0]} 冷却中`
  }
  if (activeCoolingModels.length > 1) {
    return `${activeCoolingModels.length} 个模型冷却中`
  }

  const remainingList = modelWindows
    .map(getQuotaWindowRemainingPercent)
    .filter((value): value is number => value != null)
  if (remainingList.length === 0) return creditsText || normalizeText(quota.label)

  const minimumRemaining = Math.min(...remainingList)
  if (creditsText) return creditsText
  if (remainingList.length === 1) {
    return `剩余 ${formatPercent(minimumRemaining)}`
  }
  return `最低剩余 ${formatPercent(minimumRemaining)} (${remainingList.length} 模型)`
}

function getChatGPTWebQuotaText(quota: QuotaStatusSnapshot): string | null {
  const window = getQuotaWindow(quota, 'image_gen') ?? getQuotaWindowsByScope(quota, 'account')[0] ?? null
  if (!window) return normalizeText(quota.label)

  const remainingPercent = getQuotaWindowRemainingPercent(window)
  if (typeof window.remaining_value === 'number' && typeof window.limit_value === 'number' && window.limit_value > 0) {
    return `生图剩余 ${formatQuotaValue(window.remaining_value)}/${formatQuotaValue(window.limit_value)}`
  }
  if (remainingPercent != null) {
    return `生图剩余 ${formatPercent(remainingPercent)}`
  }

  if (typeof window.remaining_value === 'number') {
    return `生图剩余 ${formatQuotaValue(window.remaining_value)}`
  }

  return normalizeText(quota.label)
}

export function getLegacyAccountQuotaText(
  input: ProviderKeyQuotaCarrier,
): string | null {
  return normalizeText(input.account_quota)
}

export function getQuotaSnapshotFallbackText(
  input: ProviderKeyQuotaCarrier,
  fallbackProviderType?: string | null,
): string | null {
  const quota = getQuotaSnapshot(input)
  if (!quota) return null

  const providerType = getQuotaProviderType(quota, fallbackProviderType)
  switch (providerType) {
    case 'codex':
      return getCodexQuotaText(quota)
    case 'kiro':
      return getKiroQuotaText(quota)
    case 'grok':
      return getGrokQuotaText(quota)
    case 'windsurf':
      return getWindsurfQuotaText(quota)
    case 'antigravity':
      return getAntigravityQuotaText(quota)
    case 'gemini_cli':
      return getGeminiCliQuotaText(quota)
    case 'chatgpt_web':
      return getChatGPTWebQuotaText(quota)
    default:
      return normalizeText(quota.label)
  }
}

export function getQuotaDisplayText(
  input: ProviderKeyQuotaCarrier,
  fallbackProviderType?: string | null,
): string | null {
  return getQuotaSnapshotFallbackText(input, fallbackProviderType) || getLegacyAccountQuotaText(input)
}
