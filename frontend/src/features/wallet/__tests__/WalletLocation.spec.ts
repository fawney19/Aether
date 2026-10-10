import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, reactive, type App } from 'vue'

const mocks = vi.hoisted(() => ({ wallet: vi.fn(), order: vi.fn(), refund: vi.fn(), transaction: vi.fn(), ledger: vi.fn(), refunds: vi.fn(), orders: vi.fn(), wallets: vi.fn(), process: vi.fn(), complete: vi.fn(), fail: vi.fn(), credit: vi.fn(), preview: vi.fn(), error: vi.fn() }))
const query = reactive<Record<string, string>>({})
vi.mock('vue-router', () => ({ useRoute: () => ({ query }) }))
vi.mock('@/api/admin-wallets', () => ({ adminWalletApi: {
  getWalletDetail: mocks.wallet, getRefundDetail: mocks.refund, getTransactionDetail: mocks.transaction,
  listLedger: mocks.ledger, listGlobalRefunds: mocks.refunds, listAllWallets: mocks.wallets,
  processRefund: mocks.process, completeRefund: mocks.complete, failRefund: mocks.fail, getReferralRefundPreview: mocks.preview,
} }))
vi.mock('@/api/admin-payments', () => ({ adminPaymentsApi: { getOrder: mocks.order, listOrders: mocks.orders, creditOrder: mocks.credit } }))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ success: vi.fn(), error: mocks.error }) }))
vi.mock('@/components/common', async () => { const { defineComponent, h } = await import('vue'); return { EmptyState: defineComponent({ setup: () => () => h('div') }) } })
import WalletsManagement from '@/views/admin/WalletsManagement.vue'

let app: App | null = null
let root: HTMLElement
const refund = (id: string) => ({ id, wallet_id: 'wallet-1', owner_type: 'user', owner_name: 'alice', refund_no: `RF-${id}`, status: 'pending_approval', amount_usd: 5, refund_mode: 'offline_payout', reason: '申请退款', created_at: '2026-10-09T00:00:00Z' })
async function settle() { for (let i = 0; i < 15; i++) { await Promise.resolve(); await nextTick() } }
function routeTo(values: Record<string, string>) { for (const key of Object.keys(query)) delete query[key]; Object.assign(query, values) }
async function mount() { root = document.createElement('div'); document.body.appendChild(root); app = createApp(WalletsManagement); app.mount(root); await settle() }
function deferred() { let resolve!: (value: unknown) => void; const promise = new Promise(done => { resolve = done }); return { promise, resolve } }
beforeEach(() => {
  vi.resetAllMocks()
  routeTo({})
  mocks.ledger.mockResolvedValue({ items: [], total: 500 })
  mocks.refunds.mockResolvedValue({ items: [], total: 500 })
  mocks.orders.mockResolvedValue({ items: [], total: 500 })
  mocks.wallets.mockResolvedValue([])
})
afterEach(async () => {
  app?.unmount(); app = null; root?.remove()
  // 抽屉的离场动画经过两个 animation frame；等待本例的 Teleport 收尾，避免点击上一例的离场按钮。
  await new Promise(resolve => setTimeout(resolve, 60))
})

