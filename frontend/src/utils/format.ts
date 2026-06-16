const COMPACT_NUMBER_UNITS = [
  { value: 1_000_000_000_000, suffix: 'T' },
  { value: 1_000_000_000, suffix: 'B' },
  { value: 1_000_000, suffix: 'M' },
  { value: 1_000, suffix: 'K' },
] as const

interface CompactNumberOptions {
  fractionDigits?: number
  nullLabel?: string
}

function trimTrailingDecimalZeros(value: string): string {
  return value.replace(/\.0+$/, '').replace(/(\.\d*?)0+$/, '$1')
}

function compactFractionDigits(scaled: number, fixedFractionDigits?: number): number {
  if (fixedFractionDigits !== undefined) return fixedFractionDigits
  if (scaled >= 100) return 0
  if (scaled >= 10) return 1
  return 2
}

function formatCompactScaledValue(
  absValue: number,
  unitIndex: number,
  fixedFractionDigits?: number,
): string {
  const unit = COMPACT_NUMBER_UNITS[unitIndex]
  const scaled = absValue / unit.value
  const fractionDigits = compactFractionDigits(scaled, fixedFractionDigits)
  const rounded = Number(scaled.toFixed(fractionDigits))

  if (rounded >= 1000 && unitIndex > 0) {
    return formatCompactScaledValue(absValue, unitIndex - 1, fixedFractionDigits)
  }

  return `${trimTrailingDecimalZeros(scaled.toFixed(fractionDigits))}${unit.suffix}`
}

export function formatCompactNumber(
  num: number | undefined | null,
  options: CompactNumberOptions = {},
): string {
  if (num === undefined || num === null) {
    return options.nullLabel ?? '0'
  }

  const value = Number(num)
  if (!Number.isFinite(value)) {
    return options.nullLabel ?? '0'
  }

  const sign = value < 0 ? '-' : ''
  const absValue = Math.abs(value)

  if (absValue < 1_000) {
    return `${sign}${Number.isInteger(absValue) ? absValue.toString() : trimTrailingDecimalZeros(absValue.toFixed(1))}`
  }

  const unitIndex = COMPACT_NUMBER_UNITS.findIndex(unit => absValue >= unit.value)
  if (unitIndex === -1) {
    return `${sign}${Math.round(absValue)}`
  }

  return `${sign}${formatCompactScaledValue(absValue, unitIndex, options.fractionDigits)}`
}

export function formatByteSize(bytes: number | undefined | null): string {
  if (bytes === undefined || bytes === null || !Number.isFinite(bytes)) {
    return '-'
  }

  const absBytes = Math.max(0, Math.abs(bytes))
  const units = [
    { value: 1024 ** 3, suffix: 'GB' },
    { value: 1024 ** 2, suffix: 'MB' },
    { value: 1024, suffix: 'KB' },
  ] as const
  const unit = units.find(candidate => absBytes >= candidate.value) ?? units[2]
  const scaled = absBytes / unit.value
  const fractionDigits = scaled >= 100 ? 0 : scaled >= 10 ? 1 : 2
  const formatted = trimTrailingDecimalZeros(scaled.toFixed(fractionDigits))

  return `${bytes < 0 ? '-' : ''}${formatted} ${unit.suffix}`
}

// Token formatting - intelligent display based on value size
export function formatTokens(num: number | undefined | null): string {
  return formatCompactNumber(num)
}

