import type { AdminWallet } from '@/api/admin-wallets'
import type { RedeemCodeBatch } from '@/api/admin-payments'
import type { PaymentOrder, RefundRequest } from '@/api/wallet'

export type PendingOrderWarningReason = 'missing_expiry' | 'expiring_soon'

export interface PendingOrderWarning {
  id: string
  order_no: string
  amount_usd: number
  payment_method: string
  order_kind: string
  created_at: string
  expires_at: string | null
  reason: PendingOrderWarningReason
}

export interface WalletOpsOverviewInput {
  wallets: AdminWallet[]
  orders: PaymentOrder[]
  refunds: RefundRequest[]
  redeemBatches: RedeemCodeBatch[]
  nowMs?: number
  pendingExpiryWarningWindowMinutes?: number
}

export interface WalletOpsOverview {
  walletCount: number
  activeWalletCount: number
  userWalletCount: number
  apiKeyWalletCount: number
  totalAvailableBalance: number
  totalRechargeBalance: number
  totalGiftBalance: number
  totalPackageBalance: number
  activeDailyQuotaWalletCount: number
  dailyQuotaTotalUsd: number
  dailyQuotaUsedUsd: number
  dailyQuotaRemainingUsd: number
  pendingOrderCount: number
  pendingOrderAmountUsd: number
  paidOrderCount: number
  paidOrderAmountUsd: number
  creditedOrderCount: number
  creditedOrderAmountUsd: number
  expiredOrderCount: number
  pendingOrderMissingExpiryCount: number
  pendingOrderExpiringSoonCount: number
  pendingOrderAlertCount: number
  pendingRefundCount: number
  processingRefundCount: number
  pendingRefundAmountUsd: number
  completedRefundAmountUsd: number
  redeemBatchCount: number
  activeRedeemBatchCount: number
  expiringRedeemBatchCount: number
  redeemCodeCount: number
  activeRedeemCodeCount: number
  redeemedRedeemCodeCount: number
  disabledRedeemCodeCount: number
  redeemStockValueUsd: number
  redeemRedeemedValueUsd: number
  pendingOrderWarnings: PendingOrderWarning[]
}

const DEFAULT_PENDING_EXPIRY_WARNING_WINDOW_MINUTES = 15

function asFiniteNumber(value: unknown): number {
  const parsed = Number(value ?? 0)
  return Number.isFinite(parsed) ? parsed : 0
}

function parseDateMs(value: string | null | undefined): number | null {
  if (!value) return null
  const parsed = Date.parse(value)
  return Number.isFinite(parsed) ? parsed : null
}

function sumBy<T>(items: T[], selector: (item: T) => number): number {
  return items.reduce((total, item) => total + asFiniteNumber(selector(item)), 0)
}

function isPendingOrder(order: PaymentOrder): boolean {
  return order.status === 'pending'
}

function isPaidOrder(order: PaymentOrder): boolean {
  return order.status === 'paid'
}

function isCreditedOrder(order: PaymentOrder): boolean {
  return order.status === 'credited'
}

function isExpiredOrder(order: PaymentOrder): boolean {
  return order.status === 'expired'
}

function isPendingRefund(refund: RefundRequest): boolean {
  return refund.status === 'pending_approval' || refund.status === 'approved'
}

function isProcessingRefund(refund: RefundRequest): boolean {
  return refund.status === 'processing'
}

function isCompletedRefund(refund: RefundRequest): boolean {
  return refund.status === 'succeeded'
}

function isActiveRedeemBatch(batch: RedeemCodeBatch, nowMs: number): boolean {
  if (batch.status !== 'active') return false
  const expiresAtMs = parseDateMs(batch.expires_at)
  return expiresAtMs === null || expiresAtMs > nowMs
}

function isExpiringSoonRedeemBatch(batch: RedeemCodeBatch, nowMs: number): boolean {
  if (batch.status !== 'active') return false
  const expiresAtMs = parseDateMs(batch.expires_at)
  if (expiresAtMs === null) return false
  return expiresAtMs > nowMs && expiresAtMs <= nowMs + 7 * 24 * 60 * 60 * 1000
}

