/**
 * 客户端 CSV 导出工具
 * 将数据转为 CSV 并触发浏览器下载
 */

function escapeCsvField(value: string | number | null | undefined): string {
  if (value == null) return ''
  const str = String(value)
  if (str.includes(',') || str.includes('"') || str.includes('\n')) {
    return `"${str.replace(/"/g, '""')}"`
  }
  return str
}

/**
 * 生成 CSV 内容并触发下载
 * @param filename 文件名（不含扩展名）
 * @param headers 列标题
 * @param rows 数据行
 */
export function downloadCsv(
  filename: string,
  headers: string[],
  rows: (string | number | null | undefined)[][],
): void {
  const bom = '\uFEFF' // UTF-8 BOM，确保 Excel 正确识别中文
  const headerLine = headers.map(escapeCsvField).join(',')
  const dataLines = rows.map(row => row.map(escapeCsvField).join(','))
  const csv = bom + [headerLine, ...dataLines].join('\n')

  const blob = new Blob([csv], { type: 'text/csv;charset=utf-8' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `${filename}.csv`
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  URL.revokeObjectURL(url)
}

/**
 * 从对象数组生成 CSV 下载
 * @param filename 文件名
 * @param data 对象数组
 * @param columns 列定义 [{ key, label }]
 */
export function downloadCsvFromObjects<T extends Record<string, unknown>>(
  filename: string,
  data: T[],
  columns: { key: string; label: string }[],
): void {
  const headers = columns.map(c => c.label)
  const rows = data.map(item =>
    columns.map(c => item[c.key] as string | number | null),
  )
  downloadCsv(filename, headers, rows)
}
