import { AxiosHeaders, type AxiosRequestConfig, type AxiosResponse } from 'axios'
import type { AdminGlobalRefund, AdminLedgerTransaction, AdminWalletDetailResponse } from '@/api/admin-wallets'
import type { PaymentOrder } from '@/api/wallet'
import { MOCK_NORMAL_USER } from './data'

const date = '2026-10-09T00:00:00Z'
const wallet: AdminWalletDetailResponse = {
  id: 'wallet-demo-user', user_id: MOCK_NORMAL_USER.id, api_key_id: null,
  owner_type: 'user', owner_name: `${MOCK_NORMAL_USER.username}（模拟）`,
  recharge_balance: 40, gift_balance: 14.68, balance: 54.68, refundable_balance: 40,
  total_recharged: 100, total_consumed: 45.32, total_refunded: 0, total_adjusted: 0,
  currency: 'USD', status: 'active', created_at: date, updated_at: date, pending_refund_count: 0,
}
const order: PaymentOrder = {
  id: 'demo-order-referral', order_no: 'PO-DEMO-REFERRAL', wallet_id: wallet.id, user_id: MOCK_NORMAL_USER.id,
  amount_usd: 100, pay_amount: 100, pay_currency: 'USD', exchange_rate: 1,
  refunded_amount_usd: 60, refundable_amount_usd: 40, payment_method: 'stripe',
  order_kind: 'wallet_recharge', gateway_order_id: 'mock-payment-referral', gateway_response: null,
  status: 'credited', created_at: date, paid_at: date, credited_at: date, expires_at: null,
}
const fullOrder: PaymentOrder = { ...order, id: 'demo-order-referral-full', order_no: 'PO-DEMO-REFERRAL-FULL', refunded_amount_usd: 100, refundable_amount_usd: 0 }
const paidOrder: PaymentOrder = { ...order, id: 'demo-order-referral-paid', order_no: 'PO-DEMO-REFERRAL-PAID', refunded_amount_usd: 0, refundable_amount_usd: 100 }
const owner = { wallet_id: wallet.id, owner_type: wallet.owner_type, owner_name: wallet.owner_name, wallet_status: wallet.status }
const refund: AdminGlobalRefund = {
  ...owner, id: 'demo-refund-referral', refund_no: 'RF-DEMO-REFERRAL', source_type: 'payment_order', source_id: order.id, payment_order_id: order.id,
  refund_mode: 'original_channel', amount_usd: 60, status: 'succeeded', reason: '演示模拟：部分退款', failure_reason: null,
  gateway_refund_id: 'mock-refund-referral', payout_method: null, payout_reference: null,
  created_at: date, updated_at: date, processed_at: date, completed_at: date,
}
const fullRefund: AdminGlobalRefund = { ...refund, id: 'demo-refund-referral-full', refund_no: 'RF-DEMO-REFERRAL-FULL', source_id: fullOrder.id, payment_order_id: fullOrder.id, amount_usd: 100 }
const transaction: AdminLedgerTransaction = {
  ...owner, id: 'demo-ledger-referral', category: 'adjust', reason_code: 'referral_reward', amount: 10,
  balance_before: 40, balance_after: 50, recharge_balance_before: 40, recharge_balance_after: 40,
  gift_balance_before: 0, gift_balance_after: 10, link_type: 'referral_reward', link_id: 'demo-reward-4',
  description: '演示模拟：邀请返利发放，不代表真实账目', created_at: date,
}
const reversal: AdminLedgerTransaction = {
  ...transaction, id: 'demo-ledger-referral-reversal', reason_code: 'referral_reward_reversal', amount: -4,
  balance_before: 50, balance_after: 46, gift_balance_before: 10, gift_balance_after: 6,
  description: '演示模拟：邀请返利冲回，不代表真实账目',
}

function response(data: object, status = 200): AxiosResponse<unknown> {
  return { data: { ...structuredClone(data), demo_mode: true }, status, statusText: status === 200 ? 'OK' : 'Error', headers: {}, config: { headers: new AxiosHeaders() } }
}
function reject(status: number, detail: string): never { throw { response: response({ detail }, status) } }

// 仅用于演示模式的定位契约，所有财务写入明确拒绝，避免默认 mock 200 被误当成审批成功。
export function handleReferralWalletLocationMock(config: AxiosRequestConfig, isAdmin: boolean): AxiosResponse<unknown> | null {
  const path = (config.url || '').split('?')[0]
  const scoped = path === '/api/admin/wallets' || path.startsWith('/api/admin/wallets/') || path === '/api/admin/payments/orders' || path.startsWith('/api/admin/payments/orders/')
  if (!scoped) return null
  if (!isAdmin) reject(403, '需要管理员权限')
  if ((config.method || 'GET').toUpperCase() !== 'GET') reject(405, '演示定位数据只读，不支持退款审批或资金操作')
  const page = { items: [], total: 0, limit: Number(config.params?.limit) || 20, offset: Number(config.params?.offset) || 0 }
  if (path === '/api/admin/wallets') return response({ ...page, items: config.params?.offset ? [] : [wallet], total: 1 })
  if (path === '/api/admin/wallets/ledger' || path === '/api/admin/wallets/refund-requests' || path === '/api/admin/payments/orders') return response(page)
  if (path === `/api/admin/wallets/${wallet.id}`) return response(wallet)
  const matchedOrder = [order, fullOrder, paidOrder].find(item => path === `/api/admin/payments/orders/${item.id}`)
  if (matchedOrder) return response({ order: matchedOrder })
  const matchedRefund = [refund, fullRefund].find(item => path === `/api/admin/wallets/${wallet.id}/refunds/${item.id}`)
  if (matchedRefund) return response({ refund: matchedRefund })
  if (path === `/api/admin/wallets/${wallet.id}/transactions/${transaction.id}`) return response({ transaction })
  if (path === `/api/admin/wallets/${wallet.id}/transactions/${reversal.id}`) return response({ transaction: reversal })
  reject(404, '演示目标不存在，请核对模拟记录 ID')
}
