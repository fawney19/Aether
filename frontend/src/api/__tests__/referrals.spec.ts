import { beforeEach, describe, expect, it, vi } from 'vitest'
const { get, post } = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }))
vi.mock('@/api/client', () => ({ default: { get, post } }))
import { referralApi } from '@/api/referrals'
describe('administrator referral API contract', () => {
  beforeEach(() => { vi.resetAllMocks(); get.mockResolvedValue({ data: {} }); post.mockResolvedValue({ data: {} }) })
  it('uses the separate admin overview without altering the user dashboard', async () => {
    await referralApi.getAdminOverview(); expect(get).toHaveBeenLastCalledWith('/api/admin/referrals/overview')
    await referralApi.getMyReferral(); expect(get).toHaveBeenLastCalledWith('/api/users/me/referral')
  })
  it('preserves false debt and legacy order ID while adding actual order number and relationship filters', async () => {
    await referralApi.getAdminReferralRewards({ inviter: 'alice', invitee: 'bob', order_id: 'order-id', order_no: 'ORDER-001', referral_id: 'relation-id', trigger_point: 'paid_order', pending_reversal: false, limit: 20, offset: 120, status: '' })
    expect(get).toHaveBeenCalledWith('/api/admin/referral-rewards', { params: { inviter: 'alice', invitee: 'bob', order_id: 'order-id', order_no: 'ORDER-001', referral_id: 'relation-id', trigger_point: 'paid_order', pending_reversal: false, limit: 20, offset: 120 } })
  })
  it('encodes detail identifiers and passes actual action reasons', async () => {
    await referralApi.getReferralRewardDetail('reward/1'); expect(get).toHaveBeenLastCalledWith('/api/admin/referral-rewards/reward%2F1')
    await referralApi.voidReferralReward('reward-1', '资格作废'); expect(post).toHaveBeenLastCalledWith('/api/admin/referral-rewards/reward-1/void', { note: '资格作废' })
  })
})
