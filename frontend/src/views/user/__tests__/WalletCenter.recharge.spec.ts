import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
import type { PaymentOrder, WalletBalanceResponse } from '@/api/wallet'
import WalletCenter from '../WalletCenter.vue'

const walletApiMock = vi.hoisted(() => ({
  getBalance: vi.fn(),
  getFlow: vi.fn(),
  getTodayCost: vi.fn(),
  listRechargeOptions: vi.fn(),
  listRechargeOrders: vi.fn(),
  getRechargeOrder: vi.fn(),
  createRechargeOrder: vi.fn(),
}))
const toastMock = vi.hoisted(() => ({ success: vi.fn(), info: vi.fn(), error: vi.fn() }))

vi.mock('@/api/wallet', () => ({ walletApi: walletApiMock }))
vi.mock('@/composables/useToast', () => ({ useToast: () => toastMock }))
vi.mock('@/utils/logger', () => ({ log: { error: vi.fn() } }))
vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  const passthrough = defineComponent({ setup: (_, { slots }) => () => h('div', slots.default?.()) })
  const button = defineComponent({ setup: (_, { slots }) => () => h('button', slots.default?.()) })
  return {
    ...Object.fromEntries([
      'Badge', 'Card', 'Input', 'Label', 'Select', 'SelectContent', 'SelectItem',
      'SelectTrigger', 'SelectValue', 'Table', 'TableBody', 'TableCell', 'TableHead',
      'TableHeader', 'TableRow', 'Tabs', 'TabsContent', 'TabsList', 'TabsTrigger', 'Textarea',
    ].map(name => [name, passthrough])),
    Button: button,
    RefreshButton: defineComponent({ setup: () => () => h('button', { 'data-refresh': true }, '刷新') }),
    Pagination: defineComponent({
      props: { current: Number },
      emits: ['update:current'],
      setup: (_, { emit }) => () => h('button', {
        'data-next-page': true,
        onClick: () => emit('update:current', 2),
      }, '下一页'),
    }),
  }
})
vi.mock('@/components/common', async () => {
  const { defineComponent, h } = await import('vue')
  const empty = defineComponent({ setup: () => () => h('div') })
  return {
    EmptyState: empty,
    LoadingState: empty,
    StripePaymentDialog: defineComponent({
      emits: ['success'],
      setup: (_, { emit }) => () => h('button', {
        'data-stripe-success': true,
        onClick: () => emit('success', { intentId: 'pi-1', status: 'processing' }),
      }, 'Stripe 提交'),
    }),
  }
})

const mountedApps: Array<{ app: App; root: HTMLElement }> = []
let hidden = false

function walletBalance(amount: number): WalletBalanceResponse {
  return {
    wallet: {
      id: 'wallet-1', balance: amount, recharge_balance: amount, gift_balance: 0,
      refundable_balance: amount, currency: 'USD', status: 'active', total_recharged: amount,
      total_consumed: 0, total_refunded: 0, total_adjusted: 0, updated_at: '2026-09-11T00:00:00Z',
    },
    balance: amount, unlimited: false, limit_mode: 'finite', currency: 'USD',
    wallet_balance: amount, package_balance: 3, total_available_balance: amount + 3,
    daily_quota: { has_active: true, total_usd: 5, used_usd: 2, remaining_usd: 3, allow_wallet_overage: true },
  }
}

function paymentOrder(status = 'pending', overrides: Partial<PaymentOrder> = {}): PaymentOrder {
  return {
    id: 'order-1', order_no: 'RECHARGE-1', wallet_id: 'wallet-1', user_id: 'user-1',
    amount_usd: 10, pay_amount: 10, pay_currency: 'USD', exchange_rate: 1,
    refunded_amount_usd: 0, refundable_amount_usd: status === 'credited' ? 10 : 0,
    payment_method: 'epay', gateway_order_id: 'gateway-1', gateway_response: null,
    status, created_at: '2026-09-11T00:00:00Z', paid_at: null,
    credited_at: status === 'credited' ? '2026-09-11T00:01:00Z' : null, expires_at: null,
    ...overrides,
  }
}

function orderResponse(items: PaymentOrder[], amount = 2, offset = 0) {
  const { wallet_balance: _wallet, package_balance: _package, total_available_balance: _total, daily_quota: _quota, ...balance } = walletBalance(amount)
  return { ...balance, items, total: items.length, limit: 20, offset }
}

async function flushPromises() {
  for (let i = 0; i < 10; i += 1) await Promise.resolve()
  await nextTick()
}

async function mountWallet() {
  const root = document.createElement('div')
  document.body.append(root)
  const app = createApp(WalletCenter)
  app.mount(root)
  mountedApps.push({ app, root })
  await flushPromises()
  return { app, root }
}

function setHidden(value: boolean) {
  hidden = value
  document.dispatchEvent(new Event('visibilitychange'))
}

