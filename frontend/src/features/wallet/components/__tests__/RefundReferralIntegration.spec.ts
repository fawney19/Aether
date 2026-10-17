import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, reactive, type App } from 'vue'
import type { AdminWallet } from '@/api/admin-wallets'
const mocks = vi.hoisted(() => ({
  preview: vi.fn(), process: vi.fn(), complete: vi.fn(), fail: vi.fn(), transactions: vi.fn(), refunds: vi.fn(), globalRefunds: vi.fn(), ledger: vi.fn(), success: vi.fn(), error: vi.fn(),
}))
vi.mock('@/api/admin-wallets', () => ({ adminWalletApi: {
  getReferralRefundPreview: mocks.preview, processRefund: mocks.process, completeRefund: mocks.complete, failRefund: mocks.fail,
  getWalletTransactions: mocks.transactions, getWalletRefunds: mocks.refunds, listGlobalRefunds: mocks.globalRefunds, listLedger: mocks.ledger,
} }))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ success: mocks.success, error: mocks.error }) }))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => ({ confirm: vi.fn() }) }))
vi.mock('vue-router', () => ({ useRoute: () => ({ query: { tab: 'refunds' } }) }))
vi.mock('@/components/common', async () => { const { defineComponent, h } = await import('vue'); return { EmptyState: defineComponent({ setup: () => () => h('div') }) } })
vi.mock('@/api/admin-payments', () => ({ adminPaymentsApi: {} }))
import WalletOpsDrawer from '../WalletOpsDrawer.vue'
import WalletsManagement from '@/views/admin/WalletsManagement.vue'
const mounted: Array<{ app: App; root: HTMLElement }> = []
const wallet: AdminWallet = { id: 'wallet-1', owner_type: 'user', user_id: 'invitee-1', api_key_id: null, owner_name: 'invitee', recharge_balance: 50, gift_balance: 0, balance: 50, refundable_balance: 50, total_recharged: 50, total_consumed: 0, total_refunded: 0, total_adjusted: 0, currency: 'USD', status: 'active', created_at: '2026-10-08T00:00:00Z', updated_at: '2026-10-08T00:00:00Z' }
let refund: Record<string, unknown>
async function settle() { for (let i = 0; i < 12; i++) { await Promise.resolve(); await nextTick() } }
function button(text: string): HTMLButtonElement {
  const found = [...document.querySelectorAll<HTMLButtonElement>('button')].find(item => item.textContent?.trim() === text)
  if (!found) throw new Error(`Missing button ${text}`)
  return found
}
async function mount(kind: 'drawer' | 'management') {
  const root = document.createElement('div'); document.body.appendChild(root)
  const app = kind === 'drawer' ? createApp(WalletOpsDrawer, { open: true, wallet, ownerName: 'invitee' }) : createApp(WalletsManagement)
  app.mount(root); mounted.push({ app, root }); await settle()
  if (kind === 'drawer') { button('退款审批').click(); await settle() }
  else { button('审批').click(); await settle() }
}
async function approve() {
  document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click(); await settle()
  button('批准并处理退款').click(); await settle()
}
beforeEach(() => {
  vi.resetAllMocks()
  refund = { id: 'refund-1', refund_no: 'RF1', wallet_id: 'wallet-1', owner_type: 'user', owner_name: 'invitee', amount_usd: 10, status: 'pending_approval', refund_mode: 'offline_payout', created_at: '2026-10-08T00:00:00Z', reason: '退款', source_type: 'payment_order' }
  mocks.transactions.mockResolvedValue({ wallet, items: [], total: 0 })
  mocks.refunds.mockImplementation(async () => ({ wallet, items: [{ ...refund }], total: 1 }))
  mocks.globalRefunds.mockImplementation(async () => ({ items: [{ ...refund }], total: 1 }))
  mocks.ledger.mockResolvedValue({ items: [], total: 0 })
  mocks.preview.mockImplementation(async (_wallet, _refund, stage) => ({ refund_id: 'refund-1', stage, applicable: true,
    rewards: [{ inviter_user_id: 'inviter-1', inviter_username: 'alice', expected_reversal_usd: 10, available_gift_usd: 3, deductible_usd: 3, shortfall_usd: 7 }],
    total_expected_reversal_usd: 10, total_deductible_usd: 3, total_shortfall_usd: 7, confirmation_token: `${stage}-risk` }))
  mocks.process.mockImplementation(async () => { refund.status = 'processing'; return { wallet, refund } })
  mocks.complete.mockResolvedValue({ refund: { ...refund, status: 'succeeded' }, referral_reversal: { reversed_amount_usd: 3, pending_reversal_amount_usd: 7 } })
  mocks.fail.mockResolvedValue({ wallet, refund: { ...refund, status: 'failed' } })
})
afterEach(() => { for (const { app, root } of mounted.splice(0)) { app.unmount(); root.remove() } })
for (const kind of ['drawer', 'management'] as const) {
  describe(`refund referral integration: ${kind}`, () => {
    it('requires administrator review before processing and independently reviews before completion', async () => {
      await mount(kind)
      button(kind === 'drawer' ? '处理' : '处理退款').click(); await settle()
      expect(mocks.process).not.toHaveBeenCalled()
      expect(document.body.textContent).toContain('alice')
      await approve()
      expect(mocks.process).toHaveBeenCalledExactlyOnceWith('wallet-1', 'refund-1', { referral_shortfall_confirmation: 'process-risk' })
      if (kind === 'drawer') { button('完成').click(); await settle() }
      button('确认完成').click(); await settle()
      expect(mocks.preview).toHaveBeenLastCalledWith('wallet-1', 'refund-1', 'complete', expect.any(AbortSignal))
      expect(mocks.complete).not.toHaveBeenCalled()
      const completeButton = button('同意完成退款')
      expect(completeButton.disabled).toBe(true)
      document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click(); await settle()
      completeButton.click(); await settle()
      expect(mocks.complete).toHaveBeenCalledExactlyOnceWith('wallet-1', 'refund-1', { referral_shortfall_confirmation: 'complete-risk', gateway_refund_id: undefined, payout_reference: undefined })
      expect(mocks.success).toHaveBeenLastCalledWith('退款已完成；该订单累计冲回 $3.00，当前待冲回 $7.00')
    })
    it('supports administrator rejection with a reason without processing the refund', async () => {
      await mount(kind)
      button(kind === 'drawer' ? '处理' : '处理退款').click(); await settle()
      button('转为驳回退款').click(); await settle()
      const input = document.querySelector<HTMLInputElement>('input[placeholder="请填写驳回原因"]')!
      input.value = '返利不足，人工复核后拒绝'; input.dispatchEvent(new Event('input')); await settle()
      button(kind === 'drawer' ? '确认驳回' : '驳回退款').click(); await settle()
      expect(mocks.process).not.toHaveBeenCalled()
      expect(mocks.fail).toHaveBeenCalledExactlyOnceWith('wallet-1', 'refund-1', { reason: '返利不足，人工复核后拒绝' })
    })
    it('requires fresh review after the server reports that the inviter balance has changed', async () => {
      mocks.process.mockRejectedValueOnce({ response: { status: 409, data: { code: 'REFERRAL_SHORTFALL_CONFIRMATION_REQUIRED', detail: '返利余额已变化，请重新核对并确认' } } })
      await mount(kind)
      button(kind === 'drawer' ? '处理' : '处理退款').click(); await settle(); await approve()
      expect(mocks.process).toHaveBeenCalledTimes(1)
      mocks.preview.mockResolvedValue({ stage: 'process', refund_id: 'refund-1', applicable: true, rewards: [], total_expected_reversal_usd: 10,
        total_deductible_usd: 1, total_shortfall_usd: 9, confirmation_token: 'changed-risk' })
      button(kind === 'drawer' ? '处理' : '处理退款').click(); await settle()
      expect(document.body.textContent).toContain('$9.00')
      expect(button('批准并处理退款').disabled).toBe(true)
      expect(mocks.process).toHaveBeenCalledTimes(1)
      await approve()
      expect(mocks.process).toHaveBeenLastCalledWith('wallet-1', 'refund-1', { referral_shortfall_confirmation: 'changed-risk' })
    })
    it('never posts after unmounting while the preview request is pending', async () => {
      let resolve!: (value: unknown) => void
      mocks.preview.mockReturnValue(new Promise(done => { resolve = done }))
      await mount(kind)
      button(kind === 'drawer' ? '处理' : '处理退款').click(); await settle()
      const signal = mocks.preview.mock.calls[0]![3] as AbortSignal
      const { app, root } = mounted.pop()!
      app.unmount(); root.remove()
      expect(signal.aborted).toBe(true)
      resolve({ stage: 'process', refund_id: 'refund-1', applicable: true, rewards: [], total_shortfall_usd: 0, confirmation_token: null })
      await settle()
      expect(mocks.process).not.toHaveBeenCalled()
      expect(document.querySelector('[role="dialog"]')).toBeNull()
    })
    it('uses the refund completion form captured before the asynchronous review', async () => {
      refund.status = 'processing'
      await mount(kind)
      if (kind === 'drawer') { button('完成').click(); await settle() }
      const label = [...document.querySelectorAll('label')].find(item => item.textContent?.includes('打款凭证'))!
      const input = label.parentElement!.querySelector<HTMLInputElement>('input')!
      input.value = 'original-proof'; input.dispatchEvent(new Event('input')); await settle()
      button('确认完成').click(); await settle()
      input.value = 'changed-proof'; input.dispatchEvent(new Event('input')); await settle()
      document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click(); await settle()
      button('同意完成退款').click(); await settle()
      expect(mocks.complete).toHaveBeenCalledExactlyOnceWith('wallet-1', 'refund-1', {
        referral_shortfall_confirmation: 'complete-risk', gateway_refund_id: undefined, payout_reference: 'original-proof',
      })
    })
    it('stops processing when referral risk cannot be obtained', async () => {
      mocks.preview.mockRejectedValue(new Error('offline'))
      await mount(kind)
      button(kind === 'drawer' ? '处理' : '处理退款').click(); await settle()
      expect(mocks.process).not.toHaveBeenCalled()
      expect(mocks.error).toHaveBeenCalled()
    })
  })
}

