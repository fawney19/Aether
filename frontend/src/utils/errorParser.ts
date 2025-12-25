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
  'max_concurrent': '最大并发',
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
  'rpm_limit': 'RPM 限制',
  'cache_ttl_minutes': '缓存 TTL',
  'max_probe_interval_minutes': '最大探测间隔',
}

/**
 * 错误类型映射（中文化）
 */
const errorTypeMap: Record<string, (error: ValidationError) => string> = {
  'string_too_short': (error) => {
    const minLength = error.ctx?.min_length || 10
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

  const detail = err.response?.data?.detail

  // 如果没有 detail 字段
  if (!detail) {
    return err.response?.data?.message || err.message || defaultMessage
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
