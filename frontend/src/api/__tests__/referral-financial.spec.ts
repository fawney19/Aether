import { beforeEach, describe, expect, it, vi } from 'vitest'
const mocks = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn(), put: vi.fn() }))
vi.mock('@/api/client', () => ({ default: mocks }))
import { adminWalletApi } from '@/api/admin-wallets'
import { adminApi } from '@/api/admin'
import { authApi } from '@/api/auth'
beforeEach(() => { vi.resetAllMocks(); mocks.get.mockResolvedValue({ data: { valid: true } }); mocks.post.mockResolvedValue({ data: {} }); mocks.put.mockResolvedValue({ data: {} }) })
describe('referral operation API contracts', () => {
  it('validates a code without registering or silently binding it', async () => {
    await expect(authApi.validateInviteCode('CODE123')).resolves.toEqual({ valid: true })
    expect(mocks.get).toHaveBeenCalledExactlyOnceWith('/api/auth/invite-code', { params: { code: 'CODE123' } })
    expect(mocks.post).not.toHaveBeenCalled()
  })
  it('requests each refund stage preview and passes the selected risk token to approval and completion', async () => {
    await adminWalletApi.getReferralRefundPreview('wallet-1', 'refund-1', 'process')
    expect(mocks.get).toHaveBeenLastCalledWith('/api/admin/wallets/wallet-1/refunds/refund-1/referral-preview', { params: { stage: 'process' } })
    await adminWalletApi.processRefund('wallet-1', 'refund-1', { referral_shortfall_confirmation: 'process-risk' })
    expect(mocks.post).toHaveBeenLastCalledWith('/api/admin/wallets/wallet-1/refunds/refund-1/process', { referral_shortfall_confirmation: 'process-risk' })
    await adminWalletApi.getReferralRefundPreview('wallet-1', 'refund-1', 'complete')
    expect(mocks.get).toHaveBeenLastCalledWith('/api/admin/wallets/wallet-1/refunds/refund-1/referral-preview', { params: { stage: 'complete' } })
    const completed = { refund: { id: 'refund-1' }, referral_reversal: { reversed_amount_usd: 3, pending_reversal_amount_usd: 7 } }
    mocks.post.mockResolvedValue({ data: completed })
    await expect(adminWalletApi.completeRefund('wallet-1', 'refund-1', { referral_shortfall_confirmation: 'complete-risk', payout_reference: 'proof' })).resolves.toEqual(completed)
    expect(mocks.post).toHaveBeenLastCalledWith('/api/admin/wallets/wallet-1/refunds/refund-1/complete', { referral_shortfall_confirmation: 'complete-risk', payout_reference: 'proof' })
  })
  it('saves a partial referral settings update without forcing an email setting value', async () => {
    await adminApi.updateReferralSettings({ referral_enabled: true })
    expect(mocks.put).toHaveBeenCalledExactlyOnceWith('/api/admin/system/referral-settings', { referral_enabled: true })
  })
})
