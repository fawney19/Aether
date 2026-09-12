/**
 * Schema-Driven 工具函数
 *
 * 根据后端 JSON Schema（含 x-* 扩展字段）动态生成表单、构建请求、解析配置和格式化显示。
 * 替代原有的手写模板文件（new-api.ts, anyrouter.ts 等）。
 */

import type { ConnectorAuthType, SaveConfigRequest } from '@/api/providerOps'
import type {
  AuthTemplateField,
  AuthTemplateFieldGroup,
  BalanceExtraItem,
} from './types'
import { PROXY_FIELD_GROUP, buildProxyConfig, parseProxyConfig } from './types'
import { executeFieldHook } from './field-hooks'

// ==================== 类型定义 ====================

/** 后端 credentials_schema 的类型 */
export type { CredentialsSchema } from '@/api/providerCredentials'
import type { CredentialsSchema, BalanceExtraFormat, SchemaProperty, SchemaFieldGroup } from '@/api/providerCredentials'

// ==================== Schema -> 表单字段 ====================

/**
 * 从 schema 生成表单字段分组
 */
export function schemaToFieldGroups(
  schema: CredentialsSchema,
  providerWebsite?: string,
): AuthTemplateFieldGroup[] {
  const groups: AuthTemplateFieldGroup[] = []
  const fieldGroups = schema['x-field-groups']
  const properties = schema.properties
  const defaultBaseUrl = schema['x-default-base-url']

  if (fieldGroups && fieldGroups.length > 0) {
    for (const group of fieldGroups) {
      const fields: AuthTemplateField[] = []
      for (const fieldKey of group.fields) {
        const prop = properties[fieldKey]
        if (!prop) continue
        fields.push(propertyToField(fieldKey, prop, schema, providerWebsite, defaultBaseUrl, group))
      }
      if (fields.length === 0) continue

      const result: AuthTemplateFieldGroup = { fields }
      if (group.layout === 'inline') {
        result.layout = 'inline'
      }
      groups.push(result)
    }
  } else {
    // 没有分组定义，按 properties 顺序逐个展示
    for (const [key, prop] of Object.entries(properties)) {
      groups.push({
        fields: [propertyToField(key, prop, schema, providerWebsite, defaultBaseUrl)],
      })
    }
  }

  // 追加代理配置
  groups.push(PROXY_FIELD_GROUP)

  return groups
}

function propertyToField(
  key: string,
  prop: SchemaProperty,
  schema: CredentialsSchema,
  providerWebsite?: string,
  defaultBaseUrl?: string,
  group?: SchemaFieldGroup,
): AuthTemplateField {
  const isSensitive = prop['x-sensitive'] === true
  const inputType = prop['x-input-type']

  let fieldType: AuthTemplateField['type'] = 'text'
  if (inputType === 'password' || isSensitive) {
    fieldType = 'password'
  }

  // base_url 特殊处理 placeholder
  let placeholder = ''
  if (key === 'base_url') {
    placeholder = providerWebsite || defaultBaseUrl || ''
  }

  // 是否必填
  const isRequired = schema.required?.includes(key) ?? false

  // flex 值
  let flex: number | undefined
  if (group?.['x-flex']?.[key]) {
    flex = group['x-flex'][key]
  }

  // helpText
  let helpText: string | undefined
  if (prop['x-help']) {
    helpText = prop['x-help']
  } else if (group?.['x-help'] && group.fields.length === 1 && group.fields[0] === key) {
    helpText = group['x-help']
  }

  const field: AuthTemplateField = {
    key,
    label: prop.title || key,
    type: fieldType,
    placeholder,
    required: key === 'base_url' ? !providerWebsite && !defaultBaseUrl : isRequired,
    sensitive: isSensitive,
  }

  if (flex) field.flex = flex
  if (helpText) field.helpText = helpText
  if (prop['x-default-value']) field.defaultValue = prop['x-default-value']

  return field
}

// ==================== 构建请求 ====================

/**
 * 从 schema 和表单数据构建 SaveConfigRequest
 */
export function buildRequestFromSchema(
  schema: CredentialsSchema,
  architectureId: string,
  formData: Record<string, unknown>,
  providerWebsite?: string,
): SaveConfigRequest {
  const baseUrl = (formData.base_url as string) || providerWebsite || schema['x-default-base-url'] || ''
  const authType = schema['x-auth-type'] || 'api_key'

  // 构建 credentials：除 base_url 和代理字段外的所有 schema 属性
  const credentials: Record<string, unknown> = {}
  for (const key of Object.keys(schema.properties)) {
    if (key === 'base_url') continue
    const v = formData[key]
    credentials[key] = typeof v === 'string' ? v.trim() : v ?? ''
  }

  return {
    architecture_id: architectureId,
    base_url: baseUrl,
    connector: {
      auth_type: authType as ConnectorAuthType,
      config: {
        ...(schema['x-auth-method'] ? { auth_method: schema['x-auth-method'] } : {}),
        ...buildProxyConfig(formData),
      },
      credentials,
    },
    actions: {},
    schedule: {},
  }
}

