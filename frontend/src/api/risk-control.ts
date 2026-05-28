import apiClient from './client'

const MAX_KEYWORD_ITEMS = 1000
const MAX_KEYWORD_CHARS = 512
const MAX_SCOPE_ITEMS = 1000
const MAX_SCOPE_VALUE_CHARS = 200
export const MAX_REGEX_KEYWORD_ITEMS = 100
export const MAX_REGEX_PATTERN_CHARS = 256
export const MAX_REGEX_COMPLEXITY_SCORE = 600
export const MAX_REGEX_SCAN_WINDOW_CHARS = 64 * 1024
export const MAX_REGEX_TOTAL_SCAN_BUDGET_CHARS = 4 * 1024 * 1024
const MAX_THRESHOLD_ITEMS = 100
const MAX_THRESHOLD_NAME_CHARS = 100
const MAX_BLOCK_MESSAGE_CHARS = 500

export type RiskControlMode = 'off' | 'observe' | 'pre_block'
export type RiskControlKeywordMode = 'keyword_only' | 'keyword_and_api' | 'api_only'
export type RiskControlKeywordMatchMode = 'contains' | 'exact' | 'regex'
export type RiskControlModelFilterMode = 'all' | 'include' | 'exclude'
export type RiskControlScopeMode = 'all' | 'include' | 'exclude'
export type RiskControlProviderKeyStatusValue = 'unknown' | 'ok' | 'error' | 'frozen'

export interface RiskControlModelFilterConfig {
  mode: RiskControlModelFilterMode
  models: string[]
}

export interface RiskControlScopeTargetConfig {
  mode: RiskControlScopeMode
  values: string[]
}

export interface RiskControlScopeConfig {
  users: RiskControlScopeTargetConfig
  user_groups: RiskControlScopeTargetConfig
  api_keys: RiskControlScopeTargetConfig
  route_families: RiskControlScopeTargetConfig
  route_kinds: RiskControlScopeTargetConfig
  endpoints: RiskControlScopeTargetConfig
}

export interface RiskControlProviderConfig {
  base_url: string
  model: string
  api_keys: string[]
  timeout_ms: number
  max_retries: number
  key_freeze_seconds: number
  fail_closed: boolean
}

export interface RiskControlHashBlockConfig {
  enabled: boolean
  learn_from_flagged: boolean
}

export interface RiskControlAutoActionConfig {
  enabled: boolean
  violation_threshold: number
  window_seconds: number
  disable_user: boolean
  lock_api_key: boolean
}

export interface RiskControlRetentionConfig {
  hit_days: number
  non_hit_days: number
  auto_run_interval_minutes: number
}

export interface RiskControlNotificationConfig {
  enabled: boolean
  notify_on_flagged: boolean
  notify_on_auto_action: boolean
  notify_on_user_action_notice: boolean
  include_excerpt: boolean
}

export interface RiskControlObserveConfig {
  queue_capacity: number
}

export interface RiskControlConfig {
  enabled: boolean
  mode: RiskControlMode
  keyword_mode: RiskControlKeywordMode
  keyword_match_mode: RiskControlKeywordMatchMode
  keywords: string[]
  keyword_exemptions: string[]
  thresholds: Record<string, number>
  model_filter: RiskControlModelFilterConfig
  scope: RiskControlScopeConfig
  provider: RiskControlProviderConfig
  hash_block: RiskControlHashBlockConfig
  auto_action: RiskControlAutoActionConfig
  retention: RiskControlRetentionConfig
  notification: RiskControlNotificationConfig
  observe: RiskControlObserveConfig
  sample_rate: number
  max_text_chars: number
  excerpt_chars: number
  log_all: boolean
  block_status: number
  block_message: string
}

export interface RiskControlProviderKeyStatus {
  index: number
  key_hash: string
  masked: string
  status: RiskControlProviderKeyStatusValue
  failure_count: number
  success_count: number
  last_error: string | null
  last_checked_at_unix_secs: number | null
  frozen_until_unix_secs: number | null
  last_latency_ms: number | null
  last_http_status: number | null
  last_tested: boolean
  configured: boolean
}

export interface RiskControlStatus {
  enabled: boolean
  mode: RiskControlMode
  keyword_mode: RiskControlKeywordMode
  config_validated: boolean
  config_error: string | null
  notification_ready: boolean
  notification_warning: string | null
  notification_outbox: RiskControlNotificationOutboxSummary
  retention_status: RiskControlRetentionStatus
  observe_queue: RiskControlObserveQueueStatus
  logs_total: number
  flagged_total: number
  flagged_hashes_total: number
  provider_key_count: number
  provider_key_statuses: RiskControlProviderKeyStatus[]
  keyword_count: number
}

export interface RiskControlNotificationOutboxSummary {
  pending: number
  processing: number
  sent: number
  dead: number
  oldest_pending_at_unix_secs: number | null
  next_attempt_at_unix_secs: number | null
  last_error: string | null
}

export interface RiskControlRetentionStatus {
  last_started_at_unix_secs: number | null
  last_completed_at_unix_secs: number | null
  last_success: boolean | null
  last_hit_deleted: number
  last_non_hit_deleted: number
  last_error: string | null
  next_run_at_unix_secs: number | null
}