describe('invitation record wallet locations', () => {
  it('locates a refund independently of the first list page without approving or previewing it', async () => {
    routeTo({ tab: 'refunds', wallet_id: 'wallet-1', refund_id: 'refund-499' })
    mocks.refund.mockResolvedValue({ refund: refund('refund-499') })
    await mount()
    expect(mocks.refund).toHaveBeenCalledExactlyOnceWith('wallet-1', 'refund-499', expect.any(AbortSignal))
    expect(document.body.textContent).toContain('RF-refund-499')
    expect(document.body.textContent).toContain('处理退款')
    expect(mocks.preview).not.toHaveBeenCalled()
    expect(mocks.process).not.toHaveBeenCalled()
    expect(mocks.complete).not.toHaveBeenCalled()
    expect(mocks.fail).not.toHaveBeenCalled()
  })
  it('ignores late refund A after the route has switched to B', async () => {
    const late = deferred()
    mocks.refund.mockReturnValueOnce(late.promise).mockResolvedValueOnce({ refund: refund('B') })
    routeTo({ tab: 'refunds', wallet_id: 'wallet-1', refund_id: 'A' }); await mount()
    const firstSignal = mocks.refund.mock.calls[0]![2] as AbortSignal
    routeTo({ tab: 'refunds', wallet_id: 'wallet-1', refund_id: 'B' }); await settle()
    expect(firstSignal.aborted).toBe(true)
    late.resolve({ refund: refund('A') }); await settle()
    expect(document.body.textContent).toContain('RF-B')
    expect(document.body.textContent).not.toContain('RF-A')
    expect(mocks.process).not.toHaveBeenCalled()
  })
  it('updates a located refund after manual processing even when it is outside the list page', async () => {
    routeTo({ tab: 'refunds', wallet_id: 'wallet-1', refund_id: 'outside-page' })
    mocks.refund.mockResolvedValue({ refund: refund('outside-page') })
    mocks.preview.mockResolvedValue({ stage: 'process', applicable: false, rewards: [], total_shortfall_usd: 0, confirmation_token: null })
    mocks.process.mockResolvedValue({ refund: { ...refund('outside-page'), status: 'processing' } })
    await mount()
    const buttons = [...document.querySelectorAll<HTMLButtonElement>('button')].filter(item => item.textContent?.trim() === '处理退款')
    const button = buttons[buttons.length - 1]!
    button.click(); await settle()
    expect(mocks.process).not.toHaveBeenCalled()
    const approve = [...document.querySelectorAll<HTMLButtonElement>('button')].find(item => item.textContent?.trim() === '批准并处理退款')!
    approve.click(); await settle()
    expect(mocks.process).toHaveBeenCalledExactlyOnceWith('wallet-1', 'outside-page', { referral_shortfall_confirmation: undefined })
    expect(document.body.textContent).toContain('确认完成')
    expect(document.body.textContent).not.toContain('处理退款')
  })
  it('closes an existing refund approval context when the route changes', async () => {
    mocks.refund.mockResolvedValueOnce({ refund: refund('A') })
    routeTo({ tab: 'refunds', wallet_id: 'wallet-1', refund_id: 'A' }); await mount()
    const late = deferred(); mocks.preview.mockReturnValue(late.promise)
    const process = [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.trim() === '处理退款')!
    process.click(); await settle()
    routeTo({ tab: 'orders', order_id: 'order-B' })
    mocks.order.mockResolvedValue({ order: { id: 'order-B', order_no: 'PO-B', amount_usd: 8, refunded_amount_usd: 0, status: 'credited' } })
    await settle()
    expect((mocks.preview.mock.calls[0]![3] as AbortSignal).aborted).toBe(true)
    late.resolve({ applicable: true, rewards: [], total_shortfall_usd: 0 }); await settle()
    expect(mocks.process).not.toHaveBeenCalled()
    expect(document.querySelector('[role="dialog"]')).toBeNull()
  })
  it('shows a readonly order beyond the list page without opening artificial credit', async () => {
    routeTo({ tab: 'orders', order_id: 'order-499' })
    mocks.order.mockResolvedValue({ order: { id: 'order-499', order_no: 'PO-499', amount_usd: 9, refunded_amount_usd: 0, status: 'pending' } })
    await mount()
    expect(mocks.order).toHaveBeenCalledExactlyOnceWith('order-499')
    expect(document.body.textContent).toContain('PO-499')
    expect(document.body.textContent).not.toContain('确认到账')
    expect(mocks.credit).not.toHaveBeenCalled()
  })
  it('maps wallets alias to existing ledger and displays real balances separately from global results', async () => {
    routeTo({ tab: 'wallets', wallet_id: 'wallet-499' })
    mocks.wallet.mockResolvedValue({ id: 'wallet-499', owner_type: 'user', owner_name: 'inviter', recharge_balance: 21, gift_balance: 4, status: 'active' })
    await mount()
    expect(mocks.wallet).toHaveBeenCalledExactlyOnceWith('wallet-499', expect.any(AbortSignal))
    expect(mocks.ledger).toHaveBeenCalled()
    expect(document.body.textContent).toContain('inviter')
    expect(document.body.textContent).toContain('下方仍为全局列表')
    expect(document.body.textContent).toContain('$21.00')
  })
  it('reads an exact transaction instead of scanning the first ledger page', async () => {
    routeTo({ tab: 'ledger', wallet_id: 'wallet-1', transaction_id: 'tx-499' })
    mocks.transaction.mockResolvedValue({ transaction: { id: 'tx-499', wallet_id: 'wallet-1', owner_type: 'user', owner_name: 'inviter', category: 'gift', reason_code: 'referral_reward', amount: 4, balance_before: 0, balance_after: 4, recharge_balance_before: 0, recharge_balance_after: 0, gift_balance_before: 0, gift_balance_after: 4, created_at: '2026-10-09T00:00:00Z' } })
    await mount()
    expect(mocks.transaction).toHaveBeenCalledExactlyOnceWith('wallet-1', 'tx-499', expect.any(AbortSignal))
    expect(document.body.textContent).toContain('交易ID')
    expect(document.body.textContent).toContain('tx-499')
  })
  it('retains a copyable target ID when access is denied or the target was deleted', async () => {
    routeTo({ tab: 'refunds', wallet_id: 'wallet-1', refund_id: 'deleted-refund' })
    mocks.refund.mockRejectedValue({ response: { status: 404, data: { detail: '退款记录不存在' } } })
    await mount()
    expect(document.querySelector('[role="alert"]')?.textContent).toContain('退款记录不存在')
    expect(document.body.textContent).toContain('deleted-refund')
    expect(document.body.textContent).toContain('复制 ID')
    expect(document.body.textContent).not.toContain('处理退款')
  })
  it('aborts locating when the page is unmounted', async () => {
    const late = deferred(); mocks.refund.mockReturnValue(late.promise)
    routeTo({ tab: 'refunds', wallet_id: 'wallet-1', refund_id: 'A' }); await mount()
    app!.unmount(); app = null; root.remove()
    expect((mocks.refund.mock.calls[0]![2] as AbortSignal).aborted).toBe(true)
    const existingDrawers = document.querySelectorAll('.drawer-panel').length
    late.resolve({ refund: refund('A') }); await settle()
    expect(document.querySelectorAll('.drawer-panel').length).toBeLessThanOrEqual(existingDrawers)
  })
})
