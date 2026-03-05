export function walletStatusLabel(status: string | null | undefined): string {
  const labels: Record<string, string> = {
    active: '正常',
    suspended: '已冻结',
    closed: '已关闭',
  }
  if (!status) return '未知'
  return labels[status] || status
}

export function formatWalletCurrency(
  value: number | null | undefined,
  options?: { decimals?: number }
): string {
  const decimals = options?.decimals ?? 2
  const amount = Number(value ?? 0)
  return `$${amount.toFixed(decimals)}`
}

export function walletStatusBadge(status: string | null | undefined): string {
  if (status === 'active') return 'success'
  if (status === 'suspended') return 'warning'
  if (status === 'closed') return 'destructive'
  return 'secondary'
}

export function walletTransactionCategoryLabel(category: string | null | undefined): string {
  const labels: Record<string, string> = {
    recharge: '充值',
    gift: '赠款',
    adjust: '调账',
    refund: '退款',
  }
  if (!category) return '未知'
  return labels[category] || category
}

export function walletTransactionReasonLabel(reasonCode: string | null | undefined): string {
  const labels: Record<string, string> = {
    topup_admin_manual: '人工充值',
    topup_gateway: '支付充值',
    topup_card_code: '卡密充值',
    gift_initial: '初始赠款',
    gift_campaign: '活动赠款',
    gift_expire_reclaim: '赠款回收',
    adjust_admin: '人工调账',
    adjust_system: '系统调账',
    refund_out: '退款扣减',
    refund_revert: '退款回补',
  }
  if (!reasonCode) return '未知'
  return labels[reasonCode] || reasonCode
}

export function paymentMethodLabel(method: string | null | undefined): string {
  const labels: Record<string, string> = {
    alipay: '支付宝',
    wechat: '微信支付',
    admin_manual: '人工充值',
    card_code: '充值卡',
    gift_code: '礼品卡',
    card_recharge: '卡密充值',
    bank_transfer: '银行转账',
    offline: '线下转账',
  }
  if (!method) return '-'
  return labels[method] || method
}

export function paymentStatusLabel(status: string | null | undefined): string {
  const labels: Record<string, string> = {
    pending: '待支付',
    paid: '已支付',
    credited: '已到账',
    failed: '支付失败',
    expired: '已过期',
    refunding: '退款中',
    refunded: '已退款',
  }
  if (!status) return '未知'
  return labels[status] || status
}

export function walletLinkTypeLabel(type: string | null | undefined): string {
  const labels: Record<string, string> = {
    payment_order: '充值订单',
    refund_request: '退款申请',
    admin_action: '后台操作',
    system_task: '系统任务',
    campaign: '活动批次',
    usage: '用量记录',
  }
  if (!type) return '-'
  return labels[type] || '其他'
}

export function paymentStatusBadge(status: string | null | undefined): string {
  if (status === 'credited' || status === 'refunded') return 'success'
  if (status === 'paid' || status === 'refunding') return 'outline'
  if (status === 'pending') return 'secondary'
  if (status === 'expired') return 'warning'
  if (status === 'failed') return 'destructive'
  return 'secondary'
}

export function refundModeLabel(mode: string | null | undefined): string {
  const labels: Record<string, string> = {
    original_channel: '原路退回',
    offline_payout: '线下打款',
  }
  if (!mode) return '-'
  return labels[mode] || mode
}

export function refundStatusLabel(status: string | null | undefined): string {
  const labels: Record<string, string> = {
    pending_approval: '待审批',
    approved: '已审批',
    processing: '处理中',
    succeeded: '已完成',
    failed: '已失败',
    cancelled: '已取消',
  }
  if (!status) return '未知'
  return labels[status] || status
}

export function refundStatusBadge(status: string | null | undefined): string {
  if (status === 'succeeded') return 'success'
  if (status === 'processing') return 'outline'
  if (status === 'pending_approval' || status === 'approved') return 'secondary'
  if (status === 'failed' || status === 'cancelled') return 'destructive'
  return 'secondary'
}

export function callbackStatusLabel(status: string | null | undefined): string {
  const labels: Record<string, string> = {
    processed: '已处理',
    duplicate: '重复回调',
    ignored: '已忽略',
    invalid_signature: '验签失败',
    error: '处理失败',
  }
  if (!status) return '未知'
  return labels[status] || status
}

export function callbackStatusBadge(status: string | null | undefined): string {
  if (status === 'processed') return 'success'
  if (status === 'duplicate' || status === 'ignored') return 'secondary'
  if (status === 'invalid_signature' || status === 'error') return 'destructive'
  return 'outline'
}
