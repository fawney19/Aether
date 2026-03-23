export interface DailyUsageBreakdown {
  date: string
  inputTokens: number
  outputTokens: number
  cacheCreationTokens: number
  cacheReadTokens: number
  cacheCreationCost: number
  cacheReadCost: number
  cacheHitRate: number | null
  totalCacheCost: number
  totalCacheTokens: number
  totalTrackedTokens: number
  baseTokens: number
}
