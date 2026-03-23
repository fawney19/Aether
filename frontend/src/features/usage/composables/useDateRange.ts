import type { PeriodValue, DateRangeParams } from '../types'

/**
 * 格式化日期为 ISO 格式（不带毫秒，兼容 FastAPI datetime 解析）
 */
function formatDateForApi(date: Date): string {
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}

function getTimezoneParams() {
  const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone
  const tz_offset_minutes = -new Date().getTimezoneOffset()
  return { timezone, tz_offset_minutes }
}

/**
 * 根据时间段值计算日期范围
 */
export function getDateRangeFromPeriod(period: PeriodValue): DateRangeParams {
  const now = new Date()
  let startDate: Date
  let endDate = new Date(now)

  switch (period) {
    case 'today':
      startDate = new Date(now.getFullYear(), now.getMonth(), now.getDate())
      break
    case 'last7days':
      startDate = new Date(now.getTime() - 7 * 24 * 60 * 60 * 1000)
      break
    case 'last30days':
      startDate = new Date(now.getTime() - 30 * 24 * 60 * 60 * 1000)
      break
    case 'last180days':
      startDate = new Date(now.getTime() - 180 * 24 * 60 * 60 * 1000)
      break
    case 'last1year':
      startDate = new Date(now.getFullYear() - 1, now.getMonth(), now.getDate())
      break
    default:
      return {} // 返回空对象表示不过滤时间
  }

  return {
    start_date: formatDateForApi(startDate),
    end_date: formatDateForApi(endDate),
    preset: period,
    ...getTimezoneParams()
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
