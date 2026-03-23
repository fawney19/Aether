import type { AnalyticsLeaderboardItem } from '@/api/analytics'

export interface LeaderboardTableRow {
  rank: number
  id: string
  name: string
  requests: number
  tokens: number
  cost: number
  actualCost: number
}

export function mapAnalyticsLeaderboardItem(item: AnalyticsLeaderboardItem): LeaderboardTableRow {
  return {
    rank: item.rank,
    id: item.id,
    name: item.label,
    requests: item.requests_total,
    tokens: item.total_tokens,
    cost: item.total_cost_usd,
    actualCost: item.actual_total_cost_usd,
  }
}
