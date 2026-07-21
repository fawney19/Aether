import { beforeEach, describe, expect, it, vi } from 'vitest'

const { getMock, postMock, cachedRequestMock } = vi.hoisted(() => ({
  getMock: vi.fn(),
  postMock: vi.fn(),
  cachedRequestMock: vi.fn(async (_key: string, fn: () => Promise<unknown>) => fn()),
}))

vi.mock('@/api/client', () => ({
  default: {
    get: getMock,
    post: postMock,
  },
}))

vi.mock('@/utils/cache', () => ({
  cachedRequest: cachedRequestMock,
}))

import { usersApi } from '@/api/users'

describe('usersApi admin list query', () => {
  beforeEach(() => {
    getMock.mockReset()
    postMock.mockReset()
    cachedRequestMock.mockClear()
    getMock.mockResolvedValue({
      data: {
        items: [],
        total: 0,
        skip: 0,
        limit: 20,
        has_more: false,
      },
    })
  })

  it('passes creation-time sort parameters to the admin users endpoint', async () => {
    await usersApi.getAllUsersPage({
      skip: 20,
      limit: 10,
      sort_by: 'created_at',
      sort_order: 'desc',
    })

    expect(getMock).toHaveBeenCalledWith('/api/admin/users', {
      params: {
        skip: 20,
        limit: 10,
        sort_by: 'created_at',
        sort_order: 'desc',
      },
    })
  })

  it('keeps user management renderable when the group response has no items array', async () => {
    getMock.mockResolvedValueOnce({
      data: {
        message: '演示模式：该接口暂未模拟',
        demo_mode: true,
      },
    })

    await expect(usersApi.listUserGroups()).resolves.toEqual({
      message: '演示模式：该接口暂未模拟',
      demo_mode: true,
      items: [],
    })
  })

  it('creates a managed key through the selected target user route', async () => {
    postMock.mockResolvedValueOnce({
      data: {
        id: 'target-key',
        key: 'sk-target',
      },
    })
    const payload = {
      name: 'target key',
      feature_settings: {
        chat_pii_redaction: { enabled: true },
      },
    }

    await usersApi.createApiKey('target-user', payload)

    expect(postMock).toHaveBeenCalledWith(
      '/api/admin/users/target-user/api-keys',
      payload,
    )
  })

  it('reads managed keys from the production api_keys envelope', async () => {
    getMock.mockResolvedValueOnce({
      data: {
        api_keys: [{ id: 'target-key', created_at: '2026-07-17T00:00:00Z' }],
        total: 1,
      },
    })

    await expect(usersApi.getUserApiKeys('target-user')).resolves.toEqual([
      { id: 'target-key', created_at: '2026-07-17T00:00:00Z' },
    ])
    expect(getMock).toHaveBeenCalledWith('/api/admin/users/target-user/api-keys')
  })
})
