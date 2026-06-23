import apiClient from './client'

const MODULE_MANAGEMENT_ORDER_CONFIG_KEY = 'module_management.extension_order'

export interface ModuleStatus {
  name: string
  available: boolean
  enabled: boolean
  active: boolean
  config_validated: boolean
  config_error: string | null
  display_name: string
  description: string
  category: 'auth' | 'monitoring' | 'security' | 'integration'
  admin_route: string | null
  admin_menu_icon: string | null
  admin_menu_group: string | null
  admin_menu_order: number
  health: 'healthy' | 'degraded' | 'unhealthy' | 'unknown'
}

export interface AuthModuleInfo {
  name: string
  display_name: string
  active: boolean
}

export type ChatPiiRedactionTtlSeconds = 300 | 3600

export interface ChatPiiRedactionRuleFeatures {
  validator?: string | null
  [key: string]: unknown
}

export interface ChatPiiRedactionRule {
  id: string
  name: string
  pattern: string
  enabled: boolean
  system?: boolean
  features?: ChatPiiRedactionRuleFeatures | null
}

export interface ChatPiiRedactionConfig {
  enabled: boolean
  rules: ChatPiiRedactionRule[]
  cache_ttl_seconds: ChatPiiRedactionTtlSeconds
  placeholder_prefix: string
}

export type CodexAdapterSchedulingMode = 'priority' | 'sticky' | 'load_balance'

export interface CodexAdapterCandidateConfig {
  global_model: string
  enabled: boolean
  priority: number
  weight: number
}

export interface CodexAdapterRouteConfig {
  codex_model: string
  enabled: boolean
  scheduling_mode: CodexAdapterSchedulingMode
  candidates: CodexAdapterCandidateConfig[]
}

export const CHAT_PII_REDACTION_DEFAULT_RULES: ChatPiiRedactionRule[] = [
  { id: 'email', name: '邮箱', pattern: '(?i)[A-Z0-9._%+-]{1,64}@[A-Z0-9.-]{1,253}\\.[A-Z]{2,63}', enabled: true, features: { validator: 'email' }, system: true },
  { id: 'cn_phone', name: '手机号', pattern: '(?:\\+?86[- ]?)?(?:1[3-9]\\d[- ]?\\d{4}[- ]?\\d{4}|0\\d{2,3}[- ]\\d{7,8}(?:-\\d{1,6})?)', enabled: true, features: { validator: 'cn_phone' }, system: true },
  { id: 'global_phone', name: '国际号码', pattern: '\\+[1-9]\\d(?:[ -]?\\d){6,13}\\d', enabled: true, features: { validator: 'global_phone' }, system: true },
  { id: 'cn_id', name: '身份证号', pattern: '(?i)\\b\\d{17}[\\dX]\\b', enabled: true, features: { validator: 'cn_id' }, system: true },
  { id: 'payment_card', name: '银行卡号', pattern: '\\b(?:\\d[ -]?){12,18}\\d\\b', enabled: true, features: { validator: 'payment_card' }, system: true },
  { id: 'ipv4', name: 'IPv4', pattern: '\\b(?:\\d{1,3}\\.){3}\\d{1,3}\\b', enabled: true, features: { validator: 'ipv4' }, system: true },
  { id: 'ipv6', name: 'IPv6', pattern: '\\b(?:[0-9A-Fa-f]{1,4}:){2,7}[0-9A-Fa-f:.]{1,39}\\b', enabled: true, features: { validator: 'ipv6' }, system: true },
  { id: 'api_key', name: 'API Key', pattern: '\\b(?:sk-(?:proj-)?[A-Za-z0-9_-]{20,}|sk-ant-[A-Za-z0-9_-]{20,}|(?:gh[pousr]_[A-Za-z0-9_]{30,}|github_pat_[A-Za-z0-9_]{30,})|xox[baprs]-[A-Za-z0-9-]{20,}|(?:AKIA|ASIA)[0-9A-Z]{16}|[A-Za-z0-9_-]{32,})\\b', enabled: true, features: { validator: 'api_key' }, system: true },
  { id: 'access_token', name: 'Access Token', pattern: "(?i)\\baccess[_-]?token\\s*[:=]\\s*[\"']?[A-Za-z0-9._~+/=-]{20,}", enabled: true, features: { validator: 'access_token' }, system: true },
  { id: 'secret_key', name: 'Secret Key', pattern: "(?i)\\bsecret[_-]?key\\s*[:=]\\s*[\"']?[A-Za-z0-9._~+/=-]{20,}", enabled: true, features: { validator: 'secret_key' }, system: true },
  { id: 'bearer_token', name: 'Bearer Token', pattern: '(?i)\\bBearer\\s+[A-Za-z0-9._~+/=-]{20,}', enabled: true, features: { validator: 'bearer_token' }, system: true },
  { id: 'jwt', name: 'JWT', pattern: '\\b[A-Za-z0-9_-]{10,}\\.[A-Za-z0-9_-]{10,}\\.[A-Za-z0-9_-]{10,}\\b', enabled: true, features: { validator: 'jwt' }, system: true },
]