export interface RiskControlObserveQueueStatus {
  capacity: number
  queued: number
  enqueued_total: number
  dropped_total: number
  processed_total: number
  failed_total: number
}

export interface RiskControlConfigResponse {
  enabled: boolean
  config: RiskControlConfig
  config_validated?: boolean
  config_error?: string | null
}

export interface RiskControlLogItem {
  id: string
  trace_id: string | null
  request_id: string | null
  user_id: string | null
  username: string | null
  user_email: string | null
  api_key_id: string | null
  api_key_name: string | null
  route_family: string | null
  route_kind: string | null
  api_format: string | null
  endpoint: string | null
  model: string | null
  mode: string
  action: string
  decision_source: string
  flagged: boolean
  highest_category: string | null
  highest_score: number
  category_scores: Record<string, number> | null
  thresholds: Record<string, number> | null
  matched_keywords: string[] | null
  input_hash: string | null
  excerpt: string | null
  excerpt_redacted: boolean
  excerpt_redaction_reason: string | null
  latency_ms: number | null
  queue_delay_ms: number | null
  violation_count: number
  auto_action: string | null
  auto_action_enforced: boolean
  notification_sent: boolean
  notification_attempts: number
  notification_last_error: string | null
  notification_last_attempt_at: string | null
  notification_last_attempt_at_unix_secs: number | null
  notification_outbox: RiskControlNotificationOutboxItem | null
  notification_outboxes: RiskControlNotificationOutboxItem[]
  error_message: string | null
  created_at: string
  created_at_unix_secs: number
}

export interface RiskControlHashItem {
  input_hash: string
  source_log_id: string | null
  reason: string | null
  highest_category: string | null
  highest_score: number
  excerpt: string | null
  excerpt_redacted: boolean
  excerpt_redaction_reason: string | null
  first_seen_at: string
  first_seen_at_unix_secs: number
  last_seen_at: string
  last_seen_at_unix_secs: number
  hit_count: number
}

export interface RiskControlPage<T> {
  items: T[]
  total: number
  page: number
  page_size: number
  pages: number
}

export interface RiskControlLogFilters {
  page?: number
  page_size?: number
  user_id?: string
  api_key_id?: string
  flagged?: boolean | null
  action?: string
  decision_source?: string
  endpoint?: string
  model?: string
  q?: string
  from?: number
  to?: number
}

export interface RiskControlTestResult {
  action: string
  decision_source: string
  flagged: boolean
  highest_category: string | null
  highest_score: number
  category_scores: Record<string, number> | null
  matched_keywords: string[]
  regex_scan_limited?: boolean
  regex_pattern_limited?: boolean
  regex_invalid_pattern_count?: number
  regex_scan_chars?: number
  regex_pattern_count?: number
  regex_total_scan_budget_chars?: number
  error_message: string | null
}

export interface RiskControlTestResponse {
  input_excerpt: string
  result: RiskControlTestResult
  provider_key_statuses?: RiskControlProviderKeyStatus[]
}

export const DEFAULT_RISK_CONTROL_CONFIG: RiskControlConfig = {
  enabled: false,
  mode: 'observe',
  keyword_mode: 'keyword_and_api',
  keyword_match_mode: 'contains',
  keywords: [],
  keyword_exemptions: [],
  thresholds: {},
  model_filter: {
    mode: 'all',
    models: [],
  },
  scope: {
    users: {
      mode: 'all',
      values: [],
    },
    user_groups: {
      mode: 'all',
      values: [],
    },
    api_keys: {
      mode: 'all',
      values: [],
    },
    route_families: {
      mode: 'all',
      values: [],
    },
    route_kinds: {
      mode: 'all',
      values: [],
    },
    endpoints: {
      mode: 'all',
      values: [],
    },
  },
  provider: {
    base_url: 'https://api.openai.com',
    model: 'omni-moderation-latest',
    api_keys: [],
    timeout_ms: 8000,
    max_retries: 2,
    key_freeze_seconds: 300,
    fail_closed: false,
  },
  hash_block: {
    enabled: true,
    learn_from_flagged: true,
  },
  auto_action: {
    enabled: false,
    violation_threshold: 3,
    window_seconds: 86400,
    disable_user: true,
    lock_api_key: false,
  },
  retention: {
    hit_days: 90,
    non_hit_days: 14,
    auto_run_interval_minutes: 60,
  },
  notification: {
    enabled: false,
    notify_on_flagged: true,
    notify_on_auto_action: true,
    notify_on_user_action_notice: false,
    include_excerpt: false,
  },
  observe: {
    queue_capacity: 1024,
  },
  sample_rate: 1,
  max_text_chars: 65536,
  excerpt_chars: 512,
  log_all: false,
  block_status: 400,
  block_message: '请求触发风控策略，已拒绝转发。',
}

export function cloneRiskControlConfig(config: RiskControlConfig): RiskControlConfig {
  return {
    ...config,
    keywords: [...config.keywords],
    keyword_exemptions: [...config.keyword_exemptions],
    thresholds: { ...config.thresholds },
    model_filter: {
      ...config.model_filter,
      models: [...config.model_filter.models],
    },
    scope: cloneRiskControlScope(config.scope),
    provider: {
      ...config.provider,
      api_keys: [...config.provider.api_keys],
    },
    hash_block: { ...config.hash_block },
    auto_action: { ...config.auto_action },
    retention: { ...config.retention },
    notification: { ...config.notification },
    observe: { ...config.observe },
  }
}

