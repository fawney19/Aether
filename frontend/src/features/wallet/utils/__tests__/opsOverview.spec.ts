import { describe, expect, it } from 'vitest'

import { buildWalletOpsOverview } from '../opsOverview'
import type { AdminWallet } from '@/api/admin-wallets'
import type { RedeemCodeBatch } from '@/api/admin-payments'
import type { PaymentOrder, RefundRequest } from '@/api/wallet'

function wallet(overrides: Partial<AdminWallet> = {}): AdminWallet {
  return {
    id: 'wallet-1',
    user_id: 'user-1',
    api_key_id: null,
    owner_type: 'user',
    owner_name: 'Alice',
    balance: 100,
    recharge_balance: 80,
    gift_balance: 20,
    refundable_balance: 80,
    currency: 'USD',
    status: 'active',
    total_recharged: 120,
    total_consumed: 10,
    total_refunded: 0,
    total_adjusted: 0,
    created_at: '2026-05-29T00:00:00Z',
    updated_at: '2026-05-29T00:00:00Z',
    wallet_balance: 100,
    package_balance: 10,
    total_available_balance: 110,
    daily_quota: {
      has_active: true,
      total_usd: 50,
      used_usd: 20,
      remaining_usd: 30,
      allow_wallet_overage: true,
    },
    ...overrides,
  }
}

function order(overrides: Partial<PaymentOrder> = {}): PaymentOrder {
  return {
    id: 'order-1',
    order_no: 'po_1',
    wallet_id: 'wallet-1',
    user_id: 'user-1',
    amount_usd: 10,
    pay_amount: 10,
    pay_currency: 'USD',
    exchange_rate: 1,
    refunded_amount_usd: 0,
    refundable_amount_usd: 0,
    payment_method: 'stripe',
    payment_provider: 'stripe',
    payment_channel: 'card',
    order_kind: 'wallet_recharge',
    product_id: null,
    product_snapshot: null,
    fulfillment_status: 'pending',
    fulfillment_error: null,
    gateway_order_id: 'gw-1',
    gateway_response: null,
    status: 'pending',
    created_at: '2026-05-29T00:00:00Z',
    paid_at: null,
    credited_at: null,
    expires_at: '2026-05-29T00:20:00Z',
    ...overrides,
  }
}

function refund(overrides: Partial<RefundRequest> = {}): RefundRequest {
  return {
    id: 'refund-1',
    refund_no: 'rf_1',
    payment_order_id: 'order-1',
    source_type: 'payment_order',
    source_id: 'order-1',
    refund_mode: 'offline_payout',
    amount_usd: 1,
    status: 'pending_approval',
    reason: null,
    failure_reason: null,
    gateway_refund_id: null,
    payout_method: null,
    payout_reference: null,
    payout_proof: null,
    created_at: '2026-05-29T00:00:00Z',
    updated_at: '2026-05-29T00:00:00Z',
    processed_at: null,
    completed_at: null,
    ...overrides,
  }
}

function redeemBatch(overrides: Partial<RedeemCodeBatch> = {}): RedeemCodeBatch {
  return {
    id: 'batch-1',
    name: 'Campaign',
    amount_usd: 1,
    currency: 'USD',
    balance_bucket: 'recharge',
    total_count: 10,
    redeemed_count: 3,
    active_count: 7,
    status: 'active',
    description: null,
    created_by: null,
    expires_at: '2026-06-01T00:00:00Z',
    created_at: '2026-05-29T00:00:00Z',
    updated_at: '2026-05-29T00:00:00Z',
    ...overrides,
  }
}

describe('wallet ops overview', () => {
  it('aggregates wallet, order, refund, and redeem-code operating metrics', () => {
    const overview = buildWalletOpsOverview({
      nowMs: Date.parse('2026-05-29T00:00:00Z'),
      pendingExpiryWarningWindowMinutes: 15,
      wallets: [
        wallet(),
        wallet({
          id: 'wallet-2',
          user_id: null,
          api_key_id: 'key-1',
          owner_type: 'api_key',
          status: 'suspended',
          balance: 5,
          recharge_balance: 5,
          gift_balance: 0,
          package_balance: 0,
          total_available_balance: 5,
          daily_quota: null,
        }),
      ],
      orders: [
        order({
          id: 'pending-expiring',
          order_no: 'po_expiring',
          amount_usd: 12,
          expires_at: '2026-05-29T00:05:00Z',
        }),
        order({
          id: 'pending-missing-expiry',
          order_no: 'po_missing',
          amount_usd: 7,
          expires_at: null,
        }),
        order({ id: 'paid-order', status: 'paid', amount_usd: 9 }),
        order({ id: 'credited-order', status: 'credited', amount_usd: 20 }),
        order({ id: 'expired-order', status: 'expired', amount_usd: 5 }),
      ],
      refunds: [
        refund({ amount_usd: 3, status: 'pending_approval' }),
        refund({ id: 'refund-2', amount_usd: 2, status: 'approved' }),
        refund({ id: 'refund-3', amount_usd: 4, status: 'processing' }),
        refund({ id: 'refund-4', amount_usd: 6, status: 'succeeded' }),
      ],
      redeemBatches: [
        redeemBatch(),
        redeemBatch({
          id: 'batch-2',
          status: 'disabled',
          amount_usd: 2,
          total_count: 5,
          redeemed_count: 1,
          active_count: 0,
          expires_at: null,
        }),
        redeemBatch({
          id: 'batch-3',
          amount_usd: 5,
          total_count: 10,
          redeemed_count: 0,
          active_count: 10,
          expires_at: '2026-05-28T00:00:00Z',
        }),
      ],
    })

    expect(overview.walletCount).toBe(2)
    expect(overview.activeWalletCount).toBe(1)
    expect(overview.totalAvailableBalance).toBe(115)
    expect(overview.totalPackageBalance).toBe(10)
    expect(overview.activeDailyQuotaWalletCount).toBe(1)
    expect(overview.dailyQuotaRemainingUsd).toBe(30)

    expect(overview.pendingOrderCount).toBe(2)
    expect(overview.pendingOrderAmountUsd).toBe(19)
    expect(overview.paidOrderCount).toBe(1)
    expect(overview.creditedOrderAmountUsd).toBe(20)
    expect(overview.expiredOrderCount).toBe(1)
    expect(overview.pendingOrderAlertCount).toBe(2)
    expect(overview.pendingOrderMissingExpiryCount).toBe(1)
    expect(overview.pendingOrderExpiringSoonCount).toBe(1)

    expect(overview.pendingRefundCount).toBe(2)
    expect(overview.processingRefundCount).toBe(1)
    expect(overview.pendingRefundAmountUsd).toBe(5)
    expect(overview.completedRefundAmountUsd).toBe(6)

    expect(overview.redeemBatchCount).toBe(3)
    expect(overview.activeRedeemBatchCount).toBe(1)
    expect(overview.expiringRedeemBatchCount).toBe(1)
    expect(overview.activeRedeemCodeCount).toBe(7)
    expect(overview.redeemedRedeemCodeCount).toBe(4)
    expect(overview.redeemStockValueUsd).toBe(7)
    expect(overview.redeemRedeemedValueUsd).toBe(5)
  })

  it('returns empty metrics for empty input', () => {
    const overview = buildWalletOpsOverview({
      wallets: [],
      orders: [],
      refunds: [],
      redeemBatches: [],
      nowMs: Date.parse('2026-05-29T00:00:00Z'),
    })

    expect(overview.walletCount).toBe(0)
    expect(overview.pendingOrderWarnings).toEqual([])
    expect(overview.redeemStockValueUsd).toBe(0)
  })
})