it('cancels management preview A when the administrator switches to refund B before it returns', async () => {
  refund.status = 'processing'
  const secondRefund = { ...refund, id: 'refund-B', refund_no: 'RF-B' }
  mocks.globalRefunds.mockResolvedValue({ items: [{ ...refund }, secondRefund], total: 2 })
  let resolve!: (value: unknown) => void
  mocks.preview.mockReturnValueOnce(new Promise(done => { resolve = done }))
  await mount('management')
  button('确认完成').click(); await settle()
  const signal = mocks.preview.mock.calls[0]![3] as AbortSignal
  const approvals = [...document.querySelectorAll<HTMLButtonElement>('button')].filter(item => item.textContent?.trim() === '审批')
  approvals[1]!.click(); await settle()
  expect(signal.aborted).toBe(true)
  resolve({ stage: 'complete', refund_id: 'refund-1', applicable: true, rewards: [], total_shortfall_usd: 0, confirmation_token: null })
  await settle()
  expect(mocks.complete).not.toHaveBeenCalled()
  expect(document.querySelector('[role="dialog"]')).toBeNull()
  button('确认完成').click(); await settle()
  expect(mocks.preview).toHaveBeenLastCalledWith('wallet-1', 'refund-B', 'complete', expect.any(AbortSignal))
  document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click(); await settle()
  button('同意完成退款').click(); await settle()
  expect(mocks.complete).toHaveBeenCalledExactlyOnceWith('wallet-1', 'refund-B', {
    referral_shortfall_confirmation: 'complete-risk', gateway_refund_id: undefined, payout_reference: undefined,
  })
})