function clampNumber(value: unknown, fallback: number, min: number, max: number): number {
  const numberValue = typeof value === 'number' ? value : Number(value)
  if (!Number.isFinite(numberValue)) return fallback
  return Math.min(max, Math.max(min, numberValue))
}

function normalizeStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  return value
    .map(item => (typeof item === 'string' ? item.trim() : ''))
    .filter(Boolean)
}

export interface RiskControlNotificationOutboxItem {
  id: string
  log_id: string
  item_key: string
  status: 'pending' | 'processing' | 'sent' | 'dead' | string
  attempt_count: number
  max_attempts: number
  next_attempt_at: string | null
  next_attempt_at_unix_secs: number | null
  lease_until: string | null
  lease_until_unix_secs: number | null
  last_error: string | null
  created_at: string
  created_at_unix_secs: number
  updated_at: string
  updated_at_unix_secs: number
  sent_at: string | null
  sent_at_unix_secs: number | null
}

export interface RiskControlNotificationRetryResponse {
  queued: boolean
  notification: RiskControlNotificationOutboxItem | null
  notifications: RiskControlNotificationOutboxItem[]
}

function normalizeBoundedTerms(value: unknown): string[] {
  const seen = new Set<string>()
  const result: string[] = []
  for (const item of normalizeStringArray(value)) {
    const term = Array.from(item).slice(0, MAX_KEYWORD_CHARS).join('')
    const key = term.toLowerCase()
    if (seen.has(key)) continue
    seen.add(key)
    result.push(term)
    if (result.length >= MAX_KEYWORD_ITEMS) break
  }
  return result
}

function normalizeThresholds(value: unknown): Record<string, number> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {}
  const result: Record<string, number> = {}
  for (const [key, raw] of Object.entries(value as Record<string, unknown>)) {
    const threshold = Number(raw)
    const name = Array.from(key.trim()).slice(0, MAX_THRESHOLD_NAME_CHARS).join('')
    if (name && Number.isFinite(threshold)) {
      result[name] = Math.min(1, Math.max(0, threshold))
      if (Object.keys(result).length >= MAX_THRESHOLD_ITEMS) break
    }
  }
  return result
}

function normalizeBlockMessage(value: unknown): string {
  if (typeof value !== 'string' || !value.trim()) return DEFAULT_RISK_CONTROL_CONFIG.block_message
  return Array.from(value.trim()).slice(0, MAX_BLOCK_MESSAGE_CHARS).join('')
}

function normalizeMode(value: unknown): RiskControlMode {
  return value === 'off' || value === 'pre_block' || value === 'observe' ? value : 'observe'
}

function normalizeKeywordMode(value: unknown): RiskControlKeywordMode {
  return value === 'keyword_only' || value === 'api_only' || value === 'keyword_and_api'
    ? value
    : 'keyword_and_api'
}

function normalizeKeywordMatchMode(value: unknown): RiskControlKeywordMatchMode {
  return value === 'exact' || value === 'regex' || value === 'contains' ? value : 'contains'
}

function normalizeModelFilterMode(value: unknown): RiskControlModelFilterMode {
  return value === 'include' || value === 'exclude' || value === 'all' ? value : 'all'
}

function normalizeScopeMode(value: unknown): RiskControlScopeMode {
  return value === 'include' || value === 'exclude' || value === 'all' ? value : 'all'
}

function normalizeModelFilter(value: unknown): RiskControlModelFilterConfig {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return { ...DEFAULT_RISK_CONTROL_CONFIG.model_filter, models: [] }
  }
  const raw = value as Partial<RiskControlModelFilterConfig>
  const mode = normalizeModelFilterMode(raw.mode)
  return {
    mode,
    models: mode === 'all' ? [] : normalizeStringArray(raw.models).slice(0, 1000),
  }
}

function normalizeProviderKeyStatus(value: unknown): RiskControlProviderKeyStatus | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const raw = value as Partial<RiskControlProviderKeyStatus>
  const optionalNumber = (item: unknown): number | null => {
    if (item === null || item === undefined || item === '') return null
    const parsed = Number(item)
    return Number.isFinite(parsed) ? parsed : null
  }
  const status = raw.status === 'ok' || raw.status === 'error' || raw.status === 'frozen' || raw.status === 'unknown'
    ? raw.status
    : 'unknown'
  return {
    index: Math.round(clampNumber(raw.index, 0, 0, 100000)),
    key_hash: typeof raw.key_hash === 'string' ? raw.key_hash : '',
    masked: typeof raw.masked === 'string' ? raw.masked : '',
    status,
    failure_count: Math.round(clampNumber(raw.failure_count, 0, 0, Number.MAX_SAFE_INTEGER)),
    success_count: Math.round(clampNumber(raw.success_count, 0, 0, Number.MAX_SAFE_INTEGER)),
    last_error: typeof raw.last_error === 'string' && raw.last_error ? raw.last_error : null,
    last_checked_at_unix_secs: optionalNumber(raw.last_checked_at_unix_secs),
    frozen_until_unix_secs: optionalNumber(raw.frozen_until_unix_secs),
    last_latency_ms: optionalNumber(raw.last_latency_ms),
    last_http_status: optionalNumber(raw.last_http_status),
    last_tested: raw.last_tested === true,
    configured: raw.configured !== false,
  }
}