// ==================== 解析配置 ====================

/**
 * 从已有配置解析表单数据
 */
export function parseConfigFromSchema(
  schema: CredentialsSchema,
  config: Record<string, unknown> | null | undefined,
): Record<string, unknown> {
  const connector = config?.connector as Record<string, unknown> | undefined
  const connectorConfig = connector?.config as Record<string, unknown> | undefined
  const proxyData = parseProxyConfig(connectorConfig)
  const result: Record<string, unknown> = {
    base_url: (config?.base_url as string) || '',
    ...proxyData,
  }

  // 从 credentials 中提取各 schema 属性
  const credentials = connector?.credentials as Record<string, unknown> | undefined
  for (const key of Object.keys(schema.properties)) {
    if (key === 'base_url') continue
    result[key] = credentials?.[key] || ''
  }

  return result
}

// ==================== 验证 ====================

/** 安全获取字符串值并 trim（表单字段值可能为 string 或其他类型） */
function trimValue(v: unknown): string {
  return typeof v === 'string' ? v.trim() : ''
}

/**
 * 根据 schema 验证表单数据
 * @returns 错误消息，无错误返回 null
 */
export function validateFromSchema(
  schema: CredentialsSchema,
  formData: Record<string, unknown>,
): string | null {
  const validations = schema['x-validation']
  if (!validations) return null

  for (const rule of validations) {
    switch (rule.type) {
      case 'required': {
        if (!rule.fields) break
        for (const field of rule.fields) {
          if (!trimValue(formData[field])) {
            return rule.message
          }
        }
        break
      }
      case 'any_required': {
        if (!rule.fields) break
        const hasAny = rule.fields.some((f) => !!trimValue(formData[f]))
        if (!hasAny) {
          return rule.message
        }
        break
      }
      case 'conditional_required': {
        const ifField = rule.if
        const unlessField = rule.unless
        const thenFields = rule.then
        if (!ifField || !thenFields) break

        const ifHasValue = !!trimValue(formData[ifField])
        const unlessHasValue = unlessField ? !!trimValue(formData[unlessField]) : false

        if (ifHasValue && !unlessHasValue) {
          for (const field of thenFields) {
            if (!trimValue(formData[field])) {
              return rule.message
            }
          }
        }
        break
      }
    }
  }

  return null
}

// ==================== Quota 格式化 ====================

/**
 * 根据 schema 格式化 quota 显示
 */
export function formatQuotaFromSchema(
  schema: CredentialsSchema,
  quota: number,
): string {
  const divisor = schema['x-quota-divisor']
  const currency = schema['x-currency'] || 'USD'

  let value = quota
  if (divisor) {
    value = quota / divisor
  }

  const symbol = currency === 'USD' ? '$' : currency
  if (value >= 1) {
    return `${symbol}${value.toFixed(2)}`
  }
  return `${symbol}${value.toFixed(4)}`
}

// ==================== Balance Extra 格式化 ====================

/**
 * 根据 schema 格式化余额附加信息
 */
export function formatBalanceExtraFromSchema(
  schema: CredentialsSchema,
  extra: Record<string, unknown>,
): BalanceExtraItem[] {
  const formats = schema['x-balance-extra-format']
  if (!formats) return []

  const items: BalanceExtraItem[] = []

  for (const fmt of formats) {
    switch (fmt.type) {
      case 'window_limit': {
        const item = formatWindowLimitItem(extra, fmt)
        if (item) items.push(item)
        break
      }
      case 'daily_quota': {
        const item = formatDailyQuotaItem(extra, fmt)
        if (item) items.push(item)
        break
      }
      case 'weekly_spent': {
        const item = formatWeeklySpentItem(extra, fmt)
        if (item) items.push(item)
        break
      }
      case 'monthly_expiry': {
        const item = formatMonthlyExpiryItem(extra, fmt)
        if (item) items.push(item)
        break
      }
    }
  }

  return items
}

