const DATE_ONLY_PATTERN = /^(\d{4})-(\d{2})-(\d{2})$/

function padDatePart(value: number): string {
  return String(value).padStart(2, '0')
}

/**
 * 将 `YYYY-MM-DD` 解析为本地时区日期，避免浏览器按 UTC 解析后串天。
 * 其他带时间/时区的信息仍交给原生 Date 处理。
 */
export function parseDateLike(dateString: string): Date {
  const matched = DATE_ONLY_PATTERN.exec(dateString)
  if (!matched) {
    return new Date(dateString)
  }

  const [, year, month, day] = matched
  return new Date(Number(year), Number(month) - 1, Number(day))
}

export function formatDateTimeLocalInput(dateString: string | null | undefined): string {
  if (!dateString) return ''

  const date = new Date(dateString)
  if (Number.isNaN(date.getTime())) return ''

  const year = date.getFullYear()
  const month = padDatePart(date.getMonth() + 1)
  const day = padDatePart(date.getDate())
  const hours = padDatePart(date.getHours())
  const minutes = padDatePart(date.getMinutes())

  return `${year}-${month}-${day}T${hours}:${minutes}`
}

export function dateTimeLocalToRfc3339(value: string | null | undefined): string | undefined {
  const trimmed = value?.trim()
  if (!trimmed) return undefined

  const parsed = new Date(trimmed)
  return Number.isNaN(parsed.getTime()) ? undefined : parsed.toISOString()
}