function normalizeProviderKeyStatuses(value: unknown): RiskControlProviderKeyStatus[] {
  if (!Array.isArray(value)) return []
  return value
    .map(normalizeProviderKeyStatus)
    .filter((item): item is RiskControlProviderKeyStatus => item !== null)
}

function cloneRiskControlScope(scope: RiskControlScopeConfig): RiskControlScopeConfig {
  return {
    users: { ...scope.users, values: [...scope.users.values] },
    user_groups: { ...scope.user_groups, values: [...scope.user_groups.values] },
    api_keys: { ...scope.api_keys, values: [...scope.api_keys.values] },
    route_families: { ...scope.route_families, values: [...scope.route_families.values] },
    route_kinds: { ...scope.route_kinds, values: [...scope.route_kinds.values] },
    endpoints: { ...scope.endpoints, values: [...scope.endpoints.values] },
  }
}

function normalizeScopeValues(value: unknown): string[] {
  const seen = new Set<string>()
  const result: string[] = []
  for (const item of normalizeStringArray(value)) {
    const term = Array.from(item).slice(0, MAX_SCOPE_VALUE_CHARS).join('')
    const key = term.toLowerCase()
    if (seen.has(key)) continue
    seen.add(key)
    result.push(term)
    if (result.length >= MAX_SCOPE_ITEMS) break
  }
  return result
}

function normalizeScopeTarget(value: unknown): RiskControlScopeTargetConfig {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return { mode: 'all', values: [] }
  }
  const raw = value as Partial<RiskControlScopeTargetConfig>
  const mode = normalizeScopeMode(raw.mode)
  return {
    mode,
    values: mode === 'all' ? [] : normalizeScopeValues(raw.values),
  }
}

function normalizeScope(value: unknown): RiskControlScopeConfig {
  const raw = value && typeof value === 'object' && !Array.isArray(value)
    ? value as Partial<RiskControlScopeConfig>
    : {}
  return {
    users: normalizeScopeTarget(raw.users),
    user_groups: normalizeScopeTarget(raw.user_groups),
    api_keys: normalizeScopeTarget(raw.api_keys),
    route_families: normalizeScopeTarget(raw.route_families),
    route_kinds: normalizeScopeTarget(raw.route_kinds),
    endpoints: normalizeScopeTarget(raw.endpoints),
  }
}

export function riskControlRegexComplexityScore(pattern: string): number {
  let score = 0
  let escaped = false
  let inClass = false
  for (const ch of pattern) {
    score += 1
    if (escaped) {
      score += 1
      escaped = false
      continue
    }
    if (ch === '\\') {
      score += 1
      escaped = true
      continue
    }
    if (inClass) {
      if (ch === ']') inClass = false
      score += 1
      continue
    }
    if (ch === '[') {
      inClass = true
      score += 4
    } else if (ch === '(' || ch === ')' || ch === '.') {
      score += 3
    } else if (ch === '|') {
      score += 6
    } else if (ch === '*' || ch === '+' || ch === '?') {
      score += 10
    } else if (ch === '{') {
      score += 12
    }
  }
  return score
}

export function validateRiskControlRegexConfig(config: RiskControlConfig): string | null {
  if (config.keyword_match_mode !== 'regex' || config.keyword_mode === 'api_only') return null
  const regexKeywords = config.keywords.map(item => item.trim()).filter(Boolean)
  if (regexKeywords.length > MAX_REGEX_KEYWORD_ITEMS) {
    return `regex 模式最多允许 ${MAX_REGEX_KEYWORD_ITEMS} 条关键词`
  }
  for (const pattern of regexKeywords) {
    if (Array.from(pattern).length > MAX_REGEX_PATTERN_CHARS) {
      return `regex 关键词长度不能超过 ${MAX_REGEX_PATTERN_CHARS} 字符`
    }
    if (riskControlRegexComplexityScore(pattern) > MAX_REGEX_COMPLEXITY_SCORE) {
      return 'regex 关键词复杂度过高，请拆分为更简单的规则'
    }
    if (!pattern.includes('(?')) {
      try {
        const regex = new RegExp(pattern)
        if (regex.test('')) {
          return 'regex 关键词不能匹配空字符串'
        }
      } catch (err) {
        return `regex 关键词不是合法表达式：${err instanceof Error ? err.message : String(err)}`
      }
    }
  }
  return null
}

