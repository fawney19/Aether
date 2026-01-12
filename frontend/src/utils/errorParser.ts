/**
 * 解析 API 错误响应，提取友好的错误信息
 */

import { isApiError } from '@/types/api-error'

/**
 * Pydantic 验证错误项
 */
interface ValidationError {
  loc: (string | number)[]
  msg: string
  type: string
  ctx?: Record<string, unknown>
}

/**
 * 字段名称映射（中文化）
 */
const fieldNameMap: Record<string, string> = {
  'api_key': 'API 密钥',
  'priority': '优先级',
  'rpm_limit': 'RPM 限制',
  'rate_limit': '速率限制',
  'daily_limit': '每日限制',
  'monthly_limit': '每月限制',
  'allowed_models': '允许的模型',
  'note': '备注',
  'is_active': '启用状态',
  'endpoint_id': 'Endpoint ID',
  'base_url': 'API 基础 URL',
  'timeout': '超时时间',
  'max_retries': '最大重试次数',
  'weight': '权重',
  'email': '邮箱',
  'username': '用户名',
  'password': '密码',
  'name': '名称',
  'display_name': '显示名称',
  'description': '描述',
  'website': '网站',
  'provider_priority': '提供商优先级',
  'billing_type': '计费类型',
  'monthly_quota_usd': '月度配额',
  'quota_reset_day': '配额重置日',
  'quota_expires_at': '配额过期时间',
  'cache_ttl_minutes': '缓存 TTL',
  'max_probe_interval_minutes': '最大探测间隔',
}

/**
 * 错误类型映射（中文化）
 */
const errorTypeMap: Record<string, (error: ValidationError) => string> = {
  'string_too_short': (error) => {
    const minLength = error.ctx?.min_length || 3
    return `长度不能少于 ${minLength} 个字符`
  },
  'string_too_long': (error) => {
    const maxLength = error.ctx?.max_length
    return `长度不能超过 ${maxLength} 个字符`
  },
  'value_error.missing': () => '此字段为必填项',
  'missing': () => '此字段为必填项',
  'type_error.none.not_allowed': () => '此字段不能为空',
  'value_error': (error) => error.msg,
  'type_error.integer': () => '必须为整数',
  'type_error.float': () => '必须为数字',
  'value_error.number.not_ge': (error) => {
    const limit = error.ctx?.limit_value
    return limit !== undefined ? `不能小于 ${limit}` : '数值过小'
  },
  'value_error.number.not_le': (error) => {
    const limit = error.ctx?.limit_value
    return limit !== undefined ? `不能大于 ${limit}` : '数值过大'
  },
  'value_error.number.not_gt': (error) => {
    const limit = error.ctx?.limit_value
    return limit !== undefined ? `必须大于 ${limit}` : '数值过小'
  },
  'value_error.number.not_lt': (error) => {
    const limit = error.ctx?.limit_value
    return limit !== undefined ? `必须小于 ${limit}` : '数值过大'
  },
  'less_than_equal': (error) => {
    const limit = error.ctx?.le
    return limit !== undefined ? `不能大于 ${limit}` : '数值过大'
  },
  'greater_than_equal': (error) => {
    const limit = error.ctx?.ge
    return limit !== undefined ? `不能小于 ${limit}` : '数值过小'
  },
  'less_than': (error) => {
    const limit = error.ctx?.lt
    return limit !== undefined ? `必须小于 ${limit}` : '数值过大'
  },
  'greater_than': (error) => {
    const limit = error.ctx?.gt
    return limit !== undefined ? `必须大于 ${limit}` : '数值过小'
  },
  'value_error.email': () => '邮箱格式不正确',
  'value_error.url': () => 'URL 格式不正确',
  'type_error.bool': () => '必须为布尔值（true/false）',
  'type_error.list': () => '必须为数组',
  'type_error.dict': () => '必须为对象',
}

/**
 * 获取字段的中文名称
 */