const CHAT_PII_REDACTION_CONFIG_KEYS = {
  enabled: 'module.chat_pii_redaction.enabled',
  rules: 'module.chat_pii_redaction.rules',
  cache_ttl_seconds: 'module.chat_pii_redaction.cache_ttl_seconds',
  placeholder_prefix: 'module.chat_pii_redaction.placeholder_prefix',
} as const

const CODEX_ADAPTER_CONFIG_KEYS = {
  routes: 'module.codex_adapter.routes',
} as const

const CHAT_PII_REDACTION_DEFAULT_CONFIG: ChatPiiRedactionConfig = {
  enabled: false,
  rules: CHAT_PII_REDACTION_DEFAULT_RULES.map(rule => ({ ...rule })),
  cache_ttl_seconds: 300,
  placeholder_prefix: 'AETHER',
}

export function normalizeModuleManagementOrder(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  const seen = new Set<string>()
  const order: string[] = []
  for (const item of value) {
    if (typeof item !== 'string') continue
    const name = item.trim()
    if (!name || seen.has(name)) continue
    seen.add(name)
    order.push(name)
  }
  return order
}

function cloneDefaultChatPiiRedactionRules(): ChatPiiRedactionRule[] {
  return CHAT_PII_REDACTION_DEFAULT_RULES.map(rule => ({ ...rule }))
}

function normalizeChatPiiRedactionRule(value: unknown, index: number): ChatPiiRedactionRule | null {
  if (!value || typeof value !== 'object') return null
  const item = value as Record<string, unknown>
  const id = typeof item.id === 'string' && item.id.trim()
    ? item.id.trim()
    : `custom_${index + 1}`
  const name = typeof item.name === 'string' && item.name.trim()
    ? item.name.trim()
    : id
  const pattern = typeof item.pattern === 'string' ? item.pattern : ''
  if (!pattern.trim()) return null
  const rawFeatures = item.features && typeof item.features === 'object' && !Array.isArray(item.features)
    ? { ...(item.features as Record<string, unknown>) }
    : {}
  const legacyValidator = typeof item.kind === 'string' && item.kind.trim()
    ? item.kind.trim()
    : null
  const validator = typeof rawFeatures.validator === 'string' && rawFeatures.validator.trim()
    ? rawFeatures.validator.trim()
    : legacyValidator
  if (validator) {
    rawFeatures.validator = validator
  } else {
    delete rawFeatures.validator
  }
  const features = Object.keys(rawFeatures).length > 0 ? rawFeatures : null
  return {
    id,
    name,
    pattern,
    enabled: item.enabled !== false,
    system: item.system === true,
    features,
  }
}

function normalizeChatPiiRedactionRules(value: unknown): ChatPiiRedactionRule[] {
  if (!Array.isArray(value)) return cloneDefaultChatPiiRedactionRules()
  return value
    .map((item, index) => normalizeChatPiiRedactionRule(item, index))
    .filter((item): item is ChatPiiRedactionRule => item !== null)
}

function normalizeChatPiiRedactionConfig(values: {
  enabled: unknown
  rules: unknown
  cache_ttl_seconds: unknown
  placeholder_prefix: unknown
}): ChatPiiRedactionConfig {
  return {
    enabled: values.enabled === true,
    rules: normalizeChatPiiRedactionRules(values.rules),
    cache_ttl_seconds: values.cache_ttl_seconds === 3600 ? 3600 : 300,
    placeholder_prefix: normalizePlaceholderPrefix(values.placeholder_prefix),
  }
}

function normalizePlaceholderPrefix(value: unknown): string {
  if (typeof value !== 'string') return CHAT_PII_REDACTION_DEFAULT_CONFIG.placeholder_prefix
  const normalized = value.trim().toUpperCase()
  return /^[A-Z0-9_]{1,32}$/.test(normalized)
    ? normalized
    : CHAT_PII_REDACTION_DEFAULT_CONFIG.placeholder_prefix
}

export function createDefaultCodexAdapterCandidateConfig(): CodexAdapterCandidateConfig {
  return {
    global_model: '',
    enabled: true,
    priority: 0,
    weight: 100,
  }
}