export function buildWalletOpsOverview(input: WalletOpsOverviewInput): WalletOpsOverview {
  const nowMs = input.nowMs ?? Date.now()
  const pendingExpiryWindowMs = Math.max(
    1,
    input.pendingExpiryWarningWindowMinutes ?? DEFAULT_PENDING_EXPIRY_WARNING_WINDOW_MINUTES,
  ) * 60 * 1000

  const wallets = input.wallets || []
  const orders = input.orders || []
  const refunds = input.refunds || []
  const redeemBatches = input.redeemBatches || []

  const walletCount = wallets.length
  const activeWalletCount = wallets.filter(wallet => wallet.status === 'active').length
  const userWalletCount = wallets.filter(wallet => wallet.owner_type === 'user').length
  const apiKeyWalletCount = wallets.filter(wallet => wallet.owner_type === 'api_key').length
  const totalAvailableBalance = sumBy(wallets, wallet => wallet.total_available_balance ?? wallet.balance ?? 0)
  const totalRechargeBalance = sumBy(wallets, wallet => wallet.recharge_balance ?? 0)
  const totalGiftBalance = sumBy(wallets, wallet => wallet.gift_balance ?? 0)
  const totalPackageBalance = sumBy(wallets, wallet => wallet.package_balance ?? 0)
  const activeDailyQuotaWalletCount = wallets.filter(wallet => wallet.daily_quota?.has_active).length
  const dailyQuotaTotalUsd = sumBy(wallets, wallet => wallet.daily_quota?.total_usd ?? 0)
  const dailyQuotaUsedUsd = sumBy(wallets, wallet => wallet.daily_quota?.used_usd ?? 0)
  const dailyQuotaRemainingUsd = sumBy(wallets, wallet => wallet.daily_quota?.remaining_usd ?? 0)

  const pendingOrders = orders.filter(isPendingOrder)
  const paidOrders = orders.filter(isPaidOrder)
  const creditedOrders = orders.filter(isCreditedOrder)
  const expiredOrders = orders.filter(isExpiredOrder)

  const pendingOrderWarnings = pendingOrders.flatMap((order) => {
    const expiresAtMs = parseDateMs(order.expires_at)
    if (expiresAtMs === null) {
      return [{
        id: order.id,
        order_no: order.order_no,
        amount_usd: asFiniteNumber(order.amount_usd),
        payment_method: order.payment_method,
        order_kind: order.order_kind || 'unknown',
        created_at: order.created_at,
        expires_at: null,
        reason: 'missing_expiry' as const,
      }]
    }
    if (expiresAtMs <= nowMs + pendingExpiryWindowMs) {
      return [{
        id: order.id,
        order_no: order.order_no,
        amount_usd: asFiniteNumber(order.amount_usd),
        payment_method: order.payment_method,
        order_kind: order.order_kind || 'unknown',
        created_at: order.created_at,
        expires_at: order.expires_at ?? null,
        reason: 'expiring_soon' as const,
      }]
    }
    return []
  })

  const pendingOrderCount = pendingOrders.length
  const pendingOrderAmountUsd = sumBy(pendingOrders, order => order.amount_usd)
  const paidOrderCount = paidOrders.length
  const paidOrderAmountUsd = sumBy(paidOrders, order => order.amount_usd)
  const creditedOrderCount = creditedOrders.length
  const creditedOrderAmountUsd = sumBy(creditedOrders, order => order.amount_usd)
  const expiredOrderCount = expiredOrders.length
  const pendingOrderMissingExpiryCount = pendingOrderWarnings.filter(warning => warning.reason === 'missing_expiry').length
  const pendingOrderExpiringSoonCount = pendingOrderWarnings.filter(warning => warning.reason === 'expiring_soon').length
  const pendingOrderAlertCount = pendingOrderWarnings.length

  const pendingRefunds = refunds.filter(isPendingRefund)
  const processingRefunds = refunds.filter(isProcessingRefund)
  const completedRefunds = refunds.filter(isCompletedRefund)
  const pendingRefundCount = pendingRefunds.length
  const processingRefundCount = processingRefunds.length
  const pendingRefundAmountUsd = sumBy(pendingRefunds, refund => refund.amount_usd)
  const completedRefundAmountUsd = sumBy(completedRefunds, refund => refund.amount_usd)

  const activeRedeemBatches = redeemBatches.filter(batch => isActiveRedeemBatch(batch, nowMs))
  const expiringRedeemBatches = redeemBatches.filter(batch => isExpiringSoonRedeemBatch(batch, nowMs))
  const redeemCodeCount = redeemBatches.reduce((total, batch) => total + Math.max(0, Math.round(asFiniteNumber(batch.total_count))), 0)
  const activeRedeemCodeCount = activeRedeemBatches.reduce((total, batch) => total + Math.max(0, Math.round(asFiniteNumber(batch.active_count))), 0)
  const redeemedRedeemCodeCount = redeemBatches.reduce((total, batch) => total + Math.max(0, Math.round(asFiniteNumber(batch.redeemed_count))), 0)
  const disabledRedeemCodeCount = Math.max(0, redeemCodeCount - activeRedeemCodeCount - redeemedRedeemCodeCount)
  const redeemStockValueUsd = sumBy(activeRedeemBatches, batch => asFiniteNumber(batch.amount_usd) * asFiniteNumber(batch.active_count))
  const redeemRedeemedValueUsd = sumBy(redeemBatches, batch => asFiniteNumber(batch.amount_usd) * asFiniteNumber(batch.redeemed_count))

  return {
    walletCount,
    activeWalletCount,
    userWalletCount,
    apiKeyWalletCount,
    totalAvailableBalance,
    totalRechargeBalance,
    totalGiftBalance,
    totalPackageBalance,
    activeDailyQuotaWalletCount,
    dailyQuotaTotalUsd,
    dailyQuotaUsedUsd,
    dailyQuotaRemainingUsd,
    pendingOrderCount,
    pendingOrderAmountUsd,
    paidOrderCount,
    paidOrderAmountUsd,
    creditedOrderCount,
    creditedOrderAmountUsd,
    expiredOrderCount,
    pendingOrderMissingExpiryCount,
    pendingOrderExpiringSoonCount,
    pendingOrderAlertCount,
    pendingRefundCount,
    processingRefundCount,
    pendingRefundAmountUsd,
    completedRefundAmountUsd,
    redeemBatchCount: redeemBatches.length,
    activeRedeemBatchCount: activeRedeemBatches.length,
    expiringRedeemBatchCount: expiringRedeemBatches.length,
    redeemCodeCount,
    activeRedeemCodeCount,
    redeemedRedeemCodeCount,
    disabledRedeemCodeCount,
    redeemStockValueUsd,
    redeemRedeemedValueUsd,
    pendingOrderWarnings,
  }
}
