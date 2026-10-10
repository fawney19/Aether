import { afterEach, describe, expect, it, vi } from 'vitest'
vi.mock('@/config/demo', () => ({
  isDemoMode: () => true,
  DEMO_ACCOUNTS: { admin: { email: 'admin@demo.aether.io', password: 'demo123' }, user: { email: 'user@demo.aether.io', password: 'demo123' } },
}))
import { handleMockRequest, setMockUserToken } from '../handler'
import { MOCK_MODULE_STATUSES, MOCK_SYSTEM_CONFIGS } from '../data'

describe('referral demo contract', () => {
  const originalConfigs = structuredClone(MOCK_SYSTEM_CONFIGS)
  const originalReferral = structuredClone(MOCK_MODULE_STATUSES.referral)
  afterEach(() => {
    MOCK_SYSTEM_CONFIGS.splice(0, MOCK_SYSTEM_CONFIGS.length, ...structuredClone(originalConfigs))
    MOCK_MODULE_STATUSES.referral = structuredClone(originalReferral)
    setMockUserToken(null)
  })

  it('keeps both administrator switches and the ordinary user status consistent', async () => {
    setMockUserToken('demo-access-token-admin')
    await handleMockRequest({ method: 'PUT', url: '/api/admin/system/configs/referral_enabled', data: JSON.stringify({ value: true }) })
    setMockUserToken('demo-access-token-user')
    const enabled = await handleMockRequest({ method: 'GET', url: '/api/modules/user-status' })
    expect(enabled?.data).toMatchObject({ referral: { enabled: true, active: true } })
    expect(Object.keys(enabled?.data as object).sort()).toEqual(['management_tokens', 'referral'])
    expect(Object.keys((enabled?.data as Record<string, object>).referral).sort()).toEqual(['active', 'available', 'enabled', 'name'])

    const dashboard = await handleMockRequest({ method: 'GET', url: '/api/users/me/referral' })
    expect(dashboard?.data).toMatchObject({ invite_code: 'DEMOUSER', invitation_link: '/register?invite=DEMOUSER', summary: { total_invites: 0 } })
    setMockUserToken('demo-access-token-admin')
    await handleMockRequest({ method: 'PUT', url: '/api/admin/modules/status/referral/enabled', data: JSON.stringify({ enabled: false }) })
    setMockUserToken('demo-access-token-user')
    const disabled = await handleMockRequest({ method: 'GET', url: '/api/modules/user-status' })
    expect(disabled?.data).toMatchObject({ referral: { enabled: false, active: false } })
    expect(MOCK_SYSTEM_CONFIGS.find(config => config.key === 'referral_enabled')?.value).toBe(false)
  })
  it('saves related referral rules together and rejects an invalid email dependency before modifying mock state', async () => {
    setMockUserToken('demo-access-token-admin')
    const invalid = { referral_enabled: true, referral_reward_mode: 'headcount', referral_headcount_trigger: 'email_verified', require_email_verification: false }
    await expect(handleMockRequest({ method: 'PUT', url: '/api/admin/system/referral-settings', data: JSON.stringify(invalid) })).rejects.toMatchObject({ response: { status: 400 } })
    expect(MOCK_SYSTEM_CONFIGS.find(item => item.key === 'referral_enabled')?.value).not.toBe(true)
    const saved = await handleMockRequest({ method: 'PUT', url: '/api/admin/system/referral-settings', data: JSON.stringify({ ...invalid, require_email_verification: true }) })
    expect(saved?.status).toBe(200)
    expect(MOCK_SYSTEM_CONFIGS.find(item => item.key === 'referral_headcount_trigger')?.value).toBe('email_verified')
    await expect(handleMockRequest({ method: 'PUT', url: '/api/admin/system/configs/require_email_verification', data: JSON.stringify({ value: false }) })).rejects.toMatchObject({ response: { status: 400 } })
  })

  it('provides protected full-history overview and two filtered, paginated administrator queues', async () => {
    setMockUserToken('demo-access-token-user')
    await expect(handleMockRequest({ method: 'GET', url: '/api/admin/referrals/overview' })).rejects.toMatchObject({ response: { status: 403 } })
    setMockUserToken('demo-access-token-admin')
    const overview = await handleMockRequest({ method: 'GET', url: '/api/admin/referrals/overview' })
    expect(overview?.data).toMatchObject({ stats: { failed_reward_count: 1, pending_reversal_reward_usd: 2, pending_reversal_reward_count: 1 }, rules: { enabled: false } })
    const failed = await handleMockRequest({ method: 'GET', url: '/api/admin/referral-rewards', params: { status: 'failed', limit: 1, offset: 0 } })
    expect(failed?.data).toMatchObject({ total: 1, items: [{ id: 'demo-reward-1', status: 'failed' }] })
    const debt = await handleMockRequest({ method: 'GET', url: '/api/admin/referral-rewards?pending_reversal=true&limit=1&offset=1' })
    expect(debt?.data).toMatchObject({ total: 1, items: [], offset: 1 })
    const orderNumber = await handleMockRequest({ method: 'GET', url: '/api/admin/referral-rewards', params: { order_no: 'PO-DEMO-REFERRAL', referral_id: 'demo-referral-4', pending_reversal: true } })
    expect(orderNumber?.data).toMatchObject({ total: 1, items: [{ source_order_id: 'demo-order-referral', pending_reversal_amount_usd: 2 }] })
  })
  it('returns specific detail history rather than falling through to the list mock', async () => {
    setMockUserToken('demo-access-token-admin')
    const detail = await handleMockRequest({ method: 'GET', url: '/api/admin/referral-rewards/demo-reward-4' })
    expect(detail?.data).toMatchObject({ reward: { id: 'demo-reward-4', amount_usd: 10, reversed_amount_usd: 4, pending_reversal_amount_usd: 2 }, rule_snapshot: { percent_rate: 10 }, source_order: { amount_usd: 100, refunded_amount_usd: 60 }, refunds: [{ status: 'succeeded', refund_amount_usd: 60 }] })
    const missing = await handleMockRequest({ method: 'GET', url: '/api/admin/referral-rewards/demo-reward-7' })
    expect(missing?.data).toMatchObject({ rule_snapshot: null })
    await expect(handleMockRequest({ method: 'GET', url: '/api/admin/referral-rewards/missing' })).rejects.toMatchObject({ response: { status: 404 } })
  })
  it('searches selectable users and filters rewards by their selected IDs', async () => {
    setMockUserToken('demo-access-token-admin')
    const users = await handleMockRequest({ method: 'GET', url: '/api/admin/users', params: { search: '受邀用户 4', skip: 0, limit: 50 } })
    expect(users?.data).toMatchObject([{ id: 'demo-invitee-4', username: '受邀用户 4' }])
    expect(users?.data).toHaveLength(1)
    const user = await handleMockRequest({ method: 'GET', url: '/api/admin/users/demo-invitee-4' })
    expect(user?.data).toMatchObject({ id: 'demo-invitee-4', username: '受邀用户 4' })
    const records = await handleMockRequest({ method: 'GET', url: '/api/admin/referral-rewards', params: { invitee: 'demo-invitee-4' } })
    expect(records?.data).toMatchObject({ total: 1, items: [{ id: 'demo-reward-4', invitee_user_id: 'demo-invitee-4' }] })
  })
})
