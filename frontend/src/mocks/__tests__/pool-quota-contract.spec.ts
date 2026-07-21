import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/config/demo', () => ({
  isDemoMode: () => true,
  DEMO_ACCOUNTS: {
    admin: { email: 'admin@demo.aether.io', password: 'demo123' },
    user: { email: 'user@demo.aether.io', password: 'demo123' },
  },
}))

import type { QuotaWindowSnapshot } from '@/api/endpoints/types'
import { getCodexQuotaWindowPresentation } from '@/utils/codexQuotaWindow'
import { handleMockRequest, setMockUserToken } from '../handler'

interface MockPoolKey {
  key_id: string
  oauth_plan_type: string
  status_snapshot: {
    quota: {
      windows: QuotaWindowSnapshot[]
    }
  }
}

describe('pool quota demo contracts', () => {
  beforeEach(() => {
    setMockUserToken('demo-access-token-admin')
  })

  it('exposes a dedicated Codex pool in the overview and provider summary', async () => {
    const overviewResponse = await handleMockRequest({
      method: 'GET',
      url: '/api/admin/pool/overview',
    })
    const overview = overviewResponse?.data as {
      items: Array<{ provider_id: string; provider_type: string; total_keys: number }>
    }
    const provider = overview.items[0]

    expect(provider).toMatchObject({
      provider_id: 'provider-codex-pool-demo',
      provider_type: 'codex',
      total_keys: 4,
    })

    const summaryResponse = await handleMockRequest({
      method: 'GET',
      url: `/api/admin/providers/${provider.provider_id}/summary`,
    })
    expect(summaryResponse?.data).toMatchObject({
      id: provider.provider_id,
      provider_type: 'codex',
      name: 'Codex 周期额度演示',
    })
  })

  it('covers dual, weekly-only, monthly-only, and 5H-only quota windows', async () => {
    const response = await handleMockRequest({
      method: 'GET',
      url: '/api/admin/pool/provider-codex-pool-demo/keys',
      params: { page: 1, page_size: 50, status: 'all' },
    })
    const page = response?.data as { total: number; keys: MockPoolKey[] }
    const keys = new Map(page.keys.map(key => [key.key_id, key]))
    const labelsFor = (keyId: string) => keys.get(keyId)?.status_snapshot.quota.windows
      .map(getCodexQuotaWindowPresentation)
      .filter((item): item is NonNullable<typeof item> => item != null)
      .sort((left, right) => left.sortOrder - right.sortOrder)
      .map(item => item.label)

    expect(page.total).toBe(4)
    expect(labelsFor('codex-pool-plus-dual')).toEqual(['5H', '周'])
    expect(labelsFor('codex-pool-team-weekly')).toEqual(['周'])
    expect(labelsFor('codex-pool-business-monthly')).toEqual(['月'])
    expect(labelsFor('codex-pool-free-five-hour')).toEqual(['5H'])
    expect(keys.get('codex-pool-business-monthly')?.oauth_plan_type)
      .toBe('self_serve_business_usage_based')
  })
})
