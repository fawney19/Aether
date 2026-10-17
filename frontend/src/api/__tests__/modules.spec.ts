import { beforeEach, describe, expect, it, vi } from 'vitest'

const { get } = vi.hoisted(() => ({ get: vi.fn() }))
vi.mock('@/api/client', () => ({ default: { get } }))
import { modulesApi } from '@/api/modules'

describe('user module status API', () => {
  beforeEach(() => vi.resetAllMocks())

  it('loads minimal user status without calling the administrator endpoint', async () => {
    const statuses = { referral: { name: 'referral', available: true, enabled: true, active: true } }
    get.mockResolvedValue({ data: statuses })
    await expect(modulesApi.getUserStatus()).resolves.toEqual(statuses)
    expect(get).toHaveBeenCalledExactlyOnceWith('/api/modules/user-status')
  })
})
