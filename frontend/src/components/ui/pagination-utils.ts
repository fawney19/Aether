/**
 * 获取缓存的 pageSize 值
 * @param cacheKey 缓存键名
 * @param defaultValue 默认值
 * @param validOptions 有效的选项列表，用于验证缓存值是否有效
 * @returns 缓存的 pageSize 或默认值
 */
export function getPageSizeCache(
  cacheKey: string,
  defaultValue: number = 20,
  validOptions: number[] = [10, 20, 50, 100]
): number {
  const cached = localStorage.getItem(cacheKey)
  if (cached) {
    const cachedSize = parseInt(cached, 10)
    if (!isNaN(cachedSize) && validOptions.includes(cachedSize)) {
      return cachedSize
    }
  }
  return defaultValue
}