// Currency formatting with high precision for small values
export function formatCurrency(amount: number | undefined | null): string {
  if (amount === undefined || amount === null || amount === 0) {
    return '$0.00'
  }

  // For very small amounts (< $0.00001), show up to 8 decimal places
  if (amount > 0 && amount < 0.00001) {
    const formatted = amount.toFixed(8)
    // Remove trailing zeros but keep at least 2 decimal places
    const trimmed = formatted.replace(/(\.\d\d)0+$/, '$1')
    return `$${  trimmed}`
  }

  // For small amounts (< $0.0001), show up to 6 decimal places
  if (amount < 0.0001) {
    const formatted = amount.toFixed(6)
    // Remove trailing zeros but keep at least 2 decimal places
    const trimmed = formatted.replace(/(\.\d\d)0+$/, '$1')
    return `$${  trimmed}`
  }

  // For small amounts (< $0.01), show up to 5 decimal places
  if (amount < 0.01) {
    const formatted = amount.toFixed(5)
    // Remove trailing zeros but keep at least 2 decimal places
    const trimmed = formatted.replace(/(\.\d\d)0+$/, '$1')
    return `$${  trimmed}`
  }

  // For amounts less than $1, show 4 decimal places
  if (amount < 1) {
    const formatted = amount.toFixed(4)
    // Remove trailing zeros but keep at least 2 decimal places
    const trimmed = formatted.replace(/(\.\d\d)0+$/, '$1')
    return `$${  trimmed}`
  }

  // For amounts $1-$100, show 2-3 decimal places
  if (amount < 100) {
    const formatted = amount.toFixed(3)
    // Remove trailing zeros but keep at least 2 decimal places
    const trimmed = formatted.replace(/(\.\d\d)0+$/, '$1')
    return `$${  trimmed}`
  }

  // For larger amounts, show 2 decimal places
  return `$${  amount.toFixed(2)}`
}

// Number formatting with locale support
export function formatNumber(num: number | undefined | null): string {
  if (num === undefined || num === null) {
    return '0'
  }
  return num.toLocaleString('zh-CN')
}

// Date formatting
export function formatDate(dateString: string | undefined | null): string {
  if (!dateString) return '未知'

  return new Date(dateString).toLocaleDateString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit'
  })
}

// Model price formatting (already in per 1M tokens)
export function formatModelPrice(price: number | undefined | null): string {
  if (price === undefined || price === null) {
    return '$0.00'
  }

  // Price is already per 1M tokens, no conversion needed
  if (price < 1) {
    return `$${  price.toFixed(4).replace(/\.?0+$/, '').padEnd(price.toFixed(4).indexOf('.') + 3, '0')}`
  } else {
    return `$${  price.toFixed(2)}`
  }
}

// Billing type formatting
export function formatBillingType(type: string | undefined | null): string {
  const typeMap: Record<string, string> = {
    'pay_as_you_go': '按量付费',
    'monthly_quota': '月卡配额',
    'free_tier': '免费套餐'
  }
  return typeMap[type || ''] || type || '按量付费'
}

// Format cost with 4 decimal places (for cache analysis)
export function formatCost(cost: number | null | undefined): string {
  if (cost === null || cost === undefined) return '-'
  return `$${cost.toFixed(4)}`
}

// Usage count formatting (compact display for large numbers)
export function formatUsageCount(count: number): string {
  return formatCompactNumber(count, { fractionDigits: 1 })
}

// Format remaining time from unix timestamp
export function formatRemainingTime(expireAt: number | undefined, currentTime: number): string {
  if (!expireAt) return '未知'
  const remaining = expireAt - currentTime
  if (remaining <= 0) return '已过期'

  const minutes = Math.floor(remaining / 60)
  const seconds = Math.floor(remaining % 60)
  return `${minutes}分${seconds}秒`
}

// Cache hit rate formatting
export function formatHitRate(rate: number | undefined): string {
  if (typeof rate !== 'number' || Number.isNaN(rate)) return '-'
  return `${rate.toFixed(2)}%`
}

// Rate limit formatting (supports "inherit" semantics: null = inherit system default)
export function formatRateLimitInheritable(rateLimit?: number | null): string {
  if (rateLimit == null) return '跟随系统'
  if (rateLimit === 0) return '不限速'
  return `${rateLimit}/min`
}

// Rate limit formatting (simple: null/0 both mean unlimited)
export function formatRateLimitSimple(rateLimit?: number | null): string {
  if (rateLimit == null || rateLimit === 0) return '不限速'
  return `${rateLimit}/min`
}

// Rate limit state helpers
export function isRateLimitInherited(rateLimit?: number | null): boolean {
  return rateLimit == null
}

export function isRateLimitUnlimited(rateLimit?: number | null): boolean {
  return rateLimit === 0
}

export function formatShortRequestId(value: string | null | undefined): string {
  const trimmed = value?.trim()
  if (!trimmed) return '-'
  if (trimmed.length <= 12) return trimmed

  const uuidLike = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(trimmed)
  if (uuidLike) {
    return trimmed.slice(0, 8)
  }

  return `${trimmed.slice(0, 6)}...${trimmed.slice(-4)}`
}