export function createDefaultCodexAdapterRouteConfig(): CodexAdapterRouteConfig {
  return {
    codex_model: '',
    enabled: true,
    scheduling_mode: 'priority',
    candidates: [createDefaultCodexAdapterCandidateConfig()],
  }
}

function normalizeCodexAdapterSchedulingMode(value: unknown): CodexAdapterSchedulingMode {
  return value === 'sticky' || value === 'load_balance' ? value : 'priority'
}

function normalizeCodexAdapterCandidate(value: unknown): CodexAdapterCandidateConfig | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const candidate = value as Record<string, unknown>
  const globalModel = typeof candidate.global_model === 'string'
    ? candidate.global_model.trim()
    : ''
  const priority = Number.isInteger(candidate.priority) ? Number(candidate.priority) : 0
  const weight = Number.isInteger(candidate.weight) && Number(candidate.weight) > 0
    ? Number(candidate.weight)
    : 100

  return {
    global_model: globalModel,
    enabled: candidate.enabled !== false,
    priority,
    weight,
  }
}

export function normalizeCodexAdapterConfig(value: unknown): CodexAdapterRouteConfig[] {
  if (!Array.isArray(value)) return []

  const seenModels = new Set<string>()
  const routes: CodexAdapterRouteConfig[] = []

  for (const item of value) {
    if (!item || typeof item !== 'object' || Array.isArray(item)) continue
    const route = item as Record<string, unknown>
    const codexModel = typeof route.codex_model === 'string'
      ? route.codex_model.trim()
      : ''
    if (!codexModel || seenModels.has(codexModel)) continue
    seenModels.add(codexModel)

    const candidates = Array.isArray(route.candidates)
      ? route.candidates
        .map(normalizeCodexAdapterCandidate)
        .filter((candidate): candidate is CodexAdapterCandidateConfig => candidate !== null)
      : []

    routes.push({
      codex_model: codexModel,
      enabled: route.enabled !== false,
      scheduling_mode: normalizeCodexAdapterSchedulingMode(route.scheduling_mode),
      candidates,
    })
  }

  return routes
}

export function serializeCodexAdapterConfig(routes: CodexAdapterRouteConfig[]): CodexAdapterRouteConfig[] {
  return routes.map(route => ({
    codex_model: route.codex_model.trim(),
    enabled: route.enabled,
    scheduling_mode: normalizeCodexAdapterSchedulingMode(route.scheduling_mode),
    candidates: route.candidates.map(candidate => ({
      global_model: candidate.global_model.trim(),
      enabled: candidate.enabled,
      priority: Number.isFinite(Number(candidate.priority))
        ? Math.trunc(Number(candidate.priority))
        : 0,
      weight: Number.isInteger(candidate.weight) && candidate.weight > 0
        ? candidate.weight
        : Number(candidate.weight) > 0
          ? Math.trunc(Number(candidate.weight))
          : 0,
    })),
  }))
}

export function validateCodexAdapterConfig(routes: CodexAdapterRouteConfig[]): string | null {
  const normalized = serializeCodexAdapterConfig(routes)
  const routeModels = new Set<string>()

  for (let routeIndex = 0; routeIndex < normalized.length; routeIndex += 1) {
    const route = normalized[routeIndex]
    const routeLabel = `第 ${routeIndex + 1} 条路由`

    if (!route.codex_model) return `${routeLabel} 的 Codex 请求模型不能为空`
    if (routeModels.has(route.codex_model)) return `Codex 请求模型 ${route.codex_model} 不能重复`
    routeModels.add(route.codex_model)

    const candidateModels = new Set<string>()
    let enabledCandidates = 0

    for (let candidateIndex = 0; candidateIndex < route.candidates.length; candidateIndex += 1) {
      const candidate = route.candidates[candidateIndex]
      const candidateLabel = `${routeLabel} 的第 ${candidateIndex + 1} 个候选模型`

      if (!candidate.global_model) return `${candidateLabel} 不能为空`
      if (candidateModels.has(candidate.global_model)) {
        return `${route.codex_model} 下的候选模型 ${candidate.global_model} 不能重复`
      }
      candidateModels.add(candidate.global_model)

      if (!Number.isInteger(candidate.priority)) return `${candidateLabel} 的优先级必须是整数`
      if (!Number.isInteger(candidate.weight) || candidate.weight <= 0) {
        return `${candidateLabel} 的权重必须大于 0`
      }
      if (candidate.enabled) enabledCandidates += 1
    }

    if (route.enabled && enabledCandidates === 0) {
      return `${route.codex_model} 已启用时至少需要一个启用中的候选模型`
    }
  }

  return null
}