function getFieldName(loc: (string | number)[]): string {
  if (!loc || loc.length === 0) return '字段'

  const fieldPath = loc.filter(item => item !== 'body').join('.')
  const fieldKey = String(loc[loc.length - 1])

  return fieldNameMap[fieldKey] || fieldPath || '字段'
}

/**
 * 格式化单个验证错误
 */
function formatValidationError(error: ValidationError): string {
  const fieldName = getFieldName(error.loc)
  const errorFormatter = errorTypeMap[error.type]

  if (errorFormatter) {
    const errorMsg = errorFormatter(error)
    return `${fieldName}: ${errorMsg}`
  }

  // 默认格式
  return `${fieldName}: ${error.msg}`
}

/**
 * 解析 API 错误响应
 * @param err 错误对象
 * @param defaultMessage 默认错误信息
 * @returns 格式化的错误信息
 */
export function parseApiError(err: unknown, defaultMessage: string = '操作失败'): string {
  if (!err) return defaultMessage

  // 处理网络错误
  if (!isApiError(err) || !err.response) {
    if (err instanceof Error) {
      return err.message || defaultMessage
    }
    return '无法连接到服务器，请检查网络连接'
  }

  const data = err.response?.data

  // 1. 处理 {error: {type, message}} 格式（ProxyException 返回格式）
  if (data?.error?.message) {
    return data.error.message
  }

  const detail = data?.detail

  // 如果没有 detail 字段
  if (!detail) {
    return data?.message || err.message || defaultMessage
  }

  // 1. 处理 Pydantic 验证错误（数组格式）
  if (Array.isArray(detail)) {
    const errors = detail
      .map((error: ValidationError) => formatValidationError(error))
      .join('\n')
    return errors || defaultMessage
  }

  // 2. 处理字符串错误
  if (typeof detail === 'string') {
    return detail
  }

  // 3. 处理对象错误
  if (typeof detail === 'object') {
    // 可能是自定义错误对象
    if ((detail as Record<string, unknown>).message) {
      return String((detail as Record<string, unknown>).message)
    }
    // 尝试 JSON 序列化
    try {
      return JSON.stringify(detail, null, 2)
    } catch {
      return defaultMessage
    }
  }

  return defaultMessage
}

/**
 * 解析并提取第一个错误信息（用于简短提示）
 */
export function parseApiErrorShort(err: unknown, defaultMessage: string = '操作失败'): string {
  const fullError = parseApiError(err, defaultMessage)

  // 如果有多行错误，只取第一行
  const lines = fullError.split('\n')
  return lines[0] || defaultMessage
}

/**
 * 解析模型测试响应的错误信息
 * @param result 测试响应结果
 * @returns 格式化的错误信息
 */
export function parseTestModelError(result: {
  error?: string
  data?: {
    response?: {
      status_code?: number
      error?: string | { message?: string }
    }
  }
}): string {
  let errorMsg = result.error || '测试失败'

  // 检查HTTP状态码错误
  if (result.data?.response?.status_code) {
    const status = result.data.response.status_code
    if (status === 403) {
      errorMsg = '认证失败: API密钥无效或客户端类型不被允许'
    } else if (status === 401) {
      errorMsg = '认证失败: API密钥无效或已过期'
    } else if (status === 404) {
      errorMsg = '模型不存在: 请检查模型名称是否正确'
    } else if (status === 429) {
      errorMsg = '请求频率过高: 请稍后重试'
    } else if (status >= 500) {
      errorMsg = `服务器错误: HTTP ${status}`
    } else {
      errorMsg = `请求失败: HTTP ${status}`
    }
  }

  // 尝试从错误响应中提取更多信息
  if (result.data?.response?.error) {
    if (typeof result.data.response.error === 'string') {
      errorMsg = result.data.response.error
    } else if (result.data.response.error?.message) {
      errorMsg = result.data.response.error.message
    }
  }

  return errorMsg
}

