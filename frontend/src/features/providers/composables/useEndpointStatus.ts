import type { EndpointHealthDetail } from '@/api/endpoints'

// 端点状态枚举
export type EndpointStatus = 'disabled' | 'no_keys' | 'keys_disabled' | 'available'

const ENDPOINT_SORT_ORDER = [
  'claude:chat',
  'claude:cli',
  'openai:chat',
  'openai:responses',
  'openai:responses:compact',
  'gemini:chat',
  'gemini:cli',
  'openai:video',
  'gemini:video',
]

/**
 * 端点排序
 */
export function sortEndpoints<T extends { api_format: string }>(endpoints: T[]): T[] {
  return [...endpoints].sort((a, b) => {
    return ENDPOINT_SORT_ORDER.indexOf(a.api_format) - ENDPOINT_SORT_ORDER.indexOf(b.api_format)
  })
}

/**
 * 获取端点状态
 */
export function getEndpointStatus(endpoint: EndpointHealthDetail): EndpointStatus {
  if (endpoint.is_active === false) {
    return 'disabled'
  }
  if ((endpoint.active_keys ?? 0) === 0) {
    return (endpoint.total_keys ?? 0) > 0 ? 'keys_disabled' : 'no_keys'
  }
  return 'available'
}

/**
 * 判断端点是否可用
 */
export function isEndpointAvailable(endpoint: EndpointHealthDetail): boolean {
  return getEndpointStatus(endpoint) === 'available'
}

/**
 * 根据健康分数获取颜色
 */
export function getHealthScoreColor(score: number | undefined | null): string {
  if (score === undefined || score === null) {
    return 'bg-muted-foreground/40'
  }
  if (score >= 0.8) return 'bg-green-500'
  if (score >= 0.5) return 'bg-amber-500'
  return 'bg-red-500'
}

/**
 * 端点不可用时进度条颜色
 */
export function getEndpointDotColor(endpoint: EndpointHealthDetail): string {
  if (!isEndpointAvailable(endpoint)) {
    return 'bg-muted-foreground/40'
  }
  return getHealthScoreColor(endpoint.health_score)
}

/**
 * 端点提示文本
 */
export function getEndpointTooltip(endpoint: EndpointHealthDetail): string {
  const format = endpoint.api_format
  const status = getEndpointStatus(endpoint)

  switch (status) {
    case 'disabled':
      return `${format}: 端点禁用`
    case 'no_keys':
      return `${format}: 未配置密钥`
    case 'keys_disabled':
      return `${format}: 无可用密钥`
    case 'available': {
      const score = endpoint.health_score
      if (score === undefined || score === null) {
        return `${format}: 暂无健康数据`
      }
      return `${format}: 健康度 ${(score * 100).toFixed(0)}%`
    }
  }
}
