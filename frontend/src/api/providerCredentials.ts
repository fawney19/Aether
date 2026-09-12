export interface CredentialsSchema {
  type: 'object'
  properties: Record<string, SchemaProperty>
  required?: string[]
  'x-field-groups'?: SchemaFieldGroup[]
  'x-auth-type'?: string
  'x-auth-method'?: string
  'x-validation'?: SchemaValidation[]
  'x-quota-divisor'?: number | null
  'x-currency'?: string
  'x-default-base-url'?: string
  'x-balance-extra-format'?: BalanceExtraFormat[]
  'x-field-hooks'?: Record<string, { action: string; target: string }>
}

export interface SchemaProperty {
  type: string
  title?: string
  description?: string
  'x-sensitive'?: boolean
  'x-input-type'?: string
  'x-default-value'?: string
  'x-help'?: string
}

export interface SchemaFieldGroup {
  fields: string[]
  layout?: 'inline' | 'vertical'
  'x-flex'?: Record<string, number>
  'x-help'?: string
}

interface SchemaValidation {
  type: 'required' | 'any_required' | 'conditional_required'
  fields?: string[]
  message: string
  /** conditional_required: 当此字段有值时 */
  if?: string
  /** conditional_required: 除非此字段有值 */
  unless?: string
  /** conditional_required: 则这些字段必填 */
  then?: string[]
}

export interface BalanceExtraFormat {
  label: string
  type: 'window_limit' | 'daily_quota' | 'weekly_spent' | 'monthly_expiry'
  /** window_limit: extra 中的字段名 */
  source?: string
  /** window_limit: 单位除数 */
  unit_divisor?: number
  /** daily_quota / weekly_spent: limit 字段名 */
  source_limit?: string
  /** daily_quota: remaining 字段名 */
  source_remaining?: string
  /** daily_quota: 每日重置基准时间字段名（计算下次重置时间） */
  source_start_date?: string
  /** weekly_spent: spent 字段名 */
  source_spent?: string
  /** weekly_spent: resets_at 字段名 */
  source_resets_at?: string
  /** monthly_expiry: 到期日期字段名 */
  source_end_date?: string
}
