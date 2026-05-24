import type { UsageByProvider } from '@/api/usage'
import type { ProviderStatsItem } from '../types'

function metricValue(value: number | null | undefined): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

export function isUsageProviderVisible(provider: string | undefined | null): provider is string {
  const normalized = provider?.trim().toLowerCase()
  return !!normalized && !['unknown', 'unknow', 'pending'].includes(normalized)
}

export function formatProviderAverageResponseTime(avgResponseTimeMs: number | null | undefined): string {
  const value = metricValue(avgResponseTimeMs)
  return value > 0 ? `${(value / 1000).toFixed(2)}s` : '-'
}

export function normalizeUsageProviderStats(providerData: UsageByProvider[]): ProviderStatsItem[] {
  return providerData
    .filter(item => isUsageProviderVisible(item.provider))
    .map(item => ({
      providerId: item.provider_id,
      providerKey: item.provider_key,
      providerIdentitySource: item.provider_identity_source,
      provider: item.provider,
      requests: metricValue(item.request_count),
      totalTokens: metricValue(item.total_tokens),
      effectiveInputTokens: metricValue(item.effective_input_tokens),
      totalInputContext: metricValue(item.total_input_context),
      outputTokens: metricValue(item.output_tokens),
      cacheReadTokens: metricValue(item.cache_read_tokens),
      cacheCreationTokens: metricValue(item.cache_creation_tokens),
      cacheHitRate: metricValue(item.cache_hit_rate),
      totalCost: metricValue(item.total_cost),
      actualCost: typeof item.actual_cost === 'number' && Number.isFinite(item.actual_cost)
        ? item.actual_cost
        : undefined,
      successRate: metricValue(item.success_rate),
      avgResponseTime: formatProviderAverageResponseTime(item.avg_response_time_ms),
    }))
}