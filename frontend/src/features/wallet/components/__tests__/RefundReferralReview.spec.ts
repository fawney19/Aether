import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
import type { ReferralRefundPreview } from '@/api/admin-wallets'
const mocks = vi.hoisted(() => ({ preview: vi.fn() }))
vi.mock('@/api/admin-wallets', () => ({ adminWalletApi: { getReferralRefundPreview: mocks.preview } }))
import RefundReferralReview from '../RefundReferralReview.vue'
const mounted: Array<{ app: App; root: HTMLElement }> = []
const base: ReferralRefundPreview = {
  refund_id: 'refund-1', applicable: true, stage: 'process', confirmation_token: 'risk-v1',
  rewards: [{ inviter_user_id: 'inviter-1', inviter_username: 'alice', expected_reversal_usd: 10,
    available_gift_usd: 3, deductible_usd: 3, shortfall_usd: 7 }],
  total_expected_reversal_usd: 10, total_deductible_usd: 3, total_shortfall_usd: 7,
}
async function settle() { for (let i = 0; i < 6; i++) { await Promise.resolve(); await nextTick() } }
function mount() {
  const root = document.createElement('div'); document.body.appendChild(root)
  const app = createApp(RefundReferralReview)
  const vm = app.mount(root) as unknown as InstanceType<typeof RefundReferralReview>
  mounted.push({ app, root }); return vm
}
function button(text: string): HTMLButtonElement {
  const value = [...document.querySelectorAll<HTMLButtonElement>('button')].find(item => item.textContent?.includes(text))
  if (!value) throw new Error(`Missing button ${text}`)
  return value
}
beforeEach(() => { vi.resetAllMocks(); mocks.preview.mockResolvedValue(structuredClone(base)) })
afterEach(() => { for (const { app, root } of mounted.splice(0)) { app.unmount(); root.remove() } })
describe('RefundReferralReview', () => {
  it('shows inviter shortfall and allows the administrator to approve explicitly', async () => {
    const vm = mount(); const result = vm.review('wallet-1', 'refund-1', 'process'); await settle()
    expect(mocks.preview).toHaveBeenCalledWith('wallet-1', 'refund-1', 'process', expect.any(AbortSignal))
    expect(document.body.textContent).toContain('alice')
    expect(document.body.textContent).toContain('$7.00')
    expect(button('批准并处理退款').disabled).toBe(true)
    document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click(); await settle()
    expect(button('批准并处理退款').disabled).toBe(false)
    button('批准并处理退款').click()
    expect(await result).toEqual({ decision: 'approve', token: 'risk-v1' })
  })
  it('lets the administrator choose rejection without acknowledging or approving', async () => {
    const vm = mount(); const result = vm.review('wallet-1', 'refund-1', 'process'); await settle()
    button('转为驳回退款').click()
    expect(await result).toEqual({ decision: 'reject', token: undefined })
  })
  it('refreshes risk and requires a new acknowledgement before completion', async () => {
    const vm = mount(); const first = vm.review('wallet-1', 'refund-1', 'process'); await settle()
    document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click(); await settle()
    button('批准并处理退款').click(); await first
    mocks.preview.mockResolvedValue({ ...base, stage: 'complete', confirmation_token: 'risk-v2', total_shortfall_usd: 9 })
    const second = vm.review('wallet-1', 'refund-1', 'complete'); await settle()
    expect(mocks.preview).toHaveBeenLastCalledWith('wallet-1', 'refund-1', 'complete', expect.any(AbortSignal))
    expect(document.body.textContent).toContain('$9.00')
    expect(button('同意完成退款').disabled).toBe(true)
    document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click(); await settle()
    button('同意完成退款').click()
    expect(await second).toEqual({ decision: 'approve', token: 'risk-v2' })
  })
  it('does not claim there is no shortfall when risk lookup fails', async () => {
    mocks.preview.mockRejectedValue(new Error('offline'))
    const vm = mount()
    await expect(vm.review('wallet-1', 'refund-1', 'process')).rejects.toThrow('offline')
    expect(document.querySelector('[role="dialog"]')).toBeNull()
  })
  it('needs no shortfall acknowledgement when the available gift covers reversal', async () => {
    mocks.preview.mockResolvedValue({ ...base, rewards: [], total_shortfall_usd: 0, confirmation_token: null })
    const vm = mount(); const result = vm.review('wallet-1', 'refund-1', 'process'); await settle()
    expect(document.querySelector('input[type="checkbox"]')).toBeNull()
    expect(button('批准并处理退款').disabled).toBe(false)
    button('批准并处理退款').click()
    expect(await result).toEqual({ decision: 'approve', token: undefined })
  })
  it('matches the backend money tolerance so floating point dust cannot disable approval', async () => {
    mocks.preview.mockResolvedValue({ ...base, rewards: [], total_shortfall_usd: 0.000000001, confirmation_token: null })
    const vm = mount(); const result = vm.review('wallet-1', 'refund-1', 'process'); await settle()
    expect(document.querySelector('input[type="checkbox"]')).toBeNull()
    expect(button('批准并处理退款').disabled).toBe(false)
    button('批准并处理退款').click()
    expect(await result).toEqual({ decision: 'approve', token: undefined })
  })

  it('aborts and discards an in-flight preview when the component unmounts', async () => {
    let resolve!: (value: ReferralRefundPreview) => void
    mocks.preview.mockReturnValue(new Promise<ReferralRefundPreview>(done => { resolve = done }))
    const vm = mount(); const result = vm.review('wallet-1', 'refund-1', 'process')
    const signal = mocks.preview.mock.calls[0]![3] as AbortSignal
    const { app, root } = mounted.pop()!
    app.unmount(); root.remove()
    expect(signal.aborted).toBe(true)
    resolve(base)
    expect(await result).toMatchObject({ decision: 'cancel' })
    await settle()
    expect(document.querySelector('[role="dialog"]')).toBeNull()
  })

  it('discards an older preview that resolves after another refund review has started', async () => {
    let resolveA!: (value: ReferralRefundPreview) => void
    let resolveB!: (value: ReferralRefundPreview) => void
    mocks.preview.mockReturnValueOnce(new Promise<ReferralRefundPreview>(done => { resolveA = done }))
      .mockReturnValueOnce(new Promise<ReferralRefundPreview>(done => { resolveB = done }))
    const vm = mount()
    const first = vm.review('wallet-1', 'refund-A', 'process')
    const firstSignal = mocks.preview.mock.calls[0]![3] as AbortSignal
    const second = vm.review('wallet-1', 'refund-B', 'process')
    expect(firstSignal.aborted).toBe(true)
    resolveB({ ...base, refund_id: 'refund-B', confirmation_token: 'B-risk', rewards: [{ ...base.rewards[0]!, inviter_username: 'inviter-B' }] })
    await settle()
    resolveA({ ...base, refund_id: 'refund-A', rewards: [{ ...base.rewards[0]!, inviter_username: 'inviter-A' }] })
    expect(await first).toMatchObject({ decision: 'cancel' })
    await settle()
    expect(document.body.textContent).toContain('inviter-B')
    expect(document.body.textContent).not.toContain('inviter-A')
    document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click(); await settle()
    button('批准并处理退款').click()
    expect(await second).toEqual({ decision: 'approve', token: 'B-risk' })
  })

})
