import { describe, expect, it } from 'vitest'
import type { Sub2ApiRemoteQuotaGroup } from '@/api/providerOps'
import {
  formatSub2ApiGroupOption,
  formatSub2ApiWindow,
  localSyncWindowText,
} from '../sub2ApiQuota'

const group: Sub2ApiRemoteQuotaGroup = {
  group_id: '42',
  group_name: 'Pro',
  subscription_id: '9',
  daily_limit_usd: 10,
  daily_used_usd: 1.5,
  weekly_limit_usd: 50,
  weekly_used_usd: 4.5,
  monthly_limit_usd: 0,
  monthly_used_usd: 0,
  local_sync_window: 'daily',
  expires_at_unix_secs: null,
}

describe('Sub2API quota presentation', () => {
  it('shows finite daily and weekly windows even when monthly is unlimited', () => {
    expect(formatSub2ApiWindow(group, 'daily')).toBe('$1.5 / $10')
    expect(formatSub2ApiWindow(group, 'weekly')).toBe('$4.5 / $50')
    expect(formatSub2ApiWindow(group, 'monthly')).toBe('不限')

    const label = formatSub2ApiGroupOption(group)
    expect(label).toBe('Pro · 日额度')
  })

  it('explains which finite window is synchronized locally', () => {
    expect(localSyncWindowText('daily')).toBe('日额度')
    expect(localSyncWindowText(null)).toBe('不限额')
  })
})
