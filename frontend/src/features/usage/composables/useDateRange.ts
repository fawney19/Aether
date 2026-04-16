import type { PeriodValue, DateRangeParams } from '../types'

function getTimezoneParams() {
  const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone
  const tz_offset_minutes = -new Date().getTimezoneOffset()
  return { timezone, tz_offset_minutes }
}

/**
 * 根据时间段值计算日期范围
 */
export function getDateRangeFromPeriod(period: PeriodValue): DateRangeParams {
  switch (period) {
    case 'today':
    case 'yesterday':
    case 'last7days':
    case 'last30days':
    case 'last90days':
      return {
        preset: period,
        ...getTimezoneParams()
      }
    default:
      return {} // 返回空对象表示不过滤时间
  }
}

/**
 * 格式化日期时间为时分秒
 */
export function formatDateTime(dateStr: string): string {
  // 后端返回的是 UTC 时间但没有时区标识，需要手动添加 'Z'
  const utcDateStr = dateStr.includes('Z') || dateStr.includes('+') ? dateStr : `${dateStr  }Z`
  const date = new Date(utcDateStr)

  // 只显示时分秒
  const hours = String(date.getHours()).padStart(2, '0')
  const minutes = String(date.getMinutes()).padStart(2, '0')
  const seconds = String(date.getSeconds()).padStart(2, '0')

  return `${hours}:${minutes}:${seconds}`
}

/**
 * 获取成功率颜色类名
 */
export function getSuccessRateColor(rate: number): string {
  if (rate >= 95) return 'text-green-600 dark:text-green-400'
  if (rate >= 90) return 'text-yellow-600 dark:text-yellow-400'
  return 'text-red-600 dark:text-red-400'
}
