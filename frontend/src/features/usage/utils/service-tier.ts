export interface ServiceTierFacts {
  requested: string | null
}

export interface ServiceTierFactSource {
  service_tier?: unknown
}

export function resolveServiceTierFacts(
  source: ServiceTierFactSource | null | undefined,
): ServiceTierFacts {
  // The processing tier is an input-side fact: it must come from the final
  // request body sent to the provider. Response-advertised tiers and old
  // settlement snapshots can describe a different/legacy value, so they are
  // deliberately not consulted here. The billing display uses this same
  // authoritative request tier.
  return {
    requested: normalizeServiceTierFact(source?.service_tier),
  }
}

export function hasServiceTierFact(facts: ServiceTierFacts): boolean {
  return facts.requested !== null
}

export function normalizeServiceTierFact(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const normalized = value.trim()
  return normalized || null
}

/**
 * Provider contracts use both `priority` (OpenAI) and `fast` (Claude) for the
 * same user-facing processing mode. Keep the raw fact in data structures and
 * normalize only at the presentation boundary.
 */
export function formatServiceTierFact(value: unknown): string | null {
  const normalized = normalizeServiceTierFact(value)
  if (normalized === null) return null

  const canonical = normalized.toLowerCase()
  return canonical === 'priority' || canonical === 'fast'
    ? 'Fast'
    : normalized
}
