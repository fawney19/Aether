import { beforeEach, describe, expect, it, vi } from 'vitest'

const { postMock } = vi.hoisted(() => ({
  postMock: vi.fn(),
}))

vi.mock('@/api/client', () => ({
  default: {
    post: postMock,
  },
}))

import { discoverSub2ApiGroups, type VerifyAuthRequest } from '@/api/providerOps'

const request: VerifyAuthRequest = {
  architecture_id: 'sub2api',
  base_url: 'https://sub2api.example',
  connector: {
    auth_type: 'api_key',
    config: {},
    credentials: { refresh_token: 'refresh-token' },
  },
  actions: {},
  schedule: {},
}

describe('Sub2API remote quota group discovery', () => {
  beforeEach(() => {
    postMock.mockReset()
    postMock.mockResolvedValue({
      data: {
        success: true,
        data: {
          extra: {
            sub2api_groups: [{
              group_id: '42',
              group_name: 'Pro',
              subscription_id: '9',
              daily_limit_usd: 10,
              daily_used_usd: 1.5,
              weekly_limit_usd: 50,
              weekly_used_usd: 4.5,
              monthly_limit_usd: 100,
              monthly_used_usd: 12.5,
              local_sync_window: 'daily',
              expires_at_unix_secs: 1896134400,
            }],
          },
        },
      },
    })
  })

  it('uses the verify surface with explicit group discovery enabled', async () => {
    const response = await discoverSub2ApiGroups('provider-1', request)

    expect(postMock).toHaveBeenCalledWith(
      '/api/admin/provider-ops/providers/provider-1/verify',
      {
        ...request,
        discover_sub2api_groups: true,
      },
    )
    expect(response.data?.extra?.sub2api_groups?.[0].group_id).toBe('42')
  })

})