/**
 * 解析上游模型查询错误信息
 * 将后端返回的原始错误信息（如 "HTTP 401: {json...}"）转换为友好的错误提示
 * @param error 错误字符串，格式可能是 "HTTP {status}: {json_body}" 或其他
 * @returns 友好的错误信息
 */
export function parseUpstreamModelError(error: string): string {
  if (!error) return '获取上游模型失败'

  // 匹配 "HTTP {status}: {body}" 格式
  const httpMatch = error.match(/^HTTP\s+(\d+):\s*(.*)$/s)
  if (httpMatch) {
    const status = parseInt(httpMatch[1], 10)
    const body = httpMatch[2]

    // 根据状态码生成友好消息
    let friendlyMsg = ''
    if (status === 401) {
      friendlyMsg = '密钥无效或已过期'
    } else if (status === 403) {
      friendlyMsg = '密钥权限不足'
    } else if (status === 404) {
      friendlyMsg = '模型列表接口不存在'
    } else if (status === 429) {
      friendlyMsg = '请求频率过高，请稍后重试'
    } else if (status >= 500) {
      friendlyMsg = '上游服务暂时不可用'
    }

    // 尝试从 JSON body 中提取更详细的错误信息
    if (body) {
      try {
        const parsed = JSON.parse(body)
        // 常见的错误格式: {error: {message: "..."}} 或 {error: "..."} 或 {message: "..."}
        let detailMsg = ''
        if (parsed.error?.message) {
          detailMsg = parsed.error.message
        } else if (typeof parsed.error === 'string') {
          detailMsg = parsed.error
        } else if (parsed.message) {
          detailMsg = parsed.message
        } else if (parsed.detail) {
          detailMsg = typeof parsed.detail === 'string' ? parsed.detail : JSON.stringify(parsed.detail)
        }

        // 如果提取到了详细消息，用它来丰富友好消息
        if (detailMsg) {
          // 检查是否是 token/认证相关的错误
          const lowerMsg = detailMsg.toLowerCase()
          if (lowerMsg.includes('invalid token') || lowerMsg.includes('invalid api key')) {
            return '密钥无效，请检查密钥是否正确'
          }
          if (lowerMsg.includes('expired')) {
            return '密钥已过期，请更新密钥'
          }
          if (lowerMsg.includes('quota') || lowerMsg.includes('exceeded')) {
            return '配额已用尽或超出限制'
          }
          if (lowerMsg.includes('rate limit')) {
            return '请求频率过高，请稍后重试'
          }
          // 没有匹配特定关键词，但有详细信息，使用它作为补充
          if (friendlyMsg) {
            const truncated = detailMsg.length > 80 ? `${detailMsg.substring(0, 80)  }...` : detailMsg
            return `${friendlyMsg}: ${truncated}`
          }
          // 没有友好消息，直接使用详细信息
          const truncated = detailMsg.length > 100 ? `${detailMsg.substring(0, 100)  }...` : detailMsg
          return truncated
        }
      } catch {
        // JSON 解析失败，忽略
      }
    }

    // 返回友好消息，附加状态码
    if (friendlyMsg) {
      return friendlyMsg
    }
    return `请求失败 (HTTP ${status})`
  }

  // 检查是否是请求错误
  if (error.startsWith('Request error:')) {
    const detail = error.replace('Request error:', '').trim()
    if (detail.toLowerCase().includes('timeout')) {
      return '请求超时，上游服务响应过慢'
    }
    if (detail.toLowerCase().includes('connection')) {
      return '无法连接到上游服务'
    }
    return '网络请求失败'
  }

  // 检查是否是未知 API 格式
  if (error.startsWith('Unknown API format:')) {
    return '不支持的 API 格式'
  }

  // 如果包含分号，可能是多个错误合并的，取第一个
  if (error.includes('; ')) {
    const firstError = error.split('; ')[0]
    return parseUpstreamModelError(firstError)
  }

  // 默认返回原始错误（截断过长的部分）
  if (error.length > 100) {
    return `${error.substring(0, 100)  }...`
  }
  return error
}