export function validateRiskControlProviderBaseUrl(value: string): string | null {
  const raw = value.trim()
  if (!raw) return 'Provider Base URL 不能为空'
  let parsed: URL
  try {
    parsed = new URL(raw)
  } catch {
    return 'Provider Base URL 不是合法 URL'
  }
  if (parsed.protocol !== 'https:') {
    return 'Provider Base URL 必须使用 HTTPS'
  }
  if (parsed.username || parsed.password) {
    return 'Provider Base URL 不能包含用户名或密码'
  }
  if (parsed.search || parsed.hash) {
    return 'Provider Base URL 不能包含 query 或 fragment'
  }
  const host = normalizeProviderHost(parsed.hostname)
  if (!host) return 'Provider Base URL 必须包含 host'
  if (host === 'localhost' || host === 'localhost.localdomain' || host.endsWith('.localhost')) {
    return 'Provider Base URL host 不能是 localhost'
  }
  const ipv4 = parseIpv4(host)
  if ((ipv4 && isBlockedProviderIpv4(ipv4)) || isBlockedProviderIpv6Literal(host)) {
    return 'Provider Base URL host 不能是内网、回环、链路本地或保留地址'
  }
  return null
}

function normalizeProviderHost(value: string): string {
  return value.trim().replace(/^\[(.*)\]$/, '$1').replace(/\.$/, '').toLowerCase()
}

function parseIpv4(value: string): number[] | null {
  const parts = value.split('.')
  if (parts.length !== 4) return null
  const octets = parts.map(part => (/^\d{1,3}$/.test(part) ? Number(part) : Number.NaN))
  return octets.every(part => Number.isInteger(part) && part >= 0 && part <= 255) ? octets : null
}

function isBlockedProviderIpv4([first, second, third, fourth]: number[]): boolean {
  return first === 0
    || first === 10
    || first === 127
    || (first === 169 && second === 254)
    || (first === 172 && second >= 16 && second <= 31)
    || (first === 192 && second === 168)
    || (first === 100 && second >= 64 && second <= 127)
    || (first === 198 && (second === 18 || second === 19))
    || (first === 192 && second === 0 && third === 0)
    || (first === 192 && second === 0 && third === 2)
    || (first === 198 && second === 51 && third === 100)
    || (first === 203 && second === 0 && third === 113)
    || first >= 224
    || (first === 255 && second === 255 && third === 255 && fourth === 255)
}

function isBlockedProviderIpv6Literal(value: string): boolean {
  if (!value.includes(':')) return false
  const lower = value.toLowerCase()
  const segments = lower.split(':')
  const first = parseInt(segments[0] || '0', 16)
  const second = parseInt(segments[1] || '0', 16)
  const third = parseInt(segments[2] || '0', 16)
  if (lower === '::' || lower === '::1' || /^::[0-9.]+$/.test(lower)) {
    return true
  }
  if (lower.startsWith('::ffff:')) {
    const embedded = lower.split(':').pop()
    const embeddedIpv4 = embedded ? parseIpv4(embedded) : null
    return !embeddedIpv4 || isBlockedProviderIpv4(embeddedIpv4)
  }
  return (first >= 0xfc00 && first <= 0xfdff)
    || (first >= 0xfe80 && first <= 0xfebf)
    || (first >= 0xfec0 && first <= 0xfeff)
    || (first >= 0xff00 && first <= 0xffff)
    || (first === 0x0064 && second === 0xff9b)
    || (first === 0x0100 && (second === 0 || (second === 0 && third === 1)))
    || (first === 0x2001 && (second & 0xfe00) === 0)
    || (first === 0x2001 && second === 0x0db8)
    || first === 0x2002
    || first === 0x3ffe
    || ((first & 0xfff0) === 0x3ff0)
    || first === 0x5f00
}

function normalizeOptionalNumber(value: unknown): number | null {
  if (value === null || value === undefined || value === '') return null
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : null
}

function normalizeRetentionStatus(value: unknown): RiskControlRetentionStatus {
  const raw = value && typeof value === 'object' && !Array.isArray(value)
    ? value as Partial<RiskControlRetentionStatus>
    : {}
  return {
    last_started_at_unix_secs: normalizeOptionalNumber(raw.last_started_at_unix_secs),
    last_completed_at_unix_secs: normalizeOptionalNumber(raw.last_completed_at_unix_secs),
    last_success: typeof raw.last_success === 'boolean' ? raw.last_success : null,
    last_hit_deleted: Math.round(clampNumber(raw.last_hit_deleted, 0, 0, Number.MAX_SAFE_INTEGER)),
    last_non_hit_deleted: Math.round(clampNumber(raw.last_non_hit_deleted, 0, 0, Number.MAX_SAFE_INTEGER)),
    last_error: typeof raw.last_error === 'string' && raw.last_error ? raw.last_error : null,
    next_run_at_unix_secs: normalizeOptionalNumber(raw.next_run_at_unix_secs),
  }
}

function normalizeNotificationOutboxSummary(value: unknown): RiskControlNotificationOutboxSummary {
  const raw = value && typeof value === 'object' && !Array.isArray(value)
    ? value as Partial<RiskControlNotificationOutboxSummary>
    : {}
  return {
    pending: Math.round(clampNumber(raw.pending, 0, 0, Number.MAX_SAFE_INTEGER)),
    processing: Math.round(clampNumber(raw.processing, 0, 0, Number.MAX_SAFE_INTEGER)),
    sent: Math.round(clampNumber(raw.sent, 0, 0, Number.MAX_SAFE_INTEGER)),
    dead: Math.round(clampNumber(raw.dead, 0, 0, Number.MAX_SAFE_INTEGER)),
    oldest_pending_at_unix_secs: normalizeOptionalNumber(raw.oldest_pending_at_unix_secs),
    next_attempt_at_unix_secs: normalizeOptionalNumber(raw.next_attempt_at_unix_secs),
    last_error: typeof raw.last_error === 'string' && raw.last_error ? raw.last_error : null,
  }
}

