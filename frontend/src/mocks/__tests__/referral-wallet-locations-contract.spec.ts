import { describe, expect, it } from 'vitest'
import { handleReferralWalletLocationMock as handle } from '../referral-wallet-locations'

function get(path: string) { return handle({ method: 'GET', url: path }, true)?.data as Record<string, unknown> }
function expectStatus(action: () => unknown, status: number) {
  let caught: unknown
  try { action() } catch (error) { caught = error }
  expect(caught).toMatchObject({ response: { status } })
}
describe('readonly referral wallet location demo contract', () => {
  it('connects an exact wallet, payment order, refund, and reward transaction outside global first pages', () => {
    const wallet = get('/api/admin/wallets/wallet-demo-user')
    const { order } = get('/api/admin/payments/orders/demo-order-referral') as { order: Record<string, unknown> }
    const { refund } = get('/api/admin/wallets/wallet-demo-user/refunds/demo-refund-referral') as { refund: Record<string, unknown> }
    const { transaction } = get('/api/admin/wallets/wallet-demo-user/transactions/demo-ledger-referral') as { transaction: Record<string, unknown> }
    expect(wallet).toMatchObject({ id: 'wallet-demo-user', recharge_balance: 40, gift_balance: 14.68, demo_mode: true })
    expect(order).toMatchObject({ wallet_id: wallet.id, amount_usd: 100, refunded_amount_usd: 60, order_no: 'PO-DEMO-REFERRAL' })
    expect(refund).toMatchObject({ wallet_id: wallet.id, source_id: order.id, amount_usd: 60, status: 'succeeded' })
    expect(transaction).toMatchObject({ wallet_id: wallet.id, amount: 10, link_type: 'referral_reward' })
    expect(get('/api/admin/wallets/ledger')).toMatchObject({ items: [], total: 0 })
    expect(get('/api/admin/wallets/refund-requests')).toMatchObject({ items: [], total: 0 })
    expect(get('/api/admin/payments/orders')).toMatchObject({ items: [], total: 0 })
    const reversed = get('/api/admin/wallets/wallet-demo-user/transactions/demo-ledger-referral-reversal')
    expect(reversed.transaction).toMatchObject({ wallet_id: wallet.id, amount: -4, reason_code: 'referral_reward_reversal' })
  })
  it('rejects unknown targets and mismatched wallet scopes using the Axios error response convention', () => {
    for (const path of ['/api/admin/wallets/unknown', '/api/admin/payments/orders/unknown', '/api/admin/wallets/unknown/refunds/demo-refund-referral', '/api/admin/wallets/wallet-demo-user/transactions/unknown']) {
      expectStatus(() => get(path), 404)
    }
  })
  it('requires an administrator for list and exact detail access', () => {
    for (const url of ['/api/admin/wallets', '/api/admin/wallets/wallet-demo-user', '/api/admin/payments/orders/demo-order-referral']) {
      expectStatus(() => handle({ method: 'GET', url }, false), 403)
    }
  })
  it('rejects financial writes instead of pretending to process or approve them', () => {
    const before = get('/api/admin/wallets/wallet-demo-user/refunds/demo-refund-referral')
    for (const action of ['process', 'complete', 'fail']) {
      expectStatus(() => handle({ method: 'POST', url: `/api/admin/wallets/wallet-demo-user/refunds/demo-refund-referral/${action}` }, true), 405)
    }
    expect(get('/api/admin/wallets/wallet-demo-user/refunds/demo-refund-referral')).toEqual(before)
  })
  it('does not intercept unrelated mock routes and returns fresh data per response', () => {
    expect(handle({ url: '/api/users/me', method: 'GET' }, true)).toBeNull()
    const data = get('/api/admin/wallets/wallet-demo-user'); data.gift_balance = 0
    expect(get('/api/admin/wallets/wallet-demo-user').gift_balance).toBe(14.68)
  })
  it('keeps fully reversed and unrefunded record targets separate from the partially refunded order', () => {
    const { order: full } = get('/api/admin/payments/orders/demo-order-referral-full') as { order: Record<string, unknown> }
    const { refund: completed } = get('/api/admin/wallets/wallet-demo-user/refunds/demo-refund-referral-full') as { refund: Record<string, unknown> }
    const { order: paid } = get('/api/admin/payments/orders/demo-order-referral-paid') as { order: Record<string, unknown> }
    expect(full).toMatchObject({ amount_usd: 100, refunded_amount_usd: 100, refundable_amount_usd: 0 })
    expect(completed).toMatchObject({ source_id: full.id, amount_usd: 100, status: 'succeeded' })
    expect(paid).toMatchObject({ amount_usd: 100, refunded_amount_usd: 0, refundable_amount_usd: 100 })
  })
})
