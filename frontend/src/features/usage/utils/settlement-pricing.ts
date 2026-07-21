interface SettlementPricingSource {
  settlement?: unknown
  tiered_pricing?: unknown
}

type JsonRecord = Record<string, unknown>

const PRICING_SOURCE_LABELS: Record<string, string> = {
  provider_override: '提供商定价',
  global_default: '全局定价',
  mixed: '混合定价',
  provider: '提供商定价',
  global: '全局定价',
  unpriced: '未定价',
}

export function resolveSettlementPricingSnapshot(
  source: SettlementPricingSource | null | undefined,
): JsonRecord | null {
  const settlement = asRecord(source?.settlement)
  const settlementSnapshot = asRecord(settlement?.settlement_snapshot)
  return asRecord(settlementSnapshot?.pricing_snapshot)
}

export function resolveSettlementPricingSourceLabel(
  source: SettlementPricingSource | null | undefined,
): string | null {
  const snapshot = resolveSettlementPricingSnapshot(source)
  const legacyPricing = asRecord(source?.tiered_pricing)
  for (const value of [
    snapshot?.pricing_source,
    snapshot?.tiered_pricing_source,
    legacyPricing?.source,
  ]) {
    const key = normalizeString(value)?.toLowerCase()
    if (key && PRICING_SOURCE_LABELS[key]) return PRICING_SOURCE_LABELS[key]
  }
  return null
}

export function resolveSettlementPricingTiers(
  source: SettlementPricingSource | null | undefined,
): JsonRecord[] | null {
  const snapshot = resolveSettlementPricingSnapshot(source)
  const snapshotPricing = asRecord(snapshot?.tiered_pricing)
  const snapshotTiers = nonEmptyRecordArray(snapshotPricing?.tiers)
  if (snapshotTiers) return snapshotTiers

  const legacyPricing = asRecord(source?.tiered_pricing)
  return nonEmptyRecordArray(legacyPricing?.tiers)
}

export function resolveProcessingTierPriceMultiplier(
  source: SettlementPricingSource | null | undefined,
): number | null {
  const value = resolveSettlementPricingSnapshot(source)?.processing_tier_price_multiplier
  return typeof value === 'number' && Number.isFinite(value) && value >= 0
    ? value
    : null
}

export function formatPricePerMillion(value: unknown): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) return '-'

  const fixed = value.toFixed(4)
  return `$${Number.parseFloat(fixed).toString()}/M`
}

function normalizeString(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const normalized = value.trim()
  return normalized || null
}

function nonEmptyRecordArray(value: unknown): JsonRecord[] | null {
  if (!Array.isArray(value) || value.length === 0) return null
  const records = value.filter((item): item is JsonRecord => asRecord(item) !== null)
  return records.length > 0 ? records : null
}

function asRecord(value: unknown): JsonRecord | null {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? value as JsonRecord
    : null
}