function normalizeObserveQueueStatus(value: unknown): RiskControlObserveQueueStatus {
  const raw = value && typeof value === 'object' && !Array.isArray(value)
    ? value as Partial<RiskControlObserveQueueStatus>
    : {}
  return {
    capacity: Math.round(clampNumber(raw.capacity, 0, 0, Number.MAX_SAFE_INTEGER)),
    queued: Math.round(clampNumber(raw.queued, 0, 0, Number.MAX_SAFE_INTEGER)),
    enqueued_total: Math.round(clampNumber(raw.enqueued_total, 0, 0, Number.MAX_SAFE_INTEGER)),
    dropped_total: Math.round(clampNumber(raw.dropped_total, 0, 0, Number.MAX_SAFE_INTEGER)),
    processed_total: Math.round(clampNumber(raw.processed_total, 0, 0, Number.MAX_SAFE_INTEGER)),
    failed_total: Math.round(clampNumber(raw.failed_total, 0, 0, Number.MAX_SAFE_INTEGER)),
  }
}

function normalizeNotificationOutboxItem(value: unknown): RiskControlNotificationOutboxItem | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const raw = value as Partial<RiskControlNotificationOutboxItem>
  return {
    id: typeof raw.id === 'string' ? raw.id : '',
    log_id: typeof raw.log_id === 'string' ? raw.log_id : '',
    item_key: typeof raw.item_key === 'string' ? raw.item_key : '',
    status: typeof raw.status === 'string' ? raw.status : 'pending',
    attempt_count: Math.round(clampNumber(raw.attempt_count, 0, 0, Number.MAX_SAFE_INTEGER)),
    max_attempts: Math.round(clampNumber(raw.max_attempts, 10, 1, Number.MAX_SAFE_INTEGER)),
    next_attempt_at: typeof raw.next_attempt_at === 'string' && raw.next_attempt_at ? raw.next_attempt_at : null,
    next_attempt_at_unix_secs: normalizeOptionalNumber(raw.next_attempt_at_unix_secs),
    lease_until: typeof raw.lease_until === 'string' && raw.lease_until ? raw.lease_until : null,
    lease_until_unix_secs: normalizeOptionalNumber(raw.lease_until_unix_secs),
    last_error: typeof raw.last_error === 'string' && raw.last_error ? raw.last_error : null,
    created_at: typeof raw.created_at === 'string' ? raw.created_at : '',
    created_at_unix_secs: Math.round(clampNumber(raw.created_at_unix_secs, 0, 0, Number.MAX_SAFE_INTEGER)),
    updated_at: typeof raw.updated_at === 'string' ? raw.updated_at : '',
    updated_at_unix_secs: Math.round(clampNumber(raw.updated_at_unix_secs, 0, 0, Number.MAX_SAFE_INTEGER)),
    sent_at: typeof raw.sent_at === 'string' && raw.sent_at ? raw.sent_at : null,
    sent_at_unix_secs: normalizeOptionalNumber(raw.sent_at_unix_secs),
  }
}

function normalizeRiskControlLogItem(item: RiskControlLogItem): RiskControlLogItem {
  const notificationOutbox = normalizeNotificationOutboxItem(item.notification_outbox)
  const notificationOutboxes = Array.isArray(item.notification_outboxes)
    ? item.notification_outboxes
      .map(normalizeNotificationOutboxItem)
      .filter((entry): entry is RiskControlNotificationOutboxItem => entry !== null)
    : notificationOutbox
      ? [notificationOutbox]
      : []
  return {
    ...item,
    excerpt: item.excerpt ?? null,
    excerpt_redacted: item.excerpt_redacted === true,
    excerpt_redaction_reason: item.excerpt_redaction_reason ?? null,
    auto_action_enforced: item.auto_action_enforced === true,
    notification_attempts: Math.round(clampNumber(item.notification_attempts, 0, 0, Number.MAX_SAFE_INTEGER)),
    notification_last_error: item.notification_last_error ?? null,
    notification_last_attempt_at: item.notification_last_attempt_at ?? null,
    notification_last_attempt_at_unix_secs: normalizeOptionalNumber(item.notification_last_attempt_at_unix_secs),
    notification_outbox: notificationOutbox,
    notification_outboxes: notificationOutboxes,
  }
}

function normalizeRiskControlHashItem(item: RiskControlHashItem): RiskControlHashItem {
  return {
    ...item,
    excerpt: item.excerpt ?? null,
    excerpt_redacted: item.excerpt_redacted === true,
    excerpt_redaction_reason: item.excerpt_redaction_reason ?? null,
  }
}