function formatWindowLimitItem(
  extra: Record<string, unknown>,
  fmt: BalanceExtraFormat,
): BalanceExtraItem | null {
  if (!fmt.source) return null
  const rawLimit = extra[fmt.source]
  if (!rawLimit || typeof rawLimit !== 'object') return null
  const limit = rawLimit as Record<string, unknown>
  if (limit.remaining === undefined || limit.limit === undefined || limit.limit === 0) {
    return null
  }

  const limitVal = Number(limit.limit)
  const remainingVal = Number(limit.remaining)
  const percent = Math.round((remainingVal / limitVal) * 100)
  const divisor = fmt.unit_divisor || 1
  const remaining = (remainingVal / divisor).toFixed(2)
  const total = (limitVal / divisor).toFixed(2)

  return {
    label: fmt.label,
    value: `${percent}%`,
    percent,
    resetsAt: typeof limit.resets_at === 'number' ? limit.resets_at : undefined,
    tooltip: `$${remaining} / $${total}`,
  }
}

function formatDailyQuotaItem(
  extra: Record<string, unknown>,
  fmt: BalanceExtraFormat,
): BalanceExtraItem | null {
  const limitKey = fmt.source_limit || 'daily_quota_limit'
  const remainingKey = fmt.source_remaining || 'daily_remaining_quota'

  const limit = Number(extra[limitKey])
  const remaining = Number(extra[remainingKey])

  if (extra[limitKey] === undefined || extra[remainingKey] === undefined) return null
  if (!limit) return null

  const percent = Math.round((remaining / limit) * 100)

  // 从 source_start_date 计算下次重置时间
  let resetsAt: number | undefined
  const startDateKey = fmt.source_start_date
  if (startDateKey && extra[startDateKey]) {
    try {
      const startDate = new Date(String(extra[startDateKey]))
      const now = new Date()
      const todayReset = new Date(now)
      todayReset.setHours(startDate.getHours(), startDate.getMinutes(), startDate.getSeconds(), 0)
      if (todayReset <= now) {
        todayReset.setDate(todayReset.getDate() + 1)
      }
      resetsAt = Math.floor(todayReset.getTime() / 1000)
    } catch {
      // ignore
    }
  }

  return {
    label: fmt.label,
    value: `${percent}%`,
    percent,
    resetsAt,
  }
}

function formatMonthlyExpiryItem(
  extra: Record<string, unknown>,
  fmt: BalanceExtraFormat,
): BalanceExtraItem | null {
  const endDateKey = fmt.source_end_date || 'effective_end_date'
  if (!extra[endDateKey]) return null

  try {
    const endDate = new Date(String(extra[endDateKey]))
    const now = new Date()
    const daysLeft = Math.ceil((endDate.getTime() - now.getTime()) / (1000 * 60 * 60 * 24))
    const resetsAt = Math.floor(endDate.getTime() / 1000)
    const percent = Math.min(100, Math.max(0, Math.round((daysLeft / 30) * 100)))

    return {
      label: fmt.label,
      value: `${percent}%`,
      percent,
      resetsAt,
    }
  } catch {
    return null
  }
}

function formatWeeklySpentItem(
  extra: Record<string, unknown>,
  fmt: BalanceExtraFormat,
): BalanceExtraItem | null {
  const limitKey = fmt.source_limit || 'weekly_limit'
  const spentKey = fmt.source_spent || 'weekly_spent'
  const resetsAtKey = fmt.source_resets_at || 'weekly_resets_at'

  const limitNum = Number(extra[limitKey])
  const spentNum = Number(extra[spentKey])

  if (extra[limitKey] === undefined || limitNum <= 0 || extra[spentKey] === undefined) return null

  const remaining = Math.max(0, limitNum - spentNum)
  const percent = Math.round((remaining / limitNum) * 100)

  return {
    label: fmt.label,
    value: `${percent}%`,
    percent,
    resetsAt: typeof extra[resetsAtKey] === 'number' ? extra[resetsAtKey] : undefined,
    tooltip: `$${remaining.toFixed(2)} / $${limitNum.toFixed(2)}`,
  }
}

// ==================== Field Hooks ====================

/**
 * 处理字段变化时的钩子逻辑
 */
export function handleSchemaFieldChange(
  schema: CredentialsSchema,
  fieldKey: string,
  value: unknown,
  formData: Record<string, unknown>,
): void {
  const hooks = schema['x-field-hooks']
  if (!hooks) return

  const hook = hooks[fieldKey]
  if (!hook) return

  // 目标字段为空时才填充
  if (trimValue(formData[hook.target])) return

  const result = executeFieldHook(hook.action, typeof value === 'string' ? value : String(value ?? ''))
  if (result) {
    formData[hook.target] = result
  }
}