async function getSystemConfigValue(key: string): Promise<unknown> {
  const response = await apiClient.get<{ key: string; value: unknown }>(`/api/admin/system/configs/${key}`)
  return response.data.value
}

async function updateSystemConfigValue(key: string, value: unknown, description: string) {
  const response = await apiClient.put<{ key: string; value: unknown; description?: string }>(
    `/api/admin/system/configs/${key}`,
    { value, description },
  )
  return response.data.value
}

export const modulesApi = {
  /**
   * 获取所有模块状态（管理员）
   */
  async getAllStatus(): Promise<Record<string, ModuleStatus>> {
    const response = await apiClient.get<Record<string, ModuleStatus>>(
      '/api/admin/modules/status'
    )
    return response.data
  },

  /**
   * 获取单个模块状态（管理员）
   */
  async getStatus(moduleName: string): Promise<ModuleStatus> {
    const response = await apiClient.get<ModuleStatus>(
      `/api/admin/modules/status/${moduleName}`
    )
    return response.data
  },

  /**
   * 设置模块启用状态（管理员）
   */
  async setEnabled(moduleName: string, enabled: boolean): Promise<ModuleStatus> {
    const response = await apiClient.put<ModuleStatus>(
      `/api/admin/modules/status/${moduleName}/enabled`,
      { enabled }
    )
    return response.data
  },

  async getModuleManagementOrder(): Promise<string[]> {
    try {
      const response = await apiClient.get<{ key: string; value: unknown }>(
        `/api/admin/system/configs/${MODULE_MANAGEMENT_ORDER_CONFIG_KEY}`
      )
      return normalizeModuleManagementOrder(response.data.value)
    } catch (err) {
      const status = (err as { response?: { status?: number } }).response?.status
      if (status === 404) return []
      throw err
    }
  },

  async updateModuleManagementOrder(order: string[]): Promise<string[]> {
    const normalized = normalizeModuleManagementOrder(order)
    const response = await apiClient.put<{ key: string; value: unknown }>(
      `/api/admin/system/configs/${MODULE_MANAGEMENT_ORDER_CONFIG_KEY}`,
      {
        value: normalized,
        description: '模块管理扩展模块展示顺序',
      },
    )
    return normalizeModuleManagementOrder(response.data.value)
  },

  async getChatPiiRedactionConfig(): Promise<ChatPiiRedactionConfig> {
    const [enabled, rules, cacheTtlSeconds, placeholderPrefix] = await Promise.all([
      getSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.enabled),
      getSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.rules),
      getSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.cache_ttl_seconds),
      getSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.placeholder_prefix),
    ])

    return normalizeChatPiiRedactionConfig({
      enabled,
      rules,
      cache_ttl_seconds: cacheTtlSeconds,
      placeholder_prefix: placeholderPrefix,
    })
  },

  async updateChatPiiRedactionConfig(config: ChatPiiRedactionConfig): Promise<ChatPiiRedactionConfig> {
    const [enabled, rules, cacheTtlSeconds, placeholderPrefix] = await Promise.all([
      updateSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.enabled, config.enabled, '敏感信息保护总开关'),
      updateSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.rules, config.rules, '敏感信息保护替换规则'),
      updateSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.cache_ttl_seconds, config.cache_ttl_seconds, '敏感信息保护缓存 TTL'),
      updateSystemConfigValue(CHAT_PII_REDACTION_CONFIG_KEYS.placeholder_prefix, config.placeholder_prefix, '敏感信息保护占位符前缀'),
    ])

    return normalizeChatPiiRedactionConfig({
      enabled,
      rules,
      cache_ttl_seconds: cacheTtlSeconds,
      placeholder_prefix: placeholderPrefix,
    })
  },

  async getCodexAdapterConfig(): Promise<CodexAdapterRouteConfig[]> {
    const routes = await getSystemConfigValue(CODEX_ADAPTER_CONFIG_KEYS.routes)
    return normalizeCodexAdapterConfig(routes)
  },

  async updateCodexAdapterConfig(config: CodexAdapterRouteConfig[]): Promise<CodexAdapterRouteConfig[]> {
    const routes = await updateSystemConfigValue(
      CODEX_ADAPTER_CONFIG_KEYS.routes,
      serializeCodexAdapterConfig(config),
      'Codex 适配器模型映射与候选调度配置',
    )
    return normalizeCodexAdapterConfig(routes)
  },

  /**
   * 获取认证模块状态（公开接口，供登录页使用）
   */
  async getAuthModulesStatus(): Promise<AuthModuleInfo[]> {
    const response = await apiClient.get<AuthModuleInfo[]>('/api/modules/auth-status')
    return response.data
  },
}