export function normalizeRiskControlConfig(value: unknown): RiskControlConfig {
  const raw = value && typeof value === 'object' ? value as Partial<RiskControlConfig> : {}
  const provider = raw.provider && typeof raw.provider === 'object' ? raw.provider : DEFAULT_RISK_CONTROL_CONFIG.provider
  const hashBlock = raw.hash_block && typeof raw.hash_block === 'object' ? raw.hash_block : DEFAULT_RISK_CONTROL_CONFIG.hash_block
  const autoAction = raw.auto_action && typeof raw.auto_action === 'object' ? raw.auto_action : DEFAULT_RISK_CONTROL_CONFIG.auto_action
  const retention = raw.retention && typeof raw.retention === 'object' ? raw.retention : DEFAULT_RISK_CONTROL_CONFIG.retention
  const notification = raw.notification && typeof raw.notification === 'object' ? raw.notification : DEFAULT_RISK_CONTROL_CONFIG.notification
  const observe = raw.observe && typeof raw.observe === 'object' ? raw.observe : DEFAULT_RISK_CONTROL_CONFIG.observe

  return {
    enabled: raw.enabled === true,
    mode: normalizeMode(raw.mode),
    keyword_mode: normalizeKeywordMode(raw.keyword_mode),
    keyword_match_mode: normalizeKeywordMatchMode(raw.keyword_match_mode),
    keywords: normalizeBoundedTerms(raw.keywords),
    keyword_exemptions: normalizeBoundedTerms(raw.keyword_exemptions),
    thresholds: normalizeThresholds(raw.thresholds),
    model_filter: normalizeModelFilter(raw.model_filter),
    scope: normalizeScope(raw.scope),
    provider: {
      base_url: typeof provider.base_url === 'string' && provider.base_url.trim()
        ? provider.base_url.trim()
        : DEFAULT_RISK_CONTROL_CONFIG.provider.base_url,
      model: typeof provider.model === 'string' && provider.model.trim()
        ? provider.model.trim()
        : DEFAULT_RISK_CONTROL_CONFIG.provider.model,
      api_keys: normalizeStringArray(provider.api_keys),
      timeout_ms: Math.round(clampNumber(provider.timeout_ms, 8000, 500, 60000)),
      max_retries: Math.round(clampNumber(provider.max_retries, 2, 0, 8)),
      key_freeze_seconds: Math.round(clampNumber(provider.key_freeze_seconds, 300, 0, 86400)),
      fail_closed: provider.fail_closed === true,
    },
    hash_block: {
      enabled: hashBlock.enabled !== false,
      learn_from_flagged: hashBlock.learn_from_flagged !== false,
    },
    auto_action: {
      enabled: autoAction.enabled === true,
      violation_threshold: Math.round(clampNumber(autoAction.violation_threshold, 3, 1, 1000)),
      window_seconds: Math.round(clampNumber(autoAction.window_seconds, 86400, 60, 31_536_000)),
      disable_user: autoAction.disable_user !== false,
      lock_api_key: autoAction.lock_api_key === true,
    },
    retention: {
      hit_days: Math.round(clampNumber(retention.hit_days, 90, 0, 3650)),
      non_hit_days: Math.round(clampNumber(retention.non_hit_days, 14, 0, 3650)),
      auto_run_interval_minutes: Math.round(clampNumber(retention.auto_run_interval_minutes, 60, 0, 60 * 24 * 7)),
    },
    notification: {
      enabled: notification.enabled === true,
      notify_on_flagged: notification.notify_on_flagged !== false,
      notify_on_auto_action: notification.notify_on_auto_action !== false,
      notify_on_user_action_notice: notification.notify_on_user_action_notice === true,
      include_excerpt: notification.include_excerpt === true,
    },
    observe: {
      queue_capacity: Math.round(clampNumber(observe.queue_capacity, 1024, 16, 65536)),
    },
    sample_rate: clampNumber(raw.sample_rate, 1, 0, 1),
    max_text_chars: Math.round(clampNumber(raw.max_text_chars, 65536, 256, 2 * 1024 * 1024)),
    excerpt_chars: Math.round(clampNumber(raw.excerpt_chars, 512, 64, 4096)),
    log_all: raw.log_all === true,
    block_status: Math.round(clampNumber(raw.block_status, 400, 400, 499)),
    block_message: normalizeBlockMessage(raw.block_message),
  }
}

function appendQuery(params: URLSearchParams, key: string, value: unknown) {
  if (value === undefined || value === null || value === '') return
  params.set(key, String(value))
}