beforeEach(() => {
  vi.useFakeTimers()
  vi.resetAllMocks()
  hidden = false
  vi.spyOn(document, 'hidden', 'get').mockImplementation(() => hidden)
  walletApiMock.getBalance.mockResolvedValue(walletBalance(2))
  walletApiMock.getFlow.mockResolvedValue({ items: [], total: 0, today_entry: null })
  walletApiMock.getTodayCost.mockResolvedValue(null)
  walletApiMock.listRechargeOptions.mockResolvedValue({ items: [] })
  walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([paymentOrder()]))
})

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  vi.restoreAllMocks()
  vi.useRealTimers()
})

describe('WalletCenter recharge synchronization', () => {
  it('waits for server credit after Stripe submission and refreshes the credited balance and flow', async () => {
    const { root } = await mountWallet()
    root.querySelector<HTMLButtonElement>('[data-stripe-success]')!.click()
    await flushPromises()
    expect(toastMock.info).toHaveBeenCalledWith('支付已提交，正在等待充值到账')
    expect(toastMock.success).not.toHaveBeenCalled()

    walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([paymentOrder('paid')]))
    await vi.advanceTimersByTimeAsync(5_000)
    expect(toastMock.success).not.toHaveBeenCalled()

    // The order query can observe the credit after its wallet snapshot was read.
    walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([paymentOrder('credited')], 2))
    walletApiMock.getBalance.mockResolvedValue(walletBalance(12))
    await vi.advanceTimersByTimeAsync(5_000)
    expect(root.textContent).toContain('$12.00')
    expect(root.textContent).toContain('$15.00')
    expect(walletApiMock.getFlow).toHaveBeenCalledTimes(2)
    expect(toastMock.success).toHaveBeenCalledWith('充值已到账，余额已更新')

    const calls = walletApiMock.listRechargeOrders.mock.calls.length
    await vi.advanceTimersByTimeAsync(15_000)
    expect(walletApiMock.listRechargeOrders).toHaveBeenCalledTimes(calls)
  })

  it('updates the wallet and preserves package quota when the orders refresh button is clicked', async () => {
    walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([paymentOrder('credited')], 2))
    const { root } = await mountWallet()
    walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([paymentOrder('credited')], 12))

    root.querySelectorAll<HTMLButtonElement>('[data-refresh]')[4]!.click()
    await flushPromises()

    expect(root.textContent).toContain('$12.00')
    expect(root.textContent).toContain('$15.00')
    expect(root.textContent).toContain('已用 $2.00 / 每日 $5.00')
    expect(walletApiMock.getBalance).toHaveBeenCalledTimes(1)
  })

  it('pauses polling while hidden and checks credit as soon as the page becomes visible', async () => {
    const { root } = await mountWallet()
    setHidden(true)
    await vi.advanceTimersByTimeAsync(20_000)
    expect(walletApiMock.listRechargeOrders).toHaveBeenCalledTimes(1)

    walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([paymentOrder('credited')], 12))
    walletApiMock.getBalance.mockResolvedValue(walletBalance(12))
    setHidden(false)
    await flushPromises()

    expect(walletApiMock.listRechargeOrders).toHaveBeenCalledTimes(2)
    expect(root.textContent).toContain('$12.00')
    expect(walletApiMock.getFlow).toHaveBeenCalledTimes(2)
  })

  it('keeps tracking a pending recharge after the user changes the order page', async () => {
    const { root } = await mountWallet()
    walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([], 2, 20))
    walletApiMock.getRechargeOrder.mockResolvedValue({ order: paymentOrder('credited') })
    walletApiMock.getBalance.mockResolvedValue(walletBalance(12))
    root.querySelectorAll<HTMLButtonElement>('[data-next-page]')[1]!.click()
    await flushPromises()

    expect(walletApiMock.getRechargeOrder).toHaveBeenCalledWith('order-1')
    expect(root.textContent).toContain('$12.00')
    expect(toastMock.success).toHaveBeenCalledTimes(1)
  })

  it('retries a temporary polling failure without showing an error toast', async () => {
    const { root } = await mountWallet()
    walletApiMock.listRechargeOrders.mockRejectedValueOnce(new Error('network unavailable'))
    await vi.advanceTimersByTimeAsync(5_000)
    expect(toastMock.error).not.toHaveBeenCalled()

    walletApiMock.listRechargeOrders.mockResolvedValue(orderResponse([paymentOrder('credited')], 12))
    walletApiMock.getBalance.mockResolvedValue(walletBalance(12))
    await vi.advanceTimersByTimeAsync(5_000)
    expect(root.textContent).toContain('$12.00')
    expect(toastMock.success).toHaveBeenCalledTimes(1)
  })

  it('does not restart polling when an in-flight response completes after unmount', async () => {
    const { app } = await mountWallet()
    let resolve!: (value: ReturnType<typeof orderResponse>) => void
    walletApiMock.listRechargeOrders.mockReturnValue(new Promise(complete => { resolve = complete }))
    await vi.advanceTimersByTimeAsync(5_000)
    app.unmount()
    mountedApps.splice(0).forEach(({ root }) => root.remove())
    resolve(orderResponse([paymentOrder()]))
    await flushPromises()
    expect(vi.getTimerCount()).toBe(0)
  })
})
