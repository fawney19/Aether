/**
 * 构建完整的 API URL
 *
 * 用于需要完整 URL 的场景（如 OAuth 重定向），
 * 处理 VITE_API_URL 环境变量和路径拼接。
 */
export function getApiUrl(path: string): string {
  const base = import.meta.env.VITE_API_URL || ''
  // 移除 base 尾部的 `/`，避免拼接成 `//api/...`
  return base ? `${base.replace(/\/$/, '')}${path}` : path
}

/**
 * 拼接 base_url 和 path（与后端 src/utils/url_utils.py:join_url 语义对齐）。
 *
 * 规则：
 * 1. 去前后空白
 * 2. base_url 末尾的 "/" 全部去掉
 * 3. path 若非空则保证以 "/" 开头
 * 4. 直接拼接，不做版本嗅探也不做路径段去重
 */
export function joinUrl(baseUrl: string | null | undefined, path: string | null | undefined): string {
  const base = (baseUrl ?? '').trim().replace(/\/+$/, '')
  let p = (path ?? '').trim()
  if (!p) return base
  if (!p.startsWith('/')) p = '/' + p
  return `${base}${p}`
}