export const riskControlApi = {
  async getStatus(): Promise<RiskControlStatus> {
    const response = await apiClient.get<RiskControlStatus>('/api/admin/risk-control/status')
    return {
      ...response.data,
      notification_ready: response.data.notification_ready === true,
      notification_warning: response.data.notification_warning ?? null,
      notification_outbox: normalizeNotificationOutboxSummary(response.data.notification_outbox),
      retention_status: normalizeRetentionStatus(response.data.retention_status),
      observe_queue: normalizeObserveQueueStatus(response.data.observe_queue),
      provider_key_statuses: normalizeProviderKeyStatuses(response.data.provider_key_statuses),
    }
  },

  async getConfig(): Promise<RiskControlConfigResponse> {
    const response = await apiClient.get<RiskControlConfigResponse>('/api/admin/risk-control/config')
    return {
      enabled: response.data.enabled === true,
      config: normalizeRiskControlConfig(response.data.config),
      config_validated: response.data.config_validated,
      config_error: response.data.config_error ?? null,
    }
  },

  async updateConfig(enabled: boolean, config: RiskControlConfig): Promise<RiskControlConfigResponse> {
    const normalizedConfig = normalizeRiskControlConfig(config)
    const response = await apiClient.put<RiskControlConfigResponse>('/api/admin/risk-control/config', {
      enabled,
      config: normalizedConfig,
    })
    return {
      enabled: response.data.enabled === true,
      config: normalizeRiskControlConfig(response.data.config),
      config_validated: response.data.config_validated,
      config_error: response.data.config_error ?? null,
    }
  },

  async listLogs(filters: RiskControlLogFilters): Promise<RiskControlPage<RiskControlLogItem>> {
    const params = new URLSearchParams()
    Object.entries(filters).forEach(([key, value]) => appendQuery(params, key, value))
    const query = params.toString()
    const response = await apiClient.get<RiskControlPage<RiskControlLogItem>>(
      `/api/admin/risk-control/logs${query ? `?${query}` : ''}`,
    )
    return {
      ...response.data,
      items: response.data.items.map(normalizeRiskControlLogItem),
    }
  },

  async listHashes(page = 1, pageSize = 20): Promise<RiskControlPage<RiskControlHashItem>> {
    const params = new URLSearchParams()
    appendQuery(params, 'page', page)
    appendQuery(params, 'page_size', pageSize)
    const response = await apiClient.get<RiskControlPage<RiskControlHashItem>>(
      `/api/admin/risk-control/hashes?${params.toString()}`,
    )
    return {
      ...response.data,
      items: response.data.items.map(normalizeRiskControlHashItem),
    }
  },

  async deleteHash(inputHash: string): Promise<{ deleted: boolean }> {
    const response = await apiClient.delete<{ deleted: boolean }>(
      `/api/admin/risk-control/hashes/${encodeURIComponent(inputHash)}`,
    )
    return response.data
  },

  async clearHashes(): Promise<{ deleted: number }> {
    const response = await apiClient.delete<{ deleted: number }>('/api/admin/risk-control/hashes')
    return response.data
  },

  async testText(text: string, config?: RiskControlConfig): Promise<RiskControlTestResponse> {
    const response = await apiClient.post<RiskControlTestResponse>('/api/admin/risk-control/test', {
      text,
      config,
    })
    return {
      ...response.data,
      provider_key_statuses: normalizeProviderKeyStatuses(response.data.provider_key_statuses),
    }
  },

  async testProviderKeys(apiKeys: string[], config?: RiskControlConfig): Promise<RiskControlTestResponse> {
    const payload: { text: string; api_keys?: string[]; config?: RiskControlConfig } = {
      text: 'hello',
    }
    if (apiKeys.length > 0) payload.api_keys = apiKeys
    if (config) payload.config = config
    const response = await apiClient.post<RiskControlTestResponse>('/api/admin/risk-control/provider-keys/test', payload)
    return {
      ...response.data,
      provider_key_statuses: normalizeProviderKeyStatuses(response.data.provider_key_statuses),
    }
  },

  async runRetention(): Promise<{ hit_deleted: number; non_hit_deleted: number }> {
    const response = await apiClient.post<{ hit_deleted: number; non_hit_deleted: number }>(
      '/api/admin/risk-control/retention/run',
      {},
    )
    return response.data
  },

  async unbanUser(userId: string): Promise<{ updated: boolean; user?: { id: string; username: string | null; email: string | null; is_active: boolean } }> {
    const response = await apiClient.post<{ updated: boolean; user?: { id: string; username: string | null; email: string | null; is_active: boolean } }>(
      `/api/admin/risk-control/users/${encodeURIComponent(userId)}/unban`,
      {},
    )
    return response.data
  },

  async unlockUserApiKey(userId: string, apiKeyId: string): Promise<{ updated: boolean; api_key?: { id: string; user_id: string; is_locked: boolean } }> {
    const response = await apiClient.post<{ updated: boolean; api_key?: { id: string; user_id: string; is_locked: boolean } }>(
      `/api/admin/risk-control/users/${encodeURIComponent(userId)}/api-keys/${encodeURIComponent(apiKeyId)}/unlock`,
      {},
    )
    return response.data
  },

  async retryNotification(logId: string): Promise<RiskControlNotificationRetryResponse> {
    const response = await apiClient.post<{
      queued: boolean
      notification?: RiskControlNotificationOutboxItem
      notifications?: RiskControlNotificationOutboxItem[]
    }>(
      `/api/admin/risk-control/logs/${encodeURIComponent(logId)}/notification/retry`,
      {},
    )
    const notification = normalizeNotificationOutboxItem(response.data.notification)
    const notifications = Array.isArray(response.data.notifications)
      ? response.data.notifications
        .map(normalizeNotificationOutboxItem)
        .filter((item): item is RiskControlNotificationOutboxItem => item !== null)
      : notification
        ? [notification]
        : []
    return {
      queued: response.data.queued === true,
      notification,
      notifications,
    }
  },
}
