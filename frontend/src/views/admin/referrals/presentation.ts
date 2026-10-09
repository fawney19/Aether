import { getI18nLocale } from '@/i18n'
import type { ReferralRewardRecord } from '@/api/referrals'

export function usd(value: number): string {
  return `$${Number(value || 0).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 8 })}`
}
export function date(value?: number | null): string {
  return value ? new Date(value * 1000).toLocaleString(getI18nLocale()) : '未记录'
}
export function trigger(value: string): string {
  return ({ registration: '注册', email_verified: '邮箱验证', first_paid_order: '首次实际付款', payment: '实际付款', paid_order: '实际付款' } as Record<string, string>)[value] || value
}
export function rewardType(value: string): string {
  return value === 'percent' ? '比例返利' : value === 'headcount' ? '人头返利' : value
}
export function rewardStatus(value: string): string {
  return ({ pending: '待发', applying: '处理中', failed: '发放失败', applied: '已发', reversed: '已发', voided: '已作废' } as Record<string, string>)[value] || value
}
export function statusVariant(value: string): 'success' | 'warning' | 'destructive' | 'secondary' | 'outline' {
  if (value === 'applied' || value === 'reversed') return 'success'
  if (value === 'failed') return 'destructive'
  if (value === 'pending' || value === 'applying') return 'warning'
  return 'secondary'
}
export function reversalStatus(item: ReferralRewardRecord): string {
  if (item.pending_reversal_amount_usd > 0) return '待冲回'
  if (item.status === 'reversed' || (item.amount_usd > 0 && item.reversed_amount_usd >= item.amount_usd)) return '全部冲回'
  return item.reversed_amount_usd > 0 ? '部分冲回' : '无冲回'
}
export function source(item: ReferralRewardRecord): string {
  if (item.source_order_id) return item.source_order_no || item.source_order_id
  if (item.trigger_point === 'registration') return '注册奖励'
  if (item.trigger_point === 'email_verified') return '邮箱验证奖励'
  return '来源订单未记录'
}
export function orderStatus(value: string): string {
  return ({ pending: '待付款', paid: '已付款', credited: '已入账', refunded: '已退款', failed: '失败', cancelled: '已取消', expired: '已过期' } as Record<string, string>)[value] || value
}
export function refundStatus(value: string): string {
  return ({ pending: '待审批', processing: '处理中', succeeded: '已成功', failed: '失败', cancelled: '已取消' } as Record<string, string>)[value] || value
}