it('cancels drawer preview A when the selected wallet changes before the request returns', async () => {
  const state = reactive({ open: true, wallet: { ...wallet } })
  mocks.transactions.mockImplementation(async id => ({ wallet: { ...wallet, id }, items: [], total: 0 }))
  mocks.refunds.mockImplementation(async id => ({ wallet: { ...wallet, id }, items: [{ ...refund, id: id === 'wallet-1' ? 'refund-1' : 'refund-B' }], total: 1 }))
  let resolve!: (value: unknown) => void
  mocks.preview.mockReturnValueOnce(new Promise(done => { resolve = done }))
  const root = document.createElement('div'); document.body.appendChild(root)
  const app = createApp({ render: () => h(WalletOpsDrawer, { open: state.open, wallet: state.wallet }) })
  app.mount(root); mounted.push({ app, root }); await settle()
  button('退款审批').click(); await settle()
  button('处理').click(); await settle()
  const signal = mocks.preview.mock.calls[0]![3] as AbortSignal
  state.wallet = { ...wallet, id: 'wallet-B' }; await settle()
  expect(signal.aborted).toBe(true)
  resolve({ stage: 'process', refund_id: 'refund-1', applicable: true, rewards: [], total_shortfall_usd: 0, confirmation_token: null })
  await settle()
  expect(mocks.process).not.toHaveBeenCalled()
  expect(document.querySelector('[role="dialog"]')).toBeNull()
  button('退款审批').click(); await settle()
  button('处理').click(); await settle(); await approve()
  expect(mocks.process).toHaveBeenCalledExactlyOnceWith('wallet-B', 'refund-B', { referral_shortfall_confirmation: 'process-risk' })
})

it('does not reopen a completion confirmation after the drawer action form was cancelled during preview loading', async () => {
  refund.status = 'processing'
  let resolve!: (value: unknown) => void
  mocks.preview.mockReturnValueOnce(new Promise(done => { resolve = done }))
  await mount('drawer')
  button('完成').click(); await settle()
  button('确认完成').click(); await settle()
  const signal = mocks.preview.mock.calls[0]![3] as AbortSignal
  button('取消').click(); await settle()
  expect(signal.aborted).toBe(true)
  resolve({ stage: 'complete', refund_id: 'refund-1', applicable: true, rewards: [], total_shortfall_usd: 0, confirmation_token: null })
  await settle()
  expect(mocks.complete).not.toHaveBeenCalled()
  expect(document.querySelector('[role="dialog"]')).toBeNull()
})
