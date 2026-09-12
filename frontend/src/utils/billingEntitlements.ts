import { getI18nLocale } from '@/i18n'

import type {
  BillingEntitlement,
  UsagePolicyEntitlement,
  UsagePolicyRule,
  UserPlanEntitlement,
} from '@/api/billing'

export function isPlanEntitlementReplacementCandidate(
  entitlement: Pick<UserPlanEntitlement, 'status' | 'expires_at'>,
  nowMs = Date.now(),
): boolean {
  if (entitlement.status !== 'active' || !entitlement.expires_at) return false
  const expiresAtMs = Date.parse(entitlement.expires_at)
  return Number.isFinite(expiresAtMs) && expiresAtMs > nowMs
}

export function entitlementsWillReplaceExisting(
  incoming: BillingEntitlement[] | undefined,
  existing: BillingEntitlement[] | undefined
): boolean {
  const incomingItems = incoming || []
  const existingItems = existing || []
  if (
    hasEntitlementType(incomingItems, 'daily_quota')
    && hasEntitlementType(existingItems, 'daily_quota')
  ) return true
  if (
    hasEntitlementType(incomingItems, 'membership_group')
    && hasEntitlementType(existingItems, 'membership_group')
  ) return true

  const incomingGroups = new Set(entitlementReplacementGroups(incomingItems))
  return incomingGroups.size > 0
    && entitlementReplacementGroups(existingItems).some(group => incomingGroups.has(group))
}

export function entitlementReplacementGroups(
  entitlements: BillingEntitlement[] | undefined
): string[] {
  return [...new Set(
    (entitlements || [])
      .map(item => item.replacement_group?.trim() || '')
      .filter(Boolean)
  )]
}

function hasEntitlementType(
  entitlements: BillingEntitlement[],
  type: BillingEntitlement['type']
): boolean {
  return entitlements.some(item => item.type === type)
}

export function usagePolicyEntitlementLabels(policy: UsagePolicyEntitlement): string[] {
  const labels = policy.rules.map(formatUsagePolicyRuleLabel)
  const name = policy.name?.trim()

  if (name) labels.unshift(`使用策略 ${name}`)
  return labels.length > 0 ? labels : ['使用限制']
}

function formatUsagePolicyRuleLabel(rule: UsagePolicyRule): string {
  const limit = formatLimit(rule.limit)

  if (rule.metric === 'concurrency') return `并发 ${limit}`
  const amount = rule.metric === 'actual_cost_usd' ? formatUsd(rule.limit) : null

  switch (rule.window.kind) {
    case 'rolling':
      if (amount) return `滚动 ${formatWindowDuration(rule.window.seconds)} ${amount}`
      if (rule.window.seconds === 1) return `QPS ${limit}`
      if (rule.window.seconds === 60) return `RPM ${limit}`
      return `滚动 ${formatWindowDuration(rule.window.seconds)} ${limit} 次`
    case 'calendar_day':
      return `${amount ? `每日 ${amount}` : `每日 ${limit} 次`}${formatCalendarTimezone(rule.window.timezone)}`
    case 'calendar_week':
      return `${amount ? `每周 ${amount}` : `每周 ${limit} 次`}${formatCalendarWeek(rule.window.timezone, rule.window.week_start)}`
    case 'calendar_month':
      return `${amount ? `每月 ${amount}` : `每月 ${limit} 次`}${formatCalendarTimezone(rule.window.timezone)}`
    case 'subscription_period':
      return amount ? `套餐周期 ${amount}` : `套餐周期 ${limit} 次`
    case 'concurrent':
      return `并发窗口 ${limit} 次`
  }
}

function formatCalendarTimezone(timezone?: string): string {
  return `（${timezone?.trim() || '系统时区'}）`
}

function formatCalendarWeek(timezone?: string, weekStart?: number): string {
  const weekdays = ['周一', '周二', '周三', '周四', '周五', '周六', '周日']
  const normalizedWeekStart = typeof weekStart === 'number'
    && Number.isInteger(weekStart)
    && weekStart >= 1
    && weekStart <= 7
    ? weekStart
    : 1
  return `（${timezone?.trim() || '系统时区'}，${weekdays[normalizedWeekStart - 1]}开始）`
}

function formatUsd(limit: number): string {
  return `$${Number(limit || 0).toLocaleString(getI18nLocale(), { maximumFractionDigits: 8 })}`
}

function formatWindowDuration(seconds: number): string {
  if (seconds % 86_400 === 0) return `${seconds / 86_400} 天`
  if (seconds % 3_600 === 0) return `${seconds / 3_600} 小时`
  if (seconds % 60 === 0) return `${seconds / 60} 分钟`
  return `${seconds} 秒`
}

function formatLimit(limit: number): string {
  return Number(limit || 0).toLocaleString(getI18nLocale())
}
