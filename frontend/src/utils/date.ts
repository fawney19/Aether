const DATE_ONLY_PATTERN = /^(\d{4})-(\d{2})-(\d{2})$/

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
